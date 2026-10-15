//! Record reading and output. No script syntax handling.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::stream::ChunkReader;
use crate::Fatal;

/// How much is read at a time: large enough to keep system calls rare, small enough to stay in
/// cache. With `-u` input is read a byte at a time so that nothing is taken past the last record.
const BLOCK: usize = 256 * 1024;

/// File descriptor 0, which is never closed by this handle.
struct StdinFile(ManuallyDrop<File>);

impl Read for StdinFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for StdinFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

/// Standard input as it is read: one chunked source shared by the input stream and by
/// `R /dev/stdin`, so they see one stream. It reads file descriptor 0 directly and remembers
/// the offset it started at, so that the offset can be put back at the end
/// (see `restore_stdin_offset`).
struct StdinState {
    source: ChunkedSource<StdinFile>,
    /// The offset of descriptor 0 when reading began; `None` for pipes and terminals.
    start: Option<u64>,
}

thread_local! {
    static STDIN: RefCell<Option<StdinState>> = const { RefCell::new(None) };
}

fn with_stdin<T>(f: impl FnOnce(&mut StdinState) -> T, unbuffered: bool, separator: u8) -> T {
    STDIN.with(|cell| {
        let mut slot = cell.borrow_mut();
        let state = slot.get_or_insert_with(|| {
            // SAFETY: descriptor 0 is owned by the process; ManuallyDrop keeps it open.
            let mut file = StdinFile(ManuallyDrop::new(unsafe { File::from_raw_fd(0) }));
            #[allow(clippy::seek_from_current)]
            let start = file.seek(SeekFrom::Current(0)).ok();
            StdinState {
                source: ChunkedSource::new(file, separator, if unbuffered { 1 } else { BLOCK }),
                start,
            }
        });
        f(state)
    })
}

/// Reads one separator-terminated record from standard input.
pub fn stdin_record(separator: u8, unbuffered: bool) -> io::Result<Option<Record>> {
    with_stdin(|s| s.source.next_record(), unbuffered, separator)
}

/// Like `stdin_record`, reading the record into `body` (which is cleared first).
fn stdin_record_into(body: Vec<u8>, separator: u8, unbuffered: bool) -> io::Result<Option<Record>> {
    with_stdin(|s| s.source.next_record_into(body), unbuffered, separator)
}

/// Takes back `n` bytes of records that were read ahead but not used (the record looked at
/// to find out whether the current one is the last).
fn stdin_unconsume(n: u64) {
    STDIN.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            state.source.consumed = state.source.consumed.saturating_sub(n);
        }
    });
}

/// Puts the file offset of standard input where the last record sed used ends, as GNU sed
/// does when it quits early, so that the next process reads what sed left. It does nothing
/// for pipes and terminals.
pub fn restore_stdin_offset() {
    STDIN.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            if let Some(start) = state.start {
                let target = start + state.source.consumed;
                let _ = state.source.reader_mut().seek(SeekFrom::Start(target));
            }
        }
    });
}

/// Records out of a chunked byte stream. `consumed` counts the bytes of the records handed out.
pub struct ChunkedSource<R: Read> {
    reader: ChunkReader<R>,
    separator: u8,
    chunk: Vec<u8>,
    pos: usize,
    pub consumed: u64,
}

impl<R: Read> ChunkedSource<R> {
    pub fn new(reader: R, separator: u8, block: usize) -> Self {
        ChunkedSource { reader: ChunkReader::new(reader, separator, block), separator, chunk: Vec::new(), pos: 0, consumed: 0 }
    }

    pub fn reader_mut(&mut self) -> &mut R {
        self.reader.get_mut()
    }

    /// The next record, or `None` at the end of the input.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        self.next_record_into(Vec::new())
    }

    /// Skips up to `n` records without building them; returns how many there were.
    pub fn skip_records(&mut self, n: u64) -> io::Result<u64> {
        let mut done = 0u64;
        while done < n {
            if self.pos >= self.chunk.len() {
                match self.reader.next_chunk()? {
                    Some(chunk) => {
                        self.chunk = chunk.data;
                        self.pos = 0;
                    }
                    None => break,
                }
                continue;
            }
            let rest = &self.chunk[self.pos..];
            let separators = crate::search::count(self.separator, rest) as u64;
            let records = separators + u64::from(rest.last() != Some(&self.separator));
            if records <= n - done {
                done += records;
                self.consumed += rest.len() as u64;
                self.pos = self.chunk.len();
            } else {
                let mut off = 0;
                for _ in 0..n - done {
                    off += crate::search::memchr(self.separator, &rest[off..]).expect("counted") + 1;
                }
                done = n;
                self.pos += off;
                self.consumed += off as u64;
            }
        }
        Ok(done)
    }

    /// The next record, stored in `body` (cleared first, its capacity reused).
    pub fn next_record_into(&mut self, mut body: Vec<u8>) -> io::Result<Option<Record>> {
        body.clear();
        loop {
            if self.pos < self.chunk.len() {
                let rest = &self.chunk[self.pos..];
                let (used, terminated) = match crate::search::memchr(self.separator, rest) {
                    Some(i) => {
                        body.extend_from_slice(&rest[..i]);
                        (i + 1, true)
                    }
                    None => {
                        body.extend_from_slice(rest);
                        (rest.len(), false)
                    }
                };
                self.pos += used;
                self.consumed += used as u64;
                return Ok(Some(Record { body, terminated }));
            }
            match self.reader.next_chunk()? {
                Some(chunk) => {
                    self.chunk = chunk.data;
                    self.pos = 0;
                }
                None => return Ok(None),
            }
        }
    }
}

/// Set before `main` if file descriptor 1 was closed when the process started (Rust
/// reopens closed standard descriptors on `/dev/null`).
pub static STDOUT_WAS_CLOSED: AtomicBool = AtomicBool::new(false);

/// One input record: the body without its terminator, and whether the
/// terminator was present. A missing final terminator is preserved.
#[derive(Debug, PartialEq, Eq)]
pub struct Record {
    pub body: Vec<u8>,
    pub terminated: bool,
}

/// Reads one record terminated by `separator`. Returns `None` at end of input.
pub fn read_record<R: BufRead + ?Sized>(reader: &mut R, separator: u8) -> io::Result<Option<Record>> {
    let mut body = Vec::new();
    let n = reader.read_until(separator, &mut body)?;
    if n == 0 {
        return Ok(None);
    }
    let terminated = body.last() == Some(&separator);
    if terminated {
        body.pop();
    }
    Ok(Some(Record { body, terminated }))
}

/// A record together with where it came from.
#[derive(Debug)]
pub struct Fetched {
    pub record: Record,
    /// The first record of a file (the first file included).
    pub new_file: bool,
    pub file_name: Rc<str>,
}

/// A source of input records with one record of lookahead (for `$`).
pub trait RecordSource {
    fn next_record(&mut self) -> Result<Option<Fetched>, Fatal>;
    /// Skips up to `n` records (fewer at the end of the input); returns how many were skipped.
    /// Sources that cannot skip cheaply skip none.
    fn skip_records(&mut self, _n: u64) -> Result<u64, Fatal> {
        Ok(0)
    }
    /// Hands back a buffer the caller is done with, to be reused for a later record.
    fn recycle(&mut self, _buf: Vec<u8>) {}
    /// Whether the record last returned is the last one: of all input, or of
    /// its file when `separate` is set.
    fn is_last(&mut self, separate: bool) -> Result<bool, Fatal>;
}

/// The input files of a run, read as one continuous stream of records.
///
/// A file that cannot be opened is reported as `PROGRAM: can't read NAME: reason`
/// and skipped; reading continues with the next file and `had_open_error` is set
/// (exit status 2). A missing final terminator ends a record, so it never joins
/// the first record of the next file.
enum Source {
    Chunked(Box<ChunkedSource<File>>),
    Stdin,
}

pub struct FileInputs {
    names: std::vec::IntoIter<String>,
    current: Option<(Source, Rc<str>)>,
    /// A record read ahead of time, and whether it starts a new file.
    peeked: Option<Fetched>,
    /// A buffer for the next record (see `RecordSource::recycle`).
    spare: Vec<u8>,
    started_file: bool,
    separator: u8,
    program: String,
    pub had_open_error: bool,
    /// `-i`: only regular files can be edited; anything else is an error (exit status 4).
    in_place: bool,
    follow_symlinks: bool,
    /// An error that stops the run at the next read (exit status 4).
    fatal: Option<Fatal>,
    /// A read error on an input (reported, then reading continues): exit status 4.
    pub had_read_error: bool,
    pub had_edit_error: bool,
    /// `-u`: read standard input without read-ahead.
    unbuffered: bool,
}

impl FileInputs {
    /// `names` are file names; `-` is standard input. An empty list reads standard input.
    pub fn new(mut names: Vec<String>, separator: u8, program: &str) -> Self {
        if names.is_empty() {
            names.push("-".to_string());
        }
        FileInputs {
            names: names.into_iter(),
            current: None,
            peeked: None,
            spare: Vec::new(),
            started_file: false,
            separator,
            program: program.to_string(),
            had_open_error: false,
            in_place: false,
            follow_symlinks: false,
            fatal: None,
            had_read_error: false,
            had_edit_error: false,
            unbuffered: false,
        }
    }

    /// Call when the run is over: gives back a record that was only looked at, so that the
    /// offset left on standard input is exact.
    pub fn finish(&mut self) {
        if let Some(p) = self.peeked.take() {
            if &*p.file_name == "-" {
                stdin_unconsume((p.record.body.len() + usize::from(p.record.terminated)) as u64);
            }
        }
    }

    pub fn in_place(mut self, yes: bool) -> Self {
        self.in_place = yes;
        self
    }

    pub fn follow_symlinks(mut self, yes: bool) -> Self {
        self.follow_symlinks = yes;
        self
    }

    pub fn unbuffered(mut self, yes: bool) -> Self {
        self.unbuffered = yes;
        self
    }

    fn open_next(&mut self) -> bool {
        for name in self.names.by_ref() {
            if name == "-" && !self.in_place {
                self.current = Some((Source::Stdin, Rc::from("-")));
                self.started_file = false;
                return true;
            }
            // With --follow-symlinks every input file (not only an in-place one) is resolved first:
            // the resolved path is what is opened and what `F` and the diagnostics name.
            let given = name.clone();
            let name = if self.follow_symlinks {
                match follow_link_chain(std::path::Path::new(&name)) {
                    Ok(resolved) => resolved.to_string_lossy().into_owned(),
                    Err(FollowError::Link(shown, e)) => {
                        self.fatal = Some(Fatal::new(format!("couldn't readlink {}: {}", shown.display(), strerror(&e)), 4));
                        return false;
                    }
                    Err(FollowError::Loop(e)) => {
                        self.fatal = Some(Fatal::new(format!("couldn't follow symlink {}: {}", name, strerror(&e)), 4));
                        return false;
                    }
                }
            } else {
                name
            };
            if self.in_place {
                if let Ok(m) = std::fs::metadata(&name) {
                    if !m.is_file() {
                        self.fatal = Some(Fatal::new(format!("couldn't edit {}: not a regular file", name), 4));
                        return false;
                    }
                }
            }
            match File::open(os_path(&name)) {
                Ok(f) => {
                    let source = ChunkedSource::new(f, self.separator, BLOCK);
                    self.current = Some((Source::Chunked(Box::new(source)), Rc::from(name.as_str())));
                    self.started_file = false;
                    return true;
                }
                Err(e) => {
                    crate::cli::eprint_message(&format!("{}: can't read {}: {}", self.program, given, strerror(&e)));
                    self.had_open_error = true;
                }
            }
        }
        false
    }

    /// Reads ahead one record. A read error stops the run, unless `lookahead` is set: GNU sed
    /// does not notice an error while it only looks for the end of the input.
    fn fill(&mut self, lookahead: bool) -> Result<(), Fatal> {
        while self.peeked.is_none() {
            if self.current.is_none() && !self.open_next() {
                return match self.fatal.take() {
                    Some(f) => Err(f),
                    None => Ok(()),
                };
            }
            let (source, name) = self.current.as_mut().unwrap();
            let read = match source {
                Source::Chunked(source) => source.next_record_into(std::mem::take(&mut self.spare)),
                Source::Stdin => stdin_record_into(std::mem::take(&mut self.spare), self.separator, self.unbuffered),
            };
            match read {
                Ok(Some(record)) => {
                    let new_file = !self.started_file;
                    self.started_file = true;
                    self.peeked = Some(Fetched { record, new_file, file_name: name.clone() });
                }
                Ok(None) => self.current = None,
                Err(e) => {
                    if lookahead {
                        self.current = None;
                        continue;
                    }
                    return Err(Fatal::new(format!("read error on {}: {}", display_name(name), strerror(&e)), 4));
                }
            }
        }
        Ok(())
    }
}

impl RecordSource for FileInputs {
    fn next_record(&mut self) -> Result<Option<Fetched>, Fatal> {
        self.fill(false)?;
        Ok(self.peeked.take())
    }

    fn skip_records(&mut self, mut n: u64) -> Result<u64, Fatal> {
        let mut done = 0;
        if n > 0 && self.peeked.take().is_some() {
            done += 1;
            n -= 1;
        }
        while n > 0 {
            if self.current.is_none() && !self.open_next() {
                return match self.fatal.take() {
                    Some(f) => Err(f),
                    None => Ok(done),
                };
            }
            let (source, name) = self.current.as_mut().unwrap();
            let skipped = match source {
                Source::Chunked(source) => source.skip_records(n),
                Source::Stdin => with_stdin(|s| s.source.skip_records(n), self.unbuffered, self.separator),
            };
            match skipped {
                Ok(k) => {
                    done += k;
                    n -= k;
                    if k > 0 {
                        self.started_file = true;
                    }
                    if n > 0 {
                        self.current = None;
                    }
                }
                Err(e) => return Err(Fatal::new(format!("read error on {}: {}", display_name(name), strerror(&e)), 4)),
            }
        }
        Ok(done)
    }

    fn recycle(&mut self, buf: Vec<u8>) {
        if buf.capacity() > self.spare.capacity() && buf.capacity() <= 1 << 20 {
            self.spare = buf;
        }
    }

    fn is_last(&mut self, separate: bool) -> Result<bool, Fatal> {
        if separate && self.peeked.is_none() {
            // Only the current file matters: the next one is not opened (nor its errors found)
            // before sed gets there.
            let Some((source, name)) = self.current.as_mut() else { return Ok(true) };
            let read = match source {
                Source::Chunked(source) => source.next_record_into(std::mem::take(&mut self.spare)),
                Source::Stdin => stdin_record_into(std::mem::take(&mut self.spare), self.separator, self.unbuffered),
            };
            return match read {
                Ok(Some(record)) => {
                    let new_file = !self.started_file;
                    self.started_file = true;
                    self.peeked = Some(Fetched { record, new_file, file_name: name.clone() });
                    Ok(false)
                }
                // The end of the file, or an error that only shows once sed reads on.
                Ok(None) | Err(_) => {
                    self.current = None;
                    Ok(true)
                }
            };
        }
        self.fill(true)?;
        Ok(match &self.peeked {
            None => true,
            Some(next) => separate && next.new_file,
        })
    }
}

/// A single in-memory reader as a record source, for tests.
pub struct ReaderSource<R: BufRead> {
    reader: R,
    separator: u8,
    first: bool,
    peeked: Option<Record>,
}

impl<R: BufRead> ReaderSource<R> {
    pub fn new(reader: R, separator: u8) -> Self {
        ReaderSource { reader, separator, first: true, peeked: None }
    }

    fn fill(&mut self) -> Result<(), Fatal> {
        if self.peeked.is_none() {
            self.peeked = read_record(&mut self.reader, self.separator).map_err(|e| Fatal::new(strerror(&e), 4))?;
        }
        Ok(())
    }
}

impl<R: BufRead> RecordSource for ReaderSource<R> {
    fn next_record(&mut self) -> Result<Option<Fetched>, Fatal> {
        self.fill()?;
        let new_file = self.first;
        self.first = false;
        Ok(self.peeked.take().map(|record| Fetched { record, new_file, file_name: Rc::from("-") }))
    }

    fn is_last(&mut self, _separate: bool) -> Result<bool, Fatal> {
        self.fill()?;
        Ok(self.peeked.is_none())
    }
}

/// The operating system's message for an I/O error, without Rust's ` (os error N)` suffix.
pub fn strerror(e: &io::Error) -> String {
    let text = e.to_string();
    match text.find(" (os error") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

/// Output stream that writes records and remembers a missing final newline.
///
/// When a record without a terminator is written and more output follows on
/// the same stream, the missing terminator is added first, as GNU sed does.
pub struct Output {
    inner: Rc<RefCell<Box<dyn Write>>>,
    separator: u8,
    missing_separator: bool,
    pub name: String,
}

impl Output {
    pub fn new(inner: Box<dyn Write>, separator: u8, name: &str) -> Self {
        Output { inner: Rc::new(RefCell::new(inner)), separator, missing_separator: false, name: name.to_string() }
    }

    /// Another output onto the same stream with its own missing-newline state, like
    /// `w /dev/stdout` next to the normal output.
    pub fn alias(&self) -> Output {
        Output {
            inner: self.inner.clone(),
            separator: self.separator,
            missing_separator: false,
            name: self.name.clone(),
        }
    }

    /// Ends a line that was written without its terminator, if there is one (`q` does this).
    pub fn finish_line(&mut self) -> io::Result<()> {
        self.fix_missing()
    }

    fn fix_missing(&mut self) -> io::Result<()> {
        if self.missing_separator {
            self.inner.borrow_mut().write_all(&[self.separator])?;
            self.missing_separator = false;
        }
        Ok(())
    }

    pub fn write_record(&mut self, body: &[u8], terminated: bool) -> io::Result<()> {
        self.fix_missing()?;
        self.inner.borrow_mut().write_all(body)?;
        if terminated {
            self.inner.borrow_mut().write_all(&[self.separator])?;
        } else {
            self.missing_separator = true;
        }
        Ok(())
    }

    /// Writes bytes as they are (text of `a`, `i`, `c`, file contents), after
    /// restoring a missing terminator.
    pub fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.fix_missing()?;
        self.inner.borrow_mut().write_all(bytes)
    }

    /// Writes bytes without restoring a missing terminator first: the `--debug` trace goes
    /// to standard output beside the output, and does not complete an unterminated line.
    pub fn write_unchecked(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.borrow_mut().write_all(bytes)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.borrow_mut().flush()
    }

    /// Whether the last record written had no terminator (it gets one if more output follows).
    pub fn ends_unterminated(&self) -> bool {
        self.missing_separator
    }

    /// Declares that the previous output ended without a terminator, as another writer left it.
    pub fn set_unterminated(&mut self, yes: bool) {
        self.missing_separator = yes;
    }
}

/// Every output the script writes to: standard output, standard error, and the
/// files named by `w`, `W`, and `s///w` (created or truncated before the first
/// input line is read, and shared by all commands that name the same file).
pub struct Outputs {
    /// The real standard output (`w /dev/stdout` always writes here).
    pub stdout: Output,
    /// `w /dev/stdout`: the same stream, with its own missing-newline state.
    stdout_alias: Output,
    stderr: Option<Output>,
    files: HashMap<Vec<u8>, Output>,
    separator: u8,
    in_place: Option<InPlaceFile>,
    /// Flush after every record (`-u`).
    pub unbuffered: bool,
}

/// The file being edited in place: output goes to a temporary file that
/// replaces the original when the file is finished.
struct InPlaceFile {
    output: Output,
    original: std::path::PathBuf,
    temp: std::path::PathBuf,
}

impl Outputs {
    pub fn new(stdout: Box<dyn Write>, separator: u8) -> Self {
        let stdout = Output::new(stdout, separator, "stdout");
        Outputs {
            stdout_alias: stdout.alias(),
            stdout,
            stderr: None,
            files: HashMap::new(),
            separator,
            in_place: None,
            unbuffered: false,
        }
    }

    /// Where the script's normal output goes: the file being edited in place, or standard output.
    pub fn main(&mut self) -> &mut Output {
        match self.in_place.as_mut() {
            Some(f) => &mut f.output,
            None => &mut self.stdout,
        }
    }

    pub fn is_in_place(&self) -> bool {
        self.in_place.is_some()
    }

    /// Starts editing `path` in place: creates the temporary file next to it.
    pub fn begin_in_place(&mut self, path: &str, follow_symlinks: bool) -> Result<(), Fatal> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut original = std::path::PathBuf::from(path);
        if follow_symlinks {
            if let Ok(resolved) = follow_link_chain(&original) {
                original = resolved;
            }
        }
        let dir = match original.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => std::path::PathBuf::from("."),
        };
        let pid = std::process::id();
        for attempt in 0..1000u32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let mut n = (u64::from(pid) << 32) ^ u64::from(nanos) ^ (u64::from(attempt) << 8);
            let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            let mut name = String::from("sed");
            for _ in 0..6 {
                name.push(alphabet[(n % 62) as usize] as char);
                n /= 62;
                n ^= n << 7;
            }
            let temp = dir.join(name);
            match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp) {
                Ok(file) => {
                    // The mode and the owner of the file are taken when it is opened, as GNU sed
                    // does, so they are the same if the original goes away during the run. The
                    // owner is handed over after the mode, which makes the kernel drop the
                    // set-user-ID and set-group-ID bits of an unprivileged user's file.
                    if let Ok(meta) = std::fs::metadata(&original) {
                        use std::os::unix::fs::MetadataExt;
                        let _ = std::fs::set_permissions(&temp, meta.permissions());
                        let _ = std::os::unix::fs::chown(&temp, Some(meta.uid()), Some(meta.gid()));
                    }
                    let output = Output::new(Box::new(io::BufWriter::new(file)), self.separator, path);
                    self.in_place = Some(InPlaceFile { output, original, temp });
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(Fatal::new(format!("couldn't open temporary file {}: {}", temp.display(), strerror(&e)), 4));
                }
            }
        }
        Err(Fatal::new("couldn't open temporary file", 4))
    }

    /// Gives up the file being edited after a fatal error: the temporary file is removed and the
    /// original stays as it was.
    pub fn abort_in_place(&mut self) {
        if let Some(file) = self.in_place.take() {
            drop(file.output);
            let _ = std::fs::remove_file(&file.temp);
        }
    }

    /// Finishes the file being edited: flushes the temporary file, keeps a backup
    /// when `backup_suffix` is given, and moves the temporary file over the original.
    pub fn finish_in_place(&mut self, backup_suffix: Option<&str>) -> Result<(), Fatal> {
        let Some(mut file) = self.in_place.take() else { return Ok(()) };
        file.output.flush().map_err(|e| write_error(&file.output.name, &e))?;
        drop(file.output);
        let rename_error = |name: &std::path::Path, e: io::Error| {
            Fatal::new(format!("cannot rename {}: {}", name.display(), strerror(&e)), 4)
        };
        if let Some(suffix) = backup_suffix.filter(|s| !s.is_empty()) {
            // A `*` stands for the input file name exactly as it was given (with its directory);
            // without one the suffix is appended. The result is relative to the working directory.
            use std::os::unix::ffi::{OsStrExt, OsStringExt};
            let path = file.original.as_os_str().as_bytes();
            let suffix = suffix.as_bytes();
            let backup_name: Vec<u8> = if suffix.contains(&b'*') {
                let mut out = Vec::new();
                for &b in suffix {
                    if b == b'*' {
                        out.extend_from_slice(path);
                    } else {
                        out.push(b);
                    }
                }
                out
            } else {
                [path, suffix].concat()
            };
            let backup = std::path::PathBuf::from(std::ffi::OsString::from_vec(backup_name));
            if let Err(e) = std::fs::rename(&file.original, &backup) {
                let _ = std::fs::remove_file(&file.temp);
                return Err(rename_error(&file.original, e));
            }
        }
        std::fs::rename(&file.temp, &file.original).map_err(|e| rename_error(&file.temp, e))
    }

    /// Opens (truncating) the file named `name`, unless it is `/dev/stdout` or `/dev/stderr`.
    pub fn open(&mut self, name: &[u8]) -> Result<(), Fatal> {
        match name {
            b"/dev/stdout" => Ok(()),
            b"/dev/stderr" => {
                if self.stderr.is_none() {
                    self.stderr = Some(Output::new(Box::new(io::stderr()), self.separator, "stderr"));
                }
                Ok(())
            }
            _ => {
                if self.files.contains_key(name) {
                    return Ok(());
                }
                let path = String::from_utf8_lossy(name).into_owned();
                match File::create(bytes_path(name)) {
                    Ok(f) => {
                        self.files.insert(name.to_vec(), Output::new(Box::new(io::BufWriter::new(f)), self.separator, &path));
                        Ok(())
                    }
                    Err(e) => Err(Fatal::new(format!("couldn't open file {}: {}", path, strerror(&e)), 4)),
                }
            }
        }
    }

    /// The output a `w`-style file name stands for.
    pub fn get(&mut self, name: &[u8]) -> &mut Output {
        match name {
            b"/dev/stdout" => &mut self.stdout_alias,
            b"/dev/stderr" => self.stderr.as_mut().expect("opened before use"),
            _ => self.files.get_mut(name).expect("opened before use"),
        }
    }

    pub fn flush_all(&mut self) -> Result<(), Fatal> {
        let mut result = self.stdout.flush().map_err(|e| flush_error("stdout", &e));
        // A file being edited in place was finished or given up before this is called.
        debug_assert!(self.in_place.is_none());
        if let Some(s) = self.stderr.as_mut() {
            let _ = s.flush();
        }
        for out in self.files.values_mut() {
            if let Err(e) = out.flush() {
                if result.is_ok() {
                    // GNU sed does not know the name of a file it fails to flush.
                    result = Err(flush_error("<unknown>", &e));
                }
            }
        }
        if result.is_ok() && !stdout_is_open() {
            result = Err(Fatal::new("couldn't close stdout: Bad file descriptor", 4));
        }
        result
    }
}

unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Whether file descriptor 1 was open when the process started (Rust ignores writes to a
/// closed stdout, so this is checked before `main`).
fn stdout_is_open() -> bool {
    !STDOUT_WAS_CLOSED.load(Ordering::Relaxed)
}

/// Whether `fd` is a closed descriptor.
pub fn fd_is_closed(fd: i32) -> bool {
    const F_GETFD: i32 = 1;
    // SAFETY: F_GETFD only queries the descriptor flags.
    unsafe { fcntl(fd, F_GETFD) == -1 }
}

fn flush_error(name: &str, e: &io::Error) -> Fatal {
    if e.raw_os_error() == Some(9) {
        return Fatal::new(format!("couldn't close {name}: {}", strerror(e)), 4);
    }
    Fatal::new(format!("couldn't flush {name}: {}", strerror(e)), 4)
}

pub fn write_error(name: &str, e: &io::Error) -> Fatal {
    Fatal::new(format!("couldn't write 1 item to {}: {}", name, strerror(e)), 4)
}

/// How GNU sed names an input in its read errors: standard input is `stdin`, not `-`.
pub fn display_name(name: &str) -> &str {
    if name == "-" { "stdin" } else { name }
}

/// Follows a chain of symbolic links one link at a time, as `--follow-symlinks` does: a
/// relative target is joined to the directory part of the link as written (no
/// canonicalization). The error carries the path whose link could not be read.
pub fn follow_link_chain(path: &std::path::Path) -> Result<std::path::PathBuf, FollowError> {
    let mut current = path.to_path_buf();
    let mut followed = 0;
    loop {
        match std::fs::read_link(&current) {
            Ok(target) => {
                // At most 40 links are followed, as the kernel does.
                if followed == 40 {
                    return Err(FollowError::Loop(io::Error::from_raw_os_error(40)));
                }
                followed += 1;
                current = if target.is_absolute() {
                    target
                } else {
                    current.parent().unwrap_or_else(|| std::path::Path::new("")).join(target)
                };
            }
            Err(e) if e.raw_os_error() == Some(22) => return Ok(current),
            Err(e) => return Err(FollowError::Link(current, e)),
        }
    }
}

/// Why a chain of symbolic links could not be followed.
pub enum FollowError {
    /// The link at this path could not be read.
    Link(std::path::PathBuf, io::Error),
    /// The chain is longer than the system allows.
    Loop(io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_records_and_keeps_missing_terminator() {
        let mut input: &[u8] = b"a\n\nlast";
        let r = |i: &mut &[u8]| read_record(i, b'\n').unwrap();
        assert_eq!(r(&mut input), Some(Record { body: b"a".to_vec(), terminated: true }));
        assert_eq!(r(&mut input), Some(Record { body: Vec::new(), terminated: true }));
        assert_eq!(r(&mut input), Some(Record { body: b"last".to_vec(), terminated: false }));
        assert_eq!(r(&mut input), None);
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ripgsed-io-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Skipping records first uses up the record that was only looked at, then goes on over
    /// the end of a file into the next one.
    #[test]
    fn skipping_takes_the_looked_at_record_and_crosses_files() {
        let dir = temp_dir("skip");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::write(&a, "a1\na2\na3\n").unwrap();
        std::fs::write(&b, "b1\nb2\nb3\n").unwrap();
        let names = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
        let mut input = FileInputs::new(names, b'\n', "sed");
        assert_eq!(input.next_record().unwrap().unwrap().record.body, b"a1");
        assert!(!input.is_last(false).unwrap());
        assert_eq!(input.skip_records(3).unwrap(), 3);
        assert_eq!(input.next_record().unwrap().unwrap().record.body, b"b2");
        // More than is left: what there is.
        assert_eq!(input.skip_records(10).unwrap(), 1);
        assert!(input.next_record().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that goes away is reported while skipping, and the rest is skipped on.
    #[test]
    fn skipping_reports_a_missing_file() {
        let dir = temp_dir("skip-missing");
        let a = dir.join("a");
        std::fs::write(&a, "a1\na2\n").unwrap();
        let names = vec![a.to_string_lossy().into_owned(), dir.join("nosuch").to_string_lossy().into_owned()];
        let mut input = FileInputs::new(names, b'\n', "sed");
        assert_eq!(input.skip_records(5).unwrap(), 2);
        assert!(input.had_open_error);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_texts() {
        assert_eq!(strerror(&io::Error::from_raw_os_error(2)), "No such file or directory");
        assert_eq!(strerror(&io::Error::other("a plain message")), "a plain message");
        assert_eq!(flush_error("stdout", &io::Error::from_raw_os_error(9)).message, "couldn't close stdout: Bad file descriptor");
        assert_eq!(flush_error("stdout", &io::Error::from_raw_os_error(28)).message, "couldn't flush stdout: No space left on device");
    }

    #[test]
    fn a_file_that_cannot_be_created_stops_the_run() {
        let mut outputs = Outputs::new(Box::new(io::sink()), b'\n');
        let error = outputs.open(b"/nonexistent-directory/out").unwrap_err();
        assert_eq!(error.status, 4);
        assert_eq!(error.message, "couldn't open file /nonexistent-directory/out: No such file or directory");
        assert!(outputs.open(b"/dev/stdout").is_ok() && outputs.open(b"/dev/stderr").is_ok());
    }

    #[test]
    fn lookahead_knows_the_last_record() {
        let mut src = ReaderSource::new(&b"a\nb\n"[..], b'\n');
        assert!(src.next_record().unwrap().is_some());
        assert!(!src.is_last(false).unwrap());
        assert!(src.next_record().unwrap().is_some());
        assert!(src.is_last(false).unwrap());
    }
}


/// A file name given on the command line, as the path it stands for.
pub fn os_path(name: &str) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::path::PathBuf::from(std::ffi::OsString::from_vec(crate::cli::argument_bytes(name)))
}

/// A file name from the script, as the path it stands for.
pub fn bytes_path(name: &[u8]) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(name))
}

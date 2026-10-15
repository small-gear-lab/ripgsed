//! Record reading and output. No script syntax handling.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Fatal;

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

// Standard input is read through one buffered reader shared by the input stream and by
// `R /dev/stdin` and `r /dev/stdin`, so they see one stream. It reads file descriptor 0
// directly, so that the file offset can be put back at the end (see `restore_stdin_offset`).
thread_local! {
    static STDIN: RefCell<Option<BufReader<StdinFile>>> = const { RefCell::new(None) };
}

fn with_stdin<T>(f: impl FnOnce(&mut BufReader<StdinFile>) -> T, unbuffered: bool) -> T {
    STDIN.with(|cell| {
        let mut slot = cell.borrow_mut();
        let reader = slot.get_or_insert_with(|| {
            // SAFETY: descriptor 0 is owned by the process; ManuallyDrop keeps it open.
            let file = StdinFile(ManuallyDrop::new(unsafe { File::from_raw_fd(0) }));
            BufReader::with_capacity(if unbuffered { 1 } else { 8192 }, file)
        });
        f(reader)
    })
}

/// Reads one separator-terminated record from standard input.
pub fn stdin_record(separator: u8, unbuffered: bool) -> io::Result<Option<Record>> {
    with_stdin(|r| read_record(r, separator), unbuffered)
}

/// Puts the file offset of standard input where the last byte sed used ends, as GNU sed
/// does when it quits early, so that the next process reads what sed left. It does nothing
/// for pipes and terminals.
pub fn restore_stdin_offset() {
    STDIN.with(|cell| {
        if let Some(reader) = cell.borrow_mut().as_mut() {
            let _ = reader.seek(SeekFrom::Current(0));
        }
    });
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
    Reader(Box<dyn BufRead>),
    Stdin,
}

pub struct FileInputs {
    names: std::vec::IntoIter<String>,
    current: Option<(Source, Rc<str>)>,
    /// A record read ahead of time, and whether it starts a new file.
    peeked: Option<Fetched>,
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
            if self.in_place {
                if self.follow_symlinks {
                    // Follow the chain one link at a time, as GNU sed does; a link that cannot be
                    // read (a missing target, for example) stops the run.
                    let mut current = std::path::PathBuf::from(&name);
                    for _ in 0..40 {
                        match std::fs::read_link(&current) {
                            Ok(target) => {
                                current = if target.is_absolute() {
                                    target
                                } else {
                                    current.parent().unwrap_or_else(|| std::path::Path::new("")).join(target)
                                };
                            }
                            Err(e) if e.raw_os_error() == Some(22) => break,
                            Err(e) => {
                                let shown = current.display().to_string();
                                self.fatal = Some(Fatal::new(format!("couldn't readlink {}: {}", shown, strerror(&e)), 4));
                                return false;
                            }
                        }
                    }
                }
                if let Ok(m) = std::fs::metadata(&name) {
                    if !m.is_file() {
                        self.fatal = Some(Fatal::new(format!("couldn't edit {}: not a regular file", name), 4));
                        return false;
                    }
                }
            }
            match File::open(&name) {
                Ok(f) => {
                    self.current = Some((Source::Reader(Box::new(BufReader::new(f))), Rc::from(name.as_str())));
                    self.started_file = false;
                    return true;
                }
                Err(e) => {
                    eprintln!("{}: can't read {}: {}", self.program, name, strerror(&e));
                    self.had_open_error = true;
                }
            }
        }
        false
    }

    fn fill(&mut self) -> Result<(), Fatal> {
        while self.peeked.is_none() {
            if self.current.is_none() && !self.open_next() {
                return match self.fatal.take() {
                    Some(f) => Err(f),
                    None => Ok(()),
                };
            }
            let (source, name) = self.current.as_mut().unwrap();
            let read = match source {
                Source::Reader(reader) => read_record(reader, self.separator),
                Source::Stdin => stdin_record(self.separator, self.unbuffered),
            };
            match read {
                Ok(Some(record)) => {
                    let new_file = !self.started_file;
                    self.started_file = true;
                    self.peeked = Some(Fetched { record, new_file, file_name: name.clone() });
                }
                Ok(None) => self.current = None,
                Err(e) => {
                    eprintln!("{}: read error on {}: {}", self.program, name, strerror(&e));
                    self.had_read_error = true;
                    self.current = None;
                }
            }
        }
        Ok(())
    }
}

impl RecordSource for FileInputs {
    fn next_record(&mut self) -> Result<Option<Fetched>, Fatal> {
        self.fill()?;
        Ok(self.peeked.take())
    }

    fn is_last(&mut self, separate: bool) -> Result<bool, Fatal> {
        self.fill()?;
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
    inner: Box<dyn Write>,
    separator: u8,
    missing_separator: bool,
    pub name: String,
}

impl Output {
    pub fn new(inner: Box<dyn Write>, separator: u8, name: &str) -> Self {
        Output { inner, separator, missing_separator: false, name: name.to_string() }
    }

    fn fix_missing(&mut self) -> io::Result<()> {
        if self.missing_separator {
            self.inner.write_all(&[self.separator])?;
            self.missing_separator = false;
        }
        Ok(())
    }

    pub fn write_record(&mut self, body: &[u8], terminated: bool) -> io::Result<()> {
        self.fix_missing()?;
        self.inner.write_all(body)?;
        if terminated {
            self.inner.write_all(&[self.separator])?;
        } else {
            self.missing_separator = true;
        }
        Ok(())
    }

    /// Writes bytes as they are (text of `a`, `i`, `c`, file contents), after
    /// restoring a missing terminator.
    pub fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.fix_missing()?;
        self.inner.write_all(bytes)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Every output the script writes to: standard output, standard error, and the
/// files named by `w`, `W`, and `s///w` (created or truncated before the first
/// input line is read, and shared by all commands that name the same file).
pub struct Outputs {
    /// The real standard output (`w /dev/stdout` always writes here).
    pub stdout: Output,
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
        Outputs {
            stdout: Output::new(stdout, separator, "stdout"),
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
            if let Ok(resolved) = std::fs::canonicalize(&original) {
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

    /// Finishes the file being edited: flushes the temporary file, keeps a backup
    /// when `backup_suffix` is given, and moves the temporary file over the original.
    pub fn finish_in_place(&mut self, backup_suffix: Option<&str>) -> Result<(), Fatal> {
        let Some(mut file) = self.in_place.take() else { return Ok(()) };
        file.output.flush().map_err(|e| write_error(&file.output.name, &e))?;
        drop(file.output);
        if let Ok(meta) = std::fs::metadata(&file.original) {
            let _ = std::fs::set_permissions(&file.temp, meta.permissions());
        }
        let rename_error = |name: &std::path::Path, e: io::Error| {
            Fatal::new(format!("cannot rename {}: {}", name.display(), strerror(&e)), 4)
        };
        if let Some(suffix) = backup_suffix.filter(|s| !s.is_empty()) {
            let base = file.original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let backup_name = if suffix.contains('*') { suffix.replace('*', &base) } else { format!("{base}{suffix}") };
            let backup = if backup_name.starts_with('/') {
                std::path::PathBuf::from(backup_name)
            } else {
                file.original.parent().unwrap_or_else(|| std::path::Path::new("")).join(backup_name)
            };
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
                match File::create(&path) {
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
            b"/dev/stdout" => &mut self.stdout,
            b"/dev/stderr" => self.stderr.as_mut().expect("opened before use"),
            _ => self.files.get_mut(name).expect("opened before use"),
        }
    }

    pub fn flush_all(&mut self) -> Result<(), Fatal> {
        let mut result = self.stdout.flush().map_err(|e| flush_error("stdout", &e));
        if let Some(f) = self.in_place.as_mut() {
            let _ = f.output.flush();
        }
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

    #[test]
    fn lookahead_knows_the_last_record() {
        let mut src = ReaderSource::new(&b"a\nb\n"[..], b'\n');
        assert!(src.next_record().unwrap().is_some());
        assert!(!src.is_last(false).unwrap());
        assert!(src.next_record().unwrap().is_some());
        assert!(src.is_last(false).unwrap());
    }
}

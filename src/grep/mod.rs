//! The grep mode: GNU grep's command line, patterns and output, on this program's regular
//! expression engine. It runs when the program is named `grep`, `egrep`, `fgrep` or `rgrep`,
//! or when the first argument is `--grep`.

mod color;
mod glob;
mod matcher;
mod options;
mod search;

use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use color::Colors;
use glob::Skip;
use matcher::Matcher;
pub use options::Kind;
use options::{Devices, Directories, ListFiles, Options, Parsed};
use search::{Event, Printer};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};

/// The release of GNU grep that the grep mode follows where releases differ: `false` for 3.11 (the
/// grep of Ubuntu 24.04, what CI has), `true` for 3.12 (Ubuntu 26.04). Four things depend on it:
///
/// - `-u` and `--unix-byte-offsets`: a warning in 3.11, an invalid option in 3.12;
/// - `-P` in a UTF-8 locale: `\w`, `[[:alpha:]]`, `\b` see ASCII only in 3.11, Unicode in 3.12
///   (`PCRE2_UCP`);
/// - `-q` with `-m N`: 3.11 exits at the first selected line (status 0 even after an error), 3.12
///   goes on, so a later error makes the status 2;
/// - `-l` and `-L` with `-m N`: 3.12 reads the whole input and prints `binary file matches` for a
///   binary file, 3.11 stops at the first selected line and prints nothing.
///
/// To follow 3.12: set this to `true`, run `tools/gen-fixtures.sh` on a machine with 3.12 and PCRE2
/// 10.46, and put back the commands that `tools/gen-grep-fixtures.py` keeps in `ONLY_312`
/// (`GREP_FIXTURES=312 tools/gen-grep-fixtures.py`). docs/GREP.md has the list.
pub const GNU_GREP_3_12: bool = false;

/// The kind of patterns a program name stands for.
pub fn kind_for_name(name: &str) -> Option<Kind> {
    match name {
        "grep" | "rgrep" => Some(Kind::Basic),
        "egrep" => Some(Kind::Extended),
        "fgrep" => Some(Kind::Fixed),
        _ => None,
    }
}

/// What an operand turned out to be.
struct Run<'a> {
    options: &'a Options,
    program: &'a str,
    matcher: Matcher,
    printer: Printer<'a>,
    files: Skip,
    dirs: Skip,
    /// An error was reported: the status is 2.
    error_seen: bool,
    /// A match the library of `-P` gave up on: the run ends with status 2 (in a worker, the merge says so).
    fatal: bool,
    /// Something was selected.
    matched: bool,
    stdin_name: Vec<u8>,
    /// `-r` without operands: names are printed without the leading `./`.
    omit_dot_slash: bool,
    /// The device and i-node of standard output, if it is a regular file.
    stdout_id: Option<(u64, u64)>,
    /// The directories being read, for the loops that `-R` can make.
    dir_stack: Vec<(u64, u64)>,
    /// Searches are handed to other threads, and what they print is put in order afterwards.
    pool: Option<Pool>,
    /// This is one of those other threads: it does not end the process at the first selected line
    /// (the thread that puts the results in order does that, in the order of the files).
    worker: bool,
}

/// A file to search, in the order the walk met it.
struct Job {
    seq: u64,
    path: PathBuf,
    name: Vec<u8>,
    in_directory: bool,
}

/// What searching one file (or one message of the walk) printed, with the numbers of its place.
struct Done {
    seq: u64,
    events: Vec<Event>,
    matched: bool,
    error: bool,
    /// The search of this file ended the run (see `search::MatchFailure`).
    fatal: bool,
}

/// The walking thread's side of the search by several threads.
struct Pool {
    jobs: SyncSender<Job>,
    done: Sender<Done>,
    next_seq: u64,
}

pub fn main(program: &str, args: Vec<Vec<u8>>, kind: Kind) -> i32 {
    let options = match options::parse(args, program, kind) {
        Parsed::Run(o) => *o,
        Parsed::Print(text, status) => {
            let _ = io::stdout().write_all(text.as_bytes());
            return status;
        }
        Parsed::Error(status) => return status,
    };
    let locale = crate::locale::from_env();
    // No pattern at all (`-f /dev/null`) matches nothing: it is the empty pattern, inverted.
    let mut options = options;
    if options.patterns.is_empty() {
        options.invert = !options.invert;
        options.line = false;
        options.word = false;
        options.patterns = vec![Vec::new()];
    }
    let matcher = match Matcher::new(&options, locale) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{program}: {}", e.0);
            return 2;
        }
    };
    for w in &matcher.warnings {
        eprintln!("{program}: warning: {w}");
    }
    // Nothing can be selected: no line may be (`-m 0`), or `-v` with the one empty pattern, which
    // every line matches. Only `-L` still has something to say.
    let nothing_selected = options.max_count == Some(0)
        || (options.invert && !options.line && !options.word && options.patterns.len() == 1 && options.patterns[0].is_empty());
    if nothing_selected && options.list_files != ListFiles::NonMatching {
        return 1;
    }

    let colors = if options.color { Colors::from_env(program) } else { Colors::default() };

    let mut operands: Vec<Vec<u8>> = options.files.clone();
    let mut omit_dot_slash = false;
    if operands.is_empty() {
        if options.directories == Directories::Recurse {
            operands.push(b".".to_vec());
            omit_dot_slash = true;
        } else {
            operands.push(b"-".to_vec());
        }
    }
    let jobs = search_threads(&options, &operands);
    // The printer of the walk: it writes (one thread), or keeps what it would write, in order,
    // for the thread that puts the results of the others in order.
    let mut printer = if jobs >= 2 {
        Printer::capturing(&options, colors.clone(), locale)
    } else {
        Printer::new(&options, colors.clone(), locale, Box::new(io::stdout()))
    };
    configure(&mut printer, &options, operands.len());

    let mut files = Skip::default();
    let mut dirs = Skip::default();
    for (include, p) in &options.file_patterns {
        files.push(*include, p.clone());
    }
    for p in &options.exclude_dir {
        dirs.push(false, p.clone());
    }
    let mut run = Run {
        options: &options,
        program,
        matcher,
        printer,
        files,
        dirs,
        error_seen: false,
        fatal: false,
        matched: false,
        stdin_name: options.label.clone().unwrap_or_else(|| b"(standard input)".to_vec()),
        omit_dot_slash,
        stdout_id: stdout_identity(),
        dir_stack: Vec::new(),
        pool: None,
        worker: false,
    };
    if jobs >= 2 {
        return run_in_threads(run, &operands, jobs, program, locale, &colors);
    }
    for operand in &operands {
        if run.operand(operand).is_err() {
            // A write error: nothing more can be said on standard output.
            return 2;
        }
    }
    if run.printer.flush().is_err() {
        eprintln!("{program}: write error: {}", "Broken pipe");
        return 2;
    }
    if run.error_seen {
        2
    } else if run.matched {
        0
    } else {
        1
    }
}

/// Names are printed in front of the lines when there are several files, or with -H; with -r and
/// one operand, for the files found inside directories.
fn configure(printer: &mut Printer<'_>, options: &Options, operands: usize) {
    printer.with_filename = match options.with_filename {
        Some(w) => w,
        None => operands > 1,
    };
    printer.filenames_in_directories = options.with_filename.is_none() && operands <= 1 && options.directories == Directories::Recurse;
    printer.default_with_filename = printer.with_filename;
}

/// How many threads search the files: the files of a directory, or several files, are searched
/// side by side and what they print is put in order, so that the output is the one of a single
/// thread. Not with context (the separator between groups depends on the files before),
/// `--line-buffered`, or standard input among the operands. `RIPGSED_JOBS=1` is one thread.
fn search_threads(options: &Options, operands: &[Vec<u8>]) -> usize {
    let forced: Option<usize> = std::env::var("RIPGSED_JOBS").ok().and_then(|v| v.parse().ok());
    if forced.is_some_and(|n| n < 2) {
        return 1;
    }
    let several = operands.len() > 1 || options.directories == Directories::Recurse;
    if !several || options.before.is_some() || options.after.is_some() || options.line_buffered || operands.iter().any(|o| o == b"-") {
        return 1;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    forced.unwrap_or(if cores >= 2 { cores.min(16) } else { 1 })
}

/// The walk on this thread, the searches on `jobs` others, and what they print put in order on
/// one more.
fn run_in_threads(mut run: Run<'_>, operands: &[Vec<u8>], jobs: usize, program: &str, locale: crate::regex::Locale, colors: &Colors) -> i32 {
    let options = run.options;
    let exit_on_match = run.printer.exit_on_match;
    let with_filename = (run.printer.with_filename, run.printer.filenames_in_directories, run.printer.default_with_filename);
    let stdout_id = run.stdout_id;
    let (job_tx, job_rx) = sync_channel::<Job>(jobs * 4);
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (done_tx, done_rx) = channel::<Done>();
    let mut merged = (false, false);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let job_rx = Arc::clone(&job_rx);
            let done_tx = done_tx.clone();
            let colors = colors.clone();
            scope.spawn(move || {
                // Each thread has its own compiled patterns (they are not shared).
                let Ok(matcher) = Matcher::new(options, locale) else { return };
                let mut printer = Printer::capturing(options, colors, locale);
                printer.with_filename = with_filename.0;
                printer.filenames_in_directories = with_filename.1;
                printer.default_with_filename = with_filename.2;
                let mut worker = Run {
                    options,
                    program,
                    matcher,
                    printer,
                    files: Skip::default(),
                    dirs: Skip::default(),
                    error_seen: false,
                    fatal: false,
                    matched: false,
                    stdin_name: Vec::new(),
                    omit_dot_slash: false,
                    stdout_id,
                    dir_stack: Vec::new(),
                    pool: None,
                    worker: true,
                };
                loop {
                    let job = job_rx.lock().expect("job queue").recv();
                    let Ok(job) = job else { return };
                    worker.error_seen = false;
                    worker.fatal = false;
                    worker.matched = false;
                    let _ = worker.file(&job.path, &job.name, job.in_directory);
                    let done = Done { seq: job.seq, events: worker.printer.take_events(), matched: worker.matched, error: worker.error_seen, fatal: worker.fatal };
                    if done_tx.send(done).is_err() {
                        return;
                    }
                }
            });
        }
        let merger = scope.spawn(move || merge_results(done_rx, exit_on_match));
        run.pool = Some(Pool { jobs: job_tx, done: done_tx, next_seq: 0 });
        for operand in operands {
            if run.operand(operand).is_err() {
                break;
            }
        }
        run.flush_walk();
        // The walk is over: closing the queues lets the threads end.
        run.pool = None;
        merged = merger.join().unwrap_or((false, true));
    });
    if run.error_seen || merged.1 {
        2
    } else if merged.0 {
        0
    } else {
        1
    }
}

/// Writes the results of the searches in the order of the files, standard output and standard error
/// as they were interleaved. With `-q` the first result that selected a line ends the process.
/// Returns whether a line was selected and whether an error was reported.
fn merge_results(done: Receiver<Done>, exit_on_match: bool) -> (bool, bool) {
    let mut merger = crate::stream::OrderedMerger::default();
    let mut out = io::BufWriter::with_capacity(1 << 16, io::stdout());
    let (mut matched, mut error) = (false, false);
    for item in done.iter() {
        merger.push(item.seq, item);
        while let Some(item) = merger.next_ready() {
            for event in item.events {
                match event {
                    Event::Out(bytes) => {
                        if out.write_all(&bytes).is_err() {
                            eprintln!("grep: write error: Broken pipe");
                            std::process::exit(2);
                        }
                    }
                    Event::Err(bytes) => {
                        let _ = out.flush();
                        let _ = io::stderr().lock().write_all(&bytes);
                    }
                }
            }
            if item.fatal {
                let _ = out.flush();
                std::process::exit(2);
            }
            matched |= item.matched;
            error |= item.error;
            if item.matched && exit_on_match {
                let _ = out.flush();
                std::process::exit(0);
            }
        }
    }
    if out.flush().is_err() {
        eprintln!("grep: write error: Broken pipe");
        std::process::exit(2);
    }
    (matched, error)
}

/// The device and i-node of standard output when it is a regular file.
fn stdout_identity() -> Option<(u64, u64)> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    // SAFETY: descriptor 1 belongs to the process; `ManuallyDrop` keeps it open.
    let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
    let meta = file.metadata().ok()?;
    meta.is_file().then(|| (meta.dev(), meta.ino()))
}

impl Run<'_> {
    /// Whether the file with this metadata is the regular file that standard output is: searching
    /// it would read what this run writes. GNU grep skips it when it would print lines (not
    /// with -c, -l, -L or -q) and `-m 1` does not read on.
    fn is_the_output(&self, meta: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        let o = self.options;
        if o.quiet || o.count || o.list_files != ListFiles::No || o.max_count.is_some_and(|m| m <= 1) || !meta.is_file() {
            return false;
        }
        self.stdout_id.is_some_and(|(dev, ino)| dev == meta.dev() && ino == meta.ino())
    }

    /// Reports an error about `name` (`program: name: message`), unless `-s`.
    fn report(&mut self, name: &[u8], message: &str) {
        self.error_seen = true;
        if !self.options.no_messages {
            self.say(name, message.as_bytes());
        }
    }

    /// Writes `program: name: message` on standard error, where it belongs among the output.
    fn say(&mut self, name: &[u8], message: &[u8]) {
        let mut text = Vec::with_capacity(self.program.len() + name.len() + message.len() + 6);
        text.extend_from_slice(self.program.as_bytes());
        text.extend_from_slice(b": ");
        text.extend_from_slice(name);
        text.extend_from_slice(b": ");
        text.extend_from_slice(message);
        text.push(b'\n');
        self.printer.stderr_message(text);
    }

    /// Hands what the walk printed itself (a message, a directory that is read) to the thread that
    /// puts the results in order, as an item of its own.
    fn flush_walk(&mut self) {
        let Some(pool) = &mut self.pool else { return };
        let events = self.printer.take_events();
        if events.is_empty() {
            return;
        }
        let seq = pool.next_seq;
        pool.next_seq += 1;
        let _ = pool.done.send(Done { seq, events, matched: false, error: false, fatal: false });
    }

    /// Searches a file found by the walk: here, or on another thread.
    fn dispatch_file(&mut self, path: &Path, name: &[u8], in_directory: bool) -> io::Result<()> {
        if self.pool.is_none() {
            return self.file(path, name, in_directory);
        }
        self.flush_walk();
        let pool = self.pool.as_mut().expect("checked above");
        let seq = pool.next_seq;
        pool.next_seq += 1;
        let _ = pool.jobs.send(Job { seq, path: path.to_path_buf(), name: name.to_vec(), in_directory });
        Ok(())
    }

    fn operand(&mut self, operand: &[u8]) -> io::Result<()> {
        if operand == b"-" {
            let name = self.stdin_name.clone();
            return self.standard_input(&name);
        }
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(operand));
        // A name on the command line is also checked against `--include` and `--exclude`.
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                self.report(operand, &crate::io::strerror(&e));
                return Ok(());
            }
        };
        if meta.is_dir() {
            if !self.dirs.is_empty() && self.dirs.skips(operand, false) {
                return Ok(());
            }
            match self.options.directories {
                Directories::Skip => return Ok(()),
                Directories::Read => {
                    // The directory is opened and fails when it is read: it has no selected line.
                    self.report(operand, "Is a directory");
                    let found = search::Found { count: 0, binary_matches: false, last_selected_end: None, ended_early: false, max_reached: false };
                    self.printer.with_filename = self.printer.default_with_filename;
                    self.printer.summary(operand, &found)?;
                    return Ok(());
                }
                Directories::Recurse => {
                    if !self.dirs.is_empty() && self.dirs.skips(operand, false) {
                        return Ok(());
                    }
                    // (Trailing slashes are not repeated in the names found below.)
                    let mut display = operand.to_vec();
                    while display.len() > 1 && display.last() == Some(&b'/') {
                        display.pop();
                    }
                    return self.walk(&path, display);
                }
            }
        }
        if self.files.skips(operand, false) {
            return Ok(());
        }
        if !meta.is_file() && self.options.devices == Devices::Skip {
            return Ok(());
        }
        self.dispatch_file(&path, operand, false)
    }

    /// Standard input, read from descriptor 0 as it is (nothing is read ahead of what is searched).
    /// A regular file on it is left just after the last selected line when `-m` and the like
    /// end the search early.
    fn standard_input(&mut self, name: &[u8]) -> io::Result<()> {
        use std::io::{Seek, SeekFrom};
        use std::os::fd::FromRawFd;
        if crate::io::STDIN_WAS_CLOSED.load(std::sync::atomic::Ordering::Relaxed) {
            // (Rust's start-up has put /dev/null in the place of a closed standard input.)
            self.report(name, "Bad file descriptor");
            let found = search::Found { count: 0, binary_matches: false, last_selected_end: None, ended_early: false, max_reached: false };
            self.printer.with_filename = self.printer.default_with_filename;
            return self.printer.summary(name, &found);
        }
        // SAFETY: descriptor 0 belongs to the process; `ManuallyDrop` keeps it open.
        let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
        let meta = file.metadata().ok();
        let regular = meta.as_ref().is_some_and(|m| m.is_file());
        if meta.as_ref().is_some_and(|m| self.is_the_output(m)) {
            self.report(name, "input file is also the output");
            return Ok(());
        }
        let start = if regular { file.stream_position().ok() } else { None };
        let size = meta.filter(|m| m.is_file()).map(|m| m.len().saturating_sub(start.unwrap_or(0)));
        let found = self.search_reader(name, &mut *file, regular, size, false)?;
        if let (Some(start), Some(found)) = (start, found.as_ref()) {
            // The input is left where GNU grep leaves it: just after the last selected line when
            // `-m` ended the search, else where its last buffer ended.
            if found.ended_early {
                let target = if found.max_reached {
                    found.last_selected_end
                } else {
                    found.last_selected_end.map(|end| (((end.saturating_sub(1)) / 98304 + 1) * 98304).min(size.unwrap_or(u64::MAX)))
                };
                if let Some(t) = target {
                    let _ = file.seek(SeekFrom::Start(start + t));
                }
            }
        }
        if found.as_ref().is_some_and(|f| f.count > 0) && self.printer.exit_on_match {
            self.printer.flush()?;
            std::process::exit(0);
        }
        Ok(())
    }

    /// Searches the files of a directory (recursively), naming them by joining the names.
    fn walk(&mut self, dir: &Path, display: Vec<u8>) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        // A directory that is one of those being read (through a symbolic link) is a loop.
        let id = std::fs::metadata(dir).ok().map(|m| (m.dev(), m.ino()));
        if let Some(id) = id {
            if self.dir_stack.contains(&id) {
                if !self.options.no_messages {
                    self.say(&display, b"warning: recursive directory loop");
                }
                self.flush_walk();
                return Ok(());
            }
            self.dir_stack.push(id);
        }
        let result = self.walk_entries(dir, display);
        if id.is_some() {
            self.dir_stack.pop();
        }
        result
    }

    fn walk_entries(&mut self, dir: &Path, display: Vec<u8>) -> io::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                self.report(&display, &crate::io::strerror(&e));
                return Ok(());
            }
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let base = entry.file_name();
            let base_bytes = base.as_bytes();
            let path = entry.path();
            let mut shown = display.clone();
            if !(shown.is_empty() || shown.last() == Some(&b'/')) {
                shown.push(b'/');
            }
            if self.omit_dot_slash && shown == b"./" {
                shown.clear();
            }
            shown.extend_from_slice(base_bytes);
            let Ok(link_meta) = std::fs::symlink_metadata(&path) else { continue };
            let meta = if link_meta.file_type().is_symlink() {
                if !self.options.dereference_recursive {
                    continue;
                }
                match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        self.report(&shown, &crate::io::strerror(&e));
                        continue;
                    }
                }
            } else {
                link_meta
            };
            if meta.is_dir() {
                if !self.dirs.is_empty() && self.dirs.skips(base_bytes, true) {
                    continue;
                }
                self.walk(&path, shown)?;
            } else {
                if self.files.skips(base_bytes, true) {
                    continue;
                }
                if !meta.is_file() && self.options.devices != Devices::Read {
                    continue;
                }
                self.dispatch_file(&path, &shown, true)?;
            }
        }
        Ok(())
    }

    fn file(&mut self, path: &Path, name: &[u8], in_directory: bool) -> io::Result<()> {
        let mut file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                // (A file that cannot be opened is not searched: no count, no listing.)
                self.report(name, &crate::io::strerror(&e));
                return Ok(());
            }
        };
        let meta = file.metadata().ok();
        let regular = meta.as_ref().is_some_and(|m| m.is_file());
        if meta.as_ref().is_some_and(|m| self.is_the_output(m)) {
            self.report(name, "input file is also the output");
            return Ok(());
        }
        let size = meta.filter(|m| m.is_file()).map(|m| m.len());
        let found = self.search_reader(name, &mut file, regular, size, in_directory)?;
        if found.is_some_and(|f| f.count > 0) && self.printer.exit_on_match && !self.worker {
            self.printer.flush()?;
            std::process::exit(0);
        }
        Ok(())
    }

    /// Searches an input. `None` when it could not be read at all.
    fn search_reader(&mut self, name: &[u8], input: &mut dyn Read, regular: bool, size: Option<u64>, in_directory: bool) -> io::Result<Option<search::Found>> {
        // With -r and one operand, the files found in directories carry their names.
        self.printer.with_filename = self.printer.default_with_filename || (in_directory && self.printer.filenames_in_directories);
        // With -T the numbers are as wide as the largest offset the input can have.
        self.printer.offset_width = if self.options.initial_tab {
            match size {
                Some(s) => (s + 1).to_string().len(),
                None => 19,
            }
        } else {
            0
        };
        let found = match self.printer.search_stream(&self.matcher, name, input, regular) {
            Ok(f) => f,
            Err(e) if search::MatchFailure::message(&e).is_some() => {
                // (No summary: GNU grep does not print the count of the file it gave up on.)
                let message = search::MatchFailure::message(&e).unwrap_or_default();
                // (Not `report`: -s does not silence this error, as in GNU grep.)
                self.error_seen = true;
                self.say(name, message.as_bytes());
                self.fatal = true;
                if !self.worker {
                    self.printer.flush()?;
                    std::process::exit(2);
                }
                return Ok(None);
            }
            Err(e) if self.printer.is_read_error(&e) => {
                self.report(name, &crate::io::strerror(&e));
                let found = search::Found { count: 0, binary_matches: false, last_selected_end: None, ended_early: false, max_reached: false };
                self.printer.summary(name, &found)?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        self.printer.summary(name, &found)?;
        if found.binary_matches {
            self.say(name, b"binary file matches");
        }
        if found.count > 0 {
            self.matched = true;
        }
        Ok(Some(found))
    }
}

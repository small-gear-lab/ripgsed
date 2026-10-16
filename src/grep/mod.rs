//! The grep mode: GNU grep's command line, patterns and output, on this program's regular
//! expression engine. It runs when the program is named `grep`, `egrep`, `fgrep` or `rgrep`,
//! or when the first argument is `--grep`.

mod color;
mod glob;
mod matcher;
mod options;
mod pcre;
mod search;

use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use color::Colors;
use glob::Skip;
use matcher::Matcher;
pub use options::Kind;
use options::{Devices, Directories, ListFiles, Options, Parsed};
use search::Printer;

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
    /// Something was selected.
    matched: bool,
    stdin_name: Vec<u8>,
    /// `-r` without operands: names are printed without the leading `./`.
    omit_dot_slash: bool,
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
    if std::env::var_os("POSIXLY_CORRECT").is_none() {
        for w in &matcher.warnings {
            eprintln!("{program}: warning: {w}");
        }
    }
    // Nothing can be selected: no line may be (`-m 0`), or `-v` with the one empty pattern, which
    // every line matches. Only `-L` still has something to say.
    let nothing_selected = options.max_count == Some(0)
        || (options.invert && !options.line && !options.word && options.patterns.len() == 1 && options.patterns[0].is_empty());
    if nothing_selected && options.list_files != ListFiles::NonMatching {
        return 1;
    }

    let stdout = io::stdout();
    let sink: Box<dyn Write> = Box::new(stdout);
    let colors = if options.color { Colors::from_env(program) } else { Colors::default() };
    let mut printer = Printer::new(&options, colors, locale, sink);

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
    // Names are printed in front of the lines when there are several files, or with -H; with -r
    // and one operand, for the files found inside directories.
    printer.with_filename = match options.with_filename {
        Some(w) => w,
        None => operands.len() > 1,
    };
    printer.filenames_in_directories = options.with_filename.is_none() && operands.len() <= 1 && options.directories == Directories::Recurse;
    printer.default_with_filename = printer.with_filename;

    let mut files = Skip::default();
    let mut dirs = Skip::default();
    let _ = &mut files;
    for (include, p) in &options.file_patterns {
        files.push(*include, p.clone());
    }
    for p in &options.exclude_dir {
        dirs.push(false, p.clone());
    }
    let _ = &mut dirs;
    let mut run = Run {
        options: &options,
        program,
        matcher,
        printer,
        files,
        dirs,
        error_seen: false,
        matched: false,
        stdin_name: options.label.clone().unwrap_or_else(|| b"(standard input)".to_vec()),
        omit_dot_slash,
    };
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

impl Run<'_> {
    /// Reports an error about `name` (`program: name: message`), unless `-s`.
    fn report(&mut self, name: &[u8], message: &str) {
        self.error_seen = true;
        if !self.options.no_messages {
            let _ = self.printer.flush();
            let mut err = io::stderr().lock();
            let _ = err.write_all(self.program.as_bytes());
            let _ = err.write_all(b": ");
            let _ = err.write_all(name);
            let _ = err.write_all(b": ");
            let _ = err.write_all(message.as_bytes());
            let _ = err.write_all(b"\n");
        }
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
        self.file(&path, operand, false)
    }

    /// Standard input, read from descriptor 0 as it is (nothing is read ahead of what is searched).
    /// A regular file on it is left just after the last selected line when `-m` and the like
    /// end the search early.
    fn standard_input(&mut self, name: &[u8]) -> io::Result<()> {
        use std::io::{Seek, SeekFrom};
        use std::os::fd::FromRawFd;
        // SAFETY: descriptor 0 belongs to the process; `ManuallyDrop` keeps it open.
        let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
        let meta = file.metadata().ok();
        let regular = meta.as_ref().is_some_and(|m| m.is_file());
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
                self.file(&path, &shown, true)?;
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
        let size = meta.filter(|m| m.is_file()).map(|m| m.len());
        let found = self.search_reader(name, &mut file, regular, size, in_directory)?;
        if found.is_some_and(|f| f.count > 0) && self.printer.exit_on_match {
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
            let _ = self.printer.flush();
            let mut err = io::stderr().lock();
            let _ = err.write_all(self.program.as_bytes());
            let _ = err.write_all(b": ");
            let _ = err.write_all(name);
            let _ = err.write_all(b": binary file matches\n");
        }
        if found.count > 0 {
            self.matched = true;
        }
        Ok(Some(found))
    }
}

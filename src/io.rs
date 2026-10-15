//! Record reading and output. No script syntax handling.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::rc::Rc;

use crate::Fatal;

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
pub struct FileInputs {
    names: std::vec::IntoIter<String>,
    current: Option<(Box<dyn BufRead>, Rc<str>)>,
    /// A record read ahead of time, and whether it starts a new file.
    peeked: Option<Fetched>,
    started_file: bool,
    separator: u8,
    program: String,
    pub had_open_error: bool,
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
        }
    }

    fn open_next(&mut self) -> bool {
        for name in self.names.by_ref() {
            if name == "-" {
                self.current = Some((Box::new(BufReader::new(io::stdin())), Rc::from("-")));
                self.started_file = false;
                return true;
            }
            match File::open(&name) {
                Ok(f) => {
                    self.current = Some((Box::new(BufReader::new(f)), Rc::from(name.as_str())));
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
                return Ok(());
            }
            let (reader, name) = self.current.as_mut().unwrap();
            match read_record(reader, self.separator) {
                Ok(Some(record)) => {
                    let new_file = !self.started_file;
                    self.started_file = true;
                    self.peeked = Some(Fetched { record, new_file, file_name: name.clone() });
                }
                Ok(None) => self.current = None,
                Err(e) => {
                    let message = format!("read error on {}: {}", name, strerror(&e));
                    return Err(Fatal::new(message, 4));
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
    pub stdout: Output,
    stderr: Option<Output>,
    files: HashMap<Vec<u8>, Output>,
    separator: u8,
}

impl Outputs {
    pub fn new(stdout: Box<dyn Write>, separator: u8) -> Self {
        Outputs { stdout: Output::new(stdout, separator, "stdout"), stderr: None, files: HashMap::new(), separator }
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
        let mut result = self.stdout.flush().map_err(|e| write_error("stdout", &e));
        if let Some(s) = self.stderr.as_mut() {
            let _ = s.flush();
        }
        for out in self.files.values_mut() {
            if let Err(e) = out.flush() {
                if result.is_ok() {
                    result = Err(write_error(&out.name, &e));
                }
            }
        }
        result
    }
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

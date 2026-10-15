//! Record reading and output. No script syntax handling.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

/// One input record: the body without its terminator, and whether the
/// terminator was present. A missing final terminator is preserved.
#[derive(Debug, PartialEq, Eq)]
pub struct Record {
    pub body: Vec<u8>,
    pub terminated: bool,
}

/// Reads LF-terminated records from `reader`. Returns `None` at end of input.
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

/// A source of input records.
pub trait RecordSource {
    fn next_record(&mut self) -> io::Result<Option<Record>>;
}

/// A single reader as a record source.
pub struct ReaderSource<R: BufRead> {
    reader: R,
    separator: u8,
}

impl<R: BufRead> ReaderSource<R> {
    pub fn new(reader: R, separator: u8) -> Self {
        ReaderSource { reader, separator }
    }
}

impl<R: BufRead> RecordSource for ReaderSource<R> {
    fn next_record(&mut self) -> io::Result<Option<Record>> {
        read_record(&mut self.reader, self.separator)
    }
}

/// The input files of a run, read as one continuous stream of records.
///
/// A file that cannot be opened is reported as `PROGRAM: can't read NAME: reason`
/// and skipped; reading continues with the next file and `had_open_error` is set
/// (exit status 2). A missing final terminator ends a record, so it never joins
/// the first record of the next file.
pub struct FileInputs {
    names: std::vec::IntoIter<String>,
    current: Option<Box<dyn BufRead>>,
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
            separator,
            program: program.to_string(),
            had_open_error: false,
        }
    }

    fn open_next(&mut self) -> bool {
        for name in self.names.by_ref() {
            if name == "-" {
                self.current = Some(Box::new(BufReader::new(io::stdin())));
                return true;
            }
            match File::open(&name) {
                Ok(f) => {
                    self.current = Some(Box::new(BufReader::new(f)));
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
}

impl RecordSource for FileInputs {
    fn next_record(&mut self) -> io::Result<Option<Record>> {
        loop {
            if self.current.is_none() && !self.open_next() {
                return Ok(None);
            }
            let reader = self.current.as_mut().unwrap();
            match read_record(reader, self.separator)? {
                Some(record) => return Ok(Some(record)),
                None => self.current = None,
            }
        }
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
pub struct Output<W: Write> {
    inner: W,
    separator: u8,
    missing_separator: bool,
}

impl<W: Write> Output<W> {
    pub fn new(inner: W, separator: u8) -> Self {
        Output { inner, separator, missing_separator: false }
    }

    pub fn write_record(&mut self, body: &[u8], terminated: bool) -> io::Result<()> {
        if self.missing_separator {
            self.inner.write_all(&[self.separator])?;
            self.missing_separator = false;
        }
        self.inner.write_all(body)?;
        if terminated {
            self.inner.write_all(&[self.separator])?;
        } else {
            self.missing_separator = true;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
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
    fn adds_the_missing_terminator_when_more_follows() {
        let mut out = Output::new(Vec::new(), b'\n');
        out.write_record(b"x", false).unwrap();
        out.write_record(b"y", false).unwrap();
        assert_eq!(out.inner, b"x\ny");
    }
}

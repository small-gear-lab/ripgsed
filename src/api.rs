//! The library interface: [`Script`], [`Run`], [`Outcome`] and [`Error`].

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::str::FromStr;
use std::sync::OnceLock;

use crate::Fatal;
use crate::io::{Outputs, ReaderSource};
use crate::regex::Locale;
use crate::runtime::{self, Options};
use crate::syntax::{self, Chunk, ChunkSource, ParseOptions};

/// What went wrong: a script that does not parse, a mistake in how a script was built, or a
/// failure while running (reading, writing, a command that cannot run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
    status: i32,
    kind: ErrorKind,
}

/// The part of the work that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The script is not valid sed (the message is the one GNU sed gives).
    Syntax,
    /// The builder was used wrongly (a flag without `substitute`, mixed pattern syntaxes).
    Build,
    /// Running the script failed.
    Run,
}

impl Error {
    pub(crate) fn build(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
            status: 1,
            kind: ErrorKind::Build,
        }
    }

    /// The message, without the program name.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The exit status `sed` would end with.
    pub fn exit_status(&self) -> i32 {
        self.status
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<syntax::SyntaxError> for Error {
    fn from(e: syntax::SyntaxError) -> Error {
        Error {
            message: e.to_string(),
            status: e.status,
            kind: ErrorKind::Syntax,
        }
    }
}

impl From<Fatal> for Error {
    fn from(e: Fatal) -> Error {
        Error {
            message: e.message,
            status: e.status,
            kind: ErrorKind::Run,
        }
    }
}

/// A script: sed commands that can be run on any number of inputs.
///
/// Write it as sed text with [`Script::parse`], or build it with [`Edit`](crate::Edit). Both
/// give the same thing. `to_string()` is the script as sed text, which [`Script::parse`] turns
/// back into an equal script.
///
/// The text is checked when the script is made (syntax, the number of addresses a command
/// takes, labels), so a `Script` that exists can be run. Running it parses it again with the
/// settings of the [`Run`]; the cost is the order of 25 microseconds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Script {
    text: String,
    dialect: Dialect,
}

/// The syntax of the regular expressions of a script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    /// Basic regular expressions, as in sed.
    #[default]
    Basic,
    /// Extended regular expressions (`sed -E`).
    Extended,
    /// Perl-style regular expressions from the PCRE2 library (`sed -P`).
    Perl,
}

impl Script {
    /// A script in sed's own syntax, with basic regular expressions.
    pub fn parse(text: &str) -> Result<Script, Error> {
        Script::from_text(text.to_string(), Dialect::Basic)
    }

    /// A script whose regular expressions are extended (as with `sed -E`).
    pub fn parse_extended(text: &str) -> Result<Script, Error> {
        Script::from_text(text.to_string(), Dialect::Extended)
    }

    /// A script whose regular expressions are Perl-style (as with `sed -P`). They are read by the
    /// PCRE2 library.
    pub fn parse_perl(text: &str) -> Result<Script, Error> {
        Script::from_text(text.to_string(), Dialect::Perl)
    }

    pub(crate) fn from_text(text: String, dialect: Dialect) -> Result<Script, Error> {
        let script = Script { text, dialect };
        script.parsed(&Run::new().parse_options())?;
        Ok(script)
    }

    /// The syntax of the regular expressions.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    pub(crate) fn parsed(&self, options: &ParseOptions) -> Result<syntax::Script, Error> {
        Ok(syntax::parse(&self.chunks(options))?)
    }

    pub(crate) fn chunks(&self, options: &ParseOptions) -> Vec<(Chunk, ParseOptions)> {
        let options = ParseOptions {
            extended: self.dialect == Dialect::Extended,
            pcre: self.dialect == Dialect::Perl,
            ..*options
        };
        vec![(
            Chunk {
                source: ChunkSource::Expression(1),
                text: self.text.clone().into_bytes(),
            },
            options,
        )]
    }
}

impl fmt::Display for Script {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl FromStr for Script {
    type Err = Error;

    fn from_str(text: &str) -> Result<Script, Error> {
        Script::parse(text)
    }
}

/// How the text is read: as bytes, as UTF-8 characters, or as the locale of the process says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Encoding {
    /// The locale of the environment (`LC_ALL`, `LC_CTYPE`, `LANG`), as `sed` reads it. It is
    /// looked up once per process.
    #[default]
    Environment,
    /// Bytes: the locale `C`. Every byte is a character, whatever the environment says.
    Bytes,
    /// UTF-8 characters.
    Utf8,
}

/// The result of a run that ended without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// The exit status the script asked for with `q` or `Q`; 0 if it ran to the end.
    pub exit_status: i32,
}

/// How a script is run: the settings of `sed`'s options.
///
/// ```
/// use ripgsed::{Run, Script};
///
/// let script = Script::parse("s/a/b/;2q")?;
/// let out = Run::new().apply(&script, b"a\na\na\n")?;
/// assert_eq!(out, b"b\nb\n");
/// # Ok::<(), ripgsed::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Run {
    quiet: bool,
    null_data: bool,
    line_length: usize,
    encoding: Encoding,
    glibc_compat: bool,
    sandbox: bool,
    jobs: usize,
}

impl Default for Run {
    fn default() -> Self {
        Run::new()
    }
}

/// A run is spread over threads only when there is this much input; less is not worth the threads.
const PARALLEL_MIN_BYTES: usize = 1 << 20;
const PARALLEL_BLOCK: usize = 1 << 20;

fn environment_locale() -> Locale {
    static LOCALE: OnceLock<Locale> = OnceLock::new();
    *LOCALE.get_or_init(crate::locale::from_env)
}

impl Run {
    pub fn new() -> Run {
        Run {
            quiet: false,
            null_data: false,
            line_length: 70,
            encoding: Encoding::Environment,
            glibc_compat: false,
            sandbox: false,
            jobs: 1,
        }
    }

    /// `-n`: do not print the lines at the end of each cycle (only `p` and the like print).
    pub fn quiet(mut self, yes: bool) -> Run {
        self.quiet = yes;
        self
    }

    /// `-z`: lines end with a NUL byte instead of a newline.
    pub fn null_data(mut self, yes: bool) -> Run {
        self.null_data = yes;
        self
    }

    /// `-l N`: where the command `l` wraps its lines (70 by default).
    pub fn line_length(mut self, width: usize) -> Run {
        self.line_length = width;
        self
    }

    pub fn encoding(mut self, encoding: Encoding) -> Run {
        self.encoding = encoding;
        self
    }

    /// `--sandbox`: reject the commands `e`, `r` and `w`.
    pub fn sandbox(mut self, yes: bool) -> Run {
        self.sandbox = yes;
        self
    }

    /// `--glibc-compat`: also reproduce the quirks of the C library's regular expression
    /// matcher that GNU sed shows. By default a pattern means what its definition says.
    ///
    /// The setting is shared by the whole process: it is read by every run, not only this one.
    pub fn glibc_compat(mut self, yes: bool) -> Run {
        self.glibc_compat = yes;
        self
    }

    /// How many threads [`Run::run_parallel`] and [`Run::apply`] may use: 1 (the default) runs
    /// on the calling thread, 0 uses every core. Only a script whose lines can be processed one
    /// by one is spread over threads (no `n`, `N`, `D`, `P`, hold space, `$`, ranges, `r`, `w`
    /// ...); any other script runs on the calling thread. The output is the same either way.
    pub fn jobs(mut self, threads: usize) -> Run {
        self.jobs = threads;
        self
    }

    fn threads(&self) -> usize {
        if self.jobs == 0 {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(16)
        } else {
            self.jobs
        }
    }

    fn locale(&self) -> Locale {
        match self.encoding {
            Encoding::Environment => environment_locale(),
            Encoding::Bytes => Locale::C,
            Encoding::Utf8 => Locale::Utf8,
        }
    }

    fn separator(&self) -> u8 {
        if self.null_data { 0 } else { b'\n' }
    }

    fn parse_options(&self) -> ParseOptions {
        ParseOptions {
            extended: false,
            pcre: false,
            posix: false,
            posixly_correct: false,
            sandbox: self.sandbox,
            null_data: self.null_data,
            locale: self.locale(),
            glibc_compat: self.glibc_compat,
        }
    }

    fn options(&self) -> Options {
        Options {
            quiet: self.quiet,
            separator: self.separator(),
            line_length: self.line_length,
            locale: self.locale(),
            ..Options::default()
        }
    }

    /// Runs the script on everything `input` gives and writes the result to `output`.
    ///
    /// The input is read and the output is written as the run goes on, so a stream of any size
    /// is fine. The output is flushed when the run ends.
    pub fn run<R: Read, W: Write>(
        &self,
        script: &Script,
        input: R,
        mut output: W,
    ) -> Result<Outcome, Error> {
        crate::io::GLIBC_COMPAT.store(self.glibc_compat, std::sync::atomic::Ordering::Relaxed);
        let parsed = script.parsed(&self.parse_options())?;
        let mut source =
            ReaderSource::new(BufReader::with_capacity(1 << 16, input), self.separator());
        let writer: &mut dyn Write = &mut output;
        // SAFETY: the writer is borrowed for this call only. `Outputs` wants a `'static` box, but
        // it (with every alias of the stream) is dropped before `run` returns, so the reference
        // is never used after the borrow ends.
        let writer: &'static mut dyn Write =
            unsafe { std::mem::transmute::<&mut dyn Write, &'static mut dyn Write>(writer) };
        let mut outputs = Outputs::new(Box::new(Forward(writer)), self.separator());
        let result = runtime::run(&parsed, self.options(), &mut source, &mut outputs)
            .and_then(|code| outputs.flush_all().map(|()| code));
        drop(outputs);
        match result {
            Ok(exit_status) => Ok(Outcome { exit_status }),
            Err(e) => Err(e.into()),
        }
    }

    /// Runs the script on `input` and returns the result. With [`jobs`](Run::jobs) above 1 and
    /// a large input, the lines are processed on several threads.
    pub fn apply(&self, script: &Script, input: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(input.len() + input.len() / 8);
        if input.len() >= PARALLEL_MIN_BYTES {
            self.run_parallel(script, input, &mut out)?;
        } else {
            self.run(script, input, &mut out)?;
        }
        Ok(out)
    }

    /// Like [`run`](Run::run) for a reader that can move to another thread; uses the threads
    /// of [`jobs`](Run::jobs) when the script allows it, and runs on the calling thread when it
    /// does not. A script that reads ahead cannot be spread. A `q` or `Q` can: the output stops at the
    /// first record that reaches it and the exit status is the one it asked for, as in [`run`](Run::run).
    pub fn run_parallel<R: Read + Send, W: Write>(
        &self,
        script: &Script,
        input: R,
        mut output: W,
    ) -> Result<Outcome, Error> {
        let threads = self.threads();
        let parsed = script.parsed(&self.parse_options())?;
        // (A script that writes files is run on the calling thread: the files are the sequential run's to open.)
        let spread = threads >= 2
            && parsed.write_files.is_empty()
            && crate::analysis::record_local(&parsed, &self.options()).is_ok();
        if !spread {
            return self.run(script, input, output);
        }
        crate::io::GLIBC_COMPAT.store(self.glibc_compat, std::sync::atomic::Ordering::Relaxed);
        let result = crate::parallel::run_reader(
            script.chunks(&self.parse_options()),
            &self.options(),
            Box::new(input),
            threads,
            PARALLEL_BLOCK,
            &mut |bytes: &[u8], _unterminated: bool| {
                output
                    .write_all(bytes)
                    .map_err(|e| crate::io::write_error("output", &e))
            },
        )
        .and_then(|status| {
            output
                .flush()
                .map(|()| status)
                .map_err(|e| crate::io::write_error("output", &e))
        });
        match result {
            Ok(status) => Ok(Outcome {
                exit_status: status,
            }),
            Err(e) => Err(e.into()),
        }
    }
}

/// A writer the script's outputs share, over a borrowed one.
struct Forward(&'static mut dyn Write);

impl Write for Forward {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

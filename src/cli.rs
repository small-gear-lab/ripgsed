//! Mode selection, argument validation, and exit status.

use std::io::{self, BufWriter};

use crate::Fatal;
use crate::io::{FileInputs, Outputs, strerror};
use crate::regex::Locale;
use crate::runtime::{self, Options};
use crate::syntax::{Chunk, ChunkSource, ParseOptions, parse};

const EXIT_BAD_USAGE: i32 = 1;
const EXIT_BAD_INPUT: i32 = 2;
const EXIT_PANIC: i32 = 4;

/// What the command line asked for.
struct Invocation {
    options: Options,
    chunks: Vec<(Chunk, ParseOptions)>,
    files: Vec<String>,
}

/// An argument as a `String` that keeps every byte: the bytes that are not UTF-8 become the
/// private-use characters U+F780 to U+F7FF, which `argument_bytes` turns back.
fn lossless_string(arg: std::ffi::OsString) -> String {
    use std::os::unix::ffi::OsStringExt;
    let bytes = arg.into_vec();
    let mut out = String::with_capacity(bytes.len());
    let mut rest: &[u8] = &bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let bad = e.error_len().unwrap_or(after.len());
                for &b in &after[..bad] {
                    out.push(char::from_u32(0xF700 + u32::from(b)).unwrap_or('?'));
                }
                rest = &after[bad..];
            }
        }
    }
    out
}

/// A byte string as a `String` that keeps every byte, as `lossless_string` does for an argument.
pub fn lossless_bytes(bytes: &[u8]) -> String {
    lossless_string(std::os::unix::ffi::OsStringExt::from_vec(bytes.to_vec()))
}

/// Writes a message and a newline to standard error with the bytes `lossless_string` kept.
pub fn eprint_message(message: &str) {
    use std::io::Write;
    let mut bytes = argument_bytes(message);
    bytes.push(b'\n');
    let _ = io::stderr().write_all(&bytes);
}

/// The bytes of an argument made by `lossless_string`.
pub fn argument_bytes(arg: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(arg.len());
    for c in arg.chars() {
        match c as u32 {
            v @ 0xF780..=0xF7FF => out.push((v - 0xF700) as u8),
            _ => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

/// The name this program was run as, used as the prefix of every message.
pub fn program_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "sed".to_string())
}

fn locale_from_env() -> Locale {
    crate::locale::from_env()
}

fn usage_line(program: &str) -> String {
    format!("Usage: {program} [OPTION]... {{script-only-if-no-other-script}} [input-file]...")
}

/// What `--help` prints, and what a wrong command line prints after its message.
fn help_text(program: &str) -> String {
    format!(
        "{usage}

Options of sed:
  -n, --quiet, --silent     do not print the pattern space at the end of a cycle
      --debug               print the script in canonical form and annotate its execution
  -e, --expression=SCRIPT   add SCRIPT to the commands to run
  -f, --file=FILE           add the commands in FILE to the commands to run
      --follow-symlinks     follow symbolic links when editing in place
  -i[SUFFIX], --in-place[=SUFFIX]
                            edit files in place (with SUFFIX, keep a backup of each file)
  -l N, --line-length=N     wrap the output of the `l' command at N characters
      --posix               turn off the GNU extensions
  -E, -r, --regexp-extended
                            read the regular expressions of the script as extended ones
  -s, --separate            treat the files as separate streams, not as one long stream
      --sandbox             reject the commands e, r and w
  -u, --unbuffered          read and write as little at a time as possible
  -z, --null-data           lines end in a NUL byte, not in a newline
  -b, --binary              accepted and ignored
      --help                display this help text and exit
      --version             display version information and exit

Options that only ripgsed has:
  -P, --perl-regexp         read the regular expressions of the script as PCRE2 ones
      --glibc-compat        also reproduce the quirks of the C library's regular expression
                            matcher (the environment variable RIPGSED_GLIBC_COMPAT=1 does too)
      --grep                as the first argument: behave as grep (see `ripgsed --grep --help')

Without -e, -f or --expression, the first argument that is not an option is the script.
The other arguments are input files; without any, the standard input is read.
Scripts that act on one line at a time run on several threads for large files;
RIPGSED_JOBS=N sets the number of threads (1: one thread).

ripgsed home page: <https://github.com/small-gear-lab/ripgsed>
",
        usage = usage_line(program)
    )
}

/// How a long option takes its argument.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arg {
    No,
    Required,
    Optional,
}

/// The option an argument stands for, whether it was written long or short.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opt {
    Binary,
    Debug,
    Expression,
    File,
    FollowSymlinks,
    GlibcCompat,
    Help,
    InPlace,
    LineLength,
    NullData,
    Posix,
    Quiet,
    ExtendedRegexp,
    PerlRegexp,
    Sandbox,
    Separate,
    Unbuffered,
    Version,
}

const LONG_OPTIONS: &[(&str, Arg, Opt)] = &[
    ("binary", Arg::No, Opt::Binary),
    ("debug", Arg::No, Opt::Debug),
    ("expression", Arg::Required, Opt::Expression),
    ("file", Arg::Required, Opt::File),
    ("follow-symlinks", Arg::No, Opt::FollowSymlinks),
    ("glibc-compat", Arg::No, Opt::GlibcCompat),
    ("help", Arg::No, Opt::Help),
    ("in-place", Arg::Optional, Opt::InPlace),
    ("line-length", Arg::Required, Opt::LineLength),
    ("null-data", Arg::No, Opt::NullData),
    ("zero-terminated", Arg::No, Opt::NullData),
    ("perl-regexp", Arg::No, Opt::PerlRegexp),
    ("posix", Arg::No, Opt::Posix),
    ("quiet", Arg::No, Opt::Quiet),
    ("regexp-extended", Arg::No, Opt::ExtendedRegexp),
    ("sandbox", Arg::No, Opt::Sandbox),
    ("separate", Arg::No, Opt::Separate),
    ("silent", Arg::No, Opt::Quiet),
    ("unbuffered", Arg::No, Opt::Unbuffered),
    ("version", Arg::No, Opt::Version),
];

/// Short options: the letter, whether it takes an argument, and the option.
const SHORT_OPTIONS: &[(char, Arg, Opt)] = &[
    ('b', Arg::No, Opt::Binary),
    ('E', Arg::No, Opt::ExtendedRegexp),
    ('e', Arg::Required, Opt::Expression),
    ('f', Arg::Required, Opt::File),
    ('i', Arg::Optional, Opt::InPlace),
    ('l', Arg::Required, Opt::LineLength),
    ('n', Arg::No, Opt::Quiet),
    ('P', Arg::No, Opt::PerlRegexp),
    ('r', Arg::No, Opt::ExtendedRegexp),
    ('s', Arg::No, Opt::Separate),
    ('u', Arg::No, Opt::Unbuffered),
    ('z', Arg::No, Opt::NullData),
];

/// Resolves `--name`, accepting an unambiguous prefix as getopt_long does.
fn find_long(name: &str, program: &str) -> Result<(&'static str, Arg, Opt), String> {
    if let Some(&(n, a, o)) = LONG_OPTIONS.iter().find(|(n, ..)| *n == name) {
        return Ok((n, a, o));
    }
    let candidates: Vec<_> = LONG_OPTIONS
        .iter()
        .filter(|(n, ..)| n.starts_with(name))
        .collect();
    match candidates.as_slice() {
        [] => Err(format!("{program}: unrecognized option '--{name}'")),
        [(n, a, o)] => Ok((n, *a, *o)),
        many => {
            let list: Vec<String> = many.iter().map(|(n, ..)| format!("'--{n}'")).collect();
            Err(format!(
                "{program}: option '--{name}' is ambiguous; possibilities: {}",
                list.join(" ")
            ))
        }
    }
}

/// `atoi`: the leading digits of `text`, or 0.
fn atoi(text: &str) -> usize {
    let t = text.trim_start();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

struct State {
    options: Options,
    parse_base: ParseOptions,
    chunks: Vec<(Chunk, ParseOptions)>,
    expressions: usize,
    posix_mode: bool,
    help: bool,
    version: bool,
}

impl State {
    fn apply(&mut self, opt: Opt, value: Option<String>, program: &str) -> Result<(), i32> {
        match opt {
            Opt::Binary => {}
            Opt::Debug => self.options.debug = true,
            Opt::Quiet => self.options.quiet = true,
            Opt::ExtendedRegexp => {
                self.parse_base.extended = true;
                self.parse_base.pcre = false;
            }
            Opt::PerlRegexp => self.parse_base.pcre = true,
            Opt::Separate => self.options.separate = true,
            Opt::NullData => {
                self.options.separator = 0;
                self.parse_base.null_data = true;
            }
            Opt::Unbuffered => self.options.unbuffered = true,
            Opt::FollowSymlinks => self.options.follow_symlinks = true,
            Opt::Sandbox => self.parse_base.sandbox = true,
            Opt::GlibcCompat => self.parse_base.glibc_compat = true,
            Opt::Posix => {
                self.parse_base.posix = true;
                self.parse_base.posixly_correct = true;
                self.options.posixly_correct = true;
                self.posix_mode = true;
            }
            Opt::Help => self.help = true,
            Opt::Version => self.version = true,
            Opt::LineLength => self.options.line_length = atoi(&value.unwrap_or_default()),
            Opt::InPlace => {
                self.options.in_place = Some(value.unwrap_or_default());
                self.options.separate = true;
            }
            Opt::Expression => {
                self.expressions += 1;
                let text = argument_bytes(&value.unwrap_or_default());
                self.chunks.push((
                    Chunk {
                        source: ChunkSource::Expression(self.expressions),
                        text,
                    },
                    self.parse_base,
                ));
            }
            Opt::File => {
                let name = value.unwrap_or_default();
                match read_script_file(&name) {
                    Ok(text) => self.chunks.push((
                        Chunk {
                            source: ChunkSource::File(name),
                            text,
                        },
                        self.parse_base,
                    )),
                    Err(e) => {
                        eprint_message(&format!(
                            "{program}: couldn't open file {name}: {}",
                            strerror(&e)
                        ));
                        return Err(EXIT_PANIC);
                    }
                }
            }
        }
        Ok(())
    }
}

fn parse_args(args: Vec<String>, program: &str) -> Result<Result<Invocation, ()>, i32> {
    let posixly_correct = std::env::var_os("POSIXLY_CORRECT").is_some();
    let locale = locale_from_env();
    // GNU sed takes the default wrap width of `l` from COLS (one less than the terminal width).
    let mut options = Options {
        posixly_correct,
        locale,
        ..Options::default()
    };
    if let Some(cols) = std::env::var("COLS")
        .ok()
        .map(|c| atoi(&c))
        .filter(|&c| c > 0)
    {
        options.line_length = cols - 1;
    }
    let mut state = State {
        options,
        parse_base: ParseOptions {
            posixly_correct,
            locale,
            glibc_compat: std::env::var_os("RIPGSED_GLIBC_COMPAT")
                .is_some_and(|v| !v.is_empty() && v != "0"),
            ..ParseOptions::default()
        },
        chunks: Vec::new(),
        expressions: 0,
        posix_mode: false,
        help: false,
        version: false,
    };
    let mut operands: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    let mut options_done = false;

    let usage_error = |message: String| -> i32 {
        eprintln!("{message}");
        eprint!("{}", help_text(program));
        EXIT_BAD_USAGE
    };

    while let Some(arg) = it.next() {
        if options_done || arg == "-" || !arg.starts_with('-') {
            if posixly_correct {
                options_done = true;
            }
            operands.push(arg);
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let (full, kind, opt) = find_long(name, program).map_err(usage_error)?;
            let value = match (kind, inline) {
                (Arg::No, Some(_)) => {
                    return Err(usage_error(format!(
                        "{program}: option '--{full}' doesn't allow an argument"
                    )));
                }
                (Arg::Required, None) => match it.next() {
                    Some(v) => Some(v),
                    None => {
                        return Err(usage_error(format!(
                            "{program}: option '--{full}' requires an argument"
                        )));
                    }
                },
                (_, v) => v,
            };
            state.apply(opt, value, program)?;
            continue;
        }
        // Short options, possibly bundled: -ne 'p', -i.bak, -l5.
        let letters: Vec<char> = arg[1..].chars().collect();
        let mut i = 0;
        while i < letters.len() {
            let c = letters[i];
            i += 1;
            let Some(&(_, kind, opt)) = SHORT_OPTIONS.iter().find(|(l, ..)| *l == c) else {
                return Err(usage_error(format!("{program}: invalid option -- '{c}'")));
            };
            let value = match kind {
                Arg::No => None,
                Arg::Optional | Arg::Required => {
                    let rest: String = letters[i..].iter().collect();
                    i = letters.len();
                    if !rest.is_empty() {
                        Some(rest)
                    } else if kind == Arg::Required {
                        match it.next() {
                            Some(v) => Some(v),
                            None => {
                                return Err(usage_error(format!(
                                    "{program}: option requires an argument -- '{c}'"
                                )));
                            }
                        }
                    } else {
                        None
                    }
                }
            };
            state.apply(opt, value, program)?;
        }
    }

    if state.help || state.version {
        let name = program;
        if state.version {
            println!("{}", version_text(name));
        } else {
            print!("{}", help_text(name));
        }
        return Ok(Err(()));
    }
    crate::io::GLIBC_COMPAT.store(
        state.parse_base.glibc_compat,
        std::sync::atomic::Ordering::Relaxed,
    );
    let _ = state.posix_mode;

    if state.chunks.is_empty() {
        if operands.is_empty() {
            eprint!("{}", help_text(program));
            return Err(EXIT_BAD_USAGE);
        }
        state.expressions += 1;
        let text = argument_bytes(&operands.remove(0));
        state.chunks.push((
            Chunk {
                source: ChunkSource::Expression(state.expressions),
                text,
            },
            state.parse_base,
        ));
    }
    Ok(Ok(Invocation {
        options: state.options,
        chunks: state.chunks,
        files: operands,
    }))
}

/// The version text names this project; it is the one deliberate difference
/// from GNU sed (see docs/DESIGN.md).
fn version_text(program: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if program == "ripgsed" {
        format!("ripgsed {version}")
    } else {
        format!(
            "{program} (ripgsed) {version}\nBehavior target: GNU sed 4.9 (experimental, see https://github.com/small-gear-lab/ripgsed)"
        )
    }
}

fn read_script_file(name: &str) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if name == "-" {
        io::stdin().read_to_end(&mut bytes)?;
    } else {
        match std::fs::read(name) {
            Ok(b) => bytes = b,
            // A directory reads as an empty script, as with GNU sed.
            Err(e) if e.raw_os_error() == Some(21) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(bytes)
}

unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

const SIGPIPE: i32 = 13;
const SIG_DFL: usize = 0;

/// The SIGPIPE disposition the process was started with, saved before Rust's runtime
/// replaces it with "ignore".
static INHERITED_SIGPIPE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(SIG_DFL);

/// Runs before `main` (and before the Rust runtime changes SIGPIPE) on Linux.
extern "C" fn save_inherited_sigpipe() {
    // SAFETY: `signal` is async-signal-safe; the handler is put straight back.
    unsafe {
        let old = signal(SIGPIPE, SIG_DFL);
        signal(SIGPIPE, old);
        INHERITED_SIGPIPE.store(old, std::sync::atomic::Ordering::Relaxed);
    }
    crate::io::STDOUT_WAS_CLOSED.store(
        crate::io::fd_is_closed(1),
        std::sync::atomic::Ordering::Relaxed,
    );
    crate::io::STDIN_WAS_CLOSED.store(
        crate::io::fd_is_closed(0),
        std::sync::atomic::Ordering::Relaxed,
    );
}

#[used]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(not(target_vendor = "apple"), unsafe(link_section = ".init_array"))]
static SAVE_SIGPIPE: extern "C" fn() = save_inherited_sigpipe;

/// Rust ignores SIGPIPE at startup; GNU sed keeps what it inherited, which is normally the
/// default, so `sed ... | head` ends the pipeline with status 141 instead of an error message.
fn restore_inherited_sigpipe() {
    let inherited = INHERITED_SIGPIPE.load(std::sync::atomic::Ordering::Relaxed);
    // SAFETY: installs the saved disposition before any other thread exists.
    unsafe {
        signal(SIGPIPE, inherited);
    }
}

/// Creates (truncates) the files named by `w` commands. `/dev/stdout` and `/dev/stderr` are special.
fn create_write_files(names: &[Vec<u8>]) -> Result<(), String> {
    for name in names {
        if name == b"/dev/stdout" || name == b"/dev/stderr" {
            continue;
        }
        let path = String::from_utf8_lossy(name).into_owned();
        if let Err(e) = std::fs::File::create(crate::io::bytes_path(name)) {
            return Err(format!("couldn't open file {path}: {}", strerror(&e)));
        }
    }
    Ok(())
}

/// The soft limit of open files of this process, where the system says (Linux).
fn open_files_limit() -> Option<u64> {
    let limits = std::fs::read_to_string("/proc/self/limits").ok()?;
    let line = limits.lines().find(|l| l.starts_with("Max open files"))?;
    // ("Max open files", then the soft limit, the hard limit and the unit.)
    line.split_whitespace().nth(3)?.parse().ok()
}

/// When the script needs only the last records of the input (`analysis::tail_window`) and the input is one file
/// that can be searched, runs it on those records, read from the end of the file, and gives the exit status.
/// `None` when this does not apply: nothing has been written then, and the run goes on as it would.
///
/// `RIPGSED_TAIL=0` turns it off, and `RIPGSED_TAIL_MIN_BYTES` is the size under which a file is read whole
/// (a megabyte; 0 for every file, which the tests use). The result is the same either way.
fn run_on_the_tail(
    script: &crate::syntax::Script,
    options: &Options,
    files: &[String],
    program: &str,
) -> Option<i32> {
    use std::io::Seek;
    if std::env::var("RIPGSED_TAIL").is_ok_and(|v| v == "0") {
        return None;
    }
    // One file named on the command line, or standard input when it is a file (not a pipe, a terminal or a socket).
    let (mut file, name, stdin) = match files {
        [name] if name != "-" => (
            std::fs::File::open(crate::io::os_path(name)).ok()?,
            name.as_str(),
            false,
        ),
        [] => (duplicate_of_stdin()?, "-", true),
        [name] if name == "-" => (duplicate_of_stdin()?, "-", true),
        _ => return None,
    };
    // The records start where a run that reads them one by one would: after what was read from standard input
    // before (the position is shared with the descriptor 0 that this one is a duplicate of).
    let from = if stdin {
        file.stream_position().ok()?
    } else {
        0
    };
    let min_bytes: u64 = std::env::var("RIPGSED_TAIL_MIN_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1 << 20);
    let window = crate::analysis::tail_window(script, options)?;
    // (A file that cannot be read is the business of the run that reports it.)
    let bytes = crate::io::read_tail(&file, options.separator, window, min_bytes, from).ok()??;
    let mut source = crate::io::TailSource::new(bytes, options.separator, name);
    let stdout = io::stdout().lock();
    let mut outputs = Outputs::new(Box::new(BufWriter::new(stdout)), options.separator);
    let result = runtime::run(script, options.clone(), &mut source, &mut outputs)
        .and_then(|code| outputs.flush_all().map(|()| code));
    if stdin {
        // A run that reads to the end leaves standard input at the end, for the command that follows.
        let _ = file.seek(std::io::SeekFrom::End(0));
    }
    Some(match result {
        Ok(code) => code,
        Err(Fatal { message, status }) => {
            eprint_message(&format!("{program}: {message}"));
            let _ = outputs.flush_all();
            status
        }
    })
}

/// A second descriptor for standard input, which shares its position and which can be closed without closing it.
fn duplicate_of_stdin() -> Option<std::fs::File> {
    use std::os::fd::BorrowedFd;
    // SAFETY: descriptor 0 stays open for as long as the process lives (this only looks at it).
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    stdin.try_clone_to_owned().ok().map(std::fs::File::from)
}

/// How many workers to use, or `None` to run sequentially.
///
/// A script runs in parallel only if it is record-local (see `analysis`) and either the
/// input is large regular files or `RIPGSED_JOBS` forces it (also for standard input). The
/// result is identical either way; only the speed differs.
fn parallel_jobs(
    script: &crate::syntax::Script,
    options: &Options,
    files: &[String],
) -> Option<usize> {
    let forced: Option<usize> = std::env::var("RIPGSED_JOBS")
        .ok()
        .and_then(|v| v.parse().ok());
    if forced.is_some_and(|n| n < 2) {
        return None;
    }
    crate::analysis::record_local(script, options).ok()?;
    // Each file is edited in its own right (`-i`), and what is read must be what the file was: the sequential
    // run reads a file named twice as it was left the first time, and a file that the script writes as it is
    // written. That is not what chunks read ahead of the writer see, so those runs stay sequential.
    if options.in_place.is_some() {
        use std::os::unix::fs::MetadataExt;
        // The names are resolved by the sequential run before it opens them, and it keeps the input open while it
        // makes the file that replaces it: with so few descriptors that makes a difference, it has to be that run.
        if options.follow_symlinks || open_files_limit().is_some_and(|limit| limit < 64) {
            return None;
        }
        if files.is_empty() || files.iter().any(|n| n == "-") {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        for name in files {
            let meta = std::fs::metadata(name).ok()?;
            if !seen.insert((meta.dev(), meta.ino())) {
                return None;
            }
        }
        for name in &script.write_files {
            if let Ok(meta) = std::fs::metadata(crate::io::bytes_path(name)) {
                if seen.contains(&(meta.dev(), meta.ino())) {
                    return None;
                }
            }
        }
    }
    // Every named file must be a readable regular file, or the sequential run reports the errors.
    let mut total = 0u64;
    for name in files.iter().filter(|n| *n != "-") {
        let meta = std::fs::metadata(name).ok().filter(|m| m.is_file())?;
        std::fs::File::open(name).ok()?;
        total += meta.len();
    }
    if let Some(n) = forced {
        return Some(n);
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let min_bytes: u64 = std::env::var("RIPGSED_PARALLEL_MIN_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8 << 20);
    // Standard input counts when it is a regular file (`sed ... < big.txt`); a pipe or a
    // terminal has no known size, and stays sequential.
    let reads_stdin = files.is_empty() || files.iter().any(|n| n == "-");
    if reads_stdin {
        match std::fs::metadata("/dev/stdin") {
            Ok(m) if m.is_file() => total += m.len(),
            _ => return None,
        }
    }
    (cores >= 2 && total >= min_bytes).then_some(cores.min(16))
}

/// Returns the process exit status.
pub fn main() -> i32 {
    restore_inherited_sigpipe();
    let program = program_name();
    // The grep mode: the program is run as `grep`, `egrep` or `fgrep`, or the first argument is `--grep`.
    {
        use std::os::unix::ffi::OsStrExt;
        let raw: Vec<Vec<u8>> = std::env::args_os()
            .skip(1)
            .map(|a| a.as_bytes().to_vec())
            .collect();
        if let Some(kind) = crate::grep::kind_for_name(&program) {
            // (`egrep` and `fgrep` are `grep -E` and `grep -F` to GNU grep, and its messages say `grep`.)
            return crate::grep::main("grep", raw, kind);
        }
        if raw.first().is_some_and(|a| a == b"--grep") {
            return crate::grep::main("grep", raw[1..].to_vec(), crate::grep::Kind::Basic);
        }
    }
    let args: Vec<String> = std::env::args_os().skip(1).map(lossless_string).collect();
    let invocation = match parse_args(args, &program) {
        Ok(Ok(i)) => i,
        Ok(Err(())) => return 0,
        Err(code) => return code,
    };

    let script = match parse(&invocation.chunks) {
        Ok(s) => s,
        Err(e) => {
            // GNU sed has already created the `w` files it read before the error, and a file
            // it cannot create stops it before the syntax error is found.
            if let Err(open_error) = create_write_files(&e.touched) {
                eprint_message(&format!("{program}: {open_error}"));
                return EXIT_PANIC;
            }
            eprint_message(&format!("{program}: {e}"));
            return e.status;
        }
    };
    if let Err(open_error) = create_write_files(&script.write_files) {
        eprint_message(&format!("{program}: {open_error}"));
        return EXIT_PANIC;
    }

    let options = invocation.options;
    if let Some(status) = run_on_the_tail(&script, &options, &invocation.files, &program) {
        return status;
    }
    if let Some(jobs) = parallel_jobs(&script, &options, &invocation.files) {
        let block = std::env::var("RIPGSED_BLOCK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1 << 20);
        let stdout = io::stdout().lock();
        let mut outputs = Outputs::new(Box::new(BufWriter::new(stdout)), options.separator);
        // The sequential run opens the files of `w` before it opens its input, and fails there if it can't.
        for name in &script.write_files {
            if let Err(Fatal { message, status }) = outputs.open(name) {
                eprint_message(&format!("{program}: {message}"));
                return status;
            }
        }
        let result = crate::parallel::run(
            invocation.chunks.clone(),
            &options,
            &invocation.files,
            jobs,
            block,
            &mut outputs,
        )
        .and_then(|status| outputs.flush_all().map(|()| status));
        return match result {
            Ok(status) => status,
            Err(Fatal { message, status }) => {
                eprint_message(&format!("{program}: {message}"));
                let _ = outputs.flush_all();
                status
            }
        };
    }
    if options.in_place.is_some() && invocation.files.is_empty() {
        eprintln!("{program}: no input files");
        if options.debug {
            // GNU sed has listed the program by then.
            use std::io::Write;
            let _ =
                io::stdout().write_all(&crate::debug::program(&script, options.locale.is_chars()));
        }
        return EXIT_PANIC;
    }
    let mut inputs = FileInputs::new(invocation.files, options.separator, &program)
        .in_place(options.in_place.is_some())
        .follow_symlinks(options.follow_symlinks)
        .unbuffered(options.unbuffered);
    let stdout = io::stdout().lock();
    let mut outputs = Outputs::new(Box::new(BufWriter::new(stdout)), options.separator);
    outputs.unbuffered = options.unbuffered;

    let result = runtime::run(&script, options, &mut inputs, &mut outputs)
        .and_then(|code| outputs.flush_all().map(|()| code));
    inputs.finish();
    crate::io::restore_stdin_offset();
    match result {
        Ok(_) if inputs.had_edit_error || inputs.had_read_error => EXIT_PANIC,
        Ok(_) if inputs.had_open_error => EXIT_BAD_INPUT,
        Ok(code) => code,
        Err(Fatal { message, status }) => {
            // GNU sed reports first, then flushes what it had written.
            eprint_message(&format!("{program}: {message}"));
            outputs.abort_in_place();
            let _ = outputs.flush_all();
            status
        }
    }
}

//! Mode selection, argument validation, and exit status.

use std::io::{self, BufWriter};

use crate::io::{strerror, FileInputs, Outputs};
use crate::regex::Locale;
use crate::runtime::{self, Options};
use crate::syntax::{parse, Chunk, ChunkSource, ParseOptions};
use crate::Fatal;

const EXIT_BAD_USAGE: i32 = 1;
const EXIT_BAD_INPUT: i32 = 2;
const EXIT_PANIC: i32 = 4;

/// What the command line asked for.
struct Invocation {
    options: Options,
    chunks: Vec<(Chunk, ParseOptions)>,
    files: Vec<String>,
}

/// The name this program was run as, used as the prefix of every message.
fn program_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|a| std::path::Path::new(&a).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "sed".to_string())
}

fn locale_from_env() -> Locale {
    let value = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if value.contains("utf-8") || value.contains("utf8") { Locale::Utf8 } else { Locale::C }
}

fn usage_line(program: &str) -> String {
    format!("Usage: {program} [OPTION]... {{script-only-if-no-other-script}} [input-file]...")
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
    Help,
    InPlace,
    LineLength,
    NullData,
    Posix,
    Quiet,
    ExtendedRegexp,
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
    ("help", Arg::No, Opt::Help),
    ("in-place", Arg::Optional, Opt::InPlace),
    ("line-length", Arg::Required, Opt::LineLength),
    ("null-data", Arg::No, Opt::NullData),
    ("zero-terminated", Arg::No, Opt::NullData),
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
    let candidates: Vec<_> = LONG_OPTIONS.iter().filter(|(n, ..)| n.starts_with(name)).collect();
    match candidates.as_slice() {
        [] => Err(format!("{program}: unrecognized option '--{name}'")),
        [(n, a, o)] => Ok((n, *a, *o)),
        many => {
            // Several spellings of the same option are not ambiguous.
            if many.iter().all(|(_, _, o)| *o == many[0].2) {
                return Ok((many[0].0, many[0].1, many[0].2));
            }
            let list: Vec<String> = many.iter().map(|(n, ..)| format!("'--{n}'")).collect();
            Err(format!("{program}: option '--{name}' is ambiguous; possibilities: {}", list.join(" ")))
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
            Opt::ExtendedRegexp => self.parse_base.extended = true,
            Opt::Separate => self.options.separate = true,
            Opt::NullData => {
                self.options.separator = 0;
                self.parse_base.null_data = true;
            }
            Opt::Unbuffered => self.options.unbuffered = true,
            Opt::FollowSymlinks => self.options.follow_symlinks = true,
            Opt::Sandbox => self.parse_base.sandbox = true,
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
                let text = value.unwrap_or_default().into_bytes();
                self.chunks.push((Chunk { source: ChunkSource::Expression(self.expressions), text }, self.parse_base));
            }
            Opt::File => {
                let name = value.unwrap_or_default();
                match read_script_file(&name) {
                    Ok(text) => self.chunks.push((Chunk { source: ChunkSource::File(name), text }, self.parse_base)),
                    Err(e) => {
                        eprintln!("{program}: couldn't open file {name}: {}", strerror(&e));
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
    let mut options = Options { posixly_correct, locale, ..Options::default() };
    if let Some(cols) = std::env::var("COLS").ok().map(|c| atoi(&c)).filter(|&c| c > 0) {
        options.line_length = cols - 1;
    }
    let mut state = State {
        options,
        parse_base: ParseOptions { posixly_correct, locale, ..ParseOptions::default() },
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
        eprintln!("{}", usage_line(program));
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
                    return Err(usage_error(format!("{program}: option '--{full}' doesn't allow an argument")));
                }
                (Arg::Required, None) => match it.next() {
                    Some(v) => Some(v),
                    None => return Err(usage_error(format!("{program}: option '--{full}' requires an argument"))),
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
                            None => return Err(usage_error(format!("{program}: option requires an argument -- '{c}'"))),
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
            println!("{}", usage_line(name));
        }
        return Ok(Err(()));
    }
    let _ = state.posix_mode;

    if state.chunks.is_empty() {
        if operands.is_empty() {
            eprintln!("{}", usage_line(program));
            return Err(EXIT_BAD_USAGE);
        }
        state.expressions += 1;
        let text = operands.remove(0).into_bytes();
        state.chunks.push((Chunk { source: ChunkSource::Expression(state.expressions), text }, state.parse_base));
    }
    Ok(Ok(Invocation { options: state.options, chunks: state.chunks, files: operands }))
}

/// The version text names this project; it is the one deliberate difference
/// from GNU sed (see docs/DESIGN.md).
fn version_text(program: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if program == "ripgsed" {
        format!("ripgsed {version}")
    } else {
        format!("{program} (ripgsed) {version}\nBehavior target: GNU sed 4.9 (not yet fully compatible)")
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
static INHERITED_SIGPIPE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(SIG_DFL);

/// Runs before `main` (and before the Rust runtime changes SIGPIPE) on Linux.
extern "C" fn save_inherited_sigpipe() {
    // SAFETY: `signal` is async-signal-safe; the handler is put straight back.
    unsafe {
        let old = signal(SIGPIPE, SIG_DFL);
        signal(SIGPIPE, old);
        INHERITED_SIGPIPE.store(old, std::sync::atomic::Ordering::Relaxed);
    }
    crate::io::STDOUT_WAS_CLOSED.store(crate::io::fd_is_closed(1), std::sync::atomic::Ordering::Relaxed);
}

#[used]
#[unsafe(link_section = ".init_array")]
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

/// Returns the process exit status.
pub fn main() -> i32 {
    restore_inherited_sigpipe();
    let program = program_name();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse_args(args, &program) {
        Ok(Ok(i)) => i,
        Ok(Err(())) => return 0,
        Err(code) => return code,
    };

    let script = match parse(&invocation.chunks) {
        Ok(s) => s,
        Err(e) => {
            // GNU sed has already created the `w` files it read before the error.
            for name in &e.touched {
                let _ = std::fs::File::create(String::from_utf8_lossy(name).as_ref());
            }
            eprintln!("{program}: {e}");
            return e.status;
        }
    };

    let options = invocation.options;
    if options.in_place.is_some() && invocation.files.is_empty() {
        eprintln!("{program}: no input files");
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
    crate::io::restore_stdin_offset();
    match result {
        Ok(_) if inputs.had_edit_error || inputs.had_read_error => EXIT_PANIC,
        Ok(_) if inputs.had_open_error => EXIT_BAD_INPUT,
        Ok(code) => code,
        Err(Fatal { message, status }) => {
            // GNU sed reports first, then flushes what it had written.
            eprintln!("{program}: {message}");
            outputs.abort_in_place();
            let _ = outputs.flush_all();
            status
        }
    }
}

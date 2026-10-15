//! Mode selection, argument validation, and exit status.

use std::io::{self, BufWriter};

use crate::io::{strerror, FileInputs, Output};
use crate::runtime::{self, Options};
use crate::syntax::{parse_chunk, Script};

const EXIT_BAD_USAGE: i32 = 1;
const EXIT_BAD_INPUT: i32 = 2;
const EXIT_PANIC: i32 = 4;

/// What the command line asked for.
struct Invocation {
    quiet: bool,
    chunks: Vec<Vec<u8>>,
    files: Vec<String>,
}

/// The name this program was run as, used as the prefix of every message.
fn program_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|a| std::path::Path::new(&a).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "sed".to_string())
}

fn parse_args(args: Vec<String>, program: &str) -> Result<Invocation, i32> {
    let mut quiet = false;
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut it = args.into_iter();
    let mut options_done = false;
    while let Some(arg) = it.next() {
        if options_done || arg == "-" || !arg.starts_with('-') {
            operands.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => options_done = true,
            "-n" | "--quiet" | "--silent" => quiet = true,
            "-e" | "--expression" => match it.next() {
                Some(script) => chunks.push(script.into_bytes()),
                None => {
                    eprintln!("{program}: option requires an argument -- 'e'");
                    return Err(EXIT_BAD_USAGE);
                }
            },
            _ if arg.starts_with("--expression=") => {
                chunks.push(arg.as_bytes()["--expression=".len()..].to_vec());
            }
            _ => {
                eprintln!("{program}: unsupported option '{arg}'");
                return Err(EXIT_BAD_USAGE);
            }
        }
    }
    if chunks.is_empty() {
        if operands.is_empty() {
            eprintln!("Usage: {program} [OPTION]... {{script-only-if-no-other-script}} [input-file]...");
            return Err(EXIT_BAD_USAGE);
        }
        chunks.push(operands.remove(0).into_bytes());
    }
    Ok(Invocation { quiet, chunks, files: operands })
}

/// Returns the process exit status.
pub fn main() -> i32 {
    let program = program_name();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse_args(args, &program) {
        Ok(i) => i,
        Err(code) => return code,
    };

    let mut script = Script::default();
    for (n, chunk) in invocation.chunks.iter().enumerate() {
        if let Err(e) = parse_chunk(&mut script, chunk, n + 1) {
            eprintln!("{program}: {e}");
            return EXIT_BAD_USAGE;
        }
    }

    let options = Options { quiet: invocation.quiet, ..Options::default() };
    let mut inputs = FileInputs::new(invocation.files, options.separator, &program);
    let stdout = io::stdout().lock();
    let mut output = Output::new(BufWriter::new(stdout), options.separator);

    let result = runtime::run(&script, options, &mut inputs, &mut output).and_then(|()| output.flush());
    match result {
        Ok(()) if inputs.had_open_error => EXIT_BAD_INPUT,
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{program}: couldn't write to stdout: {}", strerror(&e));
            EXIT_PANIC
        }
    }
}

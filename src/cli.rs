//! Mode selection, argument validation, and exit status.

use std::io::{self, BufWriter};

use crate::io::{write_error, FileInputs, Outputs};
use crate::regex::Locale;
use crate::runtime::{self, Options};
use crate::syntax::{parse, Chunk, ChunkSource, ParseOptions};
use crate::Fatal;

const EXIT_BAD_USAGE: i32 = 1;
const EXIT_BAD_INPUT: i32 = 2;

/// What the command line asked for.
struct Invocation {
    options: Options,
    parse_base: ParseOptions,
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

fn usage(program: &str) {
    eprintln!("Usage: {program} [OPTION]... {{script-only-if-no-other-script}} [input-file]...");
}

fn parse_args(args: Vec<String>, program: &str) -> Result<Invocation, i32> {
    let posixly_correct = std::env::var_os("POSIXLY_CORRECT").is_some();
    let locale = locale_from_env();
    let mut options = Options { posixly_correct, locale, ..Options::default() };
    let mut parse_base = ParseOptions { posixly_correct, locale, ..ParseOptions::default() };
    let mut chunks: Vec<(Chunk, ParseOptions)> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut expressions = 0usize;

    let mut it = args.into_iter();
    let mut options_done = false;
    while let Some(arg) = it.next() {
        if options_done || arg == "-" || !arg.starts_with('-') {
            if posixly_correct {
                options_done = true;
            }
            operands.push(arg);
            continue;
        }
        let take_value = |name: &str, it: &mut std::vec::IntoIter<String>| -> Result<String, i32> {
            it.next().ok_or_else(|| {
                eprintln!("{program}: option requires an argument -- '{name}'");
                usage(program);
                EXIT_BAD_USAGE
            })
        };
        match arg.as_str() {
            "--" => options_done = true,
            "-n" | "--quiet" | "--silent" => options.quiet = true,
            "-E" | "-r" | "--regexp-extended" => parse_base.extended = true,
            "-s" | "--separate" => options.separate = true,
            "-z" | "--null-data" | "--zero-terminated" => options.separator = 0,
            "--posix" => {
                parse_base.posix = true;
                options.posixly_correct = true;
            }
            "--sandbox" => parse_base.sandbox = true,
            "-e" | "--expression" => {
                let script = take_value("e", &mut it)?;
                expressions += 1;
                chunks.push((Chunk { source: ChunkSource::Expression(expressions), text: script.into_bytes() }, parse_base));
            }
            "-f" | "--file" => {
                let name = take_value("f", &mut it)?;
                match read_script_file(&name) {
                    Ok(text) => chunks.push((Chunk { source: ChunkSource::File(name), text }, parse_base)),
                    Err(e) => {
                        eprintln!("{program}: couldn't open file {name}: {}", crate::io::strerror(&e));
                        return Err(EXIT_BAD_USAGE.max(4));
                    }
                }
            }
            _ if arg.starts_with("--expression=") => {
                expressions += 1;
                chunks.push((
                    Chunk { source: ChunkSource::Expression(expressions), text: arg.as_bytes()["--expression=".len()..].to_vec() },
                    parse_base,
                ));
            }
            _ if arg.starts_with("--file=") => {
                let name = arg["--file=".len()..].to_string();
                match read_script_file(&name) {
                    Ok(text) => chunks.push((Chunk { source: ChunkSource::File(name), text }, parse_base)),
                    Err(e) => {
                        eprintln!("{program}: couldn't open file {name}: {}", crate::io::strerror(&e));
                        return Err(4);
                    }
                }
            }
            _ if arg.starts_with("--") => {
                eprintln!("{program}: unrecognized option '{arg}'");
                usage(program);
                return Err(EXIT_BAD_USAGE);
            }
            _ => {
                // Bundled short options such as -ne or -n.
                let mut chars = arg[1..].chars();
                while let Some(c) = chars.next() {
                    match c {
                        'n' => options.quiet = true,
                        'E' | 'r' => parse_base.extended = true,
                        's' => options.separate = true,
                        'z' => options.separator = 0,
                        'e' | 'f' => {
                            let rest: String = chars.by_ref().collect();
                            let value = if rest.is_empty() { take_value(&c.to_string(), &mut it)? } else { rest };
                            if c == 'e' {
                                expressions += 1;
                                chunks.push((
                                    Chunk { source: ChunkSource::Expression(expressions), text: value.into_bytes() },
                                    parse_base,
                                ));
                            } else {
                                match read_script_file(&value) {
                                    Ok(text) => chunks.push((Chunk { source: ChunkSource::File(value), text }, parse_base)),
                                    Err(e) => {
                                        eprintln!("{program}: couldn't open file {value}: {}", crate::io::strerror(&e));
                                        return Err(4);
                                    }
                                }
                            }
                        }
                        other => {
                            eprintln!("{program}: invalid option -- '{other}'");
                            usage(program);
                            return Err(EXIT_BAD_USAGE);
                        }
                    }
                }
            }
        }
    }

    if chunks.is_empty() {
        if operands.is_empty() {
            usage(program);
            return Err(EXIT_BAD_USAGE);
        }
        expressions += 1;
        chunks.push((
            Chunk { source: ChunkSource::Expression(expressions), text: operands.remove(0).into_bytes() },
            parse_base,
        ));
    }
    Ok(Invocation { options, parse_base, chunks, files: operands })
}

fn read_script_file(name: &str) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if name == "-" {
        io::stdin().read_to_end(&mut bytes)?;
    } else {
        bytes = std::fs::read(name)?;
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    Ok(bytes)
}

/// Returns the process exit status.
pub fn main() -> i32 {
    let program = program_name();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse_args(args, &program) {
        Ok(i) => i,
        Err(code) => return code,
    };
    let _ = invocation.parse_base;

    let script = match parse(&invocation.chunks) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{program}: {e}");
            return EXIT_BAD_USAGE;
        }
    };

    let options = invocation.options;
    let mut inputs = FileInputs::new(invocation.files, options.separator, &program);
    let stdout = io::stdout().lock();
    let mut outputs = Outputs::new(Box::new(BufWriter::new(stdout)), options.separator);

    let result = runtime::run(&script, options, &mut inputs, &mut outputs, &program)
        .and_then(|code| outputs.flush_all().map(|()| code));
    match result {
        Ok(code) if code == 0 && inputs.had_open_error => EXIT_BAD_INPUT,
        Ok(code) => code,
        Err(Fatal { message, status }) => {
            let _ = outputs.flush_all();
            eprintln!("{program}: {message}");
            status
        }
    }
}

#[allow(dead_code)]
fn unused(e: &io::Error) -> Fatal {
    write_error("stdout", e)
}

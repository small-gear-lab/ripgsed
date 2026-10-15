//! Cycle, pattern/hold space, and command evaluation (sequential VM). No CLI argument parsing.

use std::io::{self, Write};

use crate::io::{Output, RecordSource};
use crate::syntax::{Command, Script, Substitute};

/// Options that change how a script runs.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// `-n`: do not print the pattern space at the end of each cycle.
    pub quiet: bool,
    pub separator: u8,
}

impl Default for Options {
    fn default() -> Self {
        Options { quiet: false, separator: b'\n' }
    }
}

/// Runs `script` over the records of `input` and writes the result to `output`.
pub fn run<S: RecordSource + ?Sized, W: Write>(
    script: &Script,
    options: Options,
    input: &mut S,
    output: &mut Output<W>,
) -> io::Result<()> {
    while let Some(record) = input.next_record()? {
        let mut pattern = record.body;
        for command in &script.commands {
            match command {
                Command::Substitute(sub) => substitute(sub, &mut pattern),
            }
        }
        if !options.quiet {
            output.write_record(&pattern, record.terminated)?;
        }
    }
    Ok(())
}

/// Literal substitution: replaces the first match, or every non-overlapping
/// match from the left with the `g` flag.
fn substitute(sub: &Substitute, pattern: &mut Vec<u8>) {
    let needle = &sub.pattern[..];
    let mut result = Vec::with_capacity(pattern.len());
    let mut i = 0;
    let mut replaced = false;
    while i < pattern.len() {
        if (sub.global || !replaced) && pattern[i..].starts_with(needle) {
            result.extend_from_slice(&sub.replacement);
            i += needle.len();
            replaced = true;
        } else {
            result.push(pattern[i]);
            i += 1;
        }
    }
    if replaced {
        *pattern = result;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::ReaderSource;
    use crate::syntax::parse_chunk;

    fn run_script(script: &str, input: &[u8]) -> Vec<u8> {
        let mut s = Script::default();
        parse_chunk(&mut s, script.as_bytes(), 1).unwrap();
        let mut out = Output::new(Vec::new(), b'\n');
        let mut source = ReaderSource::new(input, b'\n');
        run(&s, Options::default(), &mut source, &mut out).unwrap();
        out.flush().unwrap();
        // `Output` owns the Vec; recover it through the inner writer.
        out.into_inner()
    }

    #[test]
    fn global_and_first_only() {
        assert_eq!(run_script("s/a/b/g", b"aaa\n"), b"bbb\n");
        assert_eq!(run_script("s/a/b/", b"aaa\n"), b"baa\n");
    }

    #[test]
    fn missing_final_newline_is_kept() {
        assert_eq!(run_script("s/old/new/g", b"old\nlast old"), b"new\nlast new");
    }

    #[test]
    fn no_match_leaves_the_line() {
        assert_eq!(run_script("s/x/y/", b"abc\n"), b"abc\n");
    }
}

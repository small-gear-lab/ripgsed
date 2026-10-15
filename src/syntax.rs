//! Tokenizing, AST, source spans, and syntax diagnostics. No execution or file writes.

use std::fmt;

/// A parsed script: the in-order concatenation of every `-e` and `-f` chunk.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Script {
    pub commands: Vec<Command>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Substitute(Substitute),
}

/// `s/REGEXP/REPLACEMENT/FLAGS`, restricted to a literal regexp and a literal
/// replacement until the regex engine and the replacement syntax exist.
#[derive(Debug, PartialEq, Eq)]
pub struct Substitute {
    pub pattern: Vec<u8>,
    pub replacement: Vec<u8>,
    pub global: bool,
}

/// A syntax error, reported like GNU sed: `-e expression #N, char M: message`.
#[derive(Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub expression: usize,
    pub position: usize,
    pub message: String,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "-e expression #{}, char {}: {}", self.expression, self.position, self.message)
    }
}

/// Characters that make a BRE pattern more than a literal string.
const REGEX_SPECIAL: &[u8] = b".[]*^$\\\n";

/// Parses one `-e` argument (or script operand). `expression` is its 1-based
/// number among the script chunks, used in diagnostics.
///
/// Anything outside the supported subset is a syntax error, never an
/// approximation: unsupported syntax must not be reinterpreted.
pub fn parse_chunk(script: &mut Script, text: &[u8], expression: usize) -> Result<(), SyntaxError> {
    let err = |position: usize, message: &str| SyntaxError {
        expression,
        position,
        message: message.to_string(),
    };

    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b' ' | b'\t' | b'\n' | b';' => i += 1,
            b's' => {
                i += 1;
                let delim = *text.get(i).ok_or_else(|| err(i, "unterminated `s' command"))?;
                if delim == b'\\' || delim == b'\n' {
                    return Err(err(i + 1, "unsupported delimiter for the s command"));
                }
                i += 1;
                let (pattern, next) = read_part(text, i, delim).ok_or_else(|| err(text.len(), "unterminated `s' command"))?;
                let (replacement, next) =
                    read_part(text, next, delim).ok_or_else(|| err(text.len(), "unterminated `s' command"))?;
                i = next;
                if pattern.is_empty() {
                    return Err(err(i, "unsupported empty regular expression"));
                }
                if pattern.iter().any(|b| REGEX_SPECIAL.contains(b)) {
                    return Err(err(i, "unsupported regular expression syntax"));
                }
                if replacement.iter().any(|b| matches!(b, b'&' | b'\\' | b'\n')) {
                    return Err(err(i, "unsupported replacement syntax"));
                }
                let mut global = false;
                while i < text.len() && !matches!(text[i], b';' | b'\n' | b' ' | b'\t') {
                    match text[i] {
                        b'g' => global = true,
                        _ => return Err(err(i + 1, "unknown option to `s'")),
                    }
                    i += 1;
                }
                script.commands.push(Command::Substitute(Substitute {
                    pattern: pattern.to_vec(),
                    replacement: replacement.to_vec(),
                    global,
                }));
            }
            _ => return Err(err(i + 1, "unknown command (not supported yet)")),
        }
    }
    Ok(())
}

/// Reads up to the next unescaped `delim`; returns the part and the index after the delimiter.
fn read_part(text: &[u8], start: usize, delim: u8) -> Option<(&[u8], usize)> {
    let mut i = start;
    while i < text.len() {
        if text[i] == b'\\' {
            // Escapes are not part of the supported subset; the caller rejects them.
            i += 2;
            continue;
        }
        if text[i] == delim {
            return Some((&text[start..i], i + 1));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Script, SyntaxError> {
        let mut script = Script::default();
        parse_chunk(&mut script, s.as_bytes(), 1)?;
        Ok(script)
    }

    #[test]
    fn literal_substitution() {
        let script = parse("s/old/new/g").unwrap();
        assert_eq!(
            script.commands,
            vec![Command::Substitute(Substitute { pattern: b"old".to_vec(), replacement: b"new".to_vec(), global: true })]
        );
    }

    #[test]
    fn custom_delimiter_and_separator() {
        let script = parse("s,a,b, ; s|c|d|").unwrap();
        assert_eq!(script.commands.len(), 2);
    }

    #[test]
    fn unsupported_syntax_is_an_error() {
        assert!(parse("s/a.b/x/").is_err());
        assert!(parse("s/a/&/").is_err());
        assert!(parse("s/a/b/p").is_err());
        assert!(parse("p").is_err());
    }

    #[test]
    fn unterminated() {
        assert_eq!(parse("s/a/b").unwrap_err().message, "unterminated `s' command");
    }
}

//! The patterns of a grep run: what is a match, and where it is.

use super::options::{Kind, Options};
use crate::regex::{Captures, Locale, Regex, Syntax, Wrap};

/// A pattern that does not compile: the message of GNU grep.
pub struct PatternError(pub String);

pub struct Matcher {
    /// One regular expression for each pattern. Empty: nothing matches (`-f /dev/null`).
    regexes: Vec<Regex>,
}

/// A literal as a basic regular expression: every character that has a meaning is escaped.
fn escape_fixed(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 4);
    for &b in text {
        if matches!(b, b'\\' | b'.' | b'[' | b'*' | b'^' | b'$') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

impl Matcher {
    pub fn new(options: &Options, locale: Locale) -> Result<Matcher, PatternError> {
        let wrap = if options.line {
            Wrap::Line
        } else if options.word {
            Wrap::Word
        } else {
            Wrap::None
        };
        let syntax = Syntax {
            extended: options.kind == Kind::Extended,
            icase: options.ignore_case,
            multiline: false,
            null_data: options.null_data,
            locale,
            wrap,
            ..Syntax::default()
        };
        let mut regexes = Vec::new();
        for pattern in &options.patterns {
            let text = if options.kind == Kind::Fixed { escape_fixed(pattern) } else { pattern.clone() };
            match Regex::compile(&text, &syntax) {
                Ok(re) => regexes.push(re),
                Err(e) => return Err(PatternError(e.message)),
            }
        }
        Ok(Matcher { regexes })
    }

    /// Whether the line (without its terminator) has a match.
    pub fn is_match(&self, line: &[u8]) -> bool {
        self.regexes.iter().any(|re| re.is_match(line))
    }

    /// The leftmost match that starts at or after `start`, the longest of those that start there.
    pub fn find(&self, line: &[u8], start: usize) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        let mut caps = Captures::default();
        for re in &self.regexes {
            if re.captures_at(line, start, &mut caps) {
                let (s, e) = caps.whole();
                best = match best {
                    Some((bs, be)) if bs < s || (bs == s && be >= e) => Some((bs, be)),
                    _ => Some((s, e)),
                };
            }
        }
        best
    }
}

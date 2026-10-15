//! The regular expression engine (D7): GNU sed's BRE and ERE with POSIX
//! leftmost-longest matching, written for this project.
//!
//! The rest of the code depends on this interface only (D3). The current
//! implementation is a placeholder that accepts literal patterns; the real
//! engine replaces it behind the same API.

use std::fmt;

/// The locale a pattern is compiled and matched in (D2: `C` first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    /// Byte-wise matching, as under `LC_ALL=C`.
    #[default]
    C,
    /// UTF-8 character-wise matching.
    Utf8,
}

/// The dialect and flags a pattern is read in.
#[derive(Debug, Clone, Copy, Default)]
pub struct Syntax {
    /// ERE (`-E`/`-r`) instead of BRE.
    pub extended: bool,
    /// `--posix`: the GNU extensions are restricted.
    pub posix: bool,
    /// `POSIXLY_CORRECT` is set (affects escapes inside bracket expressions).
    pub posixly_correct: bool,
    /// The `I` flag: case-insensitive matching.
    pub icase: bool,
    /// The `M` flag: `^` and `$` also match at embedded newlines.
    pub multiline: bool,
    pub locale: Locale,
}

/// A pattern that cannot be compiled. `message` is GNU's text, for example
/// `Invalid preceding regular expression`; the caller adds the location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Capture offsets of one match: group 0 is the whole match, groups 1.. are
/// the parenthesized subexpressions. A group that did not take part is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captures {
    pub groups: Vec<Option<(usize, usize)>>,
}

impl Captures {
    pub fn get(&self, group: usize) -> Option<(usize, usize)> {
        self.groups.get(group).copied().flatten()
    }

    /// Start and end of the whole match.
    pub fn whole(&self) -> (usize, usize) {
        self.get(0).expect("a match has group 0")
    }
}

/// A compiled regular expression.
#[derive(Debug, Clone)]
pub struct Regex {
    literal: Vec<u8>,
    groups: usize,
}

impl Regex {
    /// Compiles `pattern`. Patterns outside what the engine supports are an error.
    pub fn compile(pattern: &[u8], _syntax: &Syntax) -> Result<Regex, CompileError> {
        const SPECIAL: &[u8] = b".[]*^$\\\n";
        if pattern.iter().any(|b| SPECIAL.contains(b)) {
            return Err(CompileError { message: "unsupported regular expression syntax".to_string() });
        }
        Ok(Regex { literal: pattern.to_vec(), groups: 0 })
    }

    /// Number of parenthesized subexpressions.
    pub fn group_count(&self) -> usize {
        self.groups
    }

    /// Finds the leftmost-longest match in `haystack` that starts at or after
    /// `start`. The whole of `haystack` stays visible, so anchors and word
    /// boundaries behave as in the full record. Fills `caps` on a match.
    pub fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures) -> bool {
        let n = self.literal.len();
        if n == 0 {
            caps.groups = vec![Some((start, start))];
            return start <= haystack.len();
        }
        let mut i = start;
        while i + n <= haystack.len() {
            if haystack[i..i + n] == self.literal[..] {
                caps.groups = vec![Some((i, i + n))];
                return true;
            }
            i += 1;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_match() {
        let re = Regex::compile(b"old", &Syntax::default()).unwrap();
        let mut caps = Captures::default();
        assert!(re.captures_at(b"an old one", 0, &mut caps));
        assert_eq!(caps.whole(), (3, 6));
        assert!(!re.captures_at(b"an old one", 4, &mut caps));
    }

    #[test]
    fn special_characters_are_not_silently_literal() {
        assert!(Regex::compile(b"a.c", &Syntax::default()).is_err());
    }
}

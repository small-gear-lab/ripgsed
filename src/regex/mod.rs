//! The regular expression engine (D7): GNU sed's BRE and ERE with POSIX
//! leftmost-longest matching, written for this project.
//!
//! The rest of the code depends on this interface only (D3). The
//! specification is the observable behavior of GNU sed 4.9; every rule was
//! learned by black-box experiments with it, and `tests/regex_differential.rs`
//! compares this engine with the installed GNU sed.
//!
//! # Design
//!
//! A pattern goes through three stages. `parse` reads BRE or ERE (with the
//! GNU extensions, or restricted as under `--posix`) into a tree (`ast`),
//! reporting GNU's error messages; it also decides the context rules (a
//! leading `*` is literal in BRE, `a**` is an error in BRE, `^` and `$` are
//! anchors only in certain places in BRE, back-references must name a group
//! that is complete and not in another branch of an open alternation).
//! `compile` turns the tree into a small program of ordered instructions:
//! `Split` lists its alternatives in priority order (left alternative first,
//! repetition before exit), and intervals are expanded into copies, the
//! optional copies nested, so `x{2,4}` is `x x (x (x)?)?`.
//!
//! Matching (`exec`) has two passes. The first finds the overall match, the
//! leftmost start and, for it, the longest end. Without back-references it
//! is a breadth-first simulation of the program in which every thread
//! carries the earliest start that reaches it, so it is linear in the input
//! for a given pattern. With back-references the first pass is a
//! depth-first search from each start that memoizes visited states (the
//! instruction, the position, and the registers of the referenced groups),
//! so it never takes exponential time either. The second pass assigns the
//! subexpressions: GNU sed does not follow the POSIX subexpression rules but
//! takes the first path, in priority order, that ends exactly at the overall
//! match end (so `(a|ab)(c|bcd)(d*)` on `abcd` gives `a`, `bcd`, empty). A
//! depth-first search in priority order with a visited set of (instruction,
//! position) finds that path in linear time. One more rule completes the
//! registers: when a group that is the whole body of an optional repetition
//! matches empty after it had a value, the registers are restored to the
//! snapshot taken at the last non-empty group end (`(a?){2,3}` on `a` gives
//! `\1` = `a`).
//!
//! Locales: in `C` every unit is a byte. In UTF-8 a unit is a character:
//! `.` and bracket expressions consume one valid character and never an
//! invalid byte, classes and case folding use the Unicode tables of `std`
//! (adjusted toward glibc's `C.utf8`), and a search started inside a
//! character may still match there (GNU sed's `s///g` steps by bytes).
//!
//! The pattern given to `Regex::compile` is the text between the delimiters
//! after the sed parser has processed it: `\n`, `\t` and the other GNU escapes
//! converted to bytes. They are accepted here too.

mod ast;
mod charset;
mod compile;
mod exec;
mod parse;

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

impl CompileError {
    /// Whether GNU sed prints this error with the script location
    /// (`-e expression #1, char N: `). The `[:space:]` diagnostic comes from
    /// GNU sed's second matcher and is printed as `sed: <message>` alone.
    pub fn has_location(&self) -> bool {
        self.message != parse::E_CLASS_SYNTAX
    }

    /// The exit status GNU sed uses for this error: 4 for the `[:space:]`
    /// diagnostic, 1 for the others.
    pub fn exit_status(&self) -> i32 {
        if self.has_location() { 1 } else { 4 }
    }
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
    prog: compile::Program,
    syntax: Syntax,
}

impl Regex {
    /// Compiles `pattern`. Errors carry GNU's message text.
    pub fn compile(pattern: &[u8], syntax: &Syntax) -> Result<Regex, CompileError> {
        let parsed = parse::parse(pattern, syntax)?;
        let prog = compile::compile(&parsed.node, parsed.ngroups, parsed.referenced)?;
        Ok(Regex { prog, syntax: *syntax })
    }

    /// Number of parenthesized subexpressions.
    pub fn group_count(&self) -> usize {
        self.prog.ngroups
    }

    /// Finds the leftmost-longest match in `haystack` that starts at or after
    /// `start`. The whole of `haystack` stays visible, so anchors and word
    /// boundaries behave as in the full record. Fills `caps` on a match.
    pub fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures) -> bool {
        let searcher = exec::Searcher {
            prog: &self.prog,
            hay: haystack,
            multiline: self.syntax.multiline,
            utf8: self.syntax.locale == Locale::Utf8,
            icase: self.syntax.icase,
        };
        match searcher.search(start) {
            Some((_, groups)) => {
                caps.groups = groups;
                true
            }
            None => false,
        }
    }

    /// Whether `haystack` contains a match (convenience for addresses).
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        let mut caps = Captures::default();
        self.captures_at(haystack, 0, &mut caps)
    }
}

#[cfg(test)]
mod tests;

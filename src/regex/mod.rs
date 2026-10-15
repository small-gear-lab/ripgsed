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
//! for a given pattern. With back-references it is a depth-first search from
//! each start that memoizes the states reached after consuming input
//! (instruction, position, and the registers back-references can read), so
//! it stays polynomial. The second pass assigns the subexpressions. GNU sed
//! does not follow the POSIX subexpression rules: it takes the first path,
//! in priority order, that ends exactly at the overall end (so
//! `(a|ab)(c|bcd)(d*)` on `abcd` gives `a`, `bcd`, empty). A table of states
//! from which that end is reachable is computed backwards first, and the
//! walk keeps to it, so this pass is linear too (with back-references it
//! backtracks with the same memo instead).
//!
//! The capture rules that GNU sed showed, all reproduced in the walk:
//!
//! * Priority: greedy repetition; alternatives nest to the left and the
//!   left one wins, except that an empty first alternative yields to the
//!   second (`(|a)` tries `a` first, `(a||b)` tries `a`, empty, `b`).
//! * Loops: coming back, without consuming input, to a `Split` whose
//!   preferred branch was already taken since the last character takes the
//!   other branch (an empty iteration ends the loop).
//! * When a group that is the whole body of an optional repetition matches
//!   empty after it had a value, all registers are restored from the
//!   snapshot taken at the last non-empty group end (`(a?){2,3}` on `a`
//!   gives `a`). Copies made by expanding `+` and intervals lose the marks
//!   of the repetitions inside them, and with a minimum of 0 only the first
//!   copy is marked.
//! * A path with no assertion after its last character is preferred
//!   (`(ab|a)(\b|b)` on `ab` gives `a`, `b`).
//! * `^` inside a match also matches after a newline the match consumed,
//!   and `$` before a newline it goes on to consume, but for `$` only when
//!   the pattern has no groups: with groups, a start whose match needs that
//!   is given up (not under `-z`).
//!
//! Addresses (`Regex::is_match`) follow GNU sed's other matcher: only the
//! first pass, and in `M` mode `` \` `` and `\'` behave as `^` and `$`.
//!
//! Locales: in `C` every unit is a byte. In UTF-8 a unit is a character:
//! `.` and bracket expressions consume one valid character and never an
//! invalid byte, classes and case folding use the Unicode tables of `std`
//! adjusted toward glibc's `C.utf8` (under `I` a literal matches characters
//! with the same upper-case form, and `[:upper:]`/`[:lower:]` mean
//! `[:alpha:]`), invalid bytes count as word characters for `\b` and
//! friends, which never hold inside a character. Matches may start at any
//! byte offset (GNU sed's `s/x*/-/g` splits `é`), except under `I` or when
//! the pattern has a class-like set (`[a-c]`, `[^x]`, `[[:alpha:]]`, `[é]`,
//! `\w`, `\s`...) or a word assertion: then only at character boundaries.
//!
//! # Known divergences
//!
//! Some GNU sed 4.9 results come from glibc regex quirks that are not
//! reproduced; `tests/regex_differential.rs` lists them separately:
//!
//! * back-references to a repeated group: `(a+)+\1` on `aaaa` matches
//!   `aa` twice in GNU sed (ripgsed: once, `aaaa`), `(a+){2}\1` never
//!   matches, `(a*)*\1b` on `ab` gives an empty `\1`;
//! * assertions inside a group repeated by `+` or an interval: `(^a)+`
//!   never matches in GNU sed, `(|$.a){2}` matches `_a`;
//! * groups under stacked quantifiers or `{0,n}`: `(c+){0,2}` on `ccc`
//!   gives `\1` = `c` in GNU sed (ripgsed: `ccc`), `(b?)+*` on `b` gives empty;
//! * `\B` after a repetition: `-*\B` on ` -b` matches at offset 2 in GNU sed.
//!
//! The pattern given to `Regex::compile` is the text between the delimiters
//! after the sed parser has processed it: `\n`, `\t` and the other GNU escapes
//! converted to bytes. They are accepted here too. Backslashes inside
//! bracket expressions are literal.

mod ast;
mod charset;
mod compile;
mod dfa;
mod exec;
mod literal;
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
    /// `-z`: records end at NUL. GNU sed then anchors `^` and `$` of `M` mode
    /// at NUL bytes (not newlines), `.` and `[^...]` in `M` mode match
    /// neither NUL nor newline, and `^`/`$` inside a match no longer meet a
    /// newline the match consumes.
    pub null_data: bool,
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
    delim: u8,
    literals: literal::Literals,
    /// Working memory for the breadth-first search, reused by every search.
    scratch: std::cell::RefCell<exec::Scratch>,
    /// The syntax tree, kept to build the reversed program of the DFA.
    node: ast::Node,
    dfa: std::cell::RefCell<dfa::DfaSlot>,
    /// Off to test the DFA against the simulation.
    use_dfa: bool,
}

impl Regex {
    /// Compiles `pattern`. Errors carry GNU's message text.
    pub fn compile(pattern: &[u8], syntax: &Syntax) -> Result<Regex, CompileError> {
        let delim = if syntax.null_data { b'\0' } else { b'\n' };
        Regex::compile_with_delimiter(pattern, syntax, delim)
    }

    /// Compiles `pattern` for records delimited by `delim` (`b'\n'`, or
    /// `b'\0'` as under `sed -z`; see `Syntax::null_data`). `compile` derives
    /// the delimiter from `syntax.null_data`.
    pub fn compile_with_delimiter(pattern: &[u8], syntax: &Syntax, delim: u8) -> Result<Regex, CompileError> {
        let parsed = parse::parse(pattern, syntax, delim)?;
        let prog = compile::compile(&parsed.node, parsed.ngroups, parsed.referenced, parsed.char_starts, parsed.utf8)?;
        let literals = literal::analyze(&parsed.node, syntax.locale == Locale::Utf8);
        Ok(Regex { prog, syntax: *syntax, delim, literals, scratch: Default::default(), node: parsed.node, dfa: Default::default(), use_dfa: true })
    }

    /// Number of parenthesized subexpressions.
    pub fn group_count(&self) -> usize {
        self.prog.ngroups
    }

    /// Finds the leftmost-longest match in `haystack` that starts at or after
    /// `start`. The whole of `haystack` stays visible, so anchors and word
    /// boundaries behave as in the full record. Fills `caps` on a match.
    pub fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures) -> bool {
        if start > haystack.len() {
            return false;
        }
        if let Some(lit) = &self.literals.pure {
            // A pattern that is just a literal: the leftmost occurrence is the match.
            return match lit.find(&haystack[start..]) {
                Some(i) => {
                    caps.groups = vec![Some((start + i, start + i + lit.needle().len()))];
                    true
                }
                None => false,
            };
        }
        if let Some(lit) = &self.literals.required {
            if lit.find(&haystack[start..]).is_none() {
                return false;
            }
        }
        let searcher = exec::Searcher {
            scratch: &self.scratch,
            dfa: &self.dfa,
            node: &self.node,
            use_dfa: self.use_dfa,
            prog: &self.prog,
            hay: haystack,
            multiline: self.syntax.multiline,
            delim: self.delim,
            address: false,
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

    /// Whether `haystack` contains a match, with the semantics GNU sed has
    /// for addresses (`/re/`), which it decides without asking for match
    /// positions and which differ slightly from `s`: in `M` mode `` \` ``
    /// and `\'` match at line boundaries like `^` and `$`, and a `$` inside
    /// the match may meet a newline the match consumes even when the pattern
    /// has groups. Use this for addresses and `captures_at` for `s`.
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        if let Some(lit) = &self.literals.pure {
            return lit.find(haystack).is_some();
        }
        if let Some(lit) = &self.literals.required {
            if lit.find(haystack).is_none() {
                return false;
            }
        }
        let searcher = exec::Searcher {
            scratch: &self.scratch,
            dfa: &self.dfa,
            node: &self.node,
            use_dfa: self.use_dfa,
            prog: &self.prog,
            hay: haystack,
            multiline: self.syntax.multiline,
            delim: self.delim,
            address: true,
            utf8: self.syntax.locale == Locale::Utf8,
            icase: self.syntax.icase,
        };
        searcher.exists()
    }
}

#[cfg(test)]
mod tests;

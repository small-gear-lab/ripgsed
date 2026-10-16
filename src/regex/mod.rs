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
//! `[:alpha:]`), invalid bytes count as the Latin-1 character of the same number for `\b` and
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
//! * back-references to a repeated group, in the shapes that are not covered by the rules of
//!   `Backtracker::copy_ok` (a bounded `{m,n}` is seen after `m` or `n` iterations only;
//!   a copy of the group is not seen where another copy could also end; an empty last
//!   iteration of a loop is seen as an empty group) and of `Searcher::search` (a refused empty
//!   group ends the search): `a?(a+)+\1` on `aaaaa` matches all of it in GNU sed, with `\1`
//!   = `aa`; which iteration of a group like `(a?)*` the report shows when a back-reference
//!   follows can differ (`a?(a?)+\1` on `aa`);
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
pub use charset::{beyond_len, simple_lower, simple_upper};
mod classes_table;
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
    /// UTF-8 character-wise matching with glibc's `C.utf8` tables: what the classes contain, the
    /// letter case rules, and the order of ranges are built in.
    Utf8,
    /// UTF-8 character-wise matching in any other UTF-8 locale: the classes, the letter case
    /// rules and the order of ranges come from the C library (see `crate::locale`).
    Libc,
    /// A single-byte character set other than ASCII (ISO-8859-x, KOI8-R, Windows-125x): text is
    /// read byte by byte, and the classes, the case rules, and the order of ranges come from the
    /// C library.
    Sb,
    /// A multibyte character set other than UTF-8 (EUC-JP, Shift_JIS, GBK, Big5, EUC-KR):
    /// characters are decoded with `mbrtowc`, and everything else is the C library's, as in `Libc`.
    Mb,
}

impl Locale {
    /// Whether text is read as UTF-8 characters.
    pub fn is_utf8(self) -> bool {
        matches!(self, Locale::Utf8 | Locale::Libc)
    }

    /// Whether text is read as multibyte characters (UTF-8 or another set) rather than bytes.
    pub fn is_chars(self) -> bool {
        matches!(self, Locale::Utf8 | Locale::Libc | Locale::Mb)
    }

    /// Whether the C library answers the questions about characters (classes, ranges, case).
    pub fn is_lib(self) -> bool {
        !matches!(self, Locale::C | Locale::Utf8)
    }

    /// The length of the character at the start of `bytes` (1 for a byte that is not part of a
    /// valid character).
    pub fn char_len(self, bytes: &[u8]) -> usize {
        match self {
            Locale::Utf8 | Locale::Libc => crate::runtime::char_len(bytes),
            Locale::Mb => crate::locale::mb_decode(bytes, 0).map_or(1, |(_, l)| l),
            Locale::C | Locale::Sb => 1,
        }
    }

    /// Whether the character at the start of `bytes` is a valid one.
    pub fn is_valid_char(self, bytes: &[u8]) -> bool {
        match self {
            Locale::Utf8 | Locale::Libc => crate::runtime::char_len(bytes) > 1 || bytes.first().is_some_and(|&b| b < 0x80),
            Locale::Mb => crate::locale::mb_decode(bytes, 0).is_some(),
            Locale::C | Locale::Sb => !bytes.is_empty(),
        }
    }
}

/// What `grep -w` and `grep -x` ask of a match: the whole pattern is wrapped in assertions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wrap {
    #[default]
    None,
    /// `-w`: no word character before the match or after it.
    Word,
    /// `-x`: the match is the whole text.
    Line,
}

/// The dialect and flags a pattern is read in.
#[derive(Debug, Clone, Copy, Default)]
pub struct Syntax {
    /// `grep -w` and `grep -x`.
    pub wrap: Wrap,
    /// The syntax of GNU grep (`RE_SYNTAX_GREP` and `RE_SYNTAX_EGREP`) instead of the one of GNU
    /// sed: a repetition operator may start an expression or follow another one, an invalid
    /// interval or an unmatched `)` of an ERE is literal, and the escapes of sed (`\n`, `\t`, ...)
    /// do not exist.
    pub grep: bool,
    /// With `grep`: read the pattern as the C library's matcher does, which differs from the
    /// reading of GNU grep's DFA (the one that selects the lines) in a few places: an operator
    /// after an assertion is skipped, and only the brace of an interval at the start of an
    /// expression is. The matches of `-o` and `--color` come from this reading.
    pub regex_semantic: bool,
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
    /// Reproduce the quirks of the C library's matcher (see the notes above): off by default,
    /// where the match is the one the definition gives.
    pub glibc_compat: bool,
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
    fixed: Option<(usize, Vec<(usize, usize)>)>,
    /// See `star_before_not_word_boundary`.
    quirk_stars: usize,
    dfa: std::cell::RefCell<dfa::DfaSlot>,
    /// `Mb`: boundaries found in the line of the running `s` (see `forget_boundaries`).
    boundaries: std::cell::RefCell<exec::BoundaryCache>,
    /// `Mb`: a match can end inside a character (a lone byte of 0x80 or more in the pattern, or
    /// `--glibc-compat`), so what is read at a position there stops at the end of that character.
    tails: bool,
    /// `--glibc-compat`, UTF-8: the pattern as GNU sed's first matcher reads it, with every set it
    /// cannot decide (a range, a class, a negated set, `\\w`, a back-reference) replaced by any
    /// bytes (see `reads_beyond_unicode`). `None` when no unit changes.
    relaxed: Option<Box<Regex>>,
    /// Off to test the DFA against the simulation.
    use_dfa: bool,
    /// What GNU grep warns about in the pattern (`Syntax::grep`).
    warnings: Vec<String>,
    /// `Syntax::regex_semantic`: the pattern read by the C library's matcher, for the matches.
    regex_alt: Option<Box<Regex>>,
    /// In a locale of the C library, for a pattern with a bracket expression of digit ranges: GNU
    /// sed runs a first matcher that reads those ranges by code point (`[0-9]` holds no `٣`), and
    /// a line it does not match is not matched at all. When it does match, the second matcher
    /// (collation, `٣` is in `[0-9]`) decides where. `filter` is the pattern read the first way.
    filter: Option<Box<Regex>>,
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
        let warnings = parsed.warnings.clone();
        let divergent = parsed.divergent;
        let filter = if parsed.digit_sets {
            let exact = parse::parse_filter(pattern, syntax, delim)?;
            Some(Box::new(Regex::from_parsed(exact, syntax, delim, None)?))
        } else {
            None
        };
        let mut re = Regex::from_parsed(parsed, syntax, delim, filter)?;
        re.warnings = warnings;
        if syntax.grep && divergent && !syntax.regex_semantic {
            let alt = Syntax { regex_semantic: true, ..*syntax };
            re.regex_alt = Some(Box::new(Regex::compile_with_delimiter(pattern, &alt, delim)?));
        }
        if syntax.glibc_compat && syntax.locale.is_utf8() {
            let mut again = parse::parse(pattern, syntax, delim)?;
            if relax(&mut again.node) {
                re.relaxed = Some(Box::new(Regex::from_parsed(again, syntax, delim, None)?));
            }
        }
        Ok(re)
    }

    fn from_parsed(parsed: parse::Parsed, syntax: &Syntax, delim: u8, filter: Option<Box<Regex>>) -> Result<Regex, CompileError> {
        let prog = compile::compile(&parsed.node, parsed.ngroups, parsed.referenced, parsed.char_starts, parsed.utf8, syntax.glibc_compat)?;
        let prog_has_no_backrefs = !prog.has_backrefs;
        let literals = literal::analyze(&parsed.node, syntax.locale.is_chars(), syntax.locale == Locale::Mb);
        let prog_has_lone_high_byte = prog.units.iter().any(|u| matches!(u, ast::Unit::Byte(set) if (0x80..=0xffu8).any(|b| set.contains(b))));
        Ok(Regex { prog, syntax: *syntax, delim, literals, scratch: Default::default(), quirk_stars: if syntax.glibc_compat { star_before_not_word_boundary(&parsed.node) } else { 0 }, fixed: if prog_has_no_backrefs { dfa::fixed_groups(&parsed.node, parsed.ngroups) } else { None }, node: parsed.node, dfa: Default::default(), boundaries: Default::default(), tails: syntax.locale == Locale::Mb && (syntax.glibc_compat || prog_has_lone_high_byte), relaxed: None, warnings: Vec::new(), regex_alt: None, use_dfa: syntax.locale != Locale::Mb || (crate::locale::ascii_bytes_are_characters() && !syntax.glibc_compat), filter })
    }

    /// What GNU grep warns about in the pattern, in the order it meets it.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Number of parenthesized subexpressions.
    pub fn group_count(&self) -> usize {
        self.prog.ngroups
    }

    /// Finds the leftmost-longest match in `haystack` that starts at or after
    /// `start`. The whole of `haystack` stays visible, so anchors and word
    /// boundaries behave as in the full record. Fills `caps` on a match.
    ///
    /// A search from 0 starts a new line: later calls with larger `start` on the same text share
    /// what this one learned about character boundaries (`Mb`).
    pub fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures) -> bool {
        if let Some(alt) = &self.regex_alt {
            return alt.captures_at(haystack, start, caps);
        }
        if self.reads_beyond_unicode(haystack) {
            // GNU sed's first matcher, which does not know the sequences and cannot decide the sets
            // (`relaxed`), must find a match for glibc's matcher to be asked at all (`s/./X/g`
            // leaves a line of such sequences alone). It looks at the whole line, also when the
            // search goes on from the middle of it.
            let first = self.relaxed.as_deref().unwrap_or(self);
            if !first.captures_at_once(haystack, 0, &mut Captures::default()) {
                return false;
            }
            // (glibc takes an encoded surrogate for `.` only in a pattern with no set the first matcher
            // could not decide.)
            return charset::with_ext(self.surrogate_level(), || self.captures_at_once(haystack, start, caps));
        }
        self.captures_at_once(haystack, start, caps)
    }

    /// `--glibc-compat`, UTF-8: the text has a byte that can start a sequence glibc reads as one
    /// character although it is not one (an encoded surrogate, a value above U+10FFFF).
    fn reads_beyond_unicode(&self, haystack: &[u8]) -> bool {
        self.syntax.glibc_compat && self.syntax.locale.is_utf8() && haystack.iter().any(|&b| b == 0xed || b >= 0xf4)
    }

    /// The level of `charset::with_ext` for glibc's matcher: 2 (surrogates too) or 1.
    fn surrogate_level(&self) -> u8 {
        // (glibc takes an encoded surrogate for `.` only without such a set, and never under `I`.)
        if !self.syntax.icase && !has_multibyte_set(&self.node) { 2 } else { 1 }
    }

    fn captures_at_once(&self, haystack: &[u8], start: usize, caps: &mut Captures) -> bool {
        if start == 0 {
            self.boundaries.borrow_mut().clear();
        }
        if start > haystack.len() {
            return false;
        }
        if self.filter.as_ref().is_some_and(|f| !f.is_match_once(haystack)) {
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
        if let (Some(lit), Some((_, offsets))) = (&self.literals.grouped, &self.fixed) {
            return match lit.find(&haystack[start..]) {
                Some(i) => {
                    let s0 = start + i;
                    caps.groups = offsets.iter().enumerate().map(|(g, &(a, b))| Some(if g == 0 { (s0, s0 + lit.needle().len()) } else { (s0 + a, s0 + b) })).collect();
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
            fixed: self.fixed.as_ref(),
            quirk_stars: self.quirk_stars,
            glibc: self.syntax.glibc_compat,
            use_dfa: self.use_dfa,
            prog: &self.prog,
            hay: haystack,
            multiline: self.syntax.multiline,
            delim: self.delim,
            address: false,
            utf8: self.syntax.locale.is_chars(),
            libc: matches!(self.syntax.locale, Locale::Libc | Locale::Mb),
            mb: self.syntax.locale == Locale::Mb,
            sb: self.syntax.locale == Locale::Sb,
            icase: self.syntax.icase,
            // (`--glibc-compat`: a search may start inside a character, after a match that ended there.)
            boundary: if self.syntax.glibc_compat { 0 } else { start },
            seen: &self.boundaries,
            tails: self.tails,
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
        if self.reads_beyond_unicode(haystack) {
            let first = self.relaxed.as_deref().unwrap_or(self);
            return first.is_match_once(haystack) && charset::with_ext(self.surrogate_level(), || self.is_match_once(haystack));
        }
        self.is_match_once(haystack)
    }

    fn is_match_once(&self, haystack: &[u8]) -> bool {
        if self.filter.as_ref().is_some_and(|f| !f.is_match_once(haystack)) {
            return false;
        }
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
            fixed: self.fixed.as_ref(),
            quirk_stars: self.quirk_stars,
            glibc: self.syntax.glibc_compat,
            use_dfa: self.use_dfa,
            prog: &self.prog,
            hay: haystack,
            multiline: self.syntax.multiline,
            delim: self.delim,
            address: true,
            utf8: self.syntax.locale.is_chars(),
            libc: matches!(self.syntax.locale, Locale::Libc | Locale::Mb),
            mb: self.syntax.locale == Locale::Mb,
            sb: self.syntax.locale == Locale::Sb,
            icase: self.syntax.icase,
            boundary: 0,
            seen: &exec::BoundaryCache::default().into(),
            tails: false,
        };
        searcher.exists()
    }
}

/// Whether the pattern has a set that GNU sed takes as multibyte-aware (a range written between ends,
/// a class, a negated set, a non-ASCII character, `\\w`).
fn has_multibyte_set(node: &ast::Node) -> bool {
    use ast::{Node, Unit};
    match node {
        Node::Unit(Unit::Chars(cs)) => cs.mb,
        // (A word assertion counts too: it looks at the characters around.)
        Node::Assert(a) if a.is_word() => true,
        Node::Group(_, inner) | Node::Repeat(inner, _, _) => has_multibyte_set(inner),
        Node::Concat(items) | Node::Alt(items) => items.iter().any(has_multibyte_set),
        _ => false,
    }
}

/// GNU sed's first matcher cannot decide a set whose members depend on the character set (a range
/// between different ends, a class, a negated set, `\w`) and a back-reference: it reads each as
/// any bytes. Replaces them in the tree; whether anything changed.
fn relax(node: &mut ast::Node) -> bool {
    use ast::{Node, Unit};
    let any = || Node::Repeat(Box::new(Node::Unit(Unit::Byte(charset::ByteSet::full()))), 0, None);
    match node {
        Node::Unit(Unit::Chars(cs)) => {
            // (A set of ASCII digits only is decided exactly: `[0-9]`, `[[:digit:]]`.)
            let digits = !cs.negated && cs.classes.iter().all(|c| *c == charset::Class::Digit) && cs.ranges.iter().all(|&(lo, hi)| lo >= '0' as u32 && hi <= '9' as u32);
            if (cs.negated || !cs.classes.is_empty() || cs.ranges.iter().any(|&(lo, hi)| lo != hi)) && !digits {
                *node = any();
                true
            } else {
                false
            }
        }
        Node::Backref(_) => {
            *node = any();
            true
        }
        // (The word assertions look at the characters around, which it cannot decide either.)
        Node::Assert(a) if a.is_word() => {
            *node = Node::Empty;
            true
        }
        Node::Group(_, inner) | Node::Repeat(inner, _, _) => relax(inner),
        Node::Concat(items) | Node::Alt(items) => items.iter_mut().fold(false, |changed, n| relax(n) | changed),
        _ => false,
    }
}

/// glibc quirk: for a pattern that starts with unbounded repetitions of single characters followed
/// by `\B` (`b*\B`), a match that is empty at a position where such a run could have been
/// consumed is reported after the first character of the run. Returns how many leading repetitions there are
/// (0 when the pattern is not of that form; the units of those repetitions come first).
fn star_before_not_word_boundary(node: &ast::Node) -> usize {
    use ast::{Assert, Node};
    let Node::Concat(items) = node else { return 0 };
    fn group_star(n: &Node) -> bool {
        matches!(n, Node::Group(_, inner) if star(inner))
    }
    fn star(n: &Node) -> bool {
        match n {
            Node::Repeat(inner, 0, None) => matches!(**inner, Node::Unit(_)) || star(inner) || group_star(inner),
            // `a*\+`: at least one repetition of a star.
            Node::Repeat(inner, 1, None) => star(inner),
            _ => false,
        }
    }
    let stars = items.iter().take_while(|n| star(n)).count();
    match items.get(stars) {
        Some(Node::Assert(Assert::NotWordBoundary)) if stars > 0 && items[stars + 1..].iter().all(dfa::nullable) => stars,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;

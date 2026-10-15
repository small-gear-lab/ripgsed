//! The parser for GNU BRE and ERE, with the `--posix` restrictions, producing
//! the tree in `ast`. Every rule here was taken from black-box experiments
//! with GNU sed 4.9 (see tests/regex_differential.rs), not from its source.

use super::ast::{Assert, Node, Unit};
use super::charset::{decode_utf8, ByteSet, CharSet, Class, LibHandle};
use super::{CompileError, Locale, Syntax};
use crate::locale::{LibError, LibSet};

/// The largest repetition count of an interval (`RE_DUP_MAX`).
pub const DUP_MAX: u32 = 0x7fff;

pub const E_BADRPT: &str = "Invalid preceding regular expression";
pub const E_EPAREN: &str = "Unmatched ( or \\(";
pub const E_ERPAREN: &str = "Unmatched ) or \\)";
pub const E_EBRACK: &str = "Unmatched [, [^, [:, [., or [=";
/// A pattern that ends right after `[` or `[^`.
pub const E_BADPAT: &str = "Invalid regular expression";
pub const E_SUBREG: &str = "Invalid back reference";
pub const E_ERANGE: &str = "Invalid range end";
pub const E_ECTYPE: &str = "Invalid character class name";
pub const E_BADBR: &str = "Invalid content of \\{\\}";
pub const E_EBRACE: &str = "Unmatched \\{";
pub const E_ESIZE: &str = "Regular expression too big";
pub const E_ECOLLATE: &str = "Invalid collation character";
pub const E_EESCAPE: &str = "Trailing backslash";
/// Reported by GNU sed's second matcher (not regcomp), without a location,
/// and with exit status 4.
pub const E_CLASS_SYNTAX: &str = "character class syntax is [[:space:]], not [:space:]";

fn err(msg: &str) -> CompileError {
    CompileError { message: msg.to_string() }
}

pub struct Parsed {
    pub node: Node,
    /// UTF-8 under `I`: matches start only at character boundaries (see
    /// `Program::char_starts`, which adds the multibyte-aware sets).
    pub char_starts: bool,
    pub utf8: bool,
    pub ngroups: usize,
    /// `referenced[g]` is true when `\g` occurs in the pattern.
    pub referenced: Vec<bool>,
    /// In a locale of the C library: the pattern has a bracket expression of single characters
    /// and ranges between ASCII digits, which GNU sed's first matcher reads by code point.
    pub digit_sets: bool,
}

pub fn parse(pattern: &[u8], syntax: &Syntax, delim: u8) -> Result<Parsed, CompileError> {
    parse_with(pattern, syntax, delim, false)
}

/// The pattern as GNU sed's first matcher (gnulib's dfa) reads it: a bracket expression that
/// has only single characters and ranges between ASCII digits holds exactly those code points.
/// A line that this reading does not match is not matched at all (`Regex::filter`).
pub fn parse_filter(pattern: &[u8], syntax: &Syntax, delim: u8) -> Result<Parsed, CompileError> {
    parse_with(pattern, syntax, delim, true)
}

fn parse_with(pattern: &[u8], syntax: &Syntax, delim: u8, digit_exact: bool) -> Result<Parsed, CompileError> {
    let mut p = Parser {
        digit_exact,
        digit_sets: false,
        pat: pattern,
        pos: 0,
        ere: syntax.extended,
        gnu: !syntax.posix,
        icase: syntax.icase,
        multiline: syntax.multiline,
        delim,
        utf8: syntax.locale.is_utf8(),
        libc: syntax.locale == Locale::Libc,
        ngroups: 0,
        completed: 0,
        referenced: vec![false; 10],
        class_syntax_error: false,
    };
    let node = p.parse_alt(0)?;
    if p.pos < p.pat.len() {
        // Only an unmatched close can stop the top level early.
        return Err(err(E_ERPAREN));
    }
    // The POSIXLY_CORRECT environment variable makes `[:alpha:]` an
    // ordinary bracket expression; `--posix` alone does not. (The sed layer
    // also sets `posixly_correct` under `--posix`, so the two are told
    // apart only when `posix` is off.)
    if p.class_syntax_error && !(syntax.posixly_correct && !syntax.posix) {
        return Err(err(E_CLASS_SYNTAX));
    }
    let mut referenced = p.referenced;
    referenced.resize(p.ngroups + 1, false);
    let char_starts = p.utf8 && p.icase;
    Ok(Parsed { node, ngroups: p.ngroups, referenced, char_starts, utf8: p.utf8, digit_sets: p.digit_sets })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quant {
    Star,
    Plus,
    Question,
    Brace,
}

enum Elem {
    /// A character: a byte in the `C` locale, a code point in UTF-8.
    Ch(u32),
    /// A raw byte of an invalid sequence in UTF-8.
    Raw(u8),
    Class(Class),
    Equiv(u32),
    /// In a locale of the C library: a collating symbol, or a class name this program does not
    /// know. The library decides whether it is valid and what it stands for.
    Lib,
}

struct Parser<'a> {
    pat: &'a [u8],
    pos: usize,
    ere: bool,
    gnu: bool,
    icase: bool,
    multiline: bool,
    /// The line delimiter of `M` mode (a newline, or NUL under `sed -z`).
    delim: u8,
    utf8: bool,
    /// `Locale::Libc`: the library decides what the classes, ranges and case rules mean.
    libc: bool,
    /// See `parse_filter`.
    digit_exact: bool,
    digit_sets: bool,
    ngroups: usize,
    /// Groups 1..=9 that a back-reference may refer to at this point.
    completed: u32,
    referenced: Vec<bool>,
    class_syntax_error: bool,
}

impl<'a> Parser<'a> {
    fn at(&self, k: usize) -> Option<u8> {
        self.pat.get(self.pos + k).copied()
    }

    fn peek_alt(&self) -> bool {
        if self.ere {
            self.at(0) == Some(b'|')
        } else {
            self.gnu && self.at(0) == Some(b'\\') && self.at(1) == Some(b'|')
        }
    }

    fn peek_close(&self) -> bool {
        if self.ere {
            self.at(0) == Some(b')')
        } else {
            self.at(0) == Some(b'\\') && self.at(1) == Some(b')')
        }
    }

    fn close_len(&self) -> usize {
        if self.ere { 1 } else { 2 }
    }

    fn parse_alt(&mut self, depth: usize) -> Result<Node, CompileError> {
        let initial = self.completed;
        let mut accumulated = initial;
        let mut branches = Vec::new();
        loop {
            self.completed = initial;
            let branch = self.parse_branch(depth)?;
            accumulated |= self.completed;
            branches.push(branch);
            if self.peek_alt() {
                self.pos += if self.ere { 1 } else { 2 };
                continue;
            }
            break;
        }
        self.completed = accumulated;
        Ok(if branches.len() == 1 { branches.pop().unwrap() } else { Node::Alt(branches) })
    }

    fn parse_branch(&mut self, depth: usize) -> Result<Node, CompileError> {
        let mut items = Vec::new();
        let mut first = true;
        while self.pos < self.pat.len() {
            if self.peek_alt() {
                break;
            }
            if self.peek_close() && (depth > 0 || self.gnu) {
                break;
            }
            let (node, quantifiable) = self.parse_atom(first)?;
            first = false;
            let mut node = node;
            if quantifiable {
                let mut count = 0;
                while let Some((q, len)) = self.peek_quant() {
                    if !self.ere && count > 0 && matches!(q, Quant::Star | Quant::Brace) {
                        return Err(err(E_BADRPT));
                    }
                    self.pos += len;
                    let (min, max) = match q {
                        Quant::Star => (0, None),
                        Quant::Plus => (1, None),
                        Quant::Question => (0, Some(1)),
                        Quant::Brace => self.parse_interval()?,
                    };
                    node = Node::Repeat(Box::new(node), min, max);
                    count += 1;
                }
            } else if let Some((q, _)) = self.peek_quant() {
                // After an anchor or other assertion: ERE rejects every
                // quantifier; BRE rejects an interval and reads the others
                // as literals.
                if self.ere || q == Quant::Brace {
                    return Err(err(E_BADRPT));
                }
            }
            items.push(node);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn peek_quant(&self) -> Option<(Quant, usize)> {
        let c = self.at(0)?;
        if self.ere {
            match c {
                b'*' => Some((Quant::Star, 1)),
                b'+' => Some((Quant::Plus, 1)),
                b'?' => Some((Quant::Question, 1)),
                b'{' => Some((Quant::Brace, 1)),
                _ => None,
            }
        } else {
            match (c, self.at(1)) {
                (b'*', _) => Some((Quant::Star, 1)),
                (b'\\', Some(b'{')) => Some((Quant::Brace, 2)),
                (b'\\', Some(b'+')) if self.gnu => Some((Quant::Plus, 2)),
                (b'\\', Some(b'?')) if self.gnu => Some((Quant::Question, 2)),
                _ => None,
            }
        }
    }

    /// Parses `m`, `m,`, `m,n` or `,n` and the closing brace; the opening
    /// brace is already consumed.
    fn parse_interval(&mut self) -> Result<(u32, Option<u32>), CompileError> {
        let close: &[u8] = if self.ere { b"}" } else { b"\\}" };
        let rest = &self.pat[self.pos..];
        let Some(end) = rest.windows(close.len()).position(|w| w == close) else {
            return Err(err(E_EBRACE));
        };
        let content = &rest[..end];
        self.pos += end + close.len();
        let (min_s, max_s) = match content.iter().position(|&b| b == b',') {
            Some(k) => (&content[..k], Some(&content[k + 1..])),
            None => (content, None),
        };
        let number = |s: &[u8]| -> Result<Option<u64>, CompileError> {
            if s.is_empty() {
                return Ok(None);
            }
            if !s.iter().all(u8::is_ascii_digit) {
                return Err(err(E_BADBR));
            }
            let mut v: u64 = 0;
            for &d in s {
                v = (v * 10 + (d - b'0') as u64).min(u64::from(u32::MAX));
            }
            Ok(Some(v))
        };
        let min = number(min_s)?;
        let max = match max_s {
            None => {
                if min.is_none() {
                    return Err(err(E_BADBR));
                }
                min
            }
            Some(s) => number(s)?,
        };
        let min = min.unwrap_or(0);
        if let Some(max) = max {
            if max < min {
                return Err(err(E_BADBR));
            }
        }
        if max.unwrap_or(min) > u64::from(DUP_MAX) {
            return Err(err(E_ESIZE));
        }
        Ok((min as u32, max.map(|m| m as u32)))
    }

    /// Parses one atom. Returns the node and whether a quantifier may follow.
    fn parse_atom(&mut self, first: bool) -> Result<(Node, bool), CompileError> {
        let c = self.pat[self.pos];
        if self.ere {
            match c {
                b'^' => {
                    self.pos += 1;
                    return Ok((Node::Assert(Assert::LineStart), false));
                }
                b'$' => {
                    self.pos += 1;
                    return Ok((Node::Assert(Assert::LineEnd), false));
                }
                b'*' | b'+' | b'?' | b'{' => return Err(err(E_BADRPT)),
                b'(' => {
                    self.pos += 1;
                    return self.parse_group();
                }
                _ => {}
            }
        } else {
            match c {
                b'^' if first => {
                    self.pos += 1;
                    return Ok((Node::Assert(Assert::LineStart), false));
                }
                b'$' if self.dollar_is_anchor() => {
                    self.pos += 1;
                    return Ok((Node::Assert(Assert::LineEnd), false));
                }
                _ => {}
            }
        }
        match c {
            b'.' => {
                self.pos += 1;
                Ok((Node::Unit(self.dot()), true))
            }
            b'[' => {
                self.pos += 1;
                Ok((Node::Unit(self.parse_bracket()?), true))
            }
            b'\\' => self.parse_escape(),
            _ => Ok((self.literal_here(), true)),
        }
    }

    fn dollar_is_anchor(&self) -> bool {
        let rest = &self.pat[self.pos + 1..];
        rest.is_empty() || rest.starts_with(b"\\)") || (self.gnu && rest.starts_with(b"\\|"))
    }

    fn parse_group(&mut self) -> Result<(Node, bool), CompileError> {
        self.ngroups += 1;
        let g = self.ngroups;
        let inner = self.parse_alt(1)?;
        if !self.peek_close() {
            return Err(err(E_EPAREN));
        }
        self.pos += self.close_len();
        if g <= 9 {
            self.completed |= 1 << g;
        }
        Ok((Node::Group(g, Box::new(inner)), true))
    }

    fn parse_escape(&mut self) -> Result<(Node, bool), CompileError> {
        let Some(e) = self.at(1) else {
            return Err(err(E_EESCAPE));
        };
        if !self.ere {
            match e {
                b'(' => {
                    self.pos += 2;
                    return self.parse_group();
                }
                b'{' => return Err(err(E_BADRPT)),
                _ => {}
            }
        }
        if (b'1'..=b'9').contains(&e) {
            self.pos += 2;
            let n = (e - b'0') as usize;
            if self.completed & (1 << n) == 0 {
                return Err(err(E_SUBREG));
            }
            self.referenced[n] = true;
            return Ok((Node::Backref(n), true));
        }
        if self.gnu {
            let assert = match e {
                b'b' => Some(Assert::WordBoundary),
                b'B' => Some(Assert::NotWordBoundary),
                b'<' => Some(Assert::WordStart),
                b'>' => Some(Assert::WordEnd),
                b'`' => Some(Assert::BufStart),
                b'\'' => Some(Assert::BufEnd),
                _ => None,
            };
            if let Some(a) = assert {
                self.pos += 2;
                return Ok((Node::Assert(a), false));
            }
            let class = match e {
                b'w' => Some((false, true)),
                b'W' => Some((true, true)),
                b's' => Some((false, false)),
                b'S' => Some((true, false)),
                _ => None,
            };
            if let Some((negated, word)) = class {
                self.pos += 2;
                return Ok((Node::Unit(self.shorthand(negated, word)), true));
            }
        }
        // Escapes that GNU sed converts before compiling; accepted here too.
        let simple = match e {
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'a' => Some(0x07),
            b'f' => Some(0x0c),
            b'v' => Some(0x0b),
            b'r' => Some(b'\r'),
            _ => None,
        };
        if let Some(b) = simple {
            self.pos += 2;
            return Ok((self.literal_byte(b), true));
        }
        if matches!(e, b'd' | b'o' | b'x') {
            let (radix, max) = match e {
                b'd' => (10, 3),
                b'o' => (8, 3),
                _ => (16, 2),
            };
            let mut j = self.pos + 2;
            let mut value: u32 = 0;
            while j < self.pat.len() && j - (self.pos + 2) < max {
                match (self.pat[j] as char).to_digit(radix) {
                    Some(d) => value = value * radix + d,
                    None => break,
                }
                j += 1;
            }
            if j > self.pos + 2 {
                self.pos = j;
                return Ok((self.literal_byte((value & 0xff) as u8), true));
            }
        }
        if e == b'c' {
            if let Some(x) = self.at(2) {
                self.pos += 3;
                return Ok((self.literal_byte(x.to_ascii_uppercase() ^ 0x40), true));
            }
        }
        // Anything else stands for itself: \. \* \[ \] \^ \$ \\ \/, and in
        // ERE \( \) \{ \} \| \+ \?, and the GNU letters under --posix.
        self.pos += 1;
        Ok((self.literal_here(), true))
    }

    /// The literal character at `pos` (one byte, or one UTF-8 character).
    fn literal_here(&mut self) -> Node {
        if self.utf8 {
            if let Some((c, len)) = decode_utf8(self.pat, self.pos) {
                self.pos += len;
                return self.literal_char(c);
            }
        }
        let b = self.pat[self.pos];
        self.pos += 1;
        self.literal_byte(b)
    }

    fn literal_char(&self, c: char) -> Node {
        if c.is_ascii() {
            return self.literal_byte(c as u8);
        }
        if self.icase && self.libc {
            // The library knows which characters this one stands for under `I`.
            if let Some(node) = self.lib_literal(c) {
                return node;
            }
        } else if self.icase {
            let u = super::charset::simple_upper(c) as u32;
            return Node::Unit(Unit::Chars(Box::new(CharSet {
                ranges: vec![(u, u)],
                by_upper: true,
                ..CharSet::default()
            })));
        }
        let u = c as u32;
        Node::Unit(Unit::Chars(Box::new(CharSet { ranges: vec![(u, u)], ..CharSet::default() })))
    }

    /// A character that is read under `I` in a locale of the C library: the set of characters the
    /// library's matcher takes for it.
    fn lib_literal(&self, c: char) -> Option<Node> {
        let mut buf = [0u8; 4];
        let set = LibSet::new(c.encode_utf8(&mut buf).as_bytes(), true).ok()?;
        Some(Node::Unit(Unit::Chars(Box::new(CharSet {
            ranges: vec![(c as u32, c as u32)],
            lib: Some(std::rc::Rc::new(LibHandle(set))),
            icase_singles: true,
            ..CharSet::default()
        }))))
    }

    fn literal_byte(&self, b: u8) -> Node {
        if self.icase && self.libc && b.is_ascii_alphabetic() {
            // `I` is `ı` as well as `i` in most locales, and `i` is `İ` in Turkish.
            if let Some(node) = self.lib_literal(b as char) {
                return node;
            }
        }
        if self.icase && self.utf8 && !self.libc {
            // In UTF-8 `I` also matches the letters whose upper-case form is an ASCII
            // letter: the dotless i for `i` and `I`, the long s for `s` and `S`.
            let extra = match b {
                b'i' | b'I' => Some(0x131),
                b's' | b'S' => Some(0x17f),
                _ => None,
            };
            if let Some(extra) = extra {
                let (lo, up) = (b.to_ascii_lowercase() as u32, b.to_ascii_uppercase() as u32);
                return Node::Unit(Unit::Chars(Box::new(CharSet {
                    ranges: vec![(lo, lo), (up, up), (extra, extra)],
                    ..CharSet::default()
                })));
            }
        }
        let mut set = ByteSet::single(b);
        if self.icase {
            set.fold_ascii_case();
        }
        Node::Unit(Unit::Byte(set))
    }

    fn dot(&self) -> Unit {
        if self.utf8 {
            Unit::Any { exclude: self.multiline.then_some(self.delim) }
        } else {
            let mut set = ByteSet::full();
            if self.multiline {
                set.remove(self.delim);
                set.remove(b'\n');
            }
            Unit::Byte(set)
        }
    }

    fn shorthand(&self, negated: bool, word: bool) -> Unit {
        if self.utf8 {
            let (ranges, classes) = if word {
                (vec![(b'_' as u32, b'_' as u32)], vec![Class::Alnum])
            } else {
                (Vec::new(), vec![Class::Space])
            };
            let text: &[u8] = match (word, negated) {
                (true, false) => b"[_[:alnum:]]",
                (true, true) => b"[^_[:alnum:]]",
                (false, false) => b"[[:space:]]",
                (false, true) => b"[^[:space:]]",
            };
            let lib = if self.libc { LibSet::new(text, false).ok().map(|s| std::rc::Rc::new(LibHandle(s))) } else { None };
            return Unit::Chars(Box::new(CharSet {
                negated,
                ranges,
                classes,
                mb: true,
                lib,
                ..CharSet::default()
            }));
        }
        let mut set = ByteSet::empty();
        for b in 0..=255u8 {
            let hit = if word { super::charset::is_word_byte(b) } else { Class::Space.contains_byte(b) };
            if hit {
                set.insert(b);
            }
        }
        if negated {
            // Unlike a negated bracket expression, \W and \S still match a
            // newline in M mode.
            set.negate();
        }
        Unit::Byte(set)
    }

    /// Parses a bracket expression; `pos` is just after `[`.
    fn parse_bracket(&mut self) -> Result<Unit, CompileError> {
        let mut negated = false;
        if self.at(0) == Some(b'^') {
            negated = true;
            self.pos += 1;
        }
        let content_start = self.pos;
        let bracket_start = content_start - 1 - usize::from(negated);
        // In a locale of the C library, whether the library has to be asked about this set.
        let mut needs_lib = self.libc && self.icase;
        let mut raw_seen = false;
        // GNU sed also runs the pattern through a second matcher (gnulib's dfa) that reads a
        // bracket expression of single characters and ranges between ASCII digits by code point
        // (`[0-9]` does not take `٣` or `²`, though the collation of the locale puts them between
        // `0` and `9`), and that leaves every other bracket expression to the library. The
        // elements that are not digit ranges, and whether the rest is of that kind:
        let mut kept: Vec<(usize, usize)> = Vec::new();
        let mut digit_ranges: Vec<(u32, u32)> = Vec::new();
        let mut code_point_set = true;
        let mut bytes = ByteSet::empty();
        let mut cs = CharSet::default();
        let mut first = true;
        loop {
            let Some(c) = self.at(0) else {
                return Err(err(if first { E_BADPAT } else { E_EBRACK }));
            };
            if c == b']' && !first {
                break;
            }
            first = false;
            let elem_start = self.pos;
            let elem = self.parse_elem()?;
            let is_range = self.at(0) == Some(b'-') && self.at(1).is_some() && self.at(1) != Some(b']');
            if is_range {
                self.pos += 1;
                // An invalid byte in a range stands for the character of the same number (as
                // for `\b`); only a written non-ASCII character is a collation error.
                let lo_wide = matches!(elem, Elem::Ch(c) if c > 0x7f);
                let lo = match elem {
                    Elem::Ch(c) => c,
                    Elem::Raw(b) => b as u32,
                    Elem::Lib => 0,
                    _ => return Err(err(E_ERANGE)),
                };
                needs_lib = true;
                let start_is_char = matches!(elem, Elem::Ch(_));
                // A class or an equivalence class cannot end a range, whether or not it is valid.
                if self.at(0) == Some(b'[') {
                    if let Some(kind @ (b':' | b'=')) = self.at(1) {
                        let mut k = 2;
                        while let Some(c) = self.at(k) {
                            if c == kind && self.at(k + 1) == Some(b']') {
                                return Err(err(E_ERANGE));
                            }
                            k += 1;
                        }
                    }
                }
                let end = self.parse_elem()?;
                let hi_wide = matches!(end, Elem::Ch(c) if c > 0x7f);
                let hi = match end {
                    Elem::Ch(c) => c,
                    Elem::Raw(b) => b as u32,
                    // A class or an equivalence class ends a range only in a pattern that was
                    // rejected above.
                    Elem::Lib => 0,
                    Elem::Class(_) | Elem::Equiv(_) => unreachable!("rejected as the end of a range"),
                };
                // In a locale of the C library its collation decides (below, when the whole set is
                // compiled), not the code points.
                if self.utf8 && !self.libc && (lo_wide || hi_wide) {
                    return Err(err(E_ECOLLATE));
                }
                if !self.libc && hi < lo {
                    return Err(err(E_ERANGE));
                }
                if self.at(0) == Some(b'-') && self.at(1).is_some() && self.at(1) != Some(b']') {
                    return Err(err(E_ERANGE));
                }
                if self.utf8 {
                    cs.ranges.push((lo, hi));
                } else {
                    bytes.insert_range(lo as u8, hi as u8);
                }
                let digits = |c: u32| (b'0' as u32..=b'9' as u32).contains(&c);
                if start_is_char && matches!(end, Elem::Ch(_)) && digits(lo) && digits(hi) {
                    digit_ranges.push((lo, hi));
                } else {
                    kept.push((elem_start, self.pos));
                    code_point_set &= lo == hi && !matches!(end, Elem::Lib) && start_is_char;
                }
                continue;
            }
            kept.push((elem_start, self.pos));
            code_point_set &= matches!(elem, Elem::Ch(_) | Elem::Raw(_) | Elem::Class(Class::Digit));
            match elem {
                Elem::Lib => needs_lib = true,
                Elem::Ch(c) | Elem::Equiv(c) => {
                    if matches!(elem, Elem::Equiv(_)) {
                        needs_lib = true;
                    }
                    if self.utf8 {
                        cs.ranges.push((c, c));
                    } else {
                        bytes.insert(c as u8);
                    }
                }
                // An invalid byte alone in a bracket expression matches nothing (not even itself).
                Elem::Raw(_) => raw_seen = true,
                Elem::Class(cl) => {
                    needs_lib = true;
                    if self.utf8 {
                        cs.classes.push(cl);
                    } else {
                        for b in 0..=255u8 {
                            if cl.contains_byte(b) {
                                bytes.insert(b);
                            }
                        }
                    }
                }
            }
        }
        cs.mb = negated || !cs.classes.is_empty() || cs.ranges.iter().any(|&(lo, hi)| lo != hi || hi > 0x7f);
        if self.libc && needs_lib && !raw_seen {
            // The library compiles the whole bracket expression, as GNU sed does, and decides
            // the range ends, the classes and the case rules.
            let set = LibSet::new(&self.pat[bracket_start..=self.pos], self.icase).map_err(|e| {
                err(match e {
                    LibError::Collate => E_ECOLLATE,
                    LibError::CType => E_ECTYPE,
                    LibError::Bracket => E_EBRACK,
                    LibError::Range => E_ERANGE,
                    LibError::Other => E_BADPAT,
                })
            })?;
            cs.mb = true;
            cs.icase_singles = self.icase && !negated && cs.classes.is_empty() && cs.ranges.iter().all(|&(lo, hi)| lo == hi) && kept.len() == cs.ranges.len();
            let digit_set = code_point_set && !negated && !digit_ranges.is_empty();
            self.digit_sets |= digit_set;
            if digit_set && self.digit_exact {
                // The digit ranges are code points; the rest is the library's.
                cs.lib_or = digit_ranges;
                let mut text = vec![b'['];
                kept.iter().for_each(|&(a, b)| text.extend_from_slice(&self.pat[a..b]));
                text.push(b']');
                if kept.is_empty() {
                    cs.lib = None;
                } else if let Ok(rest) = LibSet::new(&text, self.icase) {
                    cs.lib = Some(std::rc::Rc::new(LibHandle(rest)));
                } else {
                    cs.lib = Some(std::rc::Rc::new(LibHandle(set)));
                    cs.lib_or.clear();
                }
            } else {
                cs.lib = Some(std::rc::Rc::new(LibHandle(set)));
            }
        }
        let content = &self.pat[content_start..self.pos];
        if content.len() >= 2 && content[0] == b':' && content[content.len() - 1] == b':' {
            self.class_syntax_error = true;
        }
        self.pos += 1; // the closing ]
        if self.utf8 {
            cs.negated = negated;
            cs.icase = self.icase;
            cs.exclude = (negated && self.multiline).then_some(self.delim);
            return Ok(Unit::Chars(Box::new(cs)));
        }
        if self.icase {
            bytes.fold_ascii_case();
        }
        if negated {
            bytes.negate();
            if self.multiline {
                bytes.remove(self.delim);
                bytes.remove(b'\n');
            }
        }
        Ok(Unit::Byte(bytes))
    }

    fn parse_elem(&mut self) -> Result<Elem, CompileError> {
        let c = self.pat[self.pos];
        if c == b'[' {
            if let Some(kind @ (b':' | b'.' | b'=')) = self.at(1) {
                let start = self.pos + 2;
                let mut j = start;
                let mut found = None;
                while j + 1 < self.pat.len() {
                    if self.pat[j] == kind && self.pat[j + 1] == b']' {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                let Some(end) = found else {
                    return Err(err(E_EBRACK));
                };
                let name = &self.pat[start..end];
                self.pos = end + 2;
                if kind == b':' {
                    return match Class::from_name(name) {
                        Some(class) => Ok(Elem::Class(class)),
                        None if self.libc => Ok(Elem::Lib),
                        None => Err(err(E_ECTYPE)),
                    };
                }
                if self.libc {
                    return Ok(if kind == b'.' { Elem::Lib } else { Elem::Equiv(0) });
                }
                // A collating symbol or equivalence class: one single-byte character.
                if name.len() != 1 || (self.utf8 && name[0] > 0x7f) {
                    return Err(err(E_ECOLLATE));
                }
                let ch = name[0] as u32;
                return Ok(if kind == b'.' { Elem::Ch(ch) } else { Elem::Equiv(ch) });
            }
        }
        if self.utf8 {
            if let Some((ch, len)) = decode_utf8(self.pat, self.pos) {
                self.pos += len;
                return Ok(Elem::Ch(ch as u32));
            }
            self.pos += 1;
            return Ok(Elem::Raw(c));
        }
        self.pos += 1;
        Ok(Elem::Ch(c as u32))
    }
}

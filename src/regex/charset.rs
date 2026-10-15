//! Character sets: byte sets for the `C` locale, code point sets for UTF-8,
//! the POSIX character classes, and the word-character predicate.

use super::classes_table::*;

/// A set of bytes, one bit per byte value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct ByteSet {
    bits: [u64; 4],
}

impl ByteSet {
    pub const fn empty() -> ByteSet {
        ByteSet { bits: [0; 4] }
    }

    pub fn full() -> ByteSet {
        ByteSet { bits: [u64::MAX; 4] }
    }

    pub fn single(b: u8) -> ByteSet {
        let mut s = ByteSet::empty();
        s.insert(b);
        s
    }

    pub fn insert(&mut self, b: u8) {
        self.bits[(b >> 6) as usize] |= 1u64 << (b & 63);
    }

    pub fn remove(&mut self, b: u8) {
        self.bits[(b >> 6) as usize] &= !(1u64 << (b & 63));
    }

    pub fn insert_range(&mut self, lo: u8, hi: u8) {
        for b in lo..=hi {
            self.insert(b);
        }
    }

    #[inline]
    pub fn contains(&self, b: u8) -> bool {
        self.bits[(b >> 6) as usize] & (1u64 << (b & 63)) != 0
    }

    pub fn union_with(&mut self, other: &ByteSet) {
        for i in 0..4 {
            self.bits[i] |= other.bits[i];
        }
    }

    pub fn negate(&mut self) {
        for w in &mut self.bits {
            *w = !*w;
        }
    }

    pub fn is_full(&self) -> bool {
        self.bits == [u64::MAX; 4]
    }

    /// Adds the other ASCII case of every ASCII letter in the set.
    pub fn fold_ascii_case(&mut self) {
        for b in b'a'..=b'z' {
            let u = b.to_ascii_uppercase();
            if self.contains(b) || self.contains(u) {
                self.insert(b);
                self.insert(u);
            }
        }
    }

}

/// The POSIX character classes of `[[:name:]]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Alnum,
    Alpha,
    Blank,
    Cntrl,
    Digit,
    Graph,
    Lower,
    Print,
    Punct,
    Space,
    Upper,
    Xdigit,
}

impl Class {
    pub fn from_name(name: &[u8]) -> Option<Class> {
        Some(match name {
            b"alnum" => Class::Alnum,
            b"alpha" => Class::Alpha,
            b"blank" => Class::Blank,
            b"cntrl" => Class::Cntrl,
            b"digit" => Class::Digit,
            b"graph" => Class::Graph,
            b"lower" => Class::Lower,
            b"print" => Class::Print,
            b"punct" => Class::Punct,
            b"space" => Class::Space,
            b"upper" => Class::Upper,
            b"xdigit" => Class::Xdigit,
            _ => return None,
        })
    }

    /// The code points of a UTF-8 locale that are in the class, as sorted inclusive ranges.
    pub fn ranges(self) -> &'static [(u32, u32)] {
        match self {
            Class::Alnum => ALNUM,
            Class::Alpha => ALPHA,
            Class::Blank => BLANK,
            Class::Cntrl => CNTRL,
            Class::Graph => GRAPH,
            Class::Lower => LOWER,
            Class::Print => PRINT,
            Class::Punct => PUNCT,
            Class::Space => SPACE,
            Class::Upper => UPPER,
            Class::Digit => &[(0x30, 0x39)],
            Class::Xdigit => &[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)],
        }
    }

    /// Membership in the `C` locale: ASCII only, bytes 0x80..=0xff are in no class.
    pub fn contains_byte(self, b: u8) -> bool {
        match self {
            Class::Alnum => b.is_ascii_alphanumeric(),
            Class::Alpha => b.is_ascii_alphabetic(),
            Class::Blank => b == b' ' || b == b'\t',
            Class::Cntrl => b < 0x20 || b == 0x7f,
            Class::Digit => b.is_ascii_digit(),
            Class::Graph => (0x21..0x7f).contains(&b),
            Class::Lower => b.is_ascii_lowercase(),
            Class::Print => (0x20..0x7f).contains(&b),
            Class::Punct => b.is_ascii_punctuation(),
            Class::Space => matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
            Class::Upper => b.is_ascii_uppercase(),
            Class::Xdigit => b.is_ascii_hexdigit(),
        }
    }

    /// Membership in a UTF-8 locale (`C.utf8`), approximated from the Unicode
    /// properties in `std`, adjusted where glibc differs (see the tests).
    pub fn contains_char(self, c: char) -> bool {
        if c.is_ascii() {
            return self.contains_byte(c as u8);
        }
        let table = match self {
            Class::Alnum => ALNUM,
            Class::Alpha => ALPHA,
            Class::Blank => BLANK,
            Class::Cntrl => CNTRL,
            Class::Graph => GRAPH,
            Class::Lower => LOWER,
            Class::Print => PRINT,
            Class::Punct => PUNCT,
            Class::Space => SPACE,
            Class::Upper => UPPER,
            Class::Digit | Class::Xdigit => return false,
        };
        in_table(table, c as u32)
    }
}

/// Whether a code point is in a sorted table of inclusive ranges.
fn in_table(table: &[(u32, u32)], u: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if u < lo {
                std::cmp::Ordering::Greater
            } else if u > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `\w` in the `C` locale: ASCII alphanumerics and `_`.
#[inline]
pub fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || (b >= 0x80 && crate::locale::word_byte_high(b))
}

/// `\w` in a UTF-8 locale.
pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return is_word_byte(c as u8);
    }
    in_table(ALNUM, c as u32)
}

/// `\w` in a UTF-8 locale of the C library.
pub fn is_word_char_libc(c: char) -> bool {
    crate::locale::is_word(c)
}

/// Single-character lower and upper case mappings, as `towlower`/`towupper` give.
pub fn simple_lower(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        // `İ` is the only character whose full mapping has two characters; its simple mapping is `i`.
        _ => {
            debug_assert_eq!(c, '\u{130}');
            'i'
        }
    }
}

pub fn simple_upper(c: char) -> char {
    let mut it = c.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) => u,
        // The full upper case of a Greek letter with iota subscript has two characters; its
        // simple mapping is the letter with the capital and the subscript.
        _ => match c as u32 {
            v @ (0x1f80..=0x1f87 | 0x1f90..=0x1f97 | 0x1fa0..=0x1fa7) => char::from_u32(v + 8).unwrap_or(c),
            0x1fb3 => '\u{1fbc}',
            0x1fc3 => '\u{1fcc}',
            0x1ff3 => '\u{1ffc}',
            _ => c,
        },
    }
}

/// A set of characters in a UTF-8 locale (a bracket expression, or a
/// non-ASCII literal).
#[derive(Clone, Debug, Default)]
pub struct CharSet {
    pub negated: bool,
    /// Inclusive code point ranges (single characters are ranges of one).
    pub ranges: Vec<(u32, u32)>,
    pub classes: Vec<Class>,
    pub icase: bool,
    /// A byte (the line delimiter) the set never matches (negated lists in `M` mode).
    pub exclude: Option<u8>,
    /// A case-insensitive literal: the ranges hold upper-case forms and a
    /// character matches when its upper-case form is in them (GNU sed: `ß`
    /// with `I` does not match `ẞ`, `ǆ` matches `ǅ`).
    pub by_upper: bool,
    /// GNU sed treats this set as multibyte-aware: a pattern that uses one
    /// starts matches only at character boundaries (`Program::char_starts`).
    pub mb: bool,
    /// In a locale of the C library (`Locale::Libc`): the library's answer for the whole set,
    /// which then replaces `ranges`, `classes` and the rest (they only keep the shape of the
    /// set for `mb`).
    pub lib: Option<std::rc::Rc<LibHandle>>,
    /// With `lib`: code point ranges that are members whatever the library says.
    pub lib_or: Vec<(u32, u32)>,
    /// With `lib` and `I`: the set is a literal letter or a bracket expression of single
    /// characters. GNU sed's second matcher then ignores the small Cyrillic variants
    /// U+1C80..U+1C88 (`в` does not match `ᲀ`, though both have the capital `В`) unless the
    /// pattern writes the variant itself.
    pub icase_singles: bool,
}

/// A set compiled by the C library, shared by the copies of a `CharSet`.
pub struct LibHandle(pub crate::locale::LibSet);

impl std::fmt::Debug for LibHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LibHandle")
    }
}

impl CharSet {
    fn positive(&self, c: char) -> bool {
        let u = c as u32;
        if self.ranges.iter().any(|&(lo, hi)| lo <= u && u <= hi) {
            return true;
        }
        self.classes.iter().any(|&cl| {
            // Under `I`, [:upper:] and [:lower:] stand for [:alpha:].
            let cl = if self.icase && matches!(cl, Class::Upper | Class::Lower) { Class::Alpha } else { cl };
            cl.contains_char(c)
        })
    }

    /// Like `matches_char`, for a character of a multibyte set written as `bytes`: the library is
    /// asked about those bytes.
    pub fn matches_seq(&self, c: char, bytes: &[u8]) -> bool {
        if self.exclude.is_some_and(|b| c as u32 == b as u32 || c == '\n') {
            return false;
        }
        if let Some(lib) = &self.lib {
            let u = c as u32;
            return lib.0.contains_seq(bytes) || self.lib_or.iter().any(|&(lo, hi)| lo <= u && u <= hi);
        }
        self.matches_char(c)
    }

    pub fn matches_char(&self, c: char) -> bool {
        if self.exclude.is_some_and(|b| c as u32 == b as u32 || c == '\n') {
            return false;
        }
        if let Some(lib) = &self.lib {
            let u = c as u32;
            if self.icase_singles && (0x1c80..=0x1c88).contains(&u) && !self.ranges.iter().any(|&(lo, hi)| lo <= u && u <= hi) {
                return false;
            }
            return lib.0.contains(c) || self.lib_or.iter().any(|&(lo, hi)| lo <= u && u <= hi);
        }
        if self.by_upper {
            return self.positive(simple_upper(c)) != self.negated;
        }
        let mut hit = self.positive(c);
        if !hit && self.icase {
            // Two characters are the same under `I` when their upper-case forms are equal:
            // try the other members of the family of `c`.
            let u = simple_upper(c);
            let l = simple_lower(u);
            let others = [
                Some(u),
                (simple_upper(l) == u).then_some(l),
                titlecase_of(u),
                (u == 'I').then_some('\u{131}'),
                (u == 'S').then_some('\u{17f}'),
            ];
            hit = others.into_iter().flatten().any(|x| x != c && self.positive(x));
        }
        hit != self.negated
    }
}

/// The titlecase letter of the digraphs DŽ, LJ, NJ, and DZ.
fn titlecase_of(upper: char) -> Option<char> {
    match upper as u32 {
        0x1c4 | 0x1c6 => Some('\u{1c5}'),
        0x1c7 | 0x1c9 => Some('\u{1c8}'),
        0x1ca | 0x1cc => Some('\u{1cb}'),
        0x1f1 | 0x1f3 => Some('\u{1f2}'),
        _ => None,
    }
}

/// Decodes one UTF-8 character at `pos`. Overlong forms, surrogates, and
/// truncated sequences are invalid.
#[inline]
pub fn decode_utf8(hay: &[u8], pos: usize) -> Option<(char, usize)> {
    let b0 = *hay.get(pos)?;
    if b0 < 0x80 {
        return Some((b0 as char, 1));
    }
    let (len, min, init) = match b0 {
        0xc2..=0xdf => (2, 0x80, (b0 & 0x1f) as u32),
        0xe0..=0xef => (3, 0x800, (b0 & 0x0f) as u32),
        0xf0..=0xf4 => (4, 0x10000, (b0 & 0x07) as u32),
        _ => return None,
    };
    if pos + len > hay.len() {
        return None;
    }
    let mut cp = init;
    for k in 1..len {
        let b = hay[pos + k];
        if b & 0xc0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | (b & 0x3f) as u32;
    }
    if cp < min {
        return None;
    }
    char::from_u32(cp).map(|c| (c, len))
}

/// Decodes the character that ends exactly at `pos`, if any.
pub fn decode_utf8_before(hay: &[u8], pos: usize) -> Option<(char, usize)> {
    for len in 1..=4usize {
        if len > pos {
            break;
        }
        let start = pos - len;
        if let Some((c, l)) = decode_utf8(hay, start) {
            if l == len {
                return Some((c, l));
            }
        }
        if hay[start] & 0xc0 != 0x80 {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byteset_basics() {
        let mut s = ByteSet::empty();
        s.insert_range(b'a', b'c');
        assert!(s.contains(b'b') && !s.contains(b'd'));
        s.fold_ascii_case();
        assert!(s.contains(b'B'));
        s.negate();
        assert!(!s.contains(b'B') && s.contains(0xff));
    }

    #[test]
    fn decode() {
        assert_eq!(decode_utf8("é".as_bytes(), 0), Some(('é', 2)));
        assert_eq!(decode_utf8(b"\xc0\xaf", 0), None);
        assert_eq!(decode_utf8(b"\xe4\xb8", 0), None);
        assert_eq!(decode_utf8_before("aé".as_bytes(), 3), Some(('é', 2)));
        assert_eq!(decode_utf8_before("aé".as_bytes(), 2), None);
    }

    #[test]
    fn decode_rejects_overlong_and_broken_forms() {
        assert_eq!(decode_utf8(b"\xe0\x80\x80", 0), None);
        assert_eq!(decode_utf8(b"\xf0\x80\x80\x80", 0), None);
        assert_eq!(decode_utf8(b"\xe4\x41\x80", 0), None);
        assert_eq!(decode_utf8(b"\xed\xa0\x80", 0), None);
    }

    #[test]
    fn titlecase_digraphs() {
        assert_eq!(titlecase_of(char::from_u32(0x1c4).unwrap()), Some('\u{1c5}'));
        assert_eq!(titlecase_of(char::from_u32(0x1c7).unwrap()), Some('\u{1c8}'));
        assert_eq!(titlecase_of(char::from_u32(0x1ca).unwrap()), Some('\u{1cb}'));
        assert_eq!(titlecase_of(char::from_u32(0x1f1).unwrap()), Some('\u{1f2}'));
        assert_eq!(titlecase_of(char::from_u32(0x41).unwrap()), None);
    }

    #[test]
    fn non_ascii_membership_of_every_class() {
        let classes = [
            (Class::Alnum, 'é', true),
            (Class::Alpha, 'é', true),
            (Class::Blank, '\u{3000}', true),
            (Class::Cntrl, '\u{85}', true),
            (Class::Digit, '٣', false),
            (Class::Graph, 'é', true),
            (Class::Lower, 'é', true),
            (Class::Print, 'é', true),
            (Class::Punct, '½', true),
            (Class::Space, '\u{3000}', true),
            (Class::Upper, 'É', true),
            (Class::Xdigit, 'é', false),
        ];
        for (class, c, want) in classes {
            assert_eq!(class.contains_char(c), want, "{class:?} {c:?}");
        }
        assert_eq!(simple_lower('\u{130}'), 'i');
        assert_eq!(simple_lower('É'), 'é');
    }
}

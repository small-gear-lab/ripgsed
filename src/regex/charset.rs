//! Character sets: byte sets for the `C` locale, code point sets for UTF-8,
//! the POSIX character classes, and the word-character predicate.

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
        match self {
            Class::Alnum => uni_alpha(c),
            Class::Alpha => uni_alpha(c),
            Class::Blank => uni_space(c) && !matches!(c as u32, 0x85 | 0x2028 | 0x2029),
            Class::Cntrl => c.is_control() || matches!(c as u32, 0x2028 | 0x2029),
            Class::Digit => false,
            Class::Graph => uni_print(c) && !uni_space(c),
            // glibc also lists the four titlecase digraphs among the lower-case letters.
            Class::Lower => c.is_lowercase() || matches!(c as u32, 0x1c5 | 0x1c8 | 0x1cb | 0x1f2),
            Class::Print => uni_print(c),
            Class::Punct => uni_print(c) && !uni_space(c) && !uni_alpha(c),
            Class::Space => uni_space(c),
            // ... and every titlecase letter among the upper-case ones.
            Class::Upper => c.is_uppercase() || is_titlecase(c),
            Class::Xdigit => false,
        }
    }
}

/// Numbers that are neither letters nor decimal digits (superscripts, fractions, circled
/// digits, ...): `std` counts them as numeric, glibc's `alpha` class does not.
const OTHER_NUMBERS: &[(u32, u32)] = &[
    (0xB2, 0xB3), (0xB9, 0xB9), (0xBC, 0xBE), (0x9F4, 0x9F9), (0xB72, 0xB77), (0xBF0, 0xBF2), (0xC78, 0xC7E),
    (0xD58, 0xD5E), (0xD70, 0xD78), (0xF2A, 0xF33), (0x1369, 0x137C), (0x17F0, 0x17F9), (0x19DA, 0x19DA),
    (0x2070, 0x2070), (0x2074, 0x2079), (0x2080, 0x2089), (0x2150, 0x215F), (0x2189, 0x2189), (0x2460, 0x249B),
    (0x24EA, 0x24FF), (0x2776, 0x2793), (0x2CFD, 0x2CFD), (0x3192, 0x3195), (0x3220, 0x3229), (0x3248, 0x324F),
    (0x3251, 0x325F), (0x3280, 0x3289), (0x32B1, 0x32BF), (0xA830, 0xA835), (0x10107, 0x10133), (0x10175, 0x10178),
    (0x1018A, 0x1018B), (0x102E1, 0x102FB), (0x10320, 0x10323), (0x10858, 0x1085F), (0x10879, 0x1087F),
    (0x108A7, 0x108AF), (0x108FB, 0x108FF), (0x10916, 0x1091B), (0x109BC, 0x109BD), (0x109C0, 0x109CF),
    (0x109D2, 0x109FF), (0x10A40, 0x10A48), (0x10A7D, 0x10A7E), (0x10A9D, 0x10A9F), (0x10AEB, 0x10AEF),
    (0x10B58, 0x10B5F), (0x10B78, 0x10B7F), (0x10BA9, 0x10BAF), (0x10CFA, 0x10CFF), (0x10E60, 0x10E7E),
    (0x10F1D, 0x10F26), (0x10F51, 0x10F54), (0x10FC5, 0x10FCB), (0x11052, 0x11065), (0x111E1, 0x111F4),
    (0x1173A, 0x1173B), (0x118EA, 0x118F2), (0x11C5A, 0x11C6C), (0x11FC0, 0x11FD4), (0x16B5B, 0x16B61),
    (0x16E80, 0x16E96), (0x1D2C0, 0x1D2D3), (0x1D2E0, 0x1D2F3), (0x1D360, 0x1D378), (0x1E8C7, 0x1E8CF),
    (0x1EC71, 0x1ECAB), (0x1ECAD, 0x1ECAF), (0x1ECB1, 0x1ECB4), (0x1ED01, 0x1ED2D), (0x1ED2F, 0x1ED3D),
    (0x1F100, 0x1F10C),
];

fn uni_alpha(c: char) -> bool {
    let u = c as u32;
    c.is_alphabetic() || (c.is_numeric() && !OTHER_NUMBERS.iter().any(|&(lo, hi)| lo <= u && u <= hi))
}

/// The letters of general category Lt.
fn is_titlecase(c: char) -> bool {
    matches!(
        c as u32,
        0x1c5 | 0x1c8 | 0x1cb | 0x1f2 | 0x1f88..=0x1f8f | 0x1f98..=0x1f9f | 0x1fa8..=0x1faf | 0x1fbc | 0x1fcc | 0x1ffc
    )
}

fn uni_space(c: char) -> bool {
    c.is_whitespace() && !matches!(c as u32, 0x85 | 0xa0 | 0x2007 | 0x202f)
}

fn uni_print(c: char) -> bool {
    !c.is_control() && !matches!(c as u32, 0x2028 | 0x2029) && !is_unassigned_or_special(c)
}

fn is_unassigned_or_special(c: char) -> bool {
    let u = c as u32;
    // Noncharacters and private-use planes beyond the BMP are treated as printable
    // by glibc's C.utf8 too; only surrogates (impossible in `char`) are excluded.
    (0xfdd0..=0xfdef).contains(&u) || (u & 0xfffe) == 0xfffe
}

/// `\w` in the `C` locale: ASCII alphanumerics and `_`.
#[inline]
pub fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `\w` in a UTF-8 locale.
pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return is_word_byte(c as u8);
    }
    uni_alpha(c)
}

/// Single-character lower and upper case mappings, as `towlower`/`towupper` give.
pub fn simple_lower(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

pub fn simple_upper(c: char) -> char {
    let mut it = c.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) => u,
        _ => c,
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
    /// Bytes of invalid sequences written in the pattern; they match those
    /// single bytes of the input.
    pub raw: ByteSet,
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

    pub fn matches_char(&self, c: char) -> bool {
        if self.exclude.is_some_and(|b| c as u32 == b as u32 || c == '\n') {
            return false;
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
}

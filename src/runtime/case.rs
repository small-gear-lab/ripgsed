//! Building the replacement text of `s`, including `\U \L \u \l \E`.

use crate::regex::{Captures, Locale};
use crate::syntax::{CaseConversion, ReplacementPart};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    None,
    Upper,
    Lower,
}

pub fn append_replacement(out: &mut Vec<u8>, parts: &[ReplacementPart], subject: &[u8], caps: &Captures, locale: Locale) {
    let mut mode = Mode::None;
    let mut next = Mode::None;
    for part in parts {
        match part {
            ReplacementPart::Case(c) => match c {
                CaseConversion::Upper => {
                    mode = Mode::Upper;
                    next = Mode::None;
                }
                CaseConversion::Lower => {
                    mode = Mode::Lower;
                    next = Mode::None;
                }
                CaseConversion::UpperNext => next = Mode::Upper,
                CaseConversion::LowerNext => next = Mode::Lower,
                CaseConversion::Stop => {
                    mode = Mode::None;
                    next = Mode::None;
                }
            },
            ReplacementPart::Literal(text) => push(out, text, &mut mode, &mut next, locale),
            ReplacementPart::Group(g) => {
                if let Some((s, e)) = caps.get(*g) {
                    push(out, &subject[s..e], &mut mode, &mut next, locale);
                }
            }
        }
    }
}

fn push(out: &mut Vec<u8>, text: &[u8], mode: &mut Mode, next: &mut Mode, locale: Locale) {
    if text.is_empty() {
        return;
    }
    if *mode == Mode::None && *next == Mode::None {
        out.extend_from_slice(text);
        return;
    }
    let mut i = 0;
    while i < text.len() {
        let len = match locale {
            Locale::Utf8 | Locale::Libc => super::char_len(&text[i..]),
            Locale::C => 1,
        };
        // In a multibyte locale GNU sed converts a piece of text up to the first NUL or the first
        // byte that is not part of a character; the rest of the piece is copied as it is. The
        // character where it stops still uses up a pending `\u` or `\l`.
        if locale.is_utf8() && (text[i] == 0 || (len == 1 && text[i] >= 0x80)) {
            *next = Mode::None;
            out.extend_from_slice(&text[i..]);
            return;
        }
        let ch = &text[i..i + len];
        let how = if *next != Mode::None { std::mem::replace(next, Mode::None) } else { *mode };
        convert(out, ch, how, locale);
        i += len;
    }
}

fn convert(out: &mut Vec<u8>, ch: &[u8], how: Mode, locale: Locale) {
    if locale == Locale::Libc && how != Mode::None {
        // The C library decides, also for ASCII (in Turkish `i` becomes `İ`).
        let c = std::str::from_utf8(ch).ok().and_then(|s| s.chars().next()).expect("a valid UTF-8 character");
        let c = if how == Mode::Upper { crate::locale::upper(c) } else { crate::locale::lower(c) };
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        return;
    }
    if ch.len() == 1 {
        let b = ch[0];
        out.push(match how {
            Mode::None => b,
            // In the C locale GNU sed turns a byte it cannot convert into 0xFF.
            _ if b >= 0x80 && locale == Locale::C => 0xff,
            Mode::Upper => b.to_ascii_uppercase(),
            Mode::Lower => b.to_ascii_lowercase(),
        });
        return;
    }
    // One character maps to one character (like towupper): `ß` stays `ß`, not `SS`.
    // `char_len` hands out whole, valid characters when it gives more than one byte.
    let c = std::str::from_utf8(ch).ok().and_then(|s| s.chars().next()).expect("a valid UTF-8 character");
    let c = match how {
        Mode::Upper => crate::regex::simple_upper(c),
        Mode::Lower => crate::regex::simple_lower(c),
        Mode::None => c,
    };
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
}

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
            Locale::Utf8 => super::char_len(&text[i..]),
            Locale::C => 1,
        };
        let ch = &text[i..i + len];
        let how = if *next != Mode::None { std::mem::replace(next, Mode::None) } else { *mode };
        convert(out, ch, how);
        i += len;
    }
}

fn convert(out: &mut Vec<u8>, ch: &[u8], how: Mode) {
    if ch.len() == 1 {
        out.push(match how {
            Mode::Upper => ch[0].to_ascii_uppercase(),
            Mode::Lower => ch[0].to_ascii_lowercase(),
            Mode::None => ch[0],
        });
        return;
    }
    match (std::str::from_utf8(ch), how) {
        (Ok(s), Mode::Upper) => out.extend(s.to_uppercase().bytes()),
        (Ok(s), Mode::Lower) => out.extend(s.to_lowercase().bytes()),
        _ => out.extend_from_slice(ch),
    }
}

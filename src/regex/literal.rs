//! Literals a pattern needs, for the prefilter: if a pattern is nothing but a literal, a
//! substring search answers it; if it needs some literal, a haystack without it cannot match.

use super::ast::{Node, Unit};
use super::charset::ByteSet;
use crate::search::{Finder, FoldFinder};

/// How the bytes of a pattern are read: as they are, as UTF-8, or as a multibyte character set
/// of the locale (only ASCII characters are looked for).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Bytes,
    Utf8,
    Mb,
}

/// What the pattern tells about literal text.
#[derive(Debug, Clone, Default)]
pub struct Literals {
    /// The whole pattern is this literal (no groups, assertions, or repetition).
    pub pure: Option<Finder>,
    /// Every match contains this literal.
    pub required: Option<Finder>,
    /// When there is no `required` literal because of `I`: every match contains this text in
    /// lower case, whatever the case of its ASCII letters is.
    pub required_fold: Option<FoldFinder>,
    /// The pattern is this literal with groups around parts of it (no repetition, alternation,
    /// or assertion): the match is the literal, and the groups are fixed offsets of it.
    pub grouped: Option<Finder>,
}

impl ByteSet {
    /// The byte, if the set has exactly one member.
    pub fn single_member(&self) -> Option<u8> {
        let mut found = None;
        for b in 0..=255u8 {
            if self.contains(b) {
                if found.is_some() {
                    return None;
                }
                found = Some(b);
            }
        }
        found
    }
}

/// `ascii_only`: in UTF-8 a byte of 0x80 or more may be part of a character, so only ASCII
/// bytes are used as literals there.
pub fn analyze(node: &Node, utf8: bool, mb: bool) -> Literals {
    let ascii_only = if mb { Mode::Mb } else if utf8 { Mode::Utf8 } else { Mode::Bytes };
    let mut lits = Literals::default();
    if ascii_only == Mode::Mb {
        // Only the necessary literal: a match of literal bytes may be part of another character
        // (the second byte of Shift_JIS, GBK or Big5), so a match is never decided by it, but
        // a text without the bytes of an ASCII character cannot match.
        lits.required = required(node, ascii_only).filter(|l| !l.is_empty()).map(|l| Finder::new(&l));
        if !crate::locale::ascii_is_identity() {
            // An ASCII byte need not be the ASCII character (Shift_JIS 0x5c): look for nothing.
            lits.required = None;
        }
        return lits;
    }
    if let Some(bytes) = pure(node, ascii_only) {
        // The parser gives no empty concatenation.
        debug_assert!(!bytes.is_empty());
        lits.pure = Some(Finder::new(&bytes));
        lits.required = Some(Finder::new(&bytes));
        return lits;
    }
    if let Some(bytes) = literal_of(node, ascii_only).filter(|b| !b.is_empty()) {
        lits.grouped = Some(Finder::new(&bytes));
    }
    lits.required = required(node, ascii_only).filter(|l| !l.is_empty()).map(|l| Finder::new(&l));
    if lits.required.is_none() {
        lits.required_fold = required_fold(node, ascii_only).filter(|l| !l.is_empty()).map(|l| FoldFinder::new(&l));
    }
    lits
}

/// The byte (lower case) of a unit that is one ASCII letter in either case, or one other byte.
fn folded_single(node: &Node, ascii_only: Mode) -> Option<u8> {
    match node {
        Node::Unit(Unit::Byte(set)) => {
            let mut members = (0..=255u8).filter(|&b| set.contains(b));
            let first = members.next()?;
            match (members.next(), members.next()) {
                (None, _) => (ascii_only == Mode::Bytes || first < 0x80).then_some(first.to_ascii_lowercase()),
                (Some(second), None) if first.is_ascii_alphabetic() && second == first ^ 0x20 => Some(first.to_ascii_lowercase()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn folded_literal_of(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Empty => Some(Vec::new()),
        Node::Unit(_) => folded_single(node, ascii_only).map(|b| vec![b]),
        Node::Group(_, inner) => folded_literal_of(inner, ascii_only),
        Node::Concat(children) => {
            let mut out = Vec::new();
            for child in children {
                out.extend(folded_literal_of(child, ascii_only)?);
            }
            Some(out)
        }
        _ => None,
    }
}

/// Like `required`, with the ASCII letters in either case read as their lower-case letter.
fn required_fold(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Unit(_) => folded_single(node, ascii_only).map(|b| vec![b]),
        Node::Group(_, inner) => required_fold(inner, ascii_only),
        Node::Repeat(inner, min, _) if *min >= 1 => required_fold(inner, ascii_only),
        Node::Concat(children) => {
            let mut best: Vec<u8> = Vec::new();
            let mut run: Vec<u8> = Vec::new();
            let flush = |run: &mut Vec<u8>, best: &mut Vec<u8>| {
                if run.len() > best.len() {
                    *best = std::mem::take(run);
                } else {
                    run.clear();
                }
            };
            for child in children {
                if let Some(bytes) = folded_literal_of(child, ascii_only) {
                    run.extend(bytes);
                    continue;
                }
                flush(&mut run, &mut best);
                if let Some(inner) = required_fold(child, ascii_only) {
                    if inner.len() > best.len() {
                        best = inner;
                    }
                }
            }
            flush(&mut run, &mut best);
            (!best.is_empty()).then_some(best)
        }
        _ => None,
    }
}

/// The bytes a node matches when it is one literal character: a byte, or (in UTF-8) a single
/// non-ASCII character written without `I`, whose encoding cannot start inside another one.
fn single(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Unit(Unit::Byte(set)) => set.single_member().filter(|&b| ascii_only == Mode::Bytes || b < 0x80).map(|b| vec![b]),
        Node::Unit(Unit::Chars(cs)) if ascii_only == Mode::Utf8 => {
            let plain = !cs.negated
                && cs.classes.is_empty()
                && !cs.icase
                && !cs.by_upper
                && cs.lib.is_none()
                && cs.exclude.is_none()
                && !cs.mb;
            match (plain, cs.ranges.as_slice()) {
                (true, &[(lo, hi)]) if lo == hi => {
                    let c = char::from_u32(lo)?;
                    let mut buf = [0u8; 4];
                    Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The literal, if the node is only single-byte units.
fn pure(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Concat(children) => {
            let mut out = Vec::new();
            for c in children {
                out.extend(single(c, ascii_only)?);
            }
            Some(out)
        }
        other => single(other, ascii_only),
    }
}

/// The longest literal that every match must contain.
/// The text of a node that is nothing but literal bytes (groups included).
fn literal_of(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Empty => Some(Vec::new()),
        Node::Unit(_) => single(node, ascii_only),
        Node::Group(_, inner) => literal_of(inner, ascii_only),
        Node::Concat(children) => {
            let mut out = Vec::new();
            for child in children {
                out.extend(literal_of(child, ascii_only)?);
            }
            Some(out)
        }
        _ => None,
    }
}

fn required(node: &Node, ascii_only: Mode) -> Option<Vec<u8>> {
    match node {
        Node::Unit(_) => single(node, ascii_only),
        Node::Group(_, inner) => required(inner, ascii_only),
        // At least one copy of the body is always there.
        Node::Repeat(inner, min, _) if *min >= 1 => required(inner, ascii_only),
        Node::Concat(children) => {
            let mut best: Vec<u8> = Vec::new();
            let mut run: Vec<u8> = Vec::new();
            let flush = |run: &mut Vec<u8>, best: &mut Vec<u8>| {
                if run.len() > best.len() {
                    *best = std::mem::take(run);
                } else {
                    run.clear();
                }
            };
            for child in children {
                if let Some(bytes) = literal_of(child, ascii_only) {
                    run.extend(bytes);
                    continue;
                }
                flush(&mut run, &mut best);
                if let Some(inner) = required(child, ascii_only) {
                    if inner.len() > best.len() {
                        best = inner;
                    }
                }
            }
            flush(&mut run, &mut best);
            (!best.is_empty()).then_some(best)
        }
        _ => None,
    }
}


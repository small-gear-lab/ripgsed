//! Literals a pattern needs, for the prefilter: if a pattern is nothing but a literal, a
//! substring search answers it; if it needs some literal, a haystack without it cannot match.

use super::ast::{Node, Unit};
use super::charset::ByteSet;

/// What the pattern tells about literal text.
#[derive(Debug, Clone, Default)]
pub struct Literals {
    /// The whole pattern is this literal (no groups, assertions, or repetition).
    pub pure: Option<Vec<u8>>,
    /// Every match contains this literal.
    pub required: Option<Vec<u8>>,
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
pub fn analyze(node: &Node, ascii_only: bool) -> Literals {
    let mut lits = Literals::default();
    if let Some(bytes) = pure(node, ascii_only) {
        if !bytes.is_empty() {
            lits.pure = Some(bytes.clone());
            lits.required = Some(bytes);
            return lits;
        }
    }
    lits.required = required(node, ascii_only).filter(|l| !l.is_empty());
    lits
}

fn single(node: &Node, ascii_only: bool) -> Option<u8> {
    match node {
        Node::Unit(Unit::Byte(set)) => set.single_member().filter(|&b| !ascii_only || b < 0x80),
        _ => None,
    }
}

/// The literal, if the node is only single-byte units.
fn pure(node: &Node, ascii_only: bool) -> Option<Vec<u8>> {
    match node {
        Node::Concat(children) => children.iter().map(|c| single(c, ascii_only)).collect(),
        other => single(other, ascii_only).map(|b| vec![b]),
    }
}

/// The longest literal that every match must contain.
fn required(node: &Node, ascii_only: bool) -> Option<Vec<u8>> {
    match node {
        Node::Unit(_) => single(node, ascii_only).map(|b| vec![b]),
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
                if let Some(b) = single(child, ascii_only) {
                    run.push(b);
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

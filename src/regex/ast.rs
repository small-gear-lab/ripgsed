//! The syntax tree a pattern is parsed into.

use super::charset::{ByteSet, CharSet};

/// Something that consumes input: one byte, or one character in UTF-8.
#[derive(Clone, Debug)]
pub enum Unit {
    /// One byte from the set. In the `C` locale every literal, `.`, and
    /// bracket expression is a byte set; in UTF-8 it is used for ASCII and
    /// for raw bytes written in the pattern.
    Byte(ByteSet),
    /// `.` in UTF-8: one valid character (not a newline in `M` mode).
    Any { newline: bool },
    /// A character set in UTF-8.
    Chars(Box<CharSet>),
}

/// Zero-width assertions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assert {
    /// `^`: buffer start, or after a newline in `M` mode.
    LineStart,
    /// `$`: buffer end, or before a newline in `M` mode.
    LineEnd,
    /// `` \` ``
    BufStart,
    /// `\'`
    BufEnd,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
    /// `\<`
    WordStart,
    /// `\>`
    WordEnd,
}

#[derive(Clone, Debug)]
pub enum Node {
    Empty,
    Unit(Unit),
    Assert(Assert),
    /// A parenthesized subexpression with its number (1-based).
    Group(usize, Box<Node>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    /// `min` to `max` (unbounded when `None`) repetitions.
    Repeat(Box<Node>, u32, Option<u32>),
    Backref(usize),
}

//! Tokenizing, AST, source spans, and syntax diagnostics. No execution or file writes.

mod parser;

use std::fmt;
use std::rc::Rc;

pub use parser::{parse, Chunk, ChunkSource, ParseOptions};

use crate::regex::Regex;

/// A parsed script: the in-order concatenation of every `-e` and `-f` chunk.
#[derive(Debug, Default)]
pub struct Script {
    pub commands: Vec<Command>,
    /// `#n` on the first line of the script: behaves like `-n`.
    pub quiet: bool,
}

#[derive(Debug)]
pub struct Command {
    pub selector: Option<Selector>,
    pub negate: bool,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Selector {
    Single(Addr),
    Range(Addr, Addr2),
}

#[derive(Debug)]
pub enum Addr {
    Line(u64),
    Last,
    /// `first~step`
    Step { first: u64, step: u64 },
    Regex(Pattern),
}

#[derive(Debug)]
pub enum Addr2 {
    Line(u64),
    Last,
    Regex(Pattern),
    /// `addr1,+N`
    Plus(u64),
    /// `addr1,~N`
    Multiple(u64),
}

/// A regular expression in an address or in `s`. `None` is the empty regexp,
/// which reuses the last regexp used at run time.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub regex: Option<Rc<Regex>>,
}

#[derive(Debug)]
pub enum Kind {
    /// `{`: the index of the matching `}`.
    BlockStart(usize),
    BlockEnd,
    Label,
    /// `b`, `t`, `T`: the target command index, or `None` for the end of the script.
    Branch { when: BranchWhen, target: Option<usize> },
    Append(Vec<u8>),
    Insert(Vec<u8>),
    Change(Vec<u8>),
    Delete,
    DeleteFirst,
    Next,
    NextAppend,
    Print,
    PrintFirst,
    Get,
    GetAppend,
    Hold,
    HoldAppend,
    Exchange,
    Zap,
    LineNumber,
    List(Option<usize>),
    Quit(i32),
    QuitSilent(i32),
    FileName,
    ReadFile(Vec<u8>),
    ReadLine(Vec<u8>),
    WriteFile(Vec<u8>),
    WriteFirstLine(Vec<u8>),
    Execute(Vec<u8>),
    Translate(Vec<(Vec<u8>, Vec<u8>)>),
    Substitute(Box<Substitute>),
    /// `v`: does nothing at run time.
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchWhen {
    Always,
    /// `t`: if a substitution succeeded since the last input line was read or branch taken.
    IfSubstituted,
    /// `T`: if no substitution succeeded.
    IfNotSubstituted,
}

#[derive(Debug)]
pub struct Substitute {
    pub pattern: Pattern,
    pub replacement: Vec<ReplacementPart>,
    pub global: bool,
    /// Replace only from the Nth match on.
    pub occurrence: usize,
    /// Number of `p` flags (GNU prints before and after `e` when it appears on both sides).
    pub print: u8,
    pub eval: bool,
    pub write: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementPart {
    Literal(Vec<u8>),
    /// `&` is group 0, `\1` to `\9` are groups 1 to 9.
    Group(usize),
    Case(CaseConversion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseConversion {
    Upper,
    Lower,
    UpperNext,
    LowerNext,
    Stop,
}

/// A syntax error, reported like GNU sed:
/// `-e expression #N, char M: message` or `file F line N: message`.
#[derive(Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub location: Location,
    pub message: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Location {
    Expression { number: usize, position: usize },
    File { name: String, line: usize },
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Location::Expression { number, position } => {
                write!(f, "-e expression #{number}, char {position}: {}", self.message)
            }
            Location::File { name, line } => write!(f, "file {name} line {line}: {}", self.message),
        }
    }
}

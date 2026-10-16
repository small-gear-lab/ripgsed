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
    /// The `v` command appeared: GNU extensions stay on even under POSIXLY_CORRECT.
    pub extensions: bool,
    /// Files named by `w`, `W`, and `s///w`, in script order: created before any input is read.
    pub write_files: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub struct Command {
    pub selector: Selector,
    pub negate: bool,
    pub kind: Kind,
}

#[derive(Debug)]
pub enum Selector {
    /// No address was written: every line.
    All,
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
    /// `+0`, `~0`, or a sign alone as a first address: every line.
    Null,
}

#[derive(Debug)]
pub enum Addr2 {
    Line(u64),
    Last,
    Regex(Pattern),
    /// `addr1,first~step`: ends at the first line from `first` on that is `step` lines apart.
    Step { first: u64, step: u64 },
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
    /// The pattern after its escapes are resolved (for `--debug`).
    pub source: Vec<u8>,
    pub icase: bool,
    pub multiline: bool,
}

#[derive(Debug)]
pub enum Kind {
    /// `{`: the index of the matching `}`.
    BlockStart(usize),
    BlockEnd,
    /// `:label`
    Label(Vec<u8>),
    /// `b`, `t`, `T`: the label, and the target command index (`None`: the end of the script).
    Branch { when: BranchWhen, label: Vec<u8>, target: Option<usize> },
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
    /// `l` with the width written after it, kept as the 32-bit integer GNU sed makes of it (-1 reads as no width).
    List(Option<i32>),
    /// `L`: parsed like `l N` but fails when it runs ("INTERNAL ERROR: Bad cmd L").
    Removed(Option<i32>),
    /// `q` and `Q` with the exit status written after them, if any.
    Quit(Option<i32>),
    QuitSilent(Option<i32>),
    FileName,
    ReadFile(Vec<u8>),
    /// `0r file`: GNU sed turns it into `1r file` whose file is written at once, where the command stands,
    /// instead of at the end of the cycle.
    ReadFileNow(Vec<u8>),
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
    /// `p` written before the `e` flag (or without one): print before evaluating.
    pub print_before: bool,
    /// `p` written after the `e` flag: print after evaluating.
    pub print_after: bool,
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
    /// Exit status: 1 for syntax errors, 4 for the "serious" errors GNU sed panics on.
    pub status: i32,
    /// Files named by `w` before the error: GNU sed has already created them.
    pub touched: Vec<Vec<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Location {
    Expression { number: usize, position: usize },
    File { name: String, line: usize },
    /// No location: the message is printed as it is.
    Bare,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Location::Expression { number, position } => {
                write!(f, "-e expression #{number}, char {position}: {}", self.message)
            }
            Location::File { name, line } => write!(f, "file {name} line {line}: {}", self.message),
            Location::Bare => f.write_str(&self.message),
        }
    }
}

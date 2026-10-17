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

/// What a command depends on or changes besides the record it runs on. A script is
/// record-local (and can run on chunks in parallel) when every command reaches only its record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Only the record the command runs on. `q` and `Q` count: the run ends after their record, and a
    /// chunk executor drops what comes after it.
    Record,
    /// Records before or after it, or the end of the input: the construct is named.
    OtherRecords(&'static str),
    HoldSpace,
    /// Something outside the output stream: files, other processes.
    Outside(&'static str),
}

impl Reach {
    /// The construct that makes the command not record-local, if it does.
    pub fn construct(self) -> Option<&'static str> {
        match self {
            Reach::Record => None,
            Reach::HoldSpace => Some("the hold space"),
            Reach::OtherRecords(name) | Reach::Outside(name) => Some(name),
        }
    }
}

impl Selector {
    pub fn reach(&self) -> Reach {
        match self {
            Selector::All => Reach::Record,
            Selector::Single(a) => a.reach(),
            // A range is a state that goes from record to record, but a small one: a chunk can be run with a
            // guess of the state it starts in and be run again if the chunk before it ends in another.
            Selector::Range(a1, a2) => match (a1.reach(), a2.reach()) {
                (Reach::Record, Reach::Record) => Reach::Record,
                (Reach::Record, other) | (other, _) => other,
            },
        }
    }
}

impl Addr2 {
    pub fn reach(&self) -> Reach {
        match self {
            // An empty regexp reuses the one used last, on an earlier record.
            Addr2::Regex(p) if p.regex.is_none() => Reach::OtherRecords("an empty regexp"),
            _ => Reach::Record,
        }
    }
}

impl Addr {
    pub fn reach(&self) -> Reach {
        match self {
            // A single address 0 is rejected by the parser, so `Line(0)` only starts a range.
            Addr::Line(_) | Addr::Step { .. } | Addr::Null => Reach::Record,
            // `$` is the last record of the whole input, which the chunk that holds it is told.
            Addr::Last => Reach::Record,
            // An empty regexp reuses the one used last, on an earlier record.
            Addr::Regex(p) if p.regex.is_none() => Reach::OtherRecords("an empty regexp"),
            Addr::Regex(_) => Reach::Record,
        }
    }
}

impl Command {
    /// The wider of what the address and the operation reach.
    pub fn reach(&self) -> Reach {
        match self.selector.reach() {
            Reach::Record => self.kind.reach(),
            other => other,
        }
    }
}

impl Kind {
    pub fn reach(&self) -> Reach {
        match self {
            Kind::BlockStart(_)
            | Kind::BlockEnd
            | Kind::Label(_)
            | Kind::Branch { .. }
            | Kind::Append(_)
            | Kind::Insert(_)
            | Kind::Change(_)
            | Kind::Delete
            | Kind::Print
            | Kind::PrintFirst
            | Kind::Zap
            | Kind::LineNumber
            | Kind::Quit(_)
            | Kind::QuitSilent(_)
            | Kind::List(_)
            | Kind::Translate(_)
            | Kind::Version => Reach::Record,
            Kind::Substitute(sub) => {
                if sub.eval {
                    Reach::Outside("the e flag of s")
                } else if let Some(name) = &sub.write {
                    written_file(name)
                } else if sub.pattern.regex.is_none() {
                    // `s//x/` reuses the regexp last used, which depends on the records before.
                    Reach::OtherRecords("an empty regexp")
                } else {
                    Reach::Record
                }
            }
            Kind::DeleteFirst | Kind::Next | Kind::NextAppend => Reach::OtherRecords("D, n or N"),
            Kind::Get | Kind::GetAppend | Kind::Hold | Kind::HoldAppend | Kind::Exchange => Reach::HoldSpace,
            Kind::FileName => Reach::Record,
            // A file the script writes gets the bytes in order (a chunk keeps them and the writer puts them
            // there); standard output and standard error do not wait for that. `r` streams its file (which
            // may never end, or be as big as the output is not meant to hold) and `R` keeps its place in it.
            Kind::WriteFile(name) | Kind::WriteFirstLine(name) => written_file(name),
            Kind::ReadFile(_) | Kind::ReadFileNow(_) | Kind::ReadLine(_) => Reach::Outside("a file command"),
            Kind::Execute(_) => Reach::Outside("e"),
            Kind::Removed(_) => Reach::Outside("L"),
        }
    }
}

/// What writing to the file `name` reaches: standard output and standard error are written as the run goes
/// (and are not in a file that a chunk keeps for the writer).
fn written_file(name: &[u8]) -> Reach {
    if name == b"/dev/stdout" || name == b"/dev/stderr" {
        Reach::Outside("w to /dev/stdout or /dev/stderr")
    } else {
        Reach::Record
    }
}

/// What GNU sed says about the addresses of a command letter, or `None` if it accepts them:
/// `#`, `:` and `}` take none, `q` and `Q` one, and under `--posix` `=`, `a`, `i`, `r` and `l`
/// one as well (GNU sed lets them take a range). Whether a command takes addresses at all is
/// decided here, before its arguments are read.
pub fn address_complaint(letter: u8, selector: &Selector, posix: bool) -> Option<&'static str> {
    let any = !matches!(selector, Selector::All);
    let range = matches!(selector, Selector::Range(..));
    match letter {
        b'#' if any => Some("comments don't accept any addresses"),
        b'}' if any => Some("`}' doesn't want any addresses"),
        b':' if any => Some(": doesn't want any addresses"),
        b'q' | b'Q' if range => Some("command only uses one address"),
        b'=' | b'a' | b'i' | b'r' | b'l' if posix && range => Some("command only uses one address"),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_counts_by_command_letter() {
        let none = Selector::All;
        let one = Selector::Single(Addr::Line(1));
        let range = Selector::Range(Addr::Line(1), Addr2::Line(2));
        // No address is always fine.
        for letter in *b"#:}qQ=aird{" {
            assert_eq!(address_complaint(letter, &none, false), None, "{}", letter as char);
        }
        // `#`, `:` and `}` take none.
        for (letter, message) in [(b'#', "comments don't accept any addresses"), (b':', ": doesn't want any addresses"), (b'}', "`}' doesn't want any addresses")] {
            assert_eq!(address_complaint(letter, &one, false), Some(message));
        }
        // `q` and `Q` take one; a range is too many.
        for letter in *b"qQ" {
            assert_eq!(address_complaint(letter, &one, false), None);
            assert_eq!(address_complaint(letter, &range, false), Some("command only uses one address"));
        }
        // `=`, `a`, `i`, `r` and `l` take a range, unless `--posix`.
        for letter in *b"=air" {
            assert_eq!(address_complaint(letter, &range, false), None);
            assert_eq!(address_complaint(letter, &range, true), Some("command only uses one address"));
            assert_eq!(address_complaint(letter, &one, true), None);
        }
        assert_eq!(address_complaint(b'd', &range, true), None);
    }

    #[test]
    fn reach_of_a_command_is_the_wider_of_its_address_and_operation() {
        let parse = |text: &str| {
            let chunk = Chunk { source: ChunkSource::Expression(1), text: text.as_bytes().to_vec() };
            super::parse(&[(chunk, ParseOptions::default())]).expect("parses")
        };
        let reach = |text: &str| parse(text).commands.first().expect("a command").reach();
        assert_eq!(reach("5p"), Reach::Record);
        assert_eq!(reach("$p"), Reach::Record);
        assert_eq!(reach("1,3d"), Reach::Record);
        assert_eq!(reach("/a/,//d"), Reach::OtherRecords("an empty regexp"));
        assert_eq!(reach("x"), Reach::HoldSpace);
        assert_eq!(reach("q"), Reach::Record);
        assert_eq!(reach("Q5"), Reach::Record);
        assert_eq!(reach("$q"), Reach::Record);
        assert_eq!(reach("s/a/b/w out"), Reach::Record);
        assert_eq!(reach("w out"), Reach::Record);
        assert_eq!(reach("s/a/b/w /dev/stdout"), Reach::Outside("w to /dev/stdout or /dev/stderr"));
        assert_eq!(reach("r in"), Reach::Outside("a file command"));
        assert_eq!(reach("0,/a/d"), Reach::Record);
    }
}

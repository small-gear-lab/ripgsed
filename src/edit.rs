//! Building a script with method calls: [`Edit`], [`Address`] and [`Pattern`].
//!
//! The builder writes the script as sed text and the text goes through the same parser as a
//! script written by hand, so both ways give the same commands (and the same speed). Nothing
//! here has its own idea of what a sed command means.

use std::ops::RangeInclusive;

use crate::api::{Dialect, Error, Script};

const MIXED: &str = "a script cannot mix basic, extended and Perl-style patterns";

/// A regular expression, basic (as in `sed`), extended (as with `sed -E`) or Perl-style (as with
/// `sed -P`, from the PCRE2 library).
///
/// A script uses one syntax: mixing syntaxes in one [`Edit`] is an error
/// when the script is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    text: String,
    dialect: Dialect,
    ignore_case: bool,
    multiline: bool,
}

impl Pattern {
    /// A basic regular expression (`\(`, `\{`, `\+` and `\|` are the operators).
    pub fn basic(text: impl Into<String>) -> Pattern {
        Pattern { text: text.into(), dialect: Dialect::Basic, ignore_case: false, multiline: false }
    }

    /// An extended regular expression (`(`, `{`, `+` and `|` are the operators).
    pub fn extended(text: impl Into<String>) -> Pattern {
        Pattern { text: text.into(), dialect: Dialect::Extended, ignore_case: false, multiline: false }
    }

    /// A Perl-style regular expression, read by the PCRE2 library (`sed -P`).
    pub fn perl(text: impl Into<String>) -> Pattern {
        Pattern { text: text.into(), dialect: Dialect::Perl, ignore_case: false, multiline: false }
    }

    /// Matches without regard to case (the `I` flag).
    pub fn ignore_case(mut self) -> Pattern {
        self.ignore_case = true;
        self
    }

    /// `^` and `$` also match at the embedded newlines (the `M` flag).
    pub fn multiline(mut self) -> Pattern {
        self.multiline = true;
        self
    }

    /// The text between the slashes: a `/` that is not escaped gets its backslash, and a
    /// newline is written as `\n`.
    fn delimited(&self) -> String {
        let mut out = String::with_capacity(self.text.len() + 2);
        let mut chars = self.text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    out.push('\\');
                    match chars.next() {
                        Some('\n') => out.push('n'),
                        Some(next) => out.push(next),
                        None => {}
                    }
                }
                '/' => out.push_str("\\/"),
                '\n' => out.push_str("\\n"),
                other => out.push(other),
            }
        }
        out
    }

    fn flags(&self) -> String {
        let mut flags = String::new();
        if self.ignore_case {
            flags.push('I');
        }
        if self.multiline {
            flags.push('M');
        }
        flags
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// No address: every line.
    All,
    Single,
    Range,
}

/// Which lines a command applies to. Every command has one; the default is every line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// The address as sed writes it, without the `!`.
    text: String,
    shape: Shape,
    negated: bool,
    dialect: Option<Dialect>,
    error: Option<String>,
}

impl Address {
    fn single(text: String) -> Address {
        Address { text, shape: Shape::Single, negated: false, dialect: None, error: None }
    }

    fn failed(message: &str) -> Address {
        Address { text: String::new(), shape: Shape::All, negated: false, dialect: None, error: Some(message.into()) }
    }

    /// Every line.
    pub fn all() -> Address {
        Address { text: String::new(), shape: Shape::All, negated: false, dialect: None, error: None }
    }

    /// Line number `n` (the first line is 1).
    pub fn line(n: u64) -> Address {
        Address::single(n.to_string())
    }

    /// The last line of the input.
    pub fn last() -> Address {
        Address::single("$".into())
    }

    /// The lines that match the pattern.
    pub fn matching(pattern: Pattern) -> Address {
        let mut address = Address::single(format!("/{}/{}", pattern.delimited(), pattern.flags()));
        address.dialect = Some(pattern.dialect);
        address
    }

    /// The lines from the first to the last number, both included.
    pub fn lines(range: RangeInclusive<u64>) -> Address {
        Address::range(Address::line(*range.start()), Address::line(*range.end()))
    }

    /// Every `step`th line from line `first` (`first~step`).
    pub fn step(first: u64, step: u64) -> Address {
        Address::single(format!("{first}~{step}"))
    }

    /// From the line `first` selects to the line `last` selects. Both must be single addresses
    /// (a line, `last()`, a match or a step); anything else is an error when the script is built.
    pub fn range(first: Address, last: Address) -> Address {
        if let Some(error) = first.error.clone().or_else(|| last.error.clone()) {
            return Address::failed(&error);
        }
        if first.shape != Shape::Single || last.shape != Shape::Single || first.negated || last.negated {
            return Address::failed("a range needs two single addresses that are not negated");
        }
        let dialect = match (first.dialect, last.dialect) {
            (Some(a), Some(b)) if a != b => return Address::failed(MIXED),
            (a, b) => a.or(b),
        };
        Address {
            text: format!("{},{}", first.text, last.text),
            shape: Shape::Range,
            negated: false,
            dialect,
            error: None,
        }
    }

    /// From the line `first` selects and the `count` lines after it (`first,+count`).
    pub fn following(first: Address, count: u64) -> Address {
        Address::open_range(first, format!("+{count}"))
    }

    /// From the line `first` selects to the next line whose number is a multiple of `n`
    /// (`first,~n`).
    pub fn until_multiple_of(first: Address, n: u64) -> Address {
        Address::open_range(first, format!("~{n}"))
    }

    fn open_range(first: Address, end: String) -> Address {
        if let Some(error) = first.error.clone() {
            return Address::failed(&error);
        }
        if first.shape != Shape::Single || first.negated {
            return Address::failed("a range needs a single address that is not negated");
        }
        Address { text: format!("{},{end}", first.text), shape: Shape::Range, negated: false, dialect: first.dialect, error: None }
    }

    /// The lines this address does not select (`!`). Applying it twice gives the address back.
    pub fn not(mut self) -> Address {
        self.negated = !self.negated;
        self
    }

    fn prefix(&self) -> String {
        if self.negated { format!("{}!", self.text) } else { self.text.clone() }
    }
}

/// The flags that follow a `s` command, which the methods of [`Edit`] add to the last one.
#[derive(Debug, Clone, Default)]
struct Flags {
    pattern: String,
    global: bool,
    occurrence: Option<u64>,
    print: bool,
}

#[derive(Debug, Clone)]
struct Command {
    text: String,
    /// Set for `s`, whose flags may still change.
    substitute: Option<Flags>,
}

impl Command {
    fn plain(text: String) -> Command {
        Command { text, substitute: None }
    }

    fn render(&self) -> String {
        let mut out = self.text.clone();
        if let Some(flags) = &self.substitute {
            out.push_str(&flags.pattern);
            if flags.global {
                out.push('g');
            }
            if let Some(n) = flags.occurrence {
                out.push_str(&n.to_string());
            }
            if flags.print {
                out.push('p');
            }
        }
        out
    }
}

/// A script written with method calls, one command per call.
///
/// `when` gives the address of the next command; without it the command applies to every line.
///
/// ```
/// use ripgsed::{Address, Edit, Pattern, Run};
///
/// let script = Edit::new()
///     .when(Address::lines(1..=2)).delete()
///     .substitute(Pattern::basic("foo"), "bar").global()
///     .build()?;
/// let out = Run::new().apply(&script, b"a\nb\nfoo foo\n")?;
/// assert_eq!(out, b"bar bar\n");
/// # Ok::<(), ripgsed::Error>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct Edit {
    commands: Vec<Command>,
    next: Option<Address>,
    dialect: Option<Dialect>,
    error: Option<String>,
}

impl Edit {
    pub fn new() -> Edit {
        Edit::default()
    }

    /// The address of the next command.
    pub fn when(mut self, address: Address) -> Edit {
        self.next = Some(address);
        self
    }

    fn fail(&mut self, message: &str) {
        self.error.get_or_insert_with(|| message.to_string());
    }

    fn note_dialect(&mut self, dialect: Option<Dialect>) {
        match (self.dialect, dialect) {
            (Some(a), Some(b)) if a != b => self.fail(MIXED),
            (None, Some(b)) => self.dialect = Some(b),
            _ => {}
        }
    }

    /// The text before a command: the address given by `when`, or none.
    fn address_text(&mut self) -> String {
        let address = self.next.take().unwrap_or_else(Address::all);
        if let Some(error) = &address.error {
            let error = error.clone();
            self.fail(&error);
        }
        self.note_dialect(address.dialect);
        address.prefix()
    }

    fn push(mut self, body: String) -> Edit {
        let text = format!("{}{body}", self.address_text());
        self.commands.push(Command::plain(text));
        self
    }

    /// `d`: delete the line and start the next cycle.
    pub fn delete(self) -> Edit {
        self.push("d".into())
    }

    /// `p`: print the line now.
    pub fn print(self) -> Edit {
        self.push("p".into())
    }

    /// `=`: print the line number.
    pub fn line_number(self) -> Edit {
        self.push("=".into())
    }

    /// `q`: print the line, then stop with this exit status.
    pub fn quit(self, exit_status: u8) -> Edit {
        self.push(if exit_status == 0 { "q".into() } else { format!("q{exit_status}") })
    }

    /// `Q`: stop with this exit status without printing the line.
    pub fn quit_silently(self, exit_status: u8) -> Edit {
        self.push(if exit_status == 0 { "Q".into() } else { format!("Q{exit_status}") })
    }

    /// `a`: print the text after the line. A newline in the text starts another line.
    pub fn append(self, text: &str) -> Edit {
        self.push(text_command('a', text))
    }

    /// `i`: print the text before the line.
    pub fn insert(self, text: &str) -> Edit {
        self.push(text_command('i', text))
    }

    /// `c`: replace the line (or, with a range, all of it) with the text.
    pub fn change(self, text: &str) -> Edit {
        self.push(text_command('c', text))
    }

    /// `y`: replace every character of `from` with the character of `to` at the same place.
    pub fn translate(self, from: &str, to: &str) -> Edit {
        let escape = |s: &str| {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '/' => out.push_str("\\/"),
                    '\n' => out.push_str("\\n"),
                    other => out.push(other),
                }
            }
            out
        };
        self.push(format!("y/{}/{}/", escape(from), escape(to)))
    }

    /// `s`: replace the first match of the pattern. The replacement is written as in sed: `&`
    /// is the match, `\1` to `\9` are the groups, `\n` is a newline, `\U` `\L` `\u` `\l` `\E`
    /// change the case. Use [`Edit::substitute_literal`] for text that is to be taken as it is.
    /// The methods [`global`](Edit::global), [`occurrence`](Edit::occurrence) and
    /// [`and_print`](Edit::and_print) that follow change this command.
    pub fn substitute(self, pattern: Pattern, replacement: &str) -> Edit {
        let mut replacement_text = String::with_capacity(replacement.len());
        let mut chars = replacement.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    replacement_text.push('\\');
                    if let Some(next) = chars.next() {
                        replacement_text.push(next);
                    }
                }
                '/' => replacement_text.push_str("\\/"),
                '\n' => replacement_text.push_str("\\\n"),
                other => replacement_text.push(other),
            }
        }
        self.substitution(pattern, replacement_text)
    }

    /// `s` with a replacement that is taken as it is: `&` and backslashes mean themselves.
    pub fn substitute_literal(self, pattern: Pattern, replacement: &str) -> Edit {
        let mut replacement_text = String::with_capacity(replacement.len());
        for c in replacement.chars() {
            match c {
                '\\' | '&' | '/' => {
                    replacement_text.push('\\');
                    replacement_text.push(c);
                }
                '\n' => replacement_text.push_str("\\\n"),
                other => replacement_text.push(other),
            }
        }
        self.substitution(pattern, replacement_text)
    }

    fn substitution(mut self, pattern: Pattern, replacement: String) -> Edit {
        self.note_dialect(Some(pattern.dialect));
        let text = format!("{}s/{}/{replacement}/", self.address_text(), pattern.delimited());
        self.commands.push(Command { text, substitute: Some(Flags { pattern: pattern.flags(), ..Flags::default() }) });
        self
    }

    fn flag(mut self, change: impl FnOnce(&mut Flags)) -> Edit {
        match self.commands.last_mut().and_then(|c| c.substitute.as_mut()) {
            Some(flags) => change(flags),
            None => self.fail("a flag of `s` needs a `substitute` before it"),
        }
        self
    }

    /// Of the last `substitute`: replace every match, not only the first (`g`).
    pub fn global(self) -> Edit {
        self.flag(|f| f.global = true)
    }

    /// Of the last `substitute`: replace the `n`th match (and with `global`, the ones after it).
    pub fn occurrence(self, n: u64) -> Edit {
        self.flag(|f| f.occurrence = Some(n))
    }

    /// Of the last `substitute`: print the line if something was replaced (`p`).
    pub fn and_print(self) -> Edit {
        self.flag(|f| f.print = true)
    }

    /// The commands the closure builds, applied to the lines the address of `when` selects.
    /// A block without `when` applies to every line.
    pub fn block(mut self, body: impl FnOnce(Edit) -> Edit) -> Edit {
        let address = self.address_text();
        let inner = body(Edit::new());
        self.note_dialect(inner.dialect);
        if let Some(error) = inner.error.clone() {
            self.fail(&error);
        }
        if inner.next.is_some() {
            self.fail("`when` has no command after it");
        }
        self.commands.push(Command::plain(format!("{address}{{")));
        self.commands.extend(inner.commands);
        self.commands.push(Command::plain("}".into()));
        self
    }

    /// Commands that have no method yet, written as sed writes them (one command, or several
    /// separated by newlines). It takes the address of `when` as a command does, so that
    /// `.when(Address::last()).raw("G")` works; do not write an address in the text as well.
    ///
    /// Patterns written here follow the syntax of the other patterns of the script, so in a
    /// script with extended patterns the text is read as extended (and likewise for Perl-style ones).
    pub fn raw(self, text: &str) -> Edit {
        self.push(text.to_string())
    }

    /// The script, or the first mistake found: a flag without `s`, mixed pattern syntaxes, or
    /// a command the parser rejects (with the message of sed).
    pub fn build(self) -> Result<Script, Error> {
        if let Some(message) = self.error {
            return Err(Error::build(message));
        }
        if self.next.is_some() {
            return Err(Error::build("`when` has no command after it"));
        }
        let text: Vec<String> = self.commands.iter().map(Command::render).collect();
        Script::from_text(text.join("\n"), self.dialect.unwrap_or(Dialect::Basic))
    }
}

fn text_command(letter: char, text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 3);
    out.push(letter);
    out.push_str("\\\n");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\\n"),
            other => out.push(other),
        }
    }
    out
}

//! The sed script parser.
//!
//! Scripts arrive as chunks (`-e` arguments, `-f` files, or the script operand).
//! They are joined with newlines into one buffer and parsed in one pass, so a
//! command such as `a\` can continue into the next chunk. Error locations are
//! mapped back to the chunk they came from.

use std::collections::HashMap;
use std::rc::Rc;

use super::{
    Addr, Addr2, BranchWhen, CaseConversion, Command, Kind, Location, Pattern, ReplacementPart, Script, Selector,
    Substitute, SyntaxError,
};
use crate::regex::{Locale, Regex, Syntax};

/// Where a chunk of script text came from.
#[derive(Debug, Clone)]
pub enum ChunkSource {
    /// The `n`th `-e` argument or script operand (1-based).
    Expression(usize),
    /// A `-f` script file.
    File(String),
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub source: ChunkSource,
    pub text: Vec<u8>,
}

/// Options that change how a script is read.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOptions {
    /// `-E` / `-r` seen before the chunk being parsed.
    pub extended: bool,
    pub posix: bool,
    pub posixly_correct: bool,
    pub sandbox: bool,
    pub locale: Locale,
}

/// The version GNU sed targets (D6); `v` compares against it.
const TARGET_VERSION: &str = "4.9";

struct Span {
    start: usize,
    end: usize,
    source: ChunkSource,
    options: ParseOptions,
}

struct Parser {
    src: Vec<u8>,
    pos: usize,
    spans: Vec<Span>,
    script: Script,
    /// Indexes of the open `{` commands.
    blocks: Vec<usize>,
    labels: HashMap<Vec<u8>, usize>,
    /// Branch commands waiting for their label: (command index, label, error position).
    pending_jumps: Vec<(usize, Vec<u8>, usize)>,
    /// An address used line 0 where it is not allowed; reported after the command character.
    zero_address: bool,
    /// Names of the `w`/`W`/`s///w` files seen so far.
    touched: Vec<Vec<u8>>,
}

/// Parses all chunks. `options_for` gives the options in effect for each chunk,
/// because `-E` only affects the chunks that follow it.
pub fn parse(chunks: &[(Chunk, ParseOptions)]) -> std::result::Result<Script, SyntaxError> {
    let mut src = Vec::new();
    let mut spans = Vec::new();
    for (i, (chunk, options)) in chunks.iter().enumerate() {
        if i > 0 {
            src.push(b'\n');
        }
        let start = src.len();
        src.extend_from_slice(&chunk.text);
        spans.push(Span { start, end: src.len(), source: chunk.source.clone(), options: *options });
    }
    let mut parser = Parser {
        src,
        pos: 0,
        spans,
        script: Script::default(),
        blocks: Vec::new(),
        labels: HashMap::new(),
        pending_jumps: Vec::new(),
        zero_address: false,
        touched: Vec::new(),
    };
    parser.detect_quiet_comment();
    parser.program()?;
    parser.finish()
}

type Result<T> = std::result::Result<T, SyntaxError>;

impl Parser {
    // ---- positions and diagnostics -------------------------------------------------

    fn span_at(&self, pos: usize) -> &Span {
        // The position just after a chunk's last byte still belongs to that chunk.
        self.spans
            .iter()
            .find(|s| pos >= s.start && pos <= s.end)
            .unwrap_or_else(|| self.spans.last().expect("at least one chunk"))
    }

    fn options(&self) -> ParseOptions {
        self.span_at(self.pos.saturating_sub(1)).options
    }

    fn error_at(&self, pos: usize, message: &str) -> SyntaxError {
        let span = self.span_at(pos.saturating_sub(1).min(self.src.len()));
        let location = match &span.source {
            ChunkSource::Expression(n) => Location::Expression { number: *n, position: pos.saturating_sub(span.start) },
            ChunkSource::File(name) => {
                let upto = pos.min(self.src.len());
                let line = 1 + self.src[span.start..upto.max(span.start)].iter().filter(|&&b| b == b'\n').count();
                Location::File { name: name.clone(), line }
            }
        };
        SyntaxError { location, message: message.to_string(), status: 1, touched: self.touched.clone() }
    }

    fn error(&self, message: &str) -> SyntaxError {
        self.error_at(self.pos, message)
    }

    // ---- character access ----------------------------------------------------------

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn unget(&mut self) {
        self.pos -= 1;
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn skip_blanks(&mut self) {
        while self.peek().is_some_and(|b| b == b' ' || b == b'\t') {
            self.pos += 1;
        }
    }

    // ---- top level -----------------------------------------------------------------

    /// `#n` as the very first characters of the script forces `-n`.
    fn detect_quiet_comment(&mut self) {
        if self.src.starts_with(b"#n") {
            self.script.quiet = true;
        }
    }

    fn program(&mut self) -> Result<()> {
        loop {
            // Whitespace and stray semicolons separate commands.
            while self.peek().is_some_and(|b| b == b';' || b.is_ascii_whitespace()) {
                self.pos += 1;
            }
            if self.peek().is_none() {
                return Ok(());
            }
            self.command()?;
        }
    }

    fn finish(mut self) -> Result<Script> {
        if let Some(&open) = self.blocks.last() {
            let _ = open;
            return Err(self.error_at(0, "unmatched `{'")).map_err(|mut e| {
                // GNU reports an unmatched `{` at char 0 of the expression.
                if let Location::Expression { position, .. } = &mut e.location {
                    *position = 0;
                }
                e
            });
        }
        for (index, label, pos) in std::mem::take(&mut self.pending_jumps) {
            match self.labels.get(&label) {
                Some(&target) => {
                    if let Kind::Branch { target: t, .. } = &mut self.script.commands[index].kind {
                        *t = Some(target);
                    }
                }
                None => {
                    let _ = pos;
                    return Err(SyntaxError {
                        location: Location::Bare,
                        message: format!("can't find label for jump to `{}'", String::from_utf8_lossy(&label)),
                        status: 4,
                        touched: self.touched.clone(),
                    });
                }
            }
        }
        Ok(self.script)
    }

    // ---- addresses -----------------------------------------------------------------

    fn read_number(&mut self) -> u64 {
        let mut n: u64 = 0;
        while let Some(b @ b'0'..=b'9') = self.peek() {
            n = n.saturating_mul(10).saturating_add(u64::from(b - b'0'));
            self.pos += 1;
        }
        n
    }

    /// Reads one address, or `None` if the next character does not start one.
    fn address(&mut self, first: bool) -> Result<Option<AddrParsed>> {
        let Some(ch) = self.peek() else { return Ok(None) };
        match ch {
            b'0'..=b'9' => {
                let n = self.read_number();
                if self.peek() == Some(b'~') && !self.options().posix {
                    self.pos += 1;
                    let step = self.read_number();
                    if n == 0 && step == 0 {
                        self.zero_address = true;
                    }
                    return Ok(Some(AddrParsed::Step { first: n, step }));
                }
                Ok(Some(AddrParsed::Line(n)))
            }
            b'+' | b'~' if !first && !self.options().posix => {
                self.pos += 1;
                let n = self.read_number();
                Ok(Some(if ch == b'+' { AddrParsed::Plus(n) } else { AddrParsed::Multiple(n) }))
            }
            b'+' | b'~' => {
                self.pos += 1;
                self.read_number();
                Err(self.error("invalid usage of +N or ~N as first address"))
            }
            b'$' => {
                self.pos += 1;
                Ok(Some(AddrParsed::Last))
            }
            b'/' | b'\\' => {
                self.pos += 1;
                let delim = if ch == b'\\' {
                    match self.next() {
                        Some(d) => d,
                        None => return Err(self.error("unexpected `,'")),
                    }
                } else {
                    b'/'
                };
                if delim == b'\n' || delim == b'\\' {
                    return Err(self.error("unterminated address regex"));
                }
                let Some(text) = self.read_delimited(delim, true) else {
                    return Err(self.error("unterminated address regex"));
                };
                let (mut icase, mut multiline) = (false, false);
                while !self.options().posix {
                    match self.peek() {
                        Some(b'I') => icase = true,
                        Some(b'M') => multiline = true,
                        _ => break,
                    }
                    self.pos += 1;
                }
                let pattern = self.compile_pattern(&text, icase, multiline)?;
                Ok(Some(AddrParsed::Regex(pattern)))
            }
            _ => Ok(None),
        }
    }

    /// Compiles a regular expression. The empty regexp reuses the last one at run time.
    fn compile_pattern(&mut self, text: &[u8], icase: bool, multiline: bool) -> Result<Pattern> {
        if text.is_empty() {
            if icase || multiline {
                return Err(self.error("cannot specify modifiers on empty regexp"));
            }
            return Ok(Pattern { regex: None, source: Vec::new(), icase: false, multiline: false });
        }
        let o = self.options();
        let syntax = Syntax {
            extended: o.extended,
            posix: o.posix,
            posixly_correct: o.posixly_correct,
            icase,
            multiline,
            locale: o.locale,
        };
        match Regex::compile(&normalize_escapes(text, o.posixly_correct), &syntax) {
            Ok(re) => Ok(Pattern { regex: Some(Rc::new(re)), source: text.to_vec(), icase, multiline }),
            Err(e) => Err(self.error(&e.message)),
        }
    }

    /// Reads up to the next unescaped `delim` (the opening one is already consumed).
    /// `\n` becomes a newline in a regexp, and `\` + delimiter drops the backslash.
    /// Returns `None` if the text ends first.
    fn read_delimited(&mut self, delim: u8, regex: bool) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        // Unlike the text of `a`, `i`, `c`, a delimited part never continues into the next chunk.
        let limit = self.span_at(self.pos.saturating_sub(1)).end;
        loop {
            if self.pos >= limit {
                return None;
            }
            let ch = self.next()?;
            if ch == b'\n' {
                self.pos -= 1;
                return None;
            }
            if ch == delim {
                return Some(out);
            }
            if ch == b'\\' {
                if self.pos >= limit {
                    return None;
                }
                let next = self.next()?;
                if next == b'n' && regex {
                    out.push(b'\n');
                } else if next == b'\n' {
                    out.push(b'\n');
                } else if next == delim && !(next == b'&' && !regex) {
                    // A delimiter that is special in the pattern stays escaped.
                    if regex && is_regex_special(delim) {
                        out.push(b'\\');
                    }
                    out.push(next);
                } else {
                    out.push(b'\\');
                    out.push(next);
                }
                continue;
            }
            out.push(ch);
        }
    }

    // ---- commands ------------------------------------------------------------------

    fn command(&mut self) -> Result<()> {
        let start_of_command = self.pos;
        let a1 = self.address(true)?;
        let mut selector = None;
        if let Some(a1) = a1 {
            self.skip_whitespace();
            if self.peek() == Some(b',') {
                self.pos += 1;
                self.skip_whitespace();
                let Some(a2) = self.address(false)? else {
                    self.next();
                    return Err(self.error("unexpected `,'"));
                };
                selector = Some(self.range(a1, a2)?);
            } else {
                selector = Some(self.single(a1)?);
            }
        }
        self.skip_whitespace();
        if self.zero_address {
            // GNU sed reports line address 0 after reading the character that follows the address,
            // except for `r`, which accepts it (insert a file before the first line).
            let is_read = self.peek() == Some(b'r') && matches!(selector, Some(Selector::Single(Addr::Line(0))));
            if !is_read {
                self.next();
                return Err(self.error("invalid usage of line address 0"));
            }
            self.zero_address = false;
        }

        let mut negate = false;
        if self.peek() == Some(b'!') {
            self.pos += 1;
            negate = true;
            self.skip_whitespace();
            if self.peek() == Some(b'!') {
                self.pos += 1;
                return Err(self.error("multiple `!'s"));
            }
        }

        let Some(ch) = self.next() else {
            return Err(self.error("missing command"));
        };
        let _ = start_of_command;

        if self.options().posix && matches!(ch, b'F' | b'e' | b'Q' | b'T' | b'z' | b'v' | b'W' | b'R') {
            return Err(self.error(&format!("unknown command: `{}'", ch as char)));
        }
        if self.options().sandbox && matches!(ch, b'e' | b'r' | b'R' | b'w' | b'W') {
            return Err(self.error("e/r/w commands disabled in sandbox mode"));
        }

        let kind = match ch {
            b'#' => {
                if selector.is_some() {
                    return Err(self.error("comments don't accept any addresses"));
                }
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.pos += 1;
                }
                return Ok(());
            }
            b'{' => {
                self.blocks.push(self.script.commands.len());
                Kind::BlockStart(0)
            }
            b'}' => {
                if selector.is_some() {
                    return Err(self.error("`}' doesn't want any addresses"));
                }
                let Some(open) = self.blocks.pop() else {
                    return Err(self.error("unexpected `}'"));
                };
                let end = self.script.commands.len();
                if let Kind::BlockStart(e) = &mut self.script.commands[open].kind {
                    *e = end;
                }
                self.end_of_command()?;
                Kind::BlockEnd
            }
            b':' => {
                if selector.is_some() {
                    return Err(self.error(": doesn't want any addresses"));
                }
                let label = self.read_label();
                if label.is_empty() {
                    return Err(self.error("\":\" lacks a label"));
                }
                self.labels.insert(label.clone(), self.script.commands.len());
                Kind::Label(label)
            }
            b'b' | b't' | b'T' => {
                let label = self.read_label();
                let when = match ch {
                    b'b' => BranchWhen::Always,
                    b't' => BranchWhen::IfSubstituted,
                    _ => BranchWhen::IfNotSubstituted,
                };
                if !label.is_empty() {
                    self.pending_jumps.push((self.script.commands.len(), label.clone(), self.pos));
                }
                Kind::Branch { when, label, target: None }
            }
            b'a' | b'i' | b'c' => {
                let text = self.read_text()?;
                match ch {
                    b'a' => Kind::Append(text),
                    b'i' => Kind::Insert(text),
                    _ => Kind::Change(text),
                }
            }
            b'd' => self.simple(Kind::Delete)?,
            b'D' => self.simple(Kind::DeleteFirst)?,
            b'n' => self.simple(Kind::Next)?,
            b'N' => self.simple(Kind::NextAppend)?,
            b'p' => self.simple(Kind::Print)?,
            b'P' => self.simple(Kind::PrintFirst)?,
            b'g' => self.simple(Kind::Get)?,
            b'G' => self.simple(Kind::GetAppend)?,
            b'h' => self.simple(Kind::Hold)?,
            b'H' => self.simple(Kind::HoldAppend)?,
            b'x' => self.simple(Kind::Exchange)?,
            b'z' => self.simple(Kind::Zap)?,
            b'=' => self.simple(Kind::LineNumber)?,
            b'F' => self.simple(Kind::FileName)?,
            b'l' => {
                self.skip_blanks();
                let width = if !self.options().posix && self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    Some(self.read_number() as usize)
                } else {
                    None
                };
                self.end_of_command()?;
                Kind::List(width)
            }
            b'q' | b'Q' => {
                if matches!(selector, Some(Selector::Range(..))) {
                    return Err(self.error("command only uses one address"));
                }
                self.skip_blanks();
                let code = if self.peek().is_some_and(|b| b.is_ascii_digit()) { self.read_number() as i32 } else { 0 };
                self.end_of_command()?;
                if ch == b'q' { Kind::Quit(code) } else { Kind::QuitSilent(code) }
            }
            b'r' | b'R' | b'w' | b'W' => {
                let name = self.read_filename()?;
                if self.options().sandbox {
                    return Err(self.error("e/r/w commands disabled in sandbox mode"));
                }
                if matches!(ch, b'w' | b'W') {
                    self.touched.push(name.clone());
                }
                match ch {
                    b'r' => Kind::ReadFile(name),
                    b'R' => Kind::ReadLine(name),
                    b'w' => Kind::WriteFile(name),
                    _ => Kind::WriteFirstLine(name),
                }
            }
            b'e' => {
                if self.options().sandbox {
                    return Err(self.error("e/r/w commands disabled in sandbox mode"));
                }
                self.skip_blanks();
                let start = self.pos;
                while self.peek().is_some_and(|b| b != b'\n') {
                    self.pos += 1;
                }
                Kind::Execute(self.src[start..self.pos].to_vec())
            }
            b's' => self.substitute()?,
            b'y' => self.transliterate()?,
            b'v' => {
                let label = self.read_label();
                self.check_version(&label)?;
                Kind::Version
            }
            other => {
                return Err(self.error(&format!("unknown command: `{}'", other as char)));
            }
        };
        self.script.commands.push(Command { selector, negate, kind });
        Ok(())
    }

    fn simple(&mut self, kind: Kind) -> Result<Kind> {
        self.end_of_command()?;
        Ok(kind)
    }

    /// After a command: only blanks, then end of line, `;`, `}` or `#` may follow.
    fn end_of_command(&mut self) -> Result<()> {
        self.skip_blanks();
        match self.peek() {
            None | Some(b'\n') | Some(b';') => {
                if self.peek().is_some() {
                    self.pos += 1;
                }
                Ok(())
            }
            Some(b'}') | Some(b'#') => Ok(()),
            Some(b'\r') if self.src.get(self.pos + 1) == Some(&b'\n') => {
                self.pos += 1;
                Ok(())
            }
            Some(_) => {
                self.pos += 1;
                Err(self.error("extra characters after command"))
            }
        }
    }

    fn single(&mut self, a: AddrParsed) -> Result<Selector> {
        match a {
            AddrParsed::Line(0) => {
                self.zero_address = true;
                Ok(Selector::Single(Addr::Line(0)))
            }
            AddrParsed::Line(n) => Ok(Selector::Single(Addr::Line(n))),
            AddrParsed::Last => Ok(Selector::Single(Addr::Last)),
            AddrParsed::Step { first, step } => Ok(Selector::Single(Addr::Step { first, step })),
            AddrParsed::Regex(p) => Ok(Selector::Single(Addr::Regex(p))),
            AddrParsed::Plus(_) | AddrParsed::Multiple(_) => unreachable!("rejected as a first address"),
        }
    }

    fn range(&mut self, a1: AddrParsed, a2: AddrParsed) -> Result<Selector> {
        let first = match a1 {
            AddrParsed::Line(0) => {
                // `0,/regexp/` is the only use of line 0 as a range start.
                if !matches!(a2, AddrParsed::Regex(_)) || self.options().posix {
                    self.zero_address = true;
                }
                Addr::Line(0)
            }
            AddrParsed::Line(n) => Addr::Line(n),
            AddrParsed::Last => Addr::Last,
            AddrParsed::Step { first, step } => Addr::Step { first, step },
            AddrParsed::Regex(p) => Addr::Regex(p),
            AddrParsed::Plus(_) | AddrParsed::Multiple(_) => unreachable!("rejected as a first address"),
        };
        let second = match a2 {
            AddrParsed::Line(0) => {
                self.zero_address = true;
                Addr2::Line(0)
            }
            AddrParsed::Line(n) => Addr2::Line(n),
            AddrParsed::Last => Addr2::Last,
            AddrParsed::Regex(p) => Addr2::Regex(p),
            AddrParsed::Plus(n) => Addr2::Plus(n),
            AddrParsed::Multiple(n) => Addr2::Multiple(n),
            AddrParsed::Step { first, step } => Addr2::Step { first, step },
        };
        Ok(Selector::Range(first, second))
    }

    // ---- argument readers ----------------------------------------------------------

    /// A label runs to whitespace, `;` or `}`; leading whitespace is skipped.
    fn read_label(&mut self) -> Vec<u8> {
        self.skip_blanks();
        let start = self.pos;
        while self.peek().is_some_and(|b| !b.is_ascii_whitespace() && b != b';' && b != b'}') {
            self.pos += 1;
        }
        self.src[start..self.pos].to_vec()
    }

    /// A file name runs to the end of the line.
    fn read_filename(&mut self) -> Result<Vec<u8>> {
        self.skip_blanks();
        let start = self.pos;
        while self.peek().is_some_and(|b| b != b'\n') {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("missing filename in r/R/w/W commands"));
        }
        Ok(self.src[start..self.pos].to_vec())
    }

    /// Text of `a`, `i`, `c`: the one-liner form (`a text`, or `a\text` which keeps
    /// the whitespace after the backslash) or the classic form (`a\` newline `text`),
    /// with backslash-newline continuation lines. `\t`, `\n` and the other GNU
    /// escapes are resolved. The text always ends with a newline, except that an
    /// empty `a\` at the very end of the script appends nothing.
    fn read_text(&mut self) -> Result<Vec<u8>> {
        self.skip_blanks();
        let Some(first) = self.peek() else {
            return Err(self.error("expected \\ after `a', `c' or `i'"));
        };
        if first != b'\\' && self.options().posix {
            self.pos += 1;
            return Err(self.error("expected \\ after `a', `c' or `i'"));
        }
        if first == b'\\' {
            self.pos += 1;
            match self.peek() {
                None => return Ok(Vec::new()),
                Some(b'\n') => self.pos += 1,
                Some(_) => {}
            }
        } else if first == b'\n' {
            return Err(self.error("expected \\ after `a', `c' or `i'"));
        }
        let mut raw = Vec::new();
        loop {
            let Some(ch) = self.next() else { break };
            match ch {
                b'\\' => match self.next() {
                    Some(b'\n') => raw.extend_from_slice(b"\\\n"),
                    Some(other) => {
                        raw.push(b'\\');
                        raw.push(other);
                    }
                    None => break,
                },
                b'\n' => break,
                other => raw.push(other),
            }
        }
        let mut text = Vec::with_capacity(raw.len() + 1);
        let resolved = normalize_escapes(&raw, false);
        let mut i = 0;
        while i < resolved.len() {
            if resolved[i] == b'\\' && i + 1 < resolved.len() {
                text.push(match resolved[i + 1] {
                    b'n' => b'\n',
                    other => other,
                });
                i += 2;
            } else {
                text.push(resolved[i]);
                i += 1;
            }
        }
        text.push(b'\n');
        Ok(text)
    }

    fn check_version(&self, label: &[u8]) -> Result<()> {
        if label.is_empty() {
            return Ok(());
        }
        if version_compare(label, TARGET_VERSION.as_bytes()) == std::cmp::Ordering::Greater {
            return Err(self.error("expected newer version of sed"));
        }
        Ok(())
    }

    // ---- s and y -------------------------------------------------------------------

    fn substitute(&mut self) -> Result<Kind> {
        let Some(delim) = self.next() else {
            return Err(self.error("unterminated `s' command"));
        };
        if delim == b'\n' || delim == b'\\' {
            return Err(self.error("unterminated `s' command"));
        }
        let Some(pattern_text) = self.read_delimited(delim, true) else {
            return Err(self.error("unterminated `s' command"));
        };
        let Some(replacement_text) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `s' command"));
        };

        // Flags.
        let mut global = false;
        let mut occurrence = 0usize;
        let mut print = 0u8;
        let mut eval = false;
        let mut write = None;
        let (mut icase, mut multiline) = (false, false);
        loop {
            match self.next() {
                None | Some(b'\n') | Some(b';') => break,
                Some(b'}') | Some(b'#') => {
                    self.unget();
                    break;
                }
                Some(b' ') | Some(b'\t') | Some(b'\r') => {}
                Some(b'i') | Some(b'I') | Some(b'm') | Some(b'M') | Some(b'e') if self.options().posix => {
                    return Err(self.error("unknown option to `s'"));
                }
                Some(b'i') | Some(b'I') => icase = true,
                Some(b'm') | Some(b'M') => multiline = true,
                Some(b'e') => {
                    if self.options().sandbox {
                        return Err(self.error("e/r/w commands disabled in sandbox mode"));
                    }
                    eval = true;
                }
                Some(b'p') => {
                    if print > 0 && !eval {
                        return Err(self.error("multiple `p' options to `s' command"));
                    }
                    print += 1;
                }
                Some(b'g') => {
                    if global {
                        return Err(self.error("multiple `g' options to `s' command"));
                    }
                    global = true;
                }
                Some(b'w') => {
                    if self.options().sandbox {
                        return Err(self.error("e/r/w commands disabled in sandbox mode"));
                    }
                    let name = self.read_filename()?;
                    self.touched.push(name.clone());
                    write = Some(name);
                    break;
                }
                Some(d @ b'0'..=b'9') => {
                    if occurrence != 0 {
                        return Err(self.error("multiple number options to `s' command"));
                    }
                    self.unget();
                    let _ = d;
                    let n = self.read_number() as usize;
                    if n == 0 {
                        return Err(self.error("number option to `s' command may not be zero"));
                    }
                    occurrence = n;
                }
                Some(_) => return Err(self.error("unknown option to `s'")),
            }
        }

        let pattern = self.compile_pattern(&pattern_text, icase, multiline)?;
        let replacement = self.parse_replacement(&replacement_text, &pattern)?;
        Ok(Kind::Substitute(Box::new(Substitute {
            pattern,
            replacement,
            global,
            occurrence,
            print,
            eval,
            write,
        })))
    }

    fn parse_replacement(&self, text: &[u8], pattern: &Pattern) -> Result<Vec<ReplacementPart>> {
        let text = normalize_escapes(text, self.options().posixly_correct);
        let mut parts = Vec::new();
        let mut literal = Vec::new();
        let flush = |parts: &mut Vec<ReplacementPart>, literal: &mut Vec<u8>| {
            if !literal.is_empty() {
                parts.push(ReplacementPart::Literal(std::mem::take(literal)));
            }
        };
        let groups = pattern.regex.as_ref().map(|r| r.group_count());
        let mut i = 0;
        while i < text.len() {
            let b = text[i];
            i += 1;
            match b {
                b'&' => {
                    flush(&mut parts, &mut literal);
                    parts.push(ReplacementPart::Group(0));
                }
                b'\\' if i < text.len() => {
                    let n = text[i];
                    i += 1;
                    match n {
                        b'0'..=b'9' => {
                            let g = (n - b'0') as usize;
                            if g > 0 && groups.is_some_and(|c| g > c) {
                                return Err(self.error(&format!("invalid reference \\{g} on `s' command's RHS")));
                            }
                            flush(&mut parts, &mut literal);
                            parts.push(ReplacementPart::Group(g));
                        }
                        b'L' | b'U' | b'l' | b'u' | b'E' => {
                            flush(&mut parts, &mut literal);
                            parts.push(ReplacementPart::Case(match n {
                                b'L' => CaseConversion::Lower,
                                b'U' => CaseConversion::Upper,
                                b'l' => CaseConversion::LowerNext,
                                b'u' => CaseConversion::UpperNext,
                                _ => CaseConversion::Stop,
                            }));
                        }
                        b'n' => literal.push(b'\n'),
                        other => literal.push(other),
                    }
                }
                other => literal.push(other),
            }
        }
        flush(&mut parts, &mut literal);
        Ok(parts)
    }

    fn transliterate(&mut self) -> Result<Kind> {
        let Some(delim) = self.next() else {
            return Err(self.error("unterminated `y' command"));
        };
        let Some(source) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `y' command"));
        };
        let Some(dest) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `y' command"));
        };
        let locale = self.options().locale;
        let src_chars = y_chars(&source, locale);
        let dst_chars = y_chars(&dest, locale);
        if src_chars.len() != dst_chars.len() {
            return Err(self.error("strings for `y' command are different lengths"));
        }
        self.end_of_command()?;
        Ok(Kind::Translate(src_chars.into_iter().zip(dst_chars).collect()))
    }
}

enum AddrParsed {
    Line(u64),
    Last,
    Step { first: u64, step: u64 },
    Regex(Pattern),
    Plus(u64),
    Multiple(u64),
}

fn is_regex_special(b: u8) -> bool {
    matches!(b, b'.' | b'*' | b'[' | b']' | b'^' | b'$' | b'\\' | b'+' | b'?' | b'(' | b')' | b'{' | b'}' | b'|')
}

/// Splits the strings of `y` into characters, resolving `\\`, `\n`, and `\delim`.
/// In the UTF-8 locale a character is a whole sequence, otherwise a byte.
fn y_chars(text: &[u8], locale: Locale) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let b = text[i];
        if b == b'\\' && i + 1 < text.len() {
            let n = text[i + 1];
            i += 2;
            out.push(vec![match n {
                b'n' => b'\n',
                b't' => b'\t',
                other => other,
            }]);
            continue;
        }
        let len = match locale {
            Locale::Utf8 => utf8_len(b).min(text.len() - i),
            Locale::C => 1,
        };
        out.push(text[i..i + len].to_vec());
        i += len;
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Resolves the GNU escapes `\a \f \v \t \r \cX \dNNN \oNNN \xHH` into the
/// bytes they stand for. Other escapes are kept for the next stage.
fn normalize_escapes(text: &[u8], _posixly_correct: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let b = text[i];
        if b != b'\\' || i + 1 >= text.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let n = text[i + 1];
        match n {
            b'a' => {
                out.push(0x07);
                i += 2;
            }
            b'f' => {
                out.push(0x0c);
                i += 2;
            }
            b'v' => {
                out.push(0x0b);
                i += 2;
            }
            b't' => {
                out.push(b'\t');
                i += 2;
            }
            b'r' => {
                out.push(b'\r');
                i += 2;
            }
            b'd' | b'o' | b'x' => {
                let (radix, max) = match n {
                    b'd' => (10, 3),
                    b'o' => (8, 3),
                    _ => (16, 2),
                };
                let mut j = i + 2;
                let mut value: u32 = 0;
                let mut digits = 0;
                while j < text.len() && digits < max {
                    match (text[j] as char).to_digit(radix) {
                        Some(d) => {
                            value = value * radix + d;
                            j += 1;
                            digits += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    out.push(n);
                    i += 2;
                } else {
                    out.push((value & 0xff) as u8);
                    i = j;
                }
            }
            b'c' if i + 2 < text.len() => {
                let x = text[i + 2].to_ascii_uppercase();
                out.push(x ^ 0x40);
                i += 3;
            }
            _ => {
                out.push(b);
                out.push(n);
                i += 2;
            }
        }
    }
    out
}

/// Compares version strings like `strverscmp`: runs of digits compare as numbers,
/// anything else byte by byte, and a longer string is greater when one is a prefix.
fn version_compare(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let da = a[si..i].iter().skip_while(|&&c| c == b'0').copied().collect::<Vec<_>>();
            let db = b[sj..j].iter().skip_while(|&&c| c == b'0').copied().collect::<Vec<_>>();
            let order = da.len().cmp(&db.len()).then_with(|| da.cmp(&db));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

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
    /// `-z`: the record separator is NUL.
    pub null_data: bool,
    pub locale: Locale,
    /// `--glibc-compat`: reproduce the quirks of the C library's regex matcher.
    pub glibc_compat: bool,
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
    /// Indexes of the open `{` commands, and where each was read.
    blocks: Vec<usize>,
    block_positions: Vec<usize>,
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
        block_positions: Vec::new(),
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
        let mut options = self.span_at(self.pos.saturating_sub(1)).options;
        if self.script.extensions {
            options.posixly_correct = false;
        }
        options
    }

    fn error_at(&self, pos: usize, message: &str) -> SyntaxError {
        let span = self.span_at(pos.saturating_sub(1).min(self.src.len()));
        let location = match &span.source {
            ChunkSource::Expression(n) => Location::Expression {
                number: *n,
                // The newline that joins two chunks is not part of either.
                position: pos.saturating_sub(span.start).min(span.end - span.start),
            },
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
            let opened_at = self.block_positions.last().copied().unwrap_or(0);
            return Err(self.error_at(opened_at, "unmatched `{'")).map_err(|mut e| {
                // GNU reports an unmatched `{` at char 0 of the expression.
                if let Location::Expression { position, .. } = &mut e.location {
                    *position = 0;
                }
                e
            });
        }
        // GNU sed checks the jumps from the last one written back to the first.
        for (index, label, pos) in std::mem::take(&mut self.pending_jumps).into_iter().rev() {
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
                        message: format!("can't find label for jump to `{}'", crate::cli::lossless_bytes(&label)),
                        status: 4,
                        touched: self.touched.clone(),
                    });
                }
            }
        }
        self.script.write_files = std::mem::take(&mut self.touched);
        Ok(self.script)
    }

    // ---- addresses -----------------------------------------------------------------

    fn read_number(&mut self) -> u64 {
        let mut n: u64 = 0;
        while let Some(b @ b'0'..=b'9') = self.peek() {
            // GNU sed lets a number that does not fit wrap around.
            n = n.wrapping_mul(10).wrapping_add(u64::from(b - b'0'));
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
                // Blanks may stand before and after the `~` of `first~step`.
                let blanks_start = self.pos;
                if !self.options().posix {
                    self.skip_blanks();
                }
                if self.peek() == Some(b'~') && !self.options().posix {
                    self.pos += 1;
                    self.skip_blanks();
                    let step = self.read_number();
                    if n == 0 && step == 0 {
                        self.zero_address = true;
                    }
                    return Ok(Some(AddrParsed::Step { first: n, step }));
                }
                self.pos = blanks_start;
                Ok(Some(AddrParsed::Line(n)))
            }
            b'+' | b'~' if !first && !self.options().posix => {
                self.pos += 1;
                self.skip_blanks();
                let n = self.read_number();
                Ok(Some(if ch == b'+' { AddrParsed::Plus(n) } else { AddrParsed::Multiple(n) }))
            }
            b'+' | b'~' if first && !self.options().posix => {
                self.pos += 1;
                self.skip_blanks();
                if self.read_number() != 0 {
                    return Err(self.error("invalid usage of +N or ~N as first address"));
                }
                // `+0` and `~0` (or a bare sign) as the first address select every line.
                Ok(Some(AddrParsed::Null))
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
                        None => return Err(self.error("unterminated address regex")),
                    }
                } else {
                    b'/'
                };
                self.check_delimiter(delim)?;
                if delim == b'\\' {
                    return Err(self.error("unterminated address regex"));
                }
                let Some(text) = self.read_delimited(delim, true) else {
                    return Err(self.error("unterminated address regex"));
                };
                let (mut icase, mut multiline) = (false, false);
                while !self.options().posix {
                    // Blanks may stand between the regexp and its flags, and between the flags.
                    self.skip_blanks();
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
            null_data: o.null_data,
            locale: o.locale,
            glibc_compat: o.glibc_compat,
        };
        match Regex::compile(&normalize_escapes(text, o.posixly_correct), &syntax) {
            Ok(re) => {
                let source = normalize_escapes_for_debug(text, o.posixly_correct);
                Ok(Pattern { regex: Some(Rc::new(re)), source, icase, multiline })
            }
            Err(e) if !e.has_location() => Err(SyntaxError {
                location: Location::Bare,
                status: e.exit_status(),
                message: e.message,
                touched: self.touched.clone(),
            }),
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
                if (next == b'n' && regex) || next == b'\n' {
                    out.push(b'\n');
                } else if next == delim && !(next == b'&' && !regex) {
                    // `\` and the delimiter: just the delimiter, which then has its usual meaning.
                    out.push(next);
                } else {
                    out.push(b'\\');
                    out.push(next);
                }
                continue;
            }
            if ch == b'[' && regex {
                out.push(ch);
                self.copy_bracket(&mut out, limit)?;
                continue;
            }
            out.push(ch);
        }
    }

    /// Copies a bracket expression (after its `[`) up to and including the `]`, so that a
    /// delimiter inside it does not end the regexp. `None` if it is not closed on the line.
    fn copy_bracket(&mut self, out: &mut Vec<u8>, limit: usize) -> Option<()> {
        let mut ch = self.bracket_char(limit)?;
        if ch == b'^' {
            out.push(ch);
            ch = self.bracket_char(limit)?;
        }
        if ch == b']' {
            out.push(ch);
            ch = self.bracket_char(limit)?;
        }
        loop {
            out.push(ch);
            match ch {
                b']' => return Some(()),
                b'[' => {
                    // `[.`, `[=` and `[:` run to their closing `.]`, `=]` or `:]`.
                    if let Some(kind @ (b'.' | b'=' | b':')) = self.peek() {
                        self.pos += 1;
                        out.push(kind);
                        let mut previous = 0u8;
                        loop {
                            let c = self.bracket_char(limit)?;
                            out.push(c);
                            if previous == kind && c == b']' {
                                break;
                            }
                            previous = c;
                        }
                    }
                }
                _ => {}
            }
            ch = self.bracket_char(limit)?;
        }
    }

    /// The next character of a bracket expression: a raw newline or the end ends the scan.
    fn bracket_char(&mut self, limit: usize) -> Option<u8> {
        if self.pos >= limit {
            return None;
        }
        let ch = self.next()?;
        if ch == b'\n' {
            self.pos -= 1;
            return None;
        }
        Some(ch)
    }

    // ---- commands ------------------------------------------------------------------

    fn command(&mut self) -> Result<()> {
        let start_of_command = self.pos;
        let a1 = self.address(true)?;
        let mut selector = None;
        if let Some(a1) = a1 {
            self.skip_blanks();
            if self.peek() == Some(b',') {
                self.pos += 1;
                self.skip_blanks();
                let Some(a2) = self.address(false)? else {
                    self.next();
                    return Err(self.error("unexpected `,'"));
                };
                selector = Some(self.range(a1, a2)?);
            } else {
                selector = Some(self.single(a1)?);
            }
        }
        self.skip_blanks();
        if self.zero_address {
            // GNU sed reports line address 0 after reading the character that follows the address,
            // except for `r`, which accepts it (insert a file before the first line).
            let is_read = self.peek() == Some(b'r') && matches!(selector, Some(Selector::Single(Addr::Line(0) | Addr::Step { first: 0, step: 0 })));
            if !is_read || self.options().posix {
                self.next();
                return Err(self.error("invalid usage of line address 0"));
            }
            self.zero_address = false;
        }

        let mut negate = false;
        if self.peek() == Some(b'!') {
            self.pos += 1;
            negate = true;
            self.skip_blanks();
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
            return Err(self.error(&format!("unknown command: `{}'", crate::cli::lossless_bytes(&[ch]))));
        }
        if self.options().posix
            && matches!(selector, Some(Selector::Range(..)))
            && matches!(ch, b'=' | b'a' | b'i' | b'r' | b'l')
        {
            return Err(self.error("command only uses one address"));
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
                self.block_positions.push(self.pos);
                Kind::BlockStart(0)
            }
            b'}' => {
                if self.blocks.is_empty() {
                    return Err(self.error("unexpected `}'"));
                }
                if selector.is_some() {
                    return Err(self.error("`}' doesn't want any addresses"));
                }
                let open = self.blocks.pop().expect("blocks is not empty: checked above");
                self.block_positions.pop();
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
                    int_argument(self.read_number())
                } else {
                    None
                };
                self.end_of_command()?;
                Kind::List(width)
            }
            b'L' if !self.options().posix => {
                self.skip_blanks();
                let width = if self.peek().is_some_and(|b| b.is_ascii_digit()) { int_argument(self.read_number()) } else { None };
                self.end_of_command()?;
                Kind::Removed(width)
            }
            b'q' | b'Q' => {
                if matches!(selector, Some(Selector::Range(..))) {
                    return Err(self.error("command only uses one address"));
                }
                self.skip_blanks();
                let code = if !self.options().posix && self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    int_argument(self.read_number())
                } else {
                    None
                };
                self.end_of_command()?;
                if ch == b'q' { Kind::Quit(code) } else { Kind::QuitSilent(code) }
            }
            b'r' | b'R' | b'w' | b'W' => {
                // `--sandbox` has rejected these commands already (see above).
                let name = self.read_filename()?;
                if matches!(ch, b'w' | b'W') {
                    self.create_write_file(&name)?;
                }
                match ch {
                    b'r' => Kind::ReadFile(name),
                    b'R' => Kind::ReadLine(name),
                    b'w' => Kind::WriteFile(name),
                    _ => Kind::WriteFirstLine(name),
                }
            }
            b'e' => {
                self.skip_blanks();
                // The command is read like the text of `a`, `i` and `c`, newline included.
                if self.peek().is_none_or(|b| b == b'\n') { Kind::Execute(Vec::new()) } else { Kind::Execute(self.read_text()?) }
            }
            b's' => self.substitute()?,
            b'y' => self.transliterate()?,
            b'v' => {
                let label = self.read_label();
                self.check_version(&label)?;
                // `v` turns the GNU extensions on even when POSIXLY_CORRECT is set.
                self.script.extensions = true;
                Kind::Version
            }
            other => {
                return Err(self.error(&format!("unknown command: `{}'", crate::cli::lossless_bytes(&[other]))));
            }
        };
        let (selector, kind) = match (selector, kind) {
            (Some(Selector::Single(Addr::Line(0) | Addr::Step { first: 0, step: 0 })), Kind::ReadFile(name)) => {
                (Some(Selector::Single(Addr::Line(1))), Kind::ReadFileNow(name))
            }
            (selector, kind) => (selector, kind),
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
            AddrParsed::Null => Ok(Selector::Single(Addr::Null)),
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
            AddrParsed::Null => Addr::Null,
            AddrParsed::Regex(p) => Addr::Regex(p),
            AddrParsed::Plus(_) | AddrParsed::Multiple(_) => unreachable!("rejected as a first address"),
        };
        let second = match a2 {
            AddrParsed::Line(n) => Addr2::Line(n),
            AddrParsed::Last => Addr2::Last,
            AddrParsed::Regex(p) => Addr2::Regex(p),
            AddrParsed::Plus(n) => Addr2::Plus(n),
            AddrParsed::Multiple(n) => Addr2::Multiple(n),
            AddrParsed::Step { first, step } => Addr2::Step { first, step },
            AddrParsed::Null => unreachable!("only a first address can be null"),
        };
        Ok(Selector::Range(first, second))
    }

    // ---- argument readers ----------------------------------------------------------

    /// A label runs to whitespace, `;` or `}`; leading whitespace is skipped.
    fn read_label(&mut self) -> Vec<u8> {
        self.skip_blanks();
        if self.peek() == Some(b'#') {
            return Vec::new();
        }
        let start = self.pos;
        // Only a space, a tab, a newline, `;` or `#` ends a label (not a carriage return or a form feed).
        while self.peek().is_some_and(|b| !matches!(b, b' ' | b'\t' | b'\n' | b';' | b'}') && b != b'#') {
            self.pos += 1;
        }
        self.src[start..self.pos].to_vec()
    }

    /// A file name runs to the end of the line, and the newline is consumed with it.
    fn read_filename(&mut self) -> Result<Vec<u8>> {
        self.skip_blanks();
        let start = self.pos;
        while self.peek().is_some_and(|b| b != b'\n') {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("missing filename in r/R/w/W commands"));
        }
        let name = self.src[start..self.pos].to_vec();
        if self.peek() == Some(b'\n') {
            self.pos += 1;
        }
        Ok(name)
    }

    /// GNU sed creates (truncates) every `w` file while it reads the script, so a file that
    /// cannot be created stops the run with status 4 even before later syntax errors. The
    /// parser has no side effects: it records the names in order and the caller creates them
    /// (see `Script::write_files` and `SyntaxError::touched`).
    fn create_write_file(&mut self, name: &[u8]) -> Result<()> {
        self.touched.push(name.to_vec());
        Ok(())
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
        }
        let mut raw = Vec::new();
        while let Some(ch) = self.next() {
            match ch {
                b'\\' => match self.next() {
                    Some(b'\n') => raw.extend_from_slice(b"\\\n"),
                    Some(other) => {
                        raw.push(b'\\');
                        raw.push(other);
                    }
                    None if self.options().posix => return Err(self.error("incomplete command")),
                    None => {
                        // A backslash that ends the script leaves the text as it was written.
                        raw.push(b'\n');
                        return Ok(raw);
                    }
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

    /// In a UTF-8 locale the byte that starts a multibyte character cannot delimit a regexp.
    fn check_delimiter(&self, delim: u8) -> Result<()> {
        if self.options().locale == Locale::Utf8 && (0xc2..=0xfd).contains(&delim) {
            return Err(self.error("delimiter character is not a single-byte character"));
        }
        Ok(())
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
        self.check_delimiter(delim)?;
        let Some(pattern_text) = self.read_delimited(delim, true) else {
            return Err(self.error("unterminated `s' command"));
        };
        let Some(replacement_text) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `s' command"));
        };
        if has_recursive_control_escape(&replacement_text) {
            return Err(self.error("recursive escaping after \\c not allowed"));
        }

        // Flags.
        let mut global = false;
        let mut occurrence = 0usize;
        let mut print_before = false;
        let mut print_after = false;
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
                    let seen = if eval { &mut print_after } else { &mut print_before };
                    if *seen {
                        return Err(self.error("multiple `p' options to `s' command"));
                    }
                    *seen = true;
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
                    self.create_write_file(&name)?;
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

        // The regexp is converted after the flags, so the error is placed after them.
        if has_recursive_control_escape(&pattern_text) {
            return Err(self.error("recursive escaping after \\c not allowed"));
        }
        let pattern = self.compile_pattern(&pattern_text, icase, multiline)?;
        let replacement = self.parse_replacement(&replacement_text, &pattern)?;
        Ok(Kind::Substitute(Box::new(Substitute {
            pattern,
            replacement,
            global,
            occurrence,
            print_before,
            print_after,
            eval,
            write,
        })))
    }

    fn parse_replacement(&self, text: &[u8], pattern: &Pattern) -> Result<Vec<ReplacementPart>> {
        let text = normalize_escapes_for(text, self.options().posixly_correct, true, false);
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
                            if g > 0 && !self.options().posix && groups.is_some_and(|c| g > c) {
                                return Err(self.error(&format!("invalid reference \\{g} on `s' command's RHS")));
                            }
                            flush(&mut parts, &mut literal);
                            parts.push(ReplacementPart::Group(g));
                        }
                        b'L' | b'U' | b'l' | b'u' | b'E' if !self.options().posix => {
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
                        b'c' if i >= text.len() => literal.push(b'\\'),
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
        self.check_delimiter(delim)?;
        let Some(source) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `y' command"));
        };
        if has_recursive_control_escape(&source) {
            return Err(self.error("recursive escaping after \\c not allowed"));
        }
        let Some(dest) = self.read_delimited(delim, false) else {
            return Err(self.error("unterminated `y' command"));
        };
        if has_recursive_control_escape(&dest) {
            return Err(self.error("recursive escaping after \\c not allowed"));
        }
        let locale = self.options().locale;
        let posixly_correct = self.options().posixly_correct;
        let src_chars = y_chars(&normalize_escapes(&source, posixly_correct), locale);
        let dst_chars = y_chars(&normalize_escapes(&dest, posixly_correct), locale);
        if src_chars.len() != dst_chars.len() {
            return Err(self.error("strings for `y' command are different lengths"));
        }
        self.end_of_command()?;
        Ok(Kind::Translate(src_chars.into_iter().zip(dst_chars).collect()))
    }
}

/// Whether the text has `\c\X` with `X` other than a second backslash: GNU sed refuses a
/// backslash as the character a control escape works on, unless it is written `\c\\`.
fn has_recursive_control_escape(text: &[u8]) -> bool {
    let mut i = 0;
    while i + 1 < text.len() {
        if text[i] != b'\\' {
            i += 1;
        } else if text[i + 1] == b'c' && text.get(i + 2) == Some(&b'\\') && text.get(i + 3).is_some() {
            if text[i + 3] != b'\\' {
                return true;
            }
            i += 4;
        } else {
            i += if text[i + 1] == b'c' && i + 2 < text.len() { 3 } else { 2 };
        }
    }
    false
}

enum AddrParsed {
    Line(u64),
    Last,
    Step { first: u64, step: u64 },
    Regex(Pattern),
    Null,
    Plus(u64),
    Multiple(u64),
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
                // `\t` was turned into a tab before this point.
                other => other,
            }]);
            continue;
        }
        let len = match locale {
            Locale::Utf8 => crate::runtime::char_len(&text[i..]),
            Locale::C => 1,
        };
        out.push(text[i..i + len].to_vec());
        i += len;
    }
    out
}

/// The number after `l`, `L`, `q` or `Q` as GNU sed keeps it: an `int`, where -1 means that there was
/// no number.
fn int_argument(n: u64) -> Option<i32> {
    match n as u32 as i32 {
        -1 => None,
        v => Some(v),
    }
}

fn normalize_escapes(text: &[u8], posixly_correct: bool) -> Vec<u8> {
    normalize_escapes_for(text, posixly_correct, false, false)
}

/// The regexp text as GNU sed keeps it, which `--debug` writes: `\n` is a newline character, and a
/// backslash made by `\x5c` stays in front of the letter after it.
fn normalize_escapes_for_debug(text: &[u8], posixly_correct: bool) -> Vec<u8> {
    normalize_escapes_for(text, posixly_correct, false, true)
}

/// Resolves the GNU escapes `\a \f \v \t \r \cX \dNNN \oNNN \xHH` into the
/// bytes they stand for. Other escapes are kept for the next stage. Inside a
/// bracket expression they are left alone under POSIXLY_CORRECT. In a replacement
/// a produced `&` or backslash stays literal.
fn normalize_escapes_for(text: &[u8], posixly_correct: bool, replacement: bool, debug: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    let mut in_bracket = false;
    while i < text.len() {
        let b = text[i];
        if !replacement {
            if !in_bracket && b == b'[' {
                in_bracket = true;
                out.push(b);
                i += 1;
                // A leading `^` and a leading `]` belong to the bracket expression.
                if text.get(i) == Some(&b'^') {
                    out.push(b'^');
                    i += 1;
                }
                if text.get(i) == Some(&b']') {
                    out.push(b']');
                    i += 1;
                }
                continue;
            }
            if in_bracket && b == b']' {
                in_bracket = false;
                out.push(b);
                i += 1;
                continue;
            }
            if in_bracket && b == b'[' {
                // `[:class:]`, `[.symbol.]` and `[=class=]` run to their own end, which is not
                // the end of the bracket expression.
                if let Some(&kind @ (b':' | b'.' | b'=')) = text.get(i + 1) {
                    if let Some(len) = text[i + 2..].windows(2).position(|w| w[0] == kind && w[1] == b']') {
                        out.extend_from_slice(&text[i..i + 2 + len + 2]);
                        i += 2 + len + 2;
                        continue;
                    }
                }
            }
        }
        if b != b'\\' || i + 1 >= text.len() || (in_bracket && posixly_correct) {
            out.push(b);
            i += 1;
            continue;
        }
        let n = text[i + 1];
        if in_bracket && n == b'n' {
            // Inside a bracket expression `\n` is a newline (GNU sed only; not under POSIXLY_CORRECT).
            out.push(b'\n');
            i += 2;
            continue;
        }
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
                    let produced = (value & 0xff) as u8;
                    if replacement && (produced == b'&' || produced == b'\\') {
                        out.push(b'\\');
                    }
                    let opens_escape = out.iter().rev().take_while(|&&c| c == b'\\').count() % 2 == 0;
                    out.push(produced);
                    i = j;
                    // A backslash made by `\x5c` is an ordinary regexp escape: the escape
                    // letters sed itself converts are not converted again, so `\x5cn` and
                    // `\x5ct` match a plain `n` and `t`.
                    if !replacement && !debug && produced == b'\\' && opens_escape {
                        if let Some(&next @ (b'n' | b't' | b'a' | b'f' | b'v' | b'r')) = text.get(i) {
                            out.pop();
                            out.push(next);
                            i += 1;
                        }
                    }
                }
            }
            b'c' if i + 2 < text.len() => {
                let mut x = text[i + 2];
                let mut used = 3;
                if x == b'\\' && text.get(i + 3) == Some(&b'\\') {
                    used = 4;
                }
                x = x.to_ascii_uppercase();
                out.push(x ^ 0x40);
                i += used;
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

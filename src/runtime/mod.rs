//! Cycle, pattern/hold space, and command evaluation (sequential VM). No CLI argument parsing.

mod case;
mod list;

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command as Process, Stdio};
use std::rc::Rc;

use crate::io::{write_error, Fetched, Outputs, RecordSource};
use crate::regex::{Captures, Locale, Regex};
use crate::syntax::{Addr, Addr2, BranchWhen, Kind, Pattern, ReplacementPart, Script, Selector, Substitute};
use crate::Fatal;

/// Options that change how a script runs.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// `-n`: do not print the pattern space at the end of each cycle.
    pub quiet: bool,
    pub separator: u8,
    /// `-s`: files are separate streams (line numbers, `$`, and ranges restart).
    pub separate: bool,
    /// `POSIXLY_CORRECT`: `n` and `N` do not print at the end of the input.
    pub posixly_correct: bool,
    /// Default wrap width of the `l` command (`-l N`).
    pub line_length: usize,
    pub locale: Locale,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            quiet: false,
            separator: b'\n',
            separate: false,
            posixly_correct: false,
            line_length: 70,
            locale: Locale::C,
        }
    }
}

/// What the script did with the cycle.
enum Flow {
    /// Ran to the end: print the pattern space (unless `-n`) and start the next cycle.
    End,
    /// `d` and friends: start the next cycle without printing.
    Discard,
    /// `D` with a newline: restart without reading a new line.
    Restart,
    /// Stop: print the pattern space if asked, flush appends, exit.
    Quit { code: i32, print: bool },
    /// `Q`: stop immediately.
    QuitNow { code: i32 },
}

#[derive(Clone, Copy)]
enum RangeState {
    Inactive,
    /// Active until this line (inclusive), or until a regexp matches.
    Active { until: Option<u64> },
}

enum Pending {
    Text(Vec<u8>),
    File(Vec<u8>),
}

struct Machine<'a, S: RecordSource + ?Sized> {
    script: &'a Script,
    options: Options,
    input: &'a mut S,
    out: &'a mut Outputs,
    program: &'a str,
    pattern: Vec<u8>,
    terminated: bool,
    hold: Vec<u8>,
    line: u64,
    file_name: Rc<str>,
    ranges: Vec<RangeState>,
    last_regex: Option<Rc<Regex>>,
    substituted: bool,
    pending: Vec<Pending>,
    read_files: HashMap<Vec<u8>, Option<std::io::BufReader<std::fs::File>>>,
    quit_status: Option<i32>,
    /// `0,/re/` ranges that have already ended; they never restart.
    zero_finished: Vec<usize>,
}

/// Runs `script` over the records of `input`. Returns the exit status requested
/// by `q` or `Q` (0 otherwise).
pub fn run<S: RecordSource + ?Sized>(
    script: &Script,
    options: Options,
    input: &mut S,
    out: &mut Outputs,
    program: &str,
) -> Result<i32, Fatal> {
    for command in &script.commands {
        match &command.kind {
            Kind::WriteFile(name) | Kind::WriteFirstLine(name) => out.open(name)?,
            Kind::Substitute(sub) => {
                if let Some(name) = &sub.write {
                    out.open(name)?;
                }
            }
            _ => {}
        }
    }
    let mut machine = Machine {
        script,
        options,
        input,
        out,
        program,
        pattern: Vec::new(),
        terminated: true,
        hold: Vec::new(),
        line: 0,
        file_name: Rc::from("-"),
        ranges: vec![RangeState::Inactive; script.commands.len()],
        last_regex: None,
        substituted: false,
        pending: Vec::new(),
        read_files: HashMap::new(),
        quit_status: None,
        zero_finished: Vec::new(),
    };
    machine.cycles()
}

impl<S: RecordSource + ?Sized> Machine<'_, S> {
    fn quiet(&self) -> bool {
        self.options.quiet || self.script.quiet
    }

    /// Reads the next record into the pattern space (replacing it or appending).
    /// Returns `false` at the end of the input. Pending output is flushed first.
    fn read_line(&mut self, append: bool) -> Result<bool, Fatal> {
        self.flush_pending()?;
        let Some(Fetched { record, new_file, file_name }) = self.input.next_record()? else {
            return Ok(false);
        };
        if new_file && self.options.separate {
            self.line = 0;
            for r in self.ranges.iter_mut() {
                *r = RangeState::Inactive;
            }
        }
        self.line += 1;
        self.file_name = file_name;
        if append {
            self.pattern.push(self.options.separator);
            self.pattern.extend_from_slice(&record.body);
        } else {
            self.pattern = record.body;
        }
        self.terminated = record.terminated;
        Ok(true)
    }

    fn cycles(&mut self) -> Result<i32, Fatal> {
        let mut restart = false;
        loop {
            if !restart {
                self.substituted = false;
                if !self.read_line(false)? {
                    break;
                }
            }
            restart = false;
            match self.execute()? {
                Flow::End => {
                    self.autoprint()?;
                }
                Flow::Discard => {}
                Flow::Restart => {
                    restart = true;
                }
                Flow::Quit { code, print } => {
                    if print {
                        self.autoprint()?;
                    }
                    self.flush_pending()?;
                    return Ok(code);
                }
                Flow::QuitNow { code } => return Ok(code),
            }
            self.flush_pending()?;
        }
        self.flush_pending()?;
        Ok(self.quit_status.unwrap_or(0))
    }

    fn autoprint(&mut self) -> Result<(), Fatal> {
        if !self.quiet() {
            self.print_pattern()?;
        }
        Ok(())
    }

    fn print_pattern(&mut self) -> Result<(), Fatal> {
        let (pattern, terminated) = (&self.pattern, self.terminated);
        self.out.stdout.write_record(pattern, terminated).map_err(|e| write_error("stdout", &e))
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<(), Fatal> {
        self.out.stdout.write_raw(bytes).map_err(|e| write_error("stdout", &e))
    }

    fn flush_pending(&mut self) -> Result<(), Fatal> {
        for item in std::mem::take(&mut self.pending) {
            match item {
                Pending::Text(text) => self.write_raw(&text)?,
                Pending::File(name) => {
                    let path = String::from_utf8_lossy(&name).into_owned();
                    match std::fs::read(&path) {
                        Ok(bytes) => self.write_raw(&bytes)?,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) if e.raw_os_error() == Some(21) => {
                            eprintln!("{}: read error on {}: Is a directory", self.program, path);
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        Ok(())
    }

    // ---- addresses -----------------------------------------------------------------

    fn is_last(&mut self) -> Result<bool, Fatal> {
        self.input.is_last(self.options.separate)
    }

    fn resolve(&mut self, pattern: &Pattern) -> Result<Rc<Regex>, Fatal> {
        match &pattern.regex {
            Some(re) => {
                self.last_regex = Some(re.clone());
                Ok(re.clone())
            }
            None => self
                .last_regex
                .clone()
                .ok_or_else(|| Fatal::new("-e expression #1, char 0: no previous regular expression", 1)),
        }
    }

    fn regex_matches(&mut self, pattern: &Pattern) -> Result<bool, Fatal> {
        let re = self.resolve(pattern)?;
        let mut caps = Captures::default();
        Ok(re.captures_at(&self.pattern, 0, &mut caps))
    }

    fn addr_matches(&mut self, addr: &Addr) -> Result<bool, Fatal> {
        Ok(match addr {
            Addr::Line(n) => self.line == *n,
            Addr::Last => self.is_last()?,
            Addr::Step { first, step } => {
                if *step == 0 {
                    self.line == *first
                } else {
                    self.line >= *first && (self.line - *first) % *step == 0
                }
            }
            Addr::Regex(p) => self.regex_matches(p)?,
        })
    }

    fn selected(&mut self, index: usize) -> Result<bool, Fatal> {
        let script = self.script;
        let command = &script.commands[index];
        let matched = match &command.selector {
            None => true,
            Some(Selector::Single(a)) => self.addr_matches(a)?,
            Some(Selector::Range(a1, a2)) => self.range_matches(index, a1, a2)?,
        };
        Ok(matched != command.negate)
    }

    fn range_matches(&mut self, index: usize, a1: &Addr, a2: &Addr2) -> Result<bool, Fatal> {
        let line = self.line;
        let mut state = self.ranges[index];
        if let RangeState::Inactive = state {
            let starts = match a1 {
                // `0,/re/`: the range is already active before the first line.
                Addr::Line(0) => {
                    if line == 1 && !self.range_zero_done(index) {
                        state = RangeState::Active { until: None };
                        true
                    } else {
                        false
                    }
                }
                other => self.addr_matches(other)?,
            };
            if !starts {
                return Ok(false);
            }
            if !matches!(a1, Addr::Line(0)) {
                match a2 {
                    Addr2::Line(n) => {
                        if *n <= line {
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some(*n) };
                    }
                    Addr2::Plus(n) => {
                        if *n == 0 {
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some(line + n) };
                    }
                    Addr2::Multiple(n) => {
                        if *n == 0 || line % n == 0 {
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some((line / n + 1) * n) };
                    }
                    Addr2::Last => {
                        if self.is_last()? {
                            return Ok(true);
                        }
                        state = RangeState::Active { until: None };
                    }
                    Addr2::Regex(_) => state = RangeState::Active { until: None },
                }
                self.ranges[index] = state;
                return Ok(true);
            }
        }
        // The range is active: does it end on this line?
        let RangeState::Active { until } = state else { unreachable!() };
        let ends = match (until, a2) {
            (Some(n), _) => {
                if line > n {
                    self.ranges[index] = RangeState::Inactive;
                    return Ok(false);
                }
                line == n
            }
            (None, Addr2::Last) => self.is_last()?,
            (None, Addr2::Regex(p)) => self.regex_matches(p)?,
            (None, Addr2::Line(n)) => line >= *n,
            (None, _) => false,
        };
        self.ranges[index] = if ends { RangeState::Inactive } else { RangeState::Active { until } };
        if matches!(a1, Addr::Line(0)) && ends {
            self.zero_done(index);
        }
        Ok(true)
    }

    fn range_zero_done(&self, index: usize) -> bool {
        // A `0,/re/` range runs once: it never restarts after it has ended.
        self.zero_finished.contains(&index)
    }

    fn zero_done(&mut self, index: usize) {
        self.zero_finished.push(index);
    }
}

impl<S: RecordSource + ?Sized> Machine<'_, S> {
    /// Runs the script once over the current pattern space.
    fn execute(&mut self) -> Result<Flow, Fatal> {
        let script = self.script;
        let mut pc = 0;
        while pc < script.commands.len() {
            let command = &script.commands[pc];
            if !self.selected(pc)? {
                if let Kind::BlockStart(end) = command.kind {
                    pc = end + 1;
                } else {
                    pc += 1;
                }
                continue;
            }
            match &command.kind {
                Kind::BlockStart(_) | Kind::BlockEnd | Kind::Label | Kind::Version => {}
                Kind::Branch { when, target } => {
                    let take = match when {
                        BranchWhen::Always => true,
                        BranchWhen::IfSubstituted => std::mem::replace(&mut self.substituted, false),
                        BranchWhen::IfNotSubstituted => !std::mem::replace(&mut self.substituted, false),
                    };
                    if take {
                        match target {
                            Some(t) => {
                                pc = *t;
                                continue;
                            }
                            None => return Ok(Flow::End),
                        }
                    }
                }
                Kind::Append(text) => self.pending.push(Pending::Text(text.clone())),
                Kind::Insert(text) => self.write_raw(text)?,
                Kind::Change(text) => {
                    let in_range = matches!(command.selector, Some(Selector::Range(..)))
                        && !command.negate
                        && matches!(self.ranges[pc], RangeState::Active { .. });
                    if !in_range {
                        self.write_raw(text)?;
                    }
                    return Ok(Flow::Discard);
                }
                Kind::Delete => return Ok(Flow::Discard),
                Kind::DeleteFirst => match self.pattern.iter().position(|&b| b == self.options.separator) {
                    None => return Ok(Flow::Discard),
                    Some(i) => {
                        self.pattern.drain(..=i);
                        return Ok(Flow::Restart);
                    }
                },
                Kind::Next => {
                    if self.is_last()? {
                        return Ok(if self.options.posixly_correct {
                            Flow::Quit { code: 0, print: false }
                        } else {
                            Flow::Quit { code: 0, print: true }
                        });
                    }
                    self.autoprint()?;
                    if !self.read_line(false)? {
                        return Ok(Flow::Quit { code: 0, print: false });
                    }
                }
                Kind::NextAppend => {
                    if self.is_last()? {
                        return Ok(if self.options.posixly_correct {
                            Flow::Quit { code: 0, print: false }
                        } else {
                            Flow::Quit { code: 0, print: true }
                        });
                    }
                    if !self.read_line(true)? {
                        return Ok(Flow::Quit { code: 0, print: true });
                    }
                }
                Kind::Print => self.print_pattern()?,
                Kind::PrintFirst => {
                    let end = self.pattern.iter().position(|&b| b == self.options.separator).unwrap_or(self.pattern.len());
                    let first = self.pattern[..end].to_vec();
                    self.out.stdout.write_record(&first, true).map_err(|e| write_error("stdout", &e))?;
                }
                Kind::Get => self.pattern = self.hold.clone(),
                Kind::GetAppend => {
                    self.pattern.push(b'\n');
                    self.pattern.extend_from_slice(&self.hold);
                }
                Kind::Hold => self.hold = self.pattern.clone(),
                Kind::HoldAppend => {
                    self.hold.push(b'\n');
                    self.hold.extend_from_slice(&self.pattern);
                }
                Kind::Exchange => std::mem::swap(&mut self.pattern, &mut self.hold),
                Kind::Zap => self.pattern.clear(),
                Kind::LineNumber => {
                    let text = format!("{}\n", self.line);
                    self.write_raw(text.as_bytes())?;
                }
                Kind::List(width) => {
                    let width = width.unwrap_or(self.options.line_length);
                    let text = list::list(&self.pattern, width, self.options.locale);
                    self.write_raw(&text)?;
                }
                Kind::Quit(code) => return Ok(Flow::Quit { code: *code, print: true }),
                Kind::QuitSilent(code) => return Ok(Flow::QuitNow { code: *code }),
                Kind::FileName => {
                    let text = format!("{}\n", self.file_name);
                    self.write_raw(text.as_bytes())?;
                }
                Kind::ReadFile(name) => self.pending.push(Pending::File(name.clone())),
                Kind::ReadLine(name) => {
                    if let Some(line) = self.read_one_line(name) {
                        self.pending.push(Pending::Text(line));
                    }
                }
                Kind::WriteFile(name) => {
                    let (pattern, terminated) = (self.pattern.clone(), self.terminated);
                    self.out.get(name).write_record(&pattern, terminated).map_err(|e| write_error("file", &e))?;
                }
                Kind::WriteFirstLine(name) => {
                    let end = self.pattern.iter().position(|&b| b == self.options.separator).unwrap_or(self.pattern.len());
                    let first = self.pattern[..end].to_vec();
                    self.out.get(name).write_record(&first, true).map_err(|e| write_error("file", &e))?;
                }
                Kind::Execute(command) => self.execute_command(command)?,
                Kind::Translate(pairs) => self.translate(pairs),
                Kind::Substitute(sub) => self.substitute(sub)?,
            }
            pc += 1;
        }
        Ok(Flow::End)
    }

    /// One line of the file named by an `R` command; the file stays open between calls.
    fn read_one_line(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        use std::io::BufRead;
        let reader = self.read_files.entry(name.to_vec()).or_insert_with(|| {
            let path = String::from_utf8_lossy(name).into_owned();
            std::fs::File::open(path).ok().map(std::io::BufReader::new)
        });
        let reader = reader.as_mut()?;
        let mut line = Vec::new();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }

    fn translate(&mut self, pairs: &[(Vec<u8>, Vec<u8>)]) {
        let mut out = Vec::with_capacity(self.pattern.len());
        let mut i = 0;
        let utf8 = self.options.locale == Locale::Utf8;
        while i < self.pattern.len() {
            let len = if utf8 { char_len(&self.pattern[i..]) } else { 1 };
            let ch = &self.pattern[i..i + len];
            match pairs.iter().find(|(from, _)| from == ch) {
                Some((_, to)) => out.extend_from_slice(to),
                None => out.extend_from_slice(ch),
            }
            i += len;
        }
        self.pattern = out;
    }

    /// `e command`: runs it and writes its output now. `e`: runs the pattern space
    /// as a command and replaces the pattern space with the output.
    fn execute_command(&mut self, command: &[u8]) -> Result<(), Fatal> {
        self.out.stdout.flush().ok();
        if command.is_empty() {
            let output = run_shell(&self.pattern);
            let mut output = output;
            if output.last() == Some(&b'\n') {
                output.pop();
            }
            self.pattern = output;
        } else {
            let output = run_shell(command);
            self.write_raw(&output)?;
        }
        Ok(())
    }

    fn substitute(&mut self, sub: &Substitute) -> Result<(), Fatal> {
        let re = self.resolve(&sub.pattern)?;
        let locale = self.options.locale;
        let mut caps = Captures::default();
        let mut result = Vec::with_capacity(self.pattern.len());
        let mut copied = 0;
        let mut pos = 0;
        let mut count = 0usize;
        let mut replaced = false;
        let wanted = sub.occurrence.max(1);
        let mut previous_end: Option<usize> = None;
        while pos <= self.pattern.len() && re.captures_at(&self.pattern, pos, &mut caps) {
            let (start, end) = caps.whole();
            // An empty match right after the previous match does not count.
            if start == end && previous_end == Some(start) {
                if start >= self.pattern.len() {
                    break;
                }
                pos = start + char_len_in(&self.pattern, start, locale);
                previous_end = None;
                continue;
            }
            count += 1;
            if count >= wanted {
                result.extend_from_slice(&self.pattern[copied..start]);
                case::append_replacement(&mut result, &sub.replacement, &self.pattern, &caps, locale);
                copied = end;
                replaced = true;
                if !sub.global {
                    break;
                }
            }
            previous_end = Some(end);
            if end == start {
                if start >= self.pattern.len() {
                    break;
                }
                pos = start + char_len_in(&self.pattern, start, locale);
            } else {
                pos = end;
            }
        }
        if !replaced {
            return Ok(());
        }
        result.extend_from_slice(&self.pattern[copied..]);
        self.pattern = result;
        self.substituted = true;
        if sub.eval {
            self.execute_command(&[])?;
        }
        for _ in 0..sub.print.min(1) {
            self.print_pattern()?;
        }
        if let Some(name) = &sub.write {
            let (pattern, terminated) = (self.pattern.clone(), self.terminated);
            self.out.get(name).write_record(&pattern, terminated).map_err(|e| write_error("file", &e))?;
        }
        Ok(())
    }
}

fn run_shell(command: &[u8]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let output = Process::new("sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(command))
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output();
    output.map(|o| o.stdout).unwrap_or_default()
}

/// Length in bytes of the character at the start of `bytes` (UTF-8), or 1 for an invalid sequence.
fn char_len(bytes: &[u8]) -> usize {
    let want = match bytes.first() {
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => return 1,
    };
    if bytes.len() >= want && std::str::from_utf8(&bytes[..want]).is_ok() { want } else { 1 }
}

fn char_len_in(bytes: &[u8], at: usize, locale: Locale) -> usize {
    match locale {
        Locale::C => 1,
        Locale::Utf8 => char_len(&bytes[at..]),
    }
}

#[allow(dead_code)]
fn unused(_: &ReplacementPart, _: &mut dyn Write) {}

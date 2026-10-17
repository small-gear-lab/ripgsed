//! Cycle, pattern/hold space, and command evaluation (sequential VM). No CLI argument parsing.

mod case;
mod list;

use std::collections::HashMap;
use std::process::{Command as Process, Stdio};
use std::rc::Rc;

use crate::debug;
use crate::io::{write_error, Fetched, Outputs, RecordSource};
use crate::regex::{Captures, Locale, Regex};
use crate::syntax::{Addr, Addr2, BranchWhen, Kind, Pattern, ReplacementPart, Script, Selector, Substitute};
use crate::Fatal;

/// Options that change how a script runs.
#[derive(Debug, Clone)]
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
    /// `-i[SUFFIX]`: edit files in place; the suffix names the backup (empty: no backup).
    pub in_place: Option<String>,
    pub follow_symlinks: bool,
    pub unbuffered: bool,
    /// `--debug`: print the program and annotate its execution.
    pub debug: bool,
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
            in_place: None,
            follow_symlinks: false,
            unbuffered: false,
            debug: false,
        }
    }
}

/// What the script did with the cycle.
enum Flow {
    /// Ran to the end: print the pattern space (unless `-n`) and start the next cycle.
    End,
    /// `d` and friends: start the next cycle without printing.
    Discard,
    /// `D` without a newline: like `d`, but the trace does not mark the end of the cycle.
    DiscardQuietly,
    /// `D` with a newline: restart without reading a new line.
    Restart,
    /// Stop: print the pattern space if asked, flush appends, exit.
    Quit { code: i32, print: bool, finish_line: bool, trace_end: bool },
    /// `Q`: stop immediately.
    QuitNow { code: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeState {
    Inactive,
    /// A range with a numeric start has ended; it cannot start again.
    Closed,
    /// Active until this line (inclusive), or until a regexp matches. For `addr,+N` a line
    /// past the end (the end line was skipped) still belongs to the range; for `addr,N` it
    /// does not (`overshoot`).
    Active { until: Option<u64>, overshoot: bool },
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
    pattern: Vec<u8>,
    terminated: bool,
    hold: Vec<u8>,
    hold_terminated: bool,
    line: u64,
    file_name: Rc<str>,
    ranges: Vec<RangeState>,
    last_regex: Option<Rc<Regex>>,
    /// The regexp last used was written in an address (`s//\N/` is checked against it).
    last_regex_from_address: bool,
    substituted: bool,
    pending: Vec<Pending>,
    read_files: HashMap<Vec<u8>, Option<std::io::BufReader<std::fs::File>>>,
    quit_status: Option<i32>,
    /// `q` or `Q` ended the run.
    quit: bool,
    /// `0,/re/` ranges that have already ended; they never restart.
    zero_finished: Vec<usize>,
    /// The indentation of the trace; it follows `{` and `}` as they are visited.
    debug_depth: usize,
    /// The registers of the first match of the last `s`, for the trace.
    debug_registers: Option<Vec<u8>>,
    /// The `e command` commands that have run: GNU sed's trace then shows the end of their text differently.
    executed_e: std::collections::HashSet<usize>,
}

/// Runs `script` over the records of `input`. Returns the exit status requested
/// by `q` or `Q` (0 otherwise).
pub fn run<S: RecordSource + ?Sized>(
    script: &Script,
    options: Options,
    input: &mut S,
    out: &mut Outputs,
) -> Result<i32, Fatal> {
    run_from(script, options, input, out, 0, None).map(|ended| ended.code)
}

/// Where `q` or `Q` ended the run inside a chunk.
pub struct ChunkQuit {
    /// The exit status asked for.
    pub code: i32,
    /// How many bytes of the chunk the run had read: up to the end of the record it quit on.
    pub consumed: usize,
}

/// What goes from one record to the next in a record-local script: the state of its ranges (the
/// `0,/re/` ranges that have ended are part of it). `None` where a chunk takes it is the state at the
/// start of the input, and a chunk that ends in that state gives `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carry {
    ranges: Vec<RangeState>,
    zero_finished: Vec<usize>,
}

impl Carry {
    fn initial(script: &Script) -> Carry {
        Carry { ranges: vec![RangeState::Inactive; script.commands.len()], zero_finished: Vec::new() }
    }
}

/// Whether a script has ranges, that is, whether what a chunk ends in is something the next one needs.
pub fn uses_ranges(script: &Script) -> bool {
    script.commands.iter().any(|c| matches!(c.selector, Selector::Range(..)))
}

/// A run of whole records for [`run_chunk`].
pub struct ChunkInput {
    pub data: Vec<u8>,
    /// The line number of the first record.
    pub first_line: u64,
    /// Whether no record follows the chunk in any input.
    pub last: bool,
    /// The input the records are from.
    pub name: std::sync::Arc<str>,
}

/// The bytes a script produced for one chunk, whether they end without a terminator, where the
/// run quit, if it did, and what the chunk leaves for the one after it.
pub struct ChunkOutput {
    pub bytes: Vec<u8>,
    pub unterminated: bool,
    pub quit: Option<ChunkQuit>,
    pub carry: Option<Carry>,
    /// What the files named by `w` got: the name, the bytes, and whether they end without a terminator.
    pub files: Vec<(Vec<u8>, Vec<u8>, bool)>,
}

/// The records of one chunk, as a record source.
struct ChunkRecords<'a> {
    data: &'a [u8],
    pos: usize,
    separator: u8,
    name: Rc<str>,
    spare: Vec<u8>,
    /// Whether no record follows the chunk in the input.
    last: bool,
}

impl RecordSource for ChunkRecords<'_> {
    fn next_record(&mut self) -> Result<Option<Fetched>, Fatal> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let rest = &self.data[self.pos..];
        let (len, used, terminated) = match crate::search::memchr(self.separator, rest) {
            Some(i) => (i, i + 1, true),
            None => (rest.len(), rest.len(), false),
        };
        let mut body = std::mem::take(&mut self.spare);
        body.clear();
        body.extend_from_slice(&rest[..len]);
        self.pos += used;
        Ok(Some(Fetched { record: crate::io::Record { body, terminated }, new_file: false, file_name: self.name.clone() }))
    }

    fn recycle(&mut self, buf: Vec<u8>) {
        self.spare = buf;
    }

    fn is_last(&mut self, _separate: bool) -> Result<bool, Fatal> {
        // (`-s` and `-i` do not run chunk by chunk, so the end of the chunk is the end of the input or not.)
        Ok(self.pos >= self.data.len() && self.last)
    }
}

/// Runs a record-local script (see `analysis`) over the records of `input`, which start in the state
/// `carry` (`None`: that of the start of the input), and returns what it printed, whether and where it
/// quit, the state it ends in, and what it wrote to files. The script must not read other records, use the hold space, or
/// touch files.
pub fn run_chunk(script: &Script, options: &Options, input: &ChunkInput, carry: Option<&Carry>) -> Result<ChunkOutput, Fatal> {
    let data = &input.data;
    let sink = Rc::new(std::cell::RefCell::new(Vec::with_capacity(data.len() + data.len() / 8)));
    let mut outputs = Outputs::capturing(Box::new(crate::io::SharedBuffer(sink.clone())), options.separator);
    let mut source = ChunkRecords { data, pos: 0, separator: options.separator, name: Rc::from(&*input.name), spare: Vec::new(), last: input.last };
    let ended = run_from(script, options.clone(), &mut source, &mut outputs, input.first_line.saturating_sub(1), carry)?;
    let unterminated = outputs.stdout.ends_unterminated();
    outputs.flush_all()?;
    let files = outputs.take_captured();
    drop(outputs);
    let bytes = std::mem::take(&mut *sink.borrow_mut());
    let quit = ended.quit.then_some(ChunkQuit { code: ended.code, consumed: source.pos });
    Ok(ChunkOutput { bytes, unterminated, quit, carry: ended.carry, files })
}

/// How a run ended.
struct Ended {
    code: i32,
    quit: bool,
    /// The state of the ranges, when the script has any and it is not that of the start.
    carry: Option<Carry>,
}

fn run_from<S: RecordSource + ?Sized>(
    script: &Script,
    options: Options,
    input: &mut S,
    out: &mut Outputs,
    line_before: u64,
    carry: Option<&Carry>,
) -> Result<Ended, Fatal> {
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
        pattern: Vec::new(),
        terminated: true,
        hold: Vec::new(),
        hold_terminated: true,
        line: line_before,
        file_name: Rc::from("-"),
        ranges: vec![RangeState::Inactive; script.commands.len()],
        last_regex: None,
        last_regex_from_address: false,
        substituted: false,
        pending: Vec::new(),
        read_files: HashMap::new(),
        quit_status: None,
        quit: false,
        zero_finished: Vec::new(),
        debug_depth: 0,
        debug_registers: None,
        executed_e: std::collections::HashSet::new(),
    };
    if machine.options.debug {
        let listing = debug::program(script, machine.options.locale.is_chars());
        machine.debug_write(&listing)?;
    }
    if let Some(carry) = carry {
        machine.ranges = carry.ranges.clone();
        machine.zero_finished = carry.zero_finished.clone();
    }
    let code = machine.cycles()?;
    let quit = machine.quit;
    let carry = uses_ranges(script)
        .then(|| Carry { ranges: std::mem::take(&mut machine.ranges), zero_finished: std::mem::take(&mut machine.zero_finished) })
        .filter(|carry| *carry != Carry::initial(script));
    Ok(Ended { code, quit, carry })
}

impl<S: RecordSource + ?Sized> Machine<'_, S> {
    /// POSIXLY_CORRECT or --posix, unless the script used `v`.
    fn posixly_correct(&self) -> bool {
        self.options.posixly_correct && !self.script.extensions
    }

    fn quiet(&self) -> bool {
        self.options.quiet || self.script.quiet
    }

    /// Reads the next record into the pattern space (replacing it or appending).
    /// Returns `false` at the end of the input. Pending output is flushed first.
    fn read_line(&mut self, append: bool) -> Result<bool, Fatal> {
        self.flush_pending()?;
        if self.options.in_place.is_some() && self.out.is_in_place() && self.input.is_last(true)? {
            // The file being edited has no more lines: it is complete before the next one is opened
            // (which may fail, or be the same file again).
            self.out.finish_in_place(self.options.in_place.as_deref())?;
        }
        let Some(Fetched { record, new_file, file_name }) = self.input.next_record()? else {
            return Ok(false);
        };
        if new_file && self.options.in_place.is_some() {
            self.out.finish_in_place(self.options.in_place.as_deref())?;
            self.out.begin_in_place(&file_name, self.options.follow_symlinks)?;
        }
        if new_file && self.options.separate {
            self.line = 0;
            for r in self.ranges.iter_mut() {
                *r = RangeState::Inactive;
            }
            self.zero_finished.clear();
            // GNU sed also empties the hold space and rewinds the `R` files at each new file.
            // (only its text: it keeps whether its last line had a terminator).
            self.hold.clear();
            self.read_files.clear();
        }
        self.line += 1;
        self.substituted = false;
        self.file_name = file_name;
        if append {
            self.pattern.push(self.options.separator);
            self.pattern.extend_from_slice(&record.body);
            self.input.recycle(record.body);
        } else {
            let old = std::mem::replace(&mut self.pattern, record.body);
            self.input.recycle(old);
        }
        self.terminated = record.terminated;
        Ok(true)
    }

    fn cycles(&mut self) -> Result<i32, Fatal> {
        match self.cycles_inner() {
            Ok(code) => {
                self.out.finish_in_place(self.options.in_place.as_deref())?;
                Ok(code)
            }
            Err(e) => {
                // A fatal error leaves the file being edited as it was.
                self.out.abort_in_place();
                Err(e)
            }
        }
    }

    /// When no command can act on the next lines (every command is guarded by a line number that
    /// is still ahead, and nothing is printed by default), skips them without running cycles.
    fn skip_ahead(&mut self) -> Result<(), Fatal> {
        if !self.quiet() || self.options.debug || self.options.separate || self.options.in_place.is_some() || self.line == 0 {
            return Ok(());
        }
        let script = self.script;
        let mut target = u64::MAX;
        let mut pc = 0;
        while pc < script.commands.len() {
            let command = &script.commands[pc];
            let ahead = match (&command.selector, &command.kind) {
                _ if command.negate => return Ok(()),
                (Selector::All, Kind::BlockEnd | Kind::Label(_) | Kind::Version) => None,
                (Selector::Single(Addr::Line(n)), _) if *n > self.line => Some(*n),
                (Selector::Single(Addr::Line(_)), _) => None,
                (Selector::Range(Addr::Line(a), _), _) => match self.ranges[pc] {
                    RangeState::Inactive if *a > self.line => Some(*a),
                    RangeState::Closed => None,
                    _ => return Ok(()),
                },
                _ => return Ok(()),
            };
            if let Some(n) = ahead {
                target = target.min(n);
            }
            // The commands inside a block run only when the block is entered.
            pc = match command.kind {
                Kind::BlockStart(end) => end + 1,
                _ => pc + 1,
            };
        }
        if target == u64::MAX || target <= self.line + 1 {
            return Ok(());
        }
        self.flush_pending()?;
        let skipped = self.input.skip_records(target - 1 - self.line)?;
        self.line += skipped;
        Ok(())
    }

    fn cycles_inner(&mut self) -> Result<i32, Fatal> {
        let mut restart = false;
        loop {
            if !restart {
                self.substituted = false;
                self.skip_ahead()?;
                if !self.read_line(false)? {
                    break;
                }
                if self.options.debug {
                    let name = if &*self.file_name == "-" { "STDIN".to_string() } else { self.file_name.to_string() };
                    self.debug_write(format!("INPUT:   '{}' line {}\n", name, self.line).as_bytes())?;
                    self.debug_pattern()?;
                }
            } else if self.options.debug {
                self.debug_pattern()?;
            }
            restart = false;
            match self.execute()? {
                Flow::End => {
                    self.debug_write(b"END-OF-CYCLE:\n")?;
                    self.autoprint()?;
                }
                Flow::Discard => {
                    self.debug_write(b"END-OF-CYCLE:\n")?;
                }
                Flow::DiscardQuietly => {}
                Flow::Restart => {
                    restart = true;
                    continue;
                }
                Flow::Quit { code, print, finish_line, trace_end } => {
                    if print {
                        self.autoprint()?;
                    }
                    if trace_end {
                        self.debug_write(b"END-OF-CYCLE:\n")?;
                    }
                    self.flush_pending()?;
                    if finish_line {
                        self.out.main().finish_line().map_err(|e| write_error("stdout", &e))?;
                    }
                    self.quit = true;
                    return Ok(code);
                }
                Flow::QuitNow { code } => {
                    self.quit = true;
                    return Ok(code);
                }
            }
            self.flush_pending()?;
            if self.options.unbuffered {
                self.out.main().flush().ok();
            }
        }
        self.flush_pending()?;
        Ok(self.quit_status.unwrap_or(0))
    }

    /// Writes to the real standard output (the trace is never written to the file edited in place).
    fn debug_write(&mut self, bytes: &[u8]) -> Result<(), Fatal> {
        if !self.options.debug {
            return Ok(());
        }
        self.out.stdout.write_unchecked(bytes).map_err(|e| write_error("stdout", &e))
    }

    fn debug_pattern(&mut self) -> Result<(), Fatal> {
        let mut line = b"PATTERN: ".to_vec();
        line.extend(debug::escape(&self.pattern));
        line.push(b'\n');
        self.debug_write(&line)
    }

    fn debug_hold(&mut self) -> Result<(), Fatal> {
        let mut line = b"HOLD:    ".to_vec();
        line.extend(debug::escape(&self.hold));
        line.push(b'\n');
        self.debug_write(&line)
    }

    fn debug_command(&mut self, pc: usize, command: &crate::syntax::Command) -> Result<(), Fatal> {
        if matches!(command.kind, Kind::BlockEnd) {
            self.debug_depth = self.debug_depth.saturating_sub(1);
        }
        let mut line = b"COMMAND: ".to_vec();
        line.extend(std::iter::repeat_n(b' ', self.debug_depth * 2));
        line.extend(debug::command_text(command, self.options.locale.is_chars()));
        if matches!(&command.kind, Kind::Execute(c) if !c.is_empty()) && self.executed_e.contains(&pc) {
            // GNU sed overwrote the newline that ends the text of the command when it ran it.
            let last = line.len() - 1;
            line[last] = 0;
        }
        line.push(b'\n');
        if matches!(command.kind, Kind::BlockStart(_)) {
            self.debug_depth += 1;
        }
        self.debug_write(&line)
    }

    /// The state the trace shows after a command: what GNU sed prints, command by command.
    fn debug_after(&mut self, kind: &Kind) -> Result<(), Fatal> {
        match kind {
            Kind::Exchange => {
                self.debug_pattern()?;
                self.debug_hold()
            }
            Kind::GetAppend | Kind::Translate(_) | Kind::Next | Kind::NextAppend | Kind::Zap => self.debug_pattern(),
            // GNU sed shows the hold space after `g` too.
            Kind::Get | Kind::Hold | Kind::HoldAppend => self.debug_hold(),
            Kind::Substitute(_) => {
                self.debug_flush_registers()?;
                self.debug_pattern()
            }
            _ => Ok(()),
        }
    }

    /// The registers of the match of an `s` command, shown before anything its flags print.
    fn debug_flush_registers(&mut self) -> Result<(), Fatal> {
        if let Some(registers) = self.debug_registers.take() {
            self.debug_write(b"MATCHED REGEX REGISTERS\n")?;
            self.debug_write(&registers)?;
        }
        Ok(())
    }

    fn autoprint(&mut self) -> Result<(), Fatal> {
        if !self.quiet() {
            self.print_pattern()?;
        }
        Ok(())
    }

    fn print_pattern(&mut self) -> Result<(), Fatal> {
        let (pattern, terminated) = (&self.pattern, self.terminated);
        self.out.main().write_record(pattern, terminated).map_err(|e| write_error("stdout", &e))
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<(), Fatal> {
        self.out.main().write_raw(bytes).map_err(|e| write_error("stdout", &e))
    }

    fn flush_pending(&mut self) -> Result<(), Fatal> {
        if self.pending.is_empty() {
            return Ok(());
        }
        for item in std::mem::take(&mut self.pending) {
            match item {
                Pending::Text(text) => self.write_raw(&text)?,
                Pending::File(name) => self.write_file(&name)?,
            }
        }
        Ok(())
    }

    /// Writes the contents of the file an `r` command names, as it reads them (the file may
    /// never end, like `/dev/zero`).
    fn write_file(&mut self, name: &[u8]) -> Result<(), Fatal> {
        use std::io::Read;
        // Even a missing file ends an unterminated last line.
        self.write_raw(&[])?;
        let Ok(mut file) = std::fs::File::open(crate::io::bytes_path(name)) else { return Ok(()) };
        let mut buffer = vec![0u8; 1 << 16];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(n) => self.write_raw(&buffer[..n])?,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    let path = String::from_utf8_lossy(name).into_owned();
                    return Err(Fatal::new(format!("read error on {path}: {}", crate::io::strerror(&e)), 4));
                }
            }
        }
    }

    // ---- addresses -----------------------------------------------------------------

    fn is_last(&mut self) -> Result<bool, Fatal> {
        self.input.is_last(self.options.separate)
    }

    fn resolve(&mut self, pattern: &Pattern, from_address: bool) -> Result<Rc<Regex>, Fatal> {
        match &pattern.regex {
            Some(re) => {
                self.last_regex = Some(re.clone());
                self.last_regex_from_address = from_address;
                Ok(re.clone())
            }
            None => self
                .last_regex
                .clone()
                .ok_or_else(|| Fatal::new("-e expression #1, char 0: no previous regular expression", 1)),
        }
    }

    fn regex_matches(&mut self, pattern: &Pattern) -> Result<bool, Fatal> {
        let re = self.resolve(pattern, true)?;
        let matched = re.is_match(&self.pattern);
        match re.take_error() {
            Some(message) => Err(Fatal::new(message, 4)),
            None => Ok(matched),
        }
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
            Addr::Null => true,
        })
    }

    fn selected(&mut self, index: usize) -> Result<bool, Fatal> {
        let script = self.script;
        let command = &script.commands[index];
        let matched = match &command.selector {
            Selector::All => true,
            Selector::Single(a) => self.addr_matches(a)?,
            Selector::Range(a1, a2) => self.range_matches(index, a1, a2)?,
        };
        Ok(matched != command.negate)
    }

    /// What a range becomes once it has ended: a numeric start never matches again, while a
    /// regexp start can begin a new range.
    fn ended(a1: &Addr) -> RangeState {
        if matches!(a1, Addr::Line(_)) { RangeState::Closed } else { RangeState::Inactive }
    }

    fn range_matches(&mut self, index: usize, a1: &Addr, a2: &Addr2) -> Result<bool, Fatal> {
        let line = self.line;
        let mut state = self.ranges[index];
        if matches!(state, RangeState::Inactive | RangeState::Closed) {
            let starts = match a1 {
                // `0,/re/`: the range is already active before the first line.
                Addr::Line(0) => {
                    // It is active from the first time it is looked at, even if that is not line 1.
                    if !self.range_zero_done(index) {
                        state = RangeState::Active { until: None, overshoot: false };
                        true
                    } else {
                        false
                    }
                }
                // A numeric start also begins the range on a later line, if the line it names was
                // skipped (by `n`, `N`, or `D`), but only once.
                Addr::Line(n) => {
                    // ... unless the end line number has already been passed.
                    let passed_end = line > *n && matches!(a2, Addr2::Line(m) if line > *m);
                    if passed_end && line >= *n {
                        self.ranges[index] = RangeState::Closed;
                    }
                    !matches!(state, RangeState::Closed) && line >= *n && !passed_end
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
                            self.ranges[index] = Self::ended(a1);
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some(*n), overshoot: false };
                    }
                    Addr2::Plus(n) => {
                        if *n == 0 {
                            self.ranges[index] = Self::ended(a1);
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some(line + n), overshoot: true };
                    }
                    Addr2::Multiple(n) => {
                        // It ends at the next multiple of N after this line (a line past it, if
                        // that one was skipped, still belongs to the range); `,~0` is a single line.
                        if *n == 0 {
                            self.ranges[index] = Self::ended(a1);
                            return Ok(true);
                        }
                        state = RangeState::Active { until: Some((line / n + 1) * n), overshoot: true };
                    }
                    Addr2::Step { .. } => state = RangeState::Active { until: None, overshoot: false },
                    Addr2::Last => {
                        if self.is_last()? {
                            self.ranges[index] = Self::ended(a1);
                            return Ok(true);
                        }
                        state = RangeState::Active { until: None, overshoot: false };
                    }
                    Addr2::Regex(_) => state = RangeState::Active { until: None, overshoot: false },
                }
                self.ranges[index] = state;
                return Ok(true);
            }
        }
        // The range is active: does it end on this line?
        let RangeState::Active { until, overshoot } = state else { unreachable!() };
        let ends = match (until, a2) {
            (Some(n), _) => {
                if line > n && !overshoot {
                    // The end line was skipped: this line is already outside the range.
                    self.ranges[index] = Self::ended(a1);
                    return Ok(false);
                }
                line >= n
            }
            (None, Addr2::Last) => self.is_last()?,
            (None, Addr2::Regex(p)) => self.regex_matches(p)?,
            (None, Addr2::Step { first, step }) => {
                line >= *first && if *step == 0 { line == *first } else { (line - *first) % *step == 0 }
            }
            // Line-number ends always carry their line in `until`.
            (None, Addr2::Line(_) | Addr2::Plus(_) | Addr2::Multiple(_)) => unreachable!("an end line number is kept in `until`"),
        };
        self.ranges[index] = if ends { Self::ended(a1) } else { RangeState::Active { until, overshoot } };
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
            if self.options.debug && !matches!(command.kind, Kind::Version) {
                self.debug_command(pc, command)?;
            }
            if !self.selected(pc)? {
                if let Kind::BlockStart(end) = command.kind {
                    // Skip the block but still visit its `}`.
                    pc = end;
                } else {
                    pc += 1;
                }
                continue;
            }
            match &command.kind {
                Kind::BlockStart(_) | Kind::BlockEnd | Kind::Label(_) | Kind::Version => {}
                Kind::Removed(_) => return Err(Fatal::new("INTERNAL ERROR: Bad cmd L", 4)),
                Kind::Branch { when, target, .. } => {
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
                Kind::Insert(text) => {
                    let text = self.with_separator(text);
                    self.write_raw(&text)?;
                }
                Kind::Change(text) => {
                    let in_range = matches!(command.selector, Selector::Range(..))
                        && !command.negate
                        && matches!(self.ranges[pc], RangeState::Active { .. });
                    if !in_range {
                        let text = self.with_separator(text);
                        self.write_raw(&text)?;
                    }
                    return Ok(Flow::Discard);
                }
                Kind::Delete => return Ok(Flow::Discard),
                Kind::DeleteFirst => match crate::search::memchr(self.options.separator, &self.pattern) {
                    None => return Ok(Flow::DiscardQuietly),
                    Some(i) => {
                        self.pattern.drain(..=i);
                        return Ok(Flow::Restart);
                    }
                },
                Kind::Next => {
                    if self.is_last()? {
                        return self.no_next_line(true);
                    }
                    self.autoprint()?;
                    let more = self.read_line(false)?;
                    debug_assert!(more, "is_last said that a line follows");
                }
                Kind::NextAppend => {
                    if self.is_last()? {
                        return self.no_next_line(false);
                    }
                    let more = self.read_line(true)?;
                    debug_assert!(more, "is_last said that a line follows");
                }
                Kind::Print => self.print_pattern()?,
                Kind::PrintFirst => {
                    let (end, terminated) = match crate::search::memchr(self.options.separator, &self.pattern) {
                        Some(end) => (end, true),
                        None => (self.pattern.len(), self.terminated),
                    };
                    self.out.main().write_record(&self.pattern[..end], terminated).map_err(|e| write_error("stdout", &e))?;
                }
                Kind::Get => {
                    self.pattern = self.hold.clone();
                    self.terminated = self.hold_terminated;
                }
                Kind::GetAppend => {
                    self.pattern.push(self.options.separator);
                    self.pattern.extend_from_slice(&self.hold);
                    self.terminated = self.hold_terminated;
                }
                Kind::Hold => {
                    self.hold = self.pattern.clone();
                    self.hold_terminated = self.terminated;
                }
                Kind::HoldAppend => {
                    self.hold.push(self.options.separator);
                    self.hold.extend_from_slice(&self.pattern);
                    self.hold_terminated = self.terminated;
                }
                Kind::Exchange => {
                    std::mem::swap(&mut self.pattern, &mut self.hold);
                    std::mem::swap(&mut self.terminated, &mut self.hold_terminated);
                }
                Kind::Zap => self.pattern.clear(),
                Kind::LineNumber => {
                    let mut text = self.line.to_string().into_bytes();
                    text.push(self.options.separator);
                    self.write_raw(&text)?;
                }
                Kind::List(width) => {
                    let width = match *width {
                        None => self.options.line_length,
                        Some(w) => w.max(0) as usize,
                    };
                    let text = list::list(&self.pattern, width, self.options.separator);
                    self.write_raw(&text)?;
                }
                Kind::Quit(code) => return Ok(Flow::Quit { code: code.unwrap_or(0), print: true, finish_line: true, trace_end: false }),
                Kind::QuitSilent(code) => return Ok(Flow::QuitNow { code: code.unwrap_or(0) }),
                Kind::FileName => {
                    let mut text = self.file_name.as_bytes().to_vec();
                    text.push(self.options.separator);
                    self.write_raw(&text)?;
                }
                Kind::ReadFile(name) => self.pending.push(Pending::File(name.clone())),
                Kind::ReadFileNow(name) => self.write_file(name)?,
                Kind::ReadLine(name) => {
                    if let Some(line) = self.read_one_line(name)? {
                        self.pending.push(Pending::Text(line));
                    }
                }
                Kind::WriteFile(name) => {
                    let (pattern, terminated) = (self.pattern.clone(), self.terminated);
                    self.out.get(name).write_record(&pattern, terminated).map_err(|e| write_error("file", &e))?;
                }
                Kind::WriteFirstLine(name) => {
                    let (first, terminated) = self.first_line();
                    self.out.get(name).write_record(&first, terminated).map_err(|e| write_error("file", &e))?;
                }
                Kind::Execute(command) => {
                    self.execute_command(command)?;
                    if !command.is_empty() {
                        self.executed_e.insert(pc);
                    }
                }
                Kind::Translate(pairs) => self.translate(pairs),
                Kind::Substitute(sub) => self.substitute(sub)?,
            }
            if self.options.debug {
                self.debug_after(&command.kind)?;
            }
            pc += 1;
        }
        Ok(Flow::End)
    }

    /// The pattern space up to the first separator, and whether it ends with one: a
    /// pattern space without a separator keeps the missing-newline state of its line.
    fn first_line(&self) -> (Vec<u8>, bool) {
        match self.pattern.iter().position(|&b| b == self.options.separator) {
            Some(end) => (self.pattern[..end].to_vec(), true),
            None => (self.pattern.clone(), self.terminated),
        }
    }

    /// `n` or `N` with no next line. GNU sed prints the pattern space (POSIX mode does
    /// not) and stops; with `-s` the end of a file only ends the cycle, and sed goes on
    /// with the next file.
    fn no_next_line(&mut self, always_print: bool) -> Result<Flow, Fatal> {
        let print = always_print || !self.posixly_correct();
        // `n` shows the output before the end of the cycle in the trace, `N` after.
        if !always_print {
            self.debug_write(b"END-OF-CYCLE:\n")?;
        }
        if self.input.is_last(false)? {
            return Ok(Flow::Quit { code: 0, print, finish_line: false, trace_end: print && always_print });
        }
        if print {
            self.autoprint()?;
        }
        // For `N` the trace has ended the cycle already.
        Ok(if always_print { Flow::Discard } else { Flow::DiscardQuietly })
    }

    /// One line of the file named by an `R` command; the file stays open between calls.
    fn read_one_line(&mut self, name: &[u8]) -> Result<Option<Vec<u8>>, Fatal> {
        use std::io::BufRead;
        if name == b"/dev/stdin" {
            let record = crate::io::stdin_record(self.options.separator, self.options.unbuffered)
                .map_err(|e| Fatal::new(format!("read error on stdin: {}", crate::io::strerror(&e)), 4))?;
            return Ok(record.map(|r| {
                let mut line = r.body;
                if r.terminated {
                    line.push(self.options.separator);
                }
                line
            }));
        }
        let reader = self.read_files.entry(name.to_vec()).or_insert_with(|| {
            let path = String::from_utf8_lossy(name).into_owned();
            let _ = path;
            std::fs::File::open(crate::io::bytes_path(name)).ok().map(std::io::BufReader::new)
        });
        let Some(reader) = reader.as_mut() else { return Ok(None) };
        let mut line = Vec::new();
        match reader.read_until(self.options.separator, &mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(line)),
            Err(e) => {
                let path = String::from_utf8_lossy(name).into_owned();
                Err(Fatal::new(format!("read error on {path}: {}", crate::io::strerror(&e)), 4))
            }
        }
    }

    /// The text of `i` and `c` ends with the record separator (a NUL under `-z`).
    fn with_separator(&self, text: &[u8]) -> Vec<u8> {
        let mut out = text.to_vec();
        if out.last() == Some(&b'\n') {
            out.pop();
            out.push(self.options.separator);
        }
        out
    }

    fn translate(&mut self, pairs: &[(Vec<u8>, Vec<u8>)]) {
        let mut out = Vec::with_capacity(self.pattern.len());
        let mut i = 0;
        let locale = self.options.locale;
        let utf8 = locale.is_chars();
        while i < self.pattern.len() {
            let len = locale.char_len(&self.pattern[i..]);
            let ch = &self.pattern[i..i + len];
            // A single-byte locale keeps a table, so the last of two equal characters wins; a
            // multibyte locale searches the list, so the first does.
            // A byte that is not a character of its own (a lead byte without its continuation
            // bytes, or a stray one) is compared with the start of each character instead.
            let partial = utf8 && len == 1 && !locale.is_valid_char(ch);
            let found = if partial {
                pairs.iter().find(|(from, _)| from.starts_with(ch))
            } else if utf8 {
                pairs.iter().find(|(from, _)| from == ch)
            } else {
                pairs.iter().rev().find(|(from, _)| from == ch)
            };
            match found {
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
        if command.is_empty() {
            let mut output = run_shell(&self.pattern)?;
            if output.last() == Some(&b'\n') {
                output.pop();
            }
            self.pattern = output;
        } else {
            // The text of the command ends with a newline that the shell does not get.
            let output = run_shell(command.strip_suffix(b"\n").unwrap_or(command))?;
            self.write_raw(&output)?;
        }
        Ok(())
    }

    fn substitute(&mut self, sub: &Substitute) -> Result<(), Fatal> {
        let re = self.resolve(&sub.pattern, false)?;
        // GNU sed checks the group references of `s//.../` only against a regexp written in an address.
        if sub.pattern.regex.is_none() && self.last_regex_from_address {
            for part in &sub.replacement {
                if let ReplacementPart::Group(g) = part {
                    if *g > re.group_count() {
                        return Err(Fatal::new(
                            format!("-e expression #1, char 0: invalid reference \\{g} on `s' command's RHS"),
                            1,
                        ));
                    }
                }
            }
        }
        let locale = self.options.locale;
        // After an empty match the search goes on one character further (one byte in UTF-8,
        // where GNU sed's matcher starts inside characters too).
        let step = |text: &[u8], at: usize| if locale == crate::regex::Locale::Mb { locale.char_len(&text[at..]) } else { 1 };
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
                pos = start + step(&self.pattern, start);
                previous_end = None;
                continue;
            }
            if count == 0 && self.options.debug {
                self.debug_registers = Some(registers_text(&self.pattern, &caps));
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
                pos = start + step(&self.pattern, start);
            } else {
                pos = end;
            }
        }
        // (A search the library gave up on said "no match": that is an error, not the end of the line.)
        if let Some(message) = re.take_error() {
            return Err(Fatal::new(message, 4));
        }
        if !replaced {
            return Ok(());
        }
        result.extend_from_slice(&self.pattern[copied..]);
        self.pattern = result;
        self.substituted = true;
        self.debug_flush_registers()?;
        if sub.print_before {
            self.print_pattern()?;
        }
        if sub.eval {
            self.execute_command(&[])?;
        }
        if sub.print_after {
            self.print_pattern()?;
        }
        if let Some(name) = &sub.write {
            let (pattern, terminated) = (self.pattern.clone(), self.terminated);
            self.out.get(name).write_record(&pattern, terminated).map_err(|e| write_error("file", &e))?;
        }
        Ok(())
    }
}

/// `  regex[N] = START-END 'text'` for every group of the first match.
fn registers_text(subject: &[u8], caps: &Captures) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, group) in caps.groups.iter().enumerate() {
        if let Some((s, e)) = group {
            out.extend_from_slice(format!("  regex[{i}] = {s}-{e} '").as_bytes());
            out.extend_from_slice(&subject[*s..*e]);
            out.extend_from_slice(b"'\n");
        }
    }
    out
}

/// Runs `command` with the shell and returns what it wrote. A command that cannot be started
/// (no descriptors left for the pipe, for instance) ends the run.
fn run_shell(command: &[u8]) -> Result<Vec<u8>, Fatal> {
    use std::os::unix::ffi::OsStrExt;
    let output = Process::new("sh")
        .arg("-c")
        // As the C library starts it, so that a command that begins with `+` or `-` is not an option.
        .arg("--")
        .arg(std::ffi::OsStr::from_bytes(command))
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output();
    output.map(|o| o.stdout).map_err(|_| Fatal::new("error in subprocess", 4))
}

/// Length in bytes of the character at the start of `bytes` (UTF-8), or 1 for an invalid sequence.
pub(crate) fn char_len(bytes: &[u8]) -> usize {
    let want = match bytes.first() {
        Some(0xC2..=0xDF) => 2,
        Some(0xE0..=0xEF) => 3,
        Some(0xF0..=0xF4) => 4,
        _ => return 1,
    };
    if bytes.len() >= want && std::str::from_utf8(&bytes[..want]).is_ok() { want } else { 1 }
}

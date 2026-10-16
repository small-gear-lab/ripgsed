//! Searching one input and writing what is selected, with its context.

use std::io::{self, Read, Write};

use super::color::Colors;
use super::matcher::Matcher;
use super::options::{BinaryFiles, ListFiles, Options};
use crate::regex::Locale;

const SEP_SELECTED: u8 = b':';
const SEP_CONTEXT: u8 = b'-';

/// Marks an error that came from writing the output.
#[derive(Debug)]
pub struct Writing(pub io::Error);

impl std::fmt::Display for Writing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Writing {}

impl Writing {
    fn wrap(e: io::Error) -> io::Error {
        io::Error::new(e.kind(), Writing(e))
    }
}

/// A literal looked for in a whole input.
enum Needle {
    Exact(crate::search::Finder),
    Fold(crate::search::FoldFinder),
}

impl Needle {
    fn find(&self, hay: &[u8]) -> Option<usize> {
        match self {
            Needle::Exact(f) => f.find(hay),
            Needle::Fold(f) => f.find(hay),
        }
    }
}

/// What searching one input found.
pub struct Found {
    /// Selected lines (stopping at `-m`, and at the first one with `-l`, `-L`, `-q`).
    pub count: u64,
    /// The input is binary and something was selected in it: the message is due.
    pub binary_matches: bool,
    /// Where the last selected line ends in the input (a seekable standard input is left there).
    pub last_selected_end: Option<u64>,
    /// The search stopped before the end of the input (`-m`, `-l`, `-q`, a binary file).
    pub ended_early: bool,
    /// ... because `-m` was reached.
    pub max_reached: bool,
}

/// What searching one input has found out so far.
struct State {
    count: u64,
    line_no: u64,
    /// The offset of the first line that is not done.
    next_offset: u64,
    /// The offset after the last line printed.
    last_out: Option<u64>,
    pending: u64,
    /// `-m` has been reached: only trailing context is left.
    stopped: bool,
    done: bool,
    /// Lines are not printed (`-c`, `-l`, `-L`, `-q`, or a binary input).
    quiet: bool,
    originally_quiet: bool,
    count_only: bool,
    done_on_match: bool,
    /// The offset from which the input is binary (lines that end there or later are).
    binary_from: Option<u64>,
    in_binary: bool,
    count_at_binary: Option<u64>,
    last_selected_end: Option<u64>,
    needles: Option<Vec<Needle>>,
}

impl State {
    fn new(o: &Options, exit_on_match: bool) -> State {
        let list = o.list_files != ListFiles::No;
        let count_only = o.count && !(o.quiet || list);
        let quiet = count_only || o.quiet || list;
        State {
            count: 0,
            line_no: 0,
            next_offset: 0,
            last_out: None,
            pending: 0,
            stopped: false,
            done: false,
            quiet,
            // (With -m, the message is not held back by -l and -L.)
            originally_quiet: count_only || o.quiet || (list && o.max_count.is_none()),
            count_only,
            // (With -m, -l and -L do not stop at the first selected line, as in GNU grep: the
            // whole input is read, so that a binary part after it is noticed.)
            done_on_match: exit_on_match || (list && o.max_count.is_none()),
            binary_from: None,
            in_binary: false,
            count_at_binary: None,
            last_selected_end: None,
            needles: None,
        }
    }

    /// Where the part of `buf` that is still needed starts: the line not finished, and (for
    /// leading context) the `before` lines before the first line not done, after the last
    /// line printed.
    fn keep_from(&self, buf: &[u8], base: u64, eol: u8, before: u64) -> usize {
        let next = (self.next_offset - base) as usize;
        let mut from = next;
        let lower = self.last_out.map_or(0, |l| l.saturating_sub(base) as usize);
        let mut k = 0;
        while k < before && from > lower {
            from = buf[lower..from - 1].iter().rposition(|&c| c == eol).map_or(lower, |i| lower + i + 1);
            k += 1;
        }
        from
    }
}

/// Standard output, the state shared by all inputs (the separator between groups is printed
/// only once something has been printed), and the settings of the output.
pub struct Printer<'a> {
    pub options: &'a Options,
    pub colors: Colors,
    pub locale: Locale,
    out: Vec<u8>,
    sink: Box<dyn Write + 'a>,
    /// Something was printed already: a group separator may be needed.
    used: bool,
    eol: u8,
    /// Print the name of the file in front of every line.
    pub with_filename: bool,
    /// `-r` with nothing but one file: the names are printed for the files found in directories.
    pub filenames_in_directories: bool,
    /// What `with_filename` is for the operands (before a file found in a directory changes it).
    pub default_with_filename: bool,
    /// `-T`: the width of line numbers and byte offsets.
    pub offset_width: usize,
    pub exit_on_match: bool,
    /// A line was not printed because its text is not valid in the encoding.
    encoding_errors: bool,
}

/// How the lines of one input are shown.
struct Mode {
    /// Lines are not printed (`-c`, `-l`, `-L`, `-q`, or a binary file).
    quiet: bool,
    /// Stop at the first selected line.
    done_on_match: bool,
}

impl<'a> Printer<'a> {
    pub fn new(options: &'a Options, colors: Colors, locale: Locale, sink: Box<dyn Write + 'a>) -> Printer<'a> {
        Printer {
            options,
            colors,
            locale,
            out: Vec::with_capacity(1 << 16),
            sink,
            used: false,
            eol: if options.null_data { 0 } else { b'\n' },
            with_filename: false,
            filenames_in_directories: false,
            default_with_filename: false,
            offset_width: 0,
            // (-m holds back the early exit of -q as well.)
            exit_on_match: options.quiet && options.max_count.is_none(),
            encoding_errors: false,
        }
    }

    /// Whether an error came from reading the input (and not from writing the output, which
    /// is wrapped in `Writing`).
    pub fn is_read_error(&self, e: &io::Error) -> bool {
        e.get_ref().is_none_or(|inner| !inner.is::<Writing>())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if !self.out.is_empty() {
            self.sink.write_all(&self.out).map_err(Writing::wrap)?;
            self.out.clear();
        }
        self.sink.flush().map_err(Writing::wrap)
    }

    fn maybe_flush(&mut self) -> io::Result<()> {
        if self.out.len() >= (1 << 16) {
            self.sink.write_all(&self.out).map_err(Writing::wrap)?;
            self.out.clear();
        }
        Ok(())
    }

    // ---- colors ---------------------------------------------------------------------

    fn sgr_start(&mut self, color: &str) {
        if !color.is_empty() {
            self.out.extend_from_slice(b"\x1b[");
            self.out.extend_from_slice(color.as_bytes());
            self.out.extend_from_slice(if self.colors.no_erase { b"m" } else { b"m\x1b[K" });
        }
    }

    fn sgr_end(&mut self, color: &str) {
        if !color.is_empty() {
            self.out.extend_from_slice(if self.colors.no_erase { b"\x1b[m" } else { b"\x1b[m\x1b[K" });
        }
    }

    fn colored(&self) -> bool {
        self.options.color
    }

    // ---- pieces of a line --------------------------------------------------------------

    fn print_filename(&mut self, name: &[u8]) {
        if self.colored() {
            let c = self.colors.filename.clone();
            self.sgr_start(&c);
            self.out.extend_from_slice(name);
            self.sgr_end(&c);
        } else {
            self.out.extend_from_slice(name);
        }
    }

    fn print_sep(&mut self, sep: u8) {
        if self.colored() {
            let c = self.colors.separator.clone();
            self.sgr_start(&c);
            self.out.push(sep);
            self.sgr_end(&c);
        } else {
            self.out.push(sep);
        }
    }

    fn print_number(&mut self, n: u64, width: usize, color: String) {
        if self.colored() {
            self.sgr_start(&color);
            self.out.extend_from_slice(format!("{n:>width$}").as_bytes());
            self.sgr_end(&color);
        } else {
            self.out.extend_from_slice(format!("{n:>width$}").as_bytes());
        }
    }

    /// The start of an output line: name, line number, byte offset, tab. `false` when the text has
    /// an encoding error, which makes the input binary (nothing more is printed from it).
    #[allow(clippy::too_many_arguments)]
    fn print_head(&mut self, name: &[u8], line_no: u64, offset: u64, sep: u8, text_len: usize, bad: bool) -> bool {
        if bad {
            return false;
        }
        let o = self.options;
        let mut any = false;
        if self.with_filename {
            self.print_filename(name);
            if o.null {
                self.out.push(0);
            } else {
                self.print_sep(sep);
            }
            any = true;
        }
        if o.line_number {
            let width = self.offset_width;
            let color = self.colors.line_number.clone();
            self.print_number(line_no, width, color);
            self.print_sep(sep);
            any = true;
        }
        if o.byte_offset {
            let width = self.offset_width;
            let color = self.colors.byte_offset.clone();
            self.print_number(offset, width, color);
            self.print_sep(sep);
            any = true;
        }
        if o.initial_tab && any && text_len > 0 {
            self.out.push(b'\t');
        }
        true
    }

    fn has_encoding_error(&self, text: &[u8]) -> bool {
        if self.options.binary_files == BinaryFiles::Text {
            return false;
        }
        match self.locale {
            Locale::Utf8 | Locale::Libc => std::str::from_utf8(text).is_err(),
            _ => false,
        }
    }

    // ---- printing a line ---------------------------------------------------------------

    /// Prints one line (`text` without its terminator) as selected (`:`) or as context (`-`).
    /// Returns `false` when the line cannot be printed (an encoding error).
    #[allow(clippy::too_many_arguments)]
    fn print_line(&mut self, m: &Matcher, name: &[u8], text: &[u8], line_no: u64, offset: u64, sep: u8, terminated: bool) -> io::Result<bool> {
        let o = self.options;
        let bad = self.has_encoding_error(text);
        if !o.only_matching && !self.print_head(name, line_no, offset, sep, text.len(), bad) {
            self.encoding_errors = true;
            return Ok(false);
        }
        let matching = (sep == SEP_SELECTED) ^ o.invert;
        let mut line_color = String::new();
        if self.colored() {
            let selected = (sep == SEP_SELECTED) ^ (o.invert && self.colors.reverse);
            line_color = if selected { self.colors.selected_line.clone() } else { self.colors.context_line.clone() };
        }
        let match_color = if self.colored() {
            if sep == SEP_SELECTED { self.colors.selected_match.clone() } else { self.colors.context_match.clone() }
        } else {
            String::new()
        };
        let mut beg = 0usize;
        if (o.only_matching && matching) || (self.colored() && (!line_color.is_empty() || !match_color.is_empty())) {
            if matching && (o.only_matching || !match_color.is_empty()) {
                match self.print_middle(m, name, text, line_no, offset, &line_color, &match_color, bad)? {
                    Some(b) => beg = b,
                    None => return Ok(false),
                }
            }
            if !o.only_matching && !line_color.is_empty() {
                beg = self.print_tail(text, beg, &line_color);
            }
        }
        if !o.only_matching {
            self.out.extend_from_slice(&text[beg..]);
            if terminated || true {
                self.out.push(self.eol);
            }
        }
        if o.line_buffered {
            self.flush()?;
        } else {
            self.maybe_flush()?;
        }
        Ok(true)
    }

    /// The text of a line up to its last match, the matches in color (or alone, with `-o`).
    #[allow(clippy::too_many_arguments)]
    fn print_middle(
        &mut self,
        m: &Matcher,
        name: &[u8],
        text: &[u8],
        line_no: u64,
        offset: u64,
        line_color: &str,
        match_color: &str,
        bad: bool,
    ) -> io::Result<Option<usize>> {
        let o = self.options;
        let mut cur = 0usize;
        let mut mid: Option<usize> = None;
        // (GNU grep's line includes its terminator: the search goes on while `cur` is before it.)
        while cur <= text.len() {
            let Some((s, e)) = m.find(text, cur) else { break };
            let b = s;
            let mut size = e - s;
            if size == 0 {
                // Make minimal progress; there may be further non-empty matches.
                size = 1;
                if mid.is_none() {
                    mid = Some(cur);
                }
            } else {
                if o.only_matching {
                    let sep = if o.invert { SEP_CONTEXT } else { SEP_SELECTED };
                    // (GNU grep looks at the match, not at the line, for an encoding error.)
                    let bad_match = self.has_encoding_error(&text[b..e.min(text.len())]);
                    let _ = bad;
                    if !self.print_head(name, line_no, offset + b as u64, sep, size, bad_match) {
                        self.encoding_errors = true;
                        return Ok(None);
                    }
                } else {
                    self.sgr_start(line_color);
                    if let Some(m0) = mid.take() {
                        cur = m0;
                    }
                    self.out.extend_from_slice(&text[cur..b]);
                }
                if !match_color.is_empty() {
                    self.sgr_start(match_color);
                }
                // (A match can end one byte after the text: its terminator, see `Matcher::find`.)
                self.out.extend_from_slice(&text[b..e.min(text.len())]);
                if e > text.len() {
                    self.out.push(self.eol);
                }
                if !match_color.is_empty() {
                    self.sgr_end(match_color);
                }
                if o.only_matching {
                    self.out.push(self.eol);
                }
            }
            cur = b + size;
        }
        if o.only_matching {
            cur = text.len();
        } else if let Some(m0) = mid {
            cur = m0;
        }
        Ok(Some(cur.min(text.len())))
    }

    /// The rest of a line in the color of the line, without its trailing carriage return.
    fn print_tail(&mut self, text: &[u8], beg: usize, color: &str) -> usize {
        let eol_size = usize::from(text.len() > beg && text[text.len() - 1] == b'\r');
        let tail_size = text.len() - eol_size - beg;
        if tail_size > 0 {
            self.sgr_start(color);
            self.out.extend_from_slice(&text[beg..beg + tail_size]);
            self.sgr_end(color);
            return beg + tail_size;
        }
        beg
    }

    // ---- one input ---------------------------------------------------------------------

    /// Searches an input, reading it as it goes, and prints what the options ask for.
    ///
    /// GNU grep reads an input in buffers and decides per buffer whether it is binary (it has a
    /// NUL byte); its output follows from that. A regular file is read in pieces of 96 KiB at
    /// fixed places, so the piece with the first NUL is known from its offset; for a pipe the
    /// pieces are the reads. The lines that end in that piece or later are not printed.
    pub fn search_stream(&mut self, m: &Matcher, name: &[u8], input: &mut dyn Read, regular: bool) -> io::Result<Found> {
        const PIECE: u64 = 98304;
        const READ: usize = 1 << 18;
        let o = self.options;
        let eol = self.eol;
        let mut st = State::new(o, self.exit_on_match);
        if o.max_count == Some(0) {
            return Ok(Found { count: 0, binary_matches: false, last_selected_end: None, ended_early: false, max_reached: false });
        }
        if !o.invert {
            st.needles = m.needles().map(|n| {
                n.into_iter()
                    .map(|(bytes, fold)| if fold { Needle::Fold(crate::search::FoldFinder::new(&bytes)) } else { Needle::Exact(crate::search::Finder::new(&bytes)) })
                    .collect()
            });
        }
        let mut buf: Vec<u8> = Vec::with_capacity(READ + 4096);
        let mut base: u64 = 0;
        self.encoding_errors = false;
        let check_nul = o.binary_files != BinaryFiles::Text && eol == b'\n';
        loop {
            let old_len = buf.len();
            buf.resize(old_len + READ, 0);
            let n = loop {
                match input.read(&mut buf[old_len..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        buf.truncate(old_len);
                        return Err(e);
                    }
                }
            };
            buf.truncate(old_len + n);
            let eof = n == 0;
            if check_nul && st.binary_from.is_none() && n > 0 {
                if let Some(i) = crate::search::memchr(0, &buf[old_len..]) {
                    let at = base + (old_len + i) as u64;
                    st.binary_from = Some(if regular { at / PIECE * PIECE } else { base + old_len as u64 });
                }
            }
            self.process(m, name, &mut st, &buf, base, eof)?;
            if st.done {
                break;
            }
            // What is kept: the line that is not finished, and the lines before it that a
            // match may want as leading context.
            let keep_from = st.keep_from(&buf, base, eol, o.before.unwrap_or(0));
            if keep_from > 0 {
                buf.drain(..keep_from);
                base += keep_from as u64;
            }
            if eof {
                break;
            }
        }
        let encoding_errors = std::mem::take(&mut self.encoding_errors) && o.binary_files == BinaryFiles::Binary;
        let binary_matches = !st.originally_quiet
            && (encoding_errors || st.count_at_binary.is_some_and(|c| st.count > c));
        Ok(Found { count: st.count, binary_matches, last_selected_end: st.last_selected_end, ended_early: st.done || st.stopped, max_reached: st.stopped })
    }

    /// Prints trailing context, as GNU grep's `prpending` does: the lines from the one after the
    /// last line printed (or the start of the buffer) up to `upto`, while context is left. A line
    /// that cannot be printed is tried again for each line of context that is left. `pos` is where
    /// the line with number `st.line_no` starts.
    #[allow(clippy::too_many_arguments)]
    fn flush_pending(&mut self, m: &Matcher, name: &[u8], st: &mut State, buf: &[u8], base: u64, pos: usize, upto: usize) -> io::Result<()> {
        let eol = self.eol;
        let mut cur = st.last_out.map_or(0, |l| l.saturating_sub(base) as usize);
        while st.pending > 0 && cur < upto {
            let e = crate::search::memchr(eol, &buf[cur..]).map_or(buf.len(), |i| cur + i);
            let n = st.line_no.wrapping_sub(crate::search::count(eol, &buf[cur.min(pos)..pos]) as u64);
            let terminated = e < buf.len();
            if self.print_line(m, name, &buf[cur..e], n, base + cur as u64, SEP_CONTEXT, terminated)? {
                cur = (e + 1).min(buf.len());
                st.last_out = Some(base + cur as u64);
            } else if st.last_out.is_none() {
                st.last_out = Some(base);
            }
            st.pending -= 1;
        }
        Ok(())
    }

    /// Processes the complete lines of `buf` (all of them, at the end of the input) that are not
    /// done yet; `base` is the offset of `buf[0]` in the input.
    fn process(&mut self, m: &Matcher, name: &[u8], st: &mut State, buf: &[u8], base: u64, eof: bool) -> io::Result<()> {
        let o = self.options;
        let eol = self.eol;
        let before = o.before;
        let after = o.after;
        let context = before.is_some() || after.is_some();
        // (-m does not stop a search that only lists.)
        let max = if o.list_files != ListFiles::No { None } else { o.max_count };
        let finders_owned: Option<Vec<Needle>> = st.needles.take();
        let finders = &finders_owned;
        let mut hits: Vec<Option<usize>> = vec![None; finders.as_ref().map_or(0, Vec::len)];
        // `pos` is the start of the next line to look at, relative to `buf`.
        let mut pos = (st.next_offset - base) as usize;
        loop {
            if pos >= buf.len() {
                break;
            }
            // Nothing to print before the next line that has one of the literals: go there.
            if let (Some(finders), 0, false) = (finders, st.pending, st.stopped) {
                let mut next = usize::MAX;
                for (i, f) in finders.iter().enumerate() {
                    let hit = match hits[i] {
                        Some(h) if h >= pos => h,
                        _ => {
                            let h = f.find(&buf[pos..]).map_or(usize::MAX, |r| pos + r);
                            hits[i] = Some(h);
                            h
                        }
                    };
                    next = next.min(hit);
                }
                if next == usize::MAX {
                    // No line in the rest of this buffer is selected: all but the line that
                    // is not finished are done.
                    let end = crate::search::memrchr(eol, &buf[pos..]).map_or(pos, |i| pos + i + 1);
                    if o.line_number {
                        st.line_no += crate::search::count(eol, &buf[pos..end]) as u64;
                    }
                    pos = end;
                    if eof && pos < buf.len() {
                        // The last line has no terminator, and no literal.
                        st.line_no += 1;
                        pos = buf.len();
                    }
                    break;
                }
                let line_start = crate::search::memrchr(eol, &buf[pos..next]).map_or(pos, |i| pos + i + 1);
                if o.line_number {
                    st.line_no += crate::search::count(eol, &buf[pos..line_start]) as u64;
                }
                pos = line_start;
            }
            // The next line: up to `eol`, or to a NUL in the part of a binary input where NUL ends lines.
            let rest = &buf[pos..];
            let line_offset = base + pos as u64;
            let mut end_rel = crate::search::memchr(eol, rest);
            if let Some(bf) = st.binary_from {
                // (A line that ends where the binary part is, or later, ends at a NUL too.)
                let eol_at = end_rel.map(|i| line_offset + i as u64);
                if eol_at.is_none_or(|e| e >= bf) && eol != 0 {
                    let limit = end_rel.unwrap_or(rest.len());
                    if let Some(z) = crate::search::memchr(0, &rest[..limit]) {
                        end_rel = Some(z);
                    }
                }
            }
            let (text_end, next, terminated) = match end_rel {
                Some(i) => (pos + i, pos + i + 1, true),
                None if eof => (buf.len(), buf.len(), false),
                None => break, // not finished: wait for the rest of it
            };
            let eol_offset = base + text_end as u64;
            // From the first line that is part of the binary piece on, nothing is printed.
            if !st.in_binary && st.binary_from.is_some_and(|bf| eol_offset >= bf) {
                if o.binary_files == BinaryFiles::WithoutMatch {
                    // A binary input has no match: what was printed before stays, the count is 0.
                    st.count = 0;
                    st.done = true;
                    break;
                }
                st.in_binary = true;
                st.count_at_binary = Some(st.count);
                if !st.count_only {
                    st.quiet = true;
                    st.done_on_match = true;
                }
            }
            let text = &buf[pos..text_end];
            st.line_no += 1;
            let selected = m.is_match(text) != o.invert;

            if st.stopped {
                // After `-m`: only the trailing context is left (selected lines included).
                if st.pending == 0 {
                    st.done = true;
                    break;
                }
                if !st.quiet {
                    self.print_line(m, name, text, st.line_no, line_offset, SEP_CONTEXT, terminated)?;
                    st.pending -= 1;
                    st.last_out = Some(base + next as u64);
                }
                pos = next;
                continue;
            }

            if selected {
                st.count += 1;
                st.last_selected_end = Some(base + next as u64);
                if !st.quiet {
                    // Trailing context that was not printed yet (a line that could not be printed
                    // leaves some).
                    self.flush_pending(m, name, st, buf, base, pos, pos)?;
                    // Leading context, back from this line to the last line printed.
                    let lower = st.last_out.map_or(0, |l| (l.saturating_sub(base)) as usize);
                    let mut first = pos;
                    let mut first_no = st.line_no;
                    if let Some(b) = before {
                        let mut k = 0;
                        while k < b && first > lower {
                            let prev_end = first - 1;
                            let prev_start = buf[lower..prev_end].iter().rposition(|&c| c == eol).map_or(lower, |i| lower + i + 1);
                            first = prev_start;
                            // (The line numbers are kept only for -n; they wrap otherwise.)
                            first_no = first_no.wrapping_sub(1);
                            k += 1;
                        }
                    }
                    let adjacent = st.last_out == Some(base + first as u64);
                    if context && self.used && !adjacent {
                        if let Some(Some(sep)) = Some(o.group_separator.clone().unwrap_or(Some(b"--".to_vec()))) {
                            if self.colored() {
                                let c = self.colors.separator.clone();
                                self.sgr_start(&c);
                                self.out.extend_from_slice(&sep);
                                self.sgr_end(&c);
                            } else {
                                self.out.extend_from_slice(&sep);
                            }
                            self.out.push(b'\n');
                        }
                    }
                    // The lines between `first` and this one are context.
                    let mut p = first;
                    let mut n = first_no;
                    while p < pos {
                        let e = crate::search::memchr(eol, &buf[p..]).map_or(buf.len(), |i| p + i);
                        self.print_line(m, name, &buf[p..e], n, base + p as u64, SEP_CONTEXT, true)?;
                        p = e + 1;
                        n = n.wrapping_add(1);
                    }
                    if self.print_line(m, name, text, st.line_no, line_offset, SEP_SELECTED, terminated)? {
                        st.last_out = Some(base + next as u64);
                    }
                    // (As in GNU grep, a line that could not be printed starts its trailing context
                    // too, and counts as output for the group separator.)
                    st.pending = after.unwrap_or(0);
                    self.used = true;
                } else {
                    // (A group separator is due once anything was selected, printed or not.)
                    self.used = true;
                }
                if self.exit_on_match {
                    st.done = true;
                    pos = next;
                    break;
                }
                if st.done_on_match {
                    st.done = true;
                    pos = next;
                    break;
                }
                if let Some(limit) = max {
                    if st.count >= limit {
                        st.stopped = true;
                    }
                }
            } else if st.pending > 0 && !st.quiet {
                self.flush_pending(m, name, st, buf, base, pos, next)?;
            }
            pos = next;
        }
        st.next_offset = base + pos as u64;
        st.needles = finders_owned;
        Ok(())
    }

    /// The summary line of an input: its count, or its name.
    pub fn summary(&mut self, name: &[u8], found: &Found) -> io::Result<()> {
        let o = self.options;
        let list = o.list_files != ListFiles::No;
        if o.count && !(o.quiet || list) {
            if self.with_filename {
                self.print_filename(name);
                if o.null {
                    self.out.push(0);
                } else {
                    self.print_sep(SEP_SELECTED);
                }
            }
            self.out.extend_from_slice(format!("{}\n", found.count).as_bytes());
        }
        let status_nonmatch = found.count == 0;
        let wanted = match o.list_files {
            ListFiles::Matching => !status_nonmatch,
            ListFiles::NonMatching => status_nonmatch,
            ListFiles::No => false,
        };
        if wanted {
            self.print_filename(name);
            self.out.push(if o.null { 0 } else { b'\n' });
        }
        if o.line_buffered {
            self.flush()?;
        } else {
            self.maybe_flush()?;
        }
        Ok(())
    }
}

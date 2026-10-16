//! Searching one input and writing what is selected, with its context.

use std::io::{self, Write};

use super::color::Colors;
use super::matcher::Matcher;
use super::options::{BinaryFiles, ListFiles, Options};
use crate::regex::Locale;

const SEP_SELECTED: u8 = b':';
const SEP_CONTEXT: u8 = b'-';

/// What searching one input found.
pub struct Found {
    /// Selected lines (stopping at `-m`, and at the first one with `-l`, `-L`, `-q`).
    pub count: u64,
    /// The input is binary and something was selected in it: the message is due.
    pub binary_matches: bool,
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
            exit_on_match: options.quiet,
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if !self.out.is_empty() {
            self.sink.write_all(&self.out)?;
            self.out.clear();
        }
        self.sink.flush()
    }

    fn maybe_flush(&mut self) -> io::Result<()> {
        if self.out.len() >= (1 << 16) {
            self.sink.write_all(&self.out)?;
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
        while cur < text.len() {
            let Some((s, e)) = m.find(text, cur) else { break };
            let b = s;
            let mut size = e - s;
            if b == text.len() {
                break;
            }
            if size == 0 {
                // Make minimal progress; there may be further non-empty matches.
                size = 1;
                if mid.is_none() {
                    mid = Some(cur);
                }
            } else {
                if o.only_matching {
                    let sep = if o.invert { SEP_CONTEXT } else { SEP_SELECTED };
                    if !self.print_head(name, line_no, offset + b as u64, sep, size, bad) {
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
                self.out.extend_from_slice(&text[b..e]);
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

    /// Searches `data` (the whole input) and prints what the options ask for.
    pub fn search(&mut self, m: &Matcher, name: &[u8], data: &[u8]) -> io::Result<Found> {
        let o = self.options;
        let eol = self.eol;
        let mut has_nul = false;
        if o.binary_files != BinaryFiles::Text && eol == b'\n' {
            has_nul = data.contains(&0);
        }
        if has_nul && o.binary_files == BinaryFiles::WithoutMatch {
            return Ok(Found { count: 0, binary_matches: false });
        }
        let list = o.list_files != ListFiles::No;
        let count_only = o.count && !(self.exit_on_match || list);
        let mut mode = Mode {
            quiet: count_only || self.exit_on_match || list,
            done_on_match: self.exit_on_match || list,
        };
        let originally_quiet = mode.quiet;
        if has_nul && !count_only {
            mode.quiet = true;
            mode.done_on_match = true;
        }
        let mut first_binary_count: Option<u64> = if has_nul { Some(0) } else { None };

        let before = o.before;
        let after = o.after;
        let context = before.is_some() || after.is_some();
        let max = o.max_count;
        let mut count = 0u64;
        let mut line_no = 0u64;
        let mut pos = 0usize;
        // The end (after the terminator) of the last line printed, if any.
        let mut last_out: Option<usize> = None;
        let mut pending = 0u64;
        let mut stopped = false;
        let mut encoding_error_output = false;

        while pos < data.len() {
            // The next line: up to `eol` (or to a NUL in a binary input, which splits lines).
            let rest = &data[pos..];
            let end_rel = if has_nul && eol != 0 {
                rest.iter().position(|&b| b == eol || b == 0)
            } else {
                crate::search::memchr(eol, rest)
            };
            let (text_end, next, terminated) = match end_rel {
                Some(i) => (pos + i, pos + i + 1, true),
                None => (data.len(), data.len(), false),
            };
            let text = &data[pos..text_end];
            line_no += 1;
            let selected = m.is_match(text) != o.invert;

            if stopped {
                // After `-m`: only the trailing context is left, and it ends at a selected line.
                if pending == 0 || selected {
                    break;
                }
                if !mode.quiet {
                    self.print_line(m, name, text, line_no, pos as u64, SEP_CONTEXT, terminated)?;
                    pending -= 1;
                    last_out = Some(next);
                }
                pos = next;
                continue;
            }

            if selected {
                count += 1;
                if first_binary_count.is_some() && first_binary_count == Some(0) && !count_only {
                    // (the count before the first NUL is 0 here: the whole input is one buffer)
                }
                if !mode.quiet {
                    // Leading context, back from this line to the last line printed.
                    let lower = last_out.unwrap_or(0);
                    let mut first = pos;
                    let mut first_no = line_no;
                    if let Some(b) = before {
                        let mut k = 0;
                        while k < b && first > lower {
                            let prev_end = first - 1;
                            let prev_start = data[lower..prev_end].iter().rposition(|&c| c == eol).map_or(lower, |i| lower + i + 1);
                            first = prev_start;
                            first_no -= 1;
                            k += 1;
                        }
                    }
                    let adjacent = last_out == Some(first);
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
                        let e = crate::search::memchr(eol, &data[p..]).map_or(data.len(), |i| p + i);
                        self.print_line(m, name, &data[p..e], n, p as u64, SEP_CONTEXT, true)?;
                        p = e + 1;
                        n += 1;
                    }
                    if !self.print_line(m, name, text, line_no, pos as u64, SEP_SELECTED, terminated)? {
                        encoding_error_output = true;
                        mode.quiet = true;
                        mode.done_on_match = true;
                    } else {
                        last_out = Some(next);
                        pending = after.unwrap_or(0);
                    }
                    self.used = true;
                }
                if self.exit_on_match {
                    return Ok(Found { count, binary_matches: false });
                }
                if mode.done_on_match {
                    break;
                }
                if let Some(limit) = max {
                    if count >= limit {
                        stopped = true;
                    }
                }
            } else if pending > 0 && !mode.quiet {
                self.print_line(m, name, text, line_no, pos as u64, SEP_CONTEXT, terminated)?;
                pending -= 1;
                last_out = Some(next);
            }
            pos = next;
        }

        let binary_matches = !originally_quiet && (encoding_error_output || (has_nul && count > 0));
        Ok(Found { count, binary_matches })
    }

    /// The summary line of an input: its count, or its name.
    pub fn summary(&mut self, name: &[u8], found: &Found) -> io::Result<()> {
        let o = self.options;
        let list = o.list_files != ListFiles::No;
        if o.count && !(self.exit_on_match || list) {
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

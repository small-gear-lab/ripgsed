//! The patterns of a grep run: what is a match, and where it is.

use super::options::{Kind, Options};
use crate::pcre::{self, Pcre};
use crate::regex::{Captures, Locale, Regex, Syntax, Wrap};

/// A pattern that does not compile: the message of GNU grep.
pub struct PatternError(pub String);

pub struct Matcher {
    /// What selects the lines: one regular expression for each pattern, with `-x` and `-w` built
    /// in. Empty: nothing matches (`-f /dev/null`).
    select: Vec<Regex>,
    /// With `-w` or `-x`: the patterns as they are. GNU grep finds the matches of `-o` and
    /// `--color` with these (and tests the words itself).
    plain: Vec<Regex>,
    word: bool,
    /// Both `-w` and `-x`: GNU grep then takes a match that reaches the end of the line for the
    /// rest of the line, its newline included (see `find_line_end`).
    both: bool,
    /// `-P`: the one pattern, in the PCRE2 library.
    perl: Option<Pcre>,
    perl_enabled: bool,
    /// `-P`: the first match the library gave up on since the last `take_error`.
    perl_error: std::cell::RefCell<Option<String>>,
    /// The text is read as characters: the class of non-word characters that `-w` adds to a
    /// pattern is one GNU grep's DFA cannot decide, so the C library's matcher selects the lines.
    multibyte: bool,
    /// UTF-8 (the characters are read here); `libc` when the C library knows them.
    utf8: bool,
    libc: bool,
    /// What GNU grep warns about in the patterns.
    pub warnings: Vec<String>,
}

/// A literal as a basic regular expression: every character that has a meaning is escaped.
fn escape_fixed(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 4);
    for &b in text {
        if matches!(b, b'\\' | b'.' | b'[' | b'*' | b'^' | b'$') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

impl Matcher {
    pub fn new(options: &Options, locale: Locale) -> Result<Matcher, PatternError> {
        if options.kind == Kind::Perl {
            return Matcher::new_perl(options, locale);
        }
        let wrap = if options.line {
            Wrap::Line
        } else if options.word {
            Wrap::Word
        } else {
            Wrap::None
        };
        let syntax = |wrap| Syntax {
            extended: options.kind == Kind::Extended,
            icase: options.ignore_case,
            multiline: false,
            null_data: options.null_data,
            locale,
            wrap,
            grep: true,
            regex_semantic: false,
            ..Syntax::default()
        };
        let mut select = Vec::new();
        let mut plain = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for pattern in &options.patterns {
            let text = if options.kind == Kind::Fixed {
                escape_fixed(pattern)
            } else {
                pattern.clone()
            };
            // The C library reads the patterns first: its mistakes come first.
            if wrap != Wrap::None {
                match Regex::compile(&text, &syntax(Wrap::None)) {
                    Ok(re) => plain.push(re),
                    Err(e) => return Err(PatternError(e.message)),
                }
            }
            match Regex::compile(&text, &syntax(wrap)) {
                Ok(re) => {
                    warnings.extend(re.warnings().iter().cloned());
                    select.push(re);
                }
                Err(e) => return Err(PatternError(e.message)),
            }
        }
        Ok(Matcher {
            select,
            plain,
            word: wrap == Wrap::Word,
            both: options.word && options.line,
            perl: None,
            perl_enabled: false,
            perl_error: Default::default(),
            multibyte: locale.is_chars(),
            utf8: locale.is_utf8(),
            libc: matches!(locale, Locale::Libc | Locale::Mb),
            warnings,
        })
    }

    /// `-P`: a single pattern, wrapped for `-x` and `-w` as GNU grep does.
    fn new_perl(options: &Options, locale: Locale) -> Result<Matcher, PatternError> {
        if options.patterns.len() > 1 {
            return Err(PatternError(
                "the -P option only supports a single pattern".to_string(),
            ));
        }
        if locale.is_chars() && !locale.is_utf8() {
            return Err(PatternError(
                "-P supports only unibyte and UTF-8 locales".to_string(),
            ));
        }
        let mut flags = pcre::DOLLAR_ENDONLY;
        if options.ignore_case {
            flags |= pcre::CASELESS;
        }
        if locale.is_utf8() {
            flags |= pcre::UTF | pcre::MATCH_INVALID_UTF;
            if super::GNU_GREP_3_12 {
                flags |= pcre::UCP;
            }
        }
        let mut text = Vec::new();
        if options.line {
            text.extend_from_slice(b"^(?:");
        } else if options.word {
            text.extend_from_slice(b"(?<!\\w)(?:");
        }
        if let Some(p) = options.patterns.first() {
            text.extend_from_slice(p);
        }
        if options.line {
            text.extend_from_slice(b")$");
        } else if options.word {
            text.extend_from_slice(b")(?!\\w)");
        }
        let perl = if options.patterns.is_empty() {
            None
        } else {
            Some(Pcre::new(&text, flags).map_err(PatternError)?)
        };
        Ok(Matcher {
            select: Vec::new(),
            plain: Vec::new(),
            word: false,
            both: false,
            perl,
            perl_enabled: true,
            perl_error: Default::default(),
            multibyte: false,
            utf8: false,
            libc: false,
            warnings: Vec::new(),
        })
    }

    /// Literals of which every selected line has one: a text without any of them has no selected
    /// line, so a search over a whole file can look for them first. `None` if some pattern has
    /// no such literal (or the matcher is not made of the patterns, as with `-P`).
    pub fn needles(&self) -> Option<Vec<(Vec<u8>, bool)>> {
        if self.perl_enabled || self.select.is_empty() {
            return None;
        }
        let mut out: Vec<(Vec<u8>, bool)> = Vec::new();
        for re in &self.select {
            let (needle, fold) = re.required_literal()?;
            let entry = (needle.to_vec(), fold);
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        Some(out)
    }

    /// `-P`: a match the library gives up on is "no match" to the caller, and is kept for `take_error`.
    fn perl_find(&self, line: &[u8], start: usize) -> Option<(usize, usize)> {
        let p = self.perl.as_ref()?;
        match p.find(line, start) {
            Ok(found) => found,
            Err(e) => {
                self.perl_error
                    .borrow_mut()
                    .get_or_insert_with(|| e.message());
                None
            }
        }
    }

    /// `-P`: the text of the error of the library in a search since the last call, if any. The
    /// search that failed said "no match", so the caller asks for it after each line.
    pub fn take_error(&self) -> Option<String> {
        self.perl_error.borrow_mut().take()
    }

    /// Whether the line (without its terminator) has a match.
    pub fn is_match(&self, line: &[u8]) -> bool {
        if self.perl_enabled {
            return self.perl_find(line, 0).is_some();
        }
        if self.word && (self.multibyte || self.plain.iter().any(Regex::has_backrefs)) {
            // With a back-reference GNU grep leaves the decision to its words test.
            return self.find(line, 0).is_some();
        }
        self.select.iter().any(|re| re.is_match(line))
    }

    /// The leftmost match that starts at or after `start`, the longest of those that start there.
    pub fn find(&self, line: &[u8], start: usize) -> Option<(usize, usize)> {
        if self.perl_enabled {
            return self.perl_find(line, start);
        }
        if self.word {
            return self.find_word(line, start);
        }
        if self.both {
            return self.find_line_end(line, start);
        }
        let mut best: Option<(usize, usize)> = None;
        let mut caps = Captures::default();
        let regexes = if self.plain.is_empty() {
            &self.select
        } else {
            &self.plain
        };
        for re in regexes {
            if re.captures_at(line, start, &mut caps) {
                let (s, e) = caps.whole();
                best = better(best, s, e);
            }
        }
        best
    }

    /// Whether the character that starts at `pos` is part of a word. (In UTF-8 a byte that does not
    /// start a valid character is not.)
    fn wordchar_next(&self, re: &Regex, line: &[u8], pos: usize) -> bool {
        if pos >= line.len() {
            return false;
        }
        let b = line[pos];
        if !self.utf8 || b < 0x80 {
            return re.word_context(line, pos).1;
        }
        std::str::from_utf8(&line[pos..(pos + 4).min(line.len())])
            .or_else(|e| std::str::from_utf8(&line[pos..pos + e.valid_up_to()]))
            .ok()
            .and_then(|s| s.chars().next())
            .is_some_and(|c| self.word_char(c))
    }

    /// Whether the character that holds the byte before `pos` is part of a word (a position inside
    /// a character sees the whole character).
    fn wordchar_prev(&self, re: &Regex, line: &[u8], pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        let b = line[pos - 1];
        if !self.utf8 || b < 0x80 {
            return re.word_context(line, pos).0;
        }
        // Back to the start of the character: at most three continuation bytes.
        let mut start = pos - 1;
        while start > 0 && pos - start < 4 && line[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        std::str::from_utf8(&line[start..(start + 4).min(line.len())])
            .or_else(|e| std::str::from_utf8(&line[start..start + e.valid_up_to()]))
            .ok()
            .and_then(|s| s.chars().next())
            .is_some_and(|c| self.word_char(c))
    }

    fn word_char(&self, c: char) -> bool {
        if self.libc {
            crate::regex::is_word_char_libc(c)
        } else {
            crate::regex::is_word_char(c)
        }
    }

    /// `-w` with `-x`: only a match that goes to the end of the line counts, and it is reported as
    /// the whole rest of the line with its terminator (the match ends one byte after the line).
    fn find_line_end(&self, line: &[u8], ptr: usize) -> Option<(usize, usize)> {
        let mut caps = Captures::default();
        for re in &self.plain {
            if re.captures_at(line, ptr, &mut caps) {
                let (m, e) = caps.whole();
                if e - m == line.len() - ptr {
                    return Some((ptr, line.len() + 1));
                }
            }
        }
        None
    }

    /// `-w`, as GNU grep does it: the leftmost match is tested for being a word; if it is not, a
    /// shorter match at the same place is tried (on the text cut off at the end of the shorter
    /// match, measured from where the search began), and then a match further on.
    fn find_word(&self, line: &[u8], ptr: usize) -> Option<(usize, usize)> {
        let end = line.len();
        let mut best: Option<(usize, usize)> = None;
        let mut best_match = end + 1;
        let mut caps = Captures::default();
        for re in &self.plain {
            if !re.captures_at(line, ptr, &mut caps) {
                continue;
            }
            let (mut m, e) = caps.whole();
            let mut len = e - m;
            if m > best_match {
                continue;
            }
            while m <= best_match {
                let after = self.wordchar_next(re, line, m + len);
                let before = self.wordchar_prev(re, line, m);
                if !after && !before {
                    // A good match: the leftmost, then the longest, of all the patterns.
                    if m < best_match
                        || best.is_none_or(|(_, bl)| m == best_match && len > bl - best_match)
                    {
                        best_match = m;
                        best = Some((m, m + len));
                    }
                    break;
                }
                let mut shorter = 0usize;
                if len > 0 {
                    len -= 1;
                    // The text is cut at `m + len - ptr` (GNU grep measures from where the search
                    // began, not from the start of the line).
                    let cut = (m + len).checked_sub(ptr);
                    if let Some(cut) = cut.filter(|&c| c >= m) {
                        if re.captures_at(&line[..cut], m, &mut caps) && caps.whole().0 == m {
                            shorter = caps.whole().1 - m;
                        }
                    }
                }
                if shorter > 0 {
                    len = shorter;
                } else {
                    if m == best_match {
                        break;
                    }
                    m += 1;
                    if m > end || !re.captures_at(line, m, &mut caps) {
                        break;
                    }
                    let (s, e) = caps.whole();
                    m = s;
                    len = e - s;
                }
            }
        }
        best
    }
}

fn better(best: Option<(usize, usize)>, s: usize, e: usize) -> Option<(usize, usize)> {
    match best {
        Some((bs, be)) if bs < s || (bs == s && be >= e) => Some((bs, be)),
        _ => Some((s, e)),
    }
}

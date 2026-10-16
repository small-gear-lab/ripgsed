//! File name patterns of `--include`, `--exclude` and `--exclude-dir` (shell wildcards), and
//! the rule that says whether a name is skipped.

/// Matches `text` against a shell pattern: `*` any run (also `/`), `?` one character, `[...]`
/// a set (`!` or `^` negates, `a-z` ranges, `[:class:]`), `\` takes the next character
/// literally. The whole text must match.
pub fn fnmatch(pattern: &[u8], text: &[u8]) -> bool {
    fn set_end(p: &[u8], mut i: usize) -> Option<usize> {
        // `i` is after `[` (and after `!` or `^`).
        if i < p.len() && p[i] == b']' {
            i += 1;
        }
        while i < p.len() {
            match p[i] {
                b'[' if i + 1 < p.len() && p[i + 1] == b':' => {
                    let mut j = i + 2;
                    while j + 1 < p.len() && !(p[j] == b':' && p[j + 1] == b']') {
                        j += 1;
                    }
                    if j + 1 >= p.len() {
                        i += 1;
                    } else {
                        i = j + 2;
                    }
                }
                b']' => return Some(i),
                _ => i += 1,
            }
        }
        None
    }
    fn class_has(name: &[u8], c: u8) -> bool {
        match name {
            b"alpha" => c.is_ascii_alphabetic(),
            b"digit" => c.is_ascii_digit(),
            b"alnum" => c.is_ascii_alphanumeric(),
            b"upper" => c.is_ascii_uppercase(),
            b"lower" => c.is_ascii_lowercase(),
            b"space" => matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
            b"blank" => matches!(c, b' ' | b'\t'),
            b"punct" => c.is_ascii_punctuation(),
            b"print" => (0x20..0x7f).contains(&c),
            b"graph" => (0x21..0x7f).contains(&c),
            b"cntrl" => c < 0x20 || c == 0x7f,
            b"xdigit" => c.is_ascii_hexdigit(),
            _ => false,
        }
    }
    fn set_matches(p: &[u8], c: u8) -> Option<(bool, usize)> {
        // `p` starts after `[`. Returns whether `c` is in the set and the length of the set text
        // (up to and including `]`).
        let negate = !p.is_empty() && (p[0] == b'!' || p[0] == b'^');
        let start = usize::from(negate);
        let end = set_end(p, start)?;
        let mut i = start;
        let mut found = false;
        while i < end {
            if p[i] == b'[' && i + 1 < end && p[i + 1] == b':' {
                if let Some(rel) = p[i + 2..end].windows(2).position(|w| w == b":]") {
                    if class_has(&p[i + 2..i + 2 + rel], c) {
                        found = true;
                    }
                    i += 2 + rel + 2;
                    continue;
                }
            }
            let lo = if p[i] == b'\\' && i + 1 < end {
                i += 1;
                p[i]
            } else {
                p[i]
            };
            if i + 2 < end && p[i + 1] == b'-' {
                let hi = if p[i + 2] == b'\\' && i + 3 < end { i += 1; p[i + 2] } else { p[i + 2] };
                if lo <= c && c <= hi {
                    found = true;
                }
                i += 3;
            } else {
                if lo == c {
                    found = true;
                }
                i += 1;
            }
        }
        Some((found != negate, end + 1))
    }
    fn go(p: &[u8], t: &[u8]) -> bool {
        let (mut pi, mut ti) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;
        loop {
            if pi < p.len() {
                match p[pi] {
                    b'*' => {
                        star = Some((pi + 1, ti));
                        pi += 1;
                        continue;
                    }
                    b'?' if ti < t.len() => {
                        pi += 1;
                        ti += 1;
                        continue;
                    }
                    b'[' if ti < t.len() => {
                        if let Some((ok, len)) = set_matches(&p[pi + 1..], t[ti]) {
                            if ok {
                                pi += 1 + len;
                                ti += 1;
                                continue;
                            }
                        } else if t[ti] == b'[' {
                            pi += 1;
                            ti += 1;
                            continue;
                        }
                    }
                    b'\\' if pi + 1 < p.len() && ti < t.len() && p[pi + 1] == t[ti] => {
                        pi += 2;
                        ti += 1;
                        continue;
                    }
                    c if c != b'?' && c != b'[' && c != b'\\' && ti < t.len() && c == t[ti] => {
                        pi += 1;
                        ti += 1;
                        continue;
                    }
                    _ => {}
                }
            } else if ti == t.len() {
                return true;
            }
            // Mismatch: let the last star take one more character.
            match star {
                Some((sp, st)) if st < t.len() => {
                    star = Some((sp, st + 1));
                    pi = sp;
                    ti = st + 1;
                }
                _ => return false,
            }
        }
    }
    go(pattern, text)
}

/// The patterns of `--include` and `--exclude` in the order they were given.
#[derive(Debug, Default, Clone)]
pub struct Skip {
    /// (is an include, pattern)
    patterns: Vec<(bool, Vec<u8>)>,
}

impl Skip {
    pub fn push(&mut self, include: bool, pattern: Vec<u8>) {
        self.patterns.push((include, pattern));
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `name` is skipped: the last pattern that matches decides (an include keeps the
    /// name, an exclude skips it); when none matches, the name is skipped if the first pattern
    /// given was an include. A name given on the command line (`anchored` false) also matches
    /// through the part of it after any `/`.
    pub fn skips(&self, name: &[u8], anchored: bool) -> bool {
        let Some(first) = self.patterns.first() else { return false };
        let matches = |pattern: &[u8]| {
            if fnmatch(pattern, name) {
                return true;
            }
            if !anchored {
                for (i, &b) in name.iter().enumerate() {
                    if b == b'/' && name.get(i + 1) != Some(&b'/') && fnmatch(pattern, &name[i + 1..]) {
                        return true;
                    }
                }
            }
            false
        };
        for (include, pattern) in self.patterns.iter().rev() {
            if matches(pattern) {
                return !include;
            }
        }
        first.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(fnmatch(b"*.txt", b"a.txt"));
        assert!(fnmatch(b"*.txt", b".txt"));
        assert!(!fnmatch(b"*.txt", b"a.log"));
        assert!(fnmatch(b"a?c", b"abc"));
        assert!(fnmatch(b"*", b"a/b"));
        assert!(fnmatch(b"[a-c]x", b"bx"));
        assert!(!fnmatch(b"[!a-c]x", b"bx"));
        assert!(fnmatch(b"[[:digit:]]*", b"1abc"));
        assert!(fnmatch(b"\\*", b"*"));
        assert!(!fnmatch(b"\\*", b"a"));
        assert!(fnmatch(b"a*b*c", b"aXXbYYc"));
        assert!(fnmatch(b"", b""));
        assert!(!fnmatch(b"", b"a"));
    }

    #[test]
    fn the_last_matching_pattern_decides() {
        let mut s = Skip::default();
        s.push(true, b"*.txt".to_vec());
        s.push(false, b"b*".to_vec());
        assert!(!s.skips(b"a.txt", true));
        assert!(s.skips(b"b.txt", true));
        assert!(s.skips(b"c.log", true));
        let mut s = Skip::default();
        s.push(false, b"b*".to_vec());
        s.push(true, b"*.txt".to_vec());
        assert!(!s.skips(b"b.txt", true));
        assert!(!s.skips(b"c.log", true));
        let mut s = Skip::default();
        s.push(false, b"b.txt".to_vec());
        assert!(s.skips(b"d/b.txt", false));
        assert!(!s.skips(b"d/b.txt", true));
    }
}

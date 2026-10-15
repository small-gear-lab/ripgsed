//! Differential tests of the regex engine against the installed GNU sed 4.9.
//!
//! Every case is a pattern in a dialect plus a few input records. GNU sed runs
//! `s/PATTERN/<&>[\1]...[\9]/g` over the records (with `-z`, so records may
//! contain newlines); the engine runs the same substitution through a small
//! reference loop that follows GNU sed's `s///g` rules. The outputs show the
//! whole match and every group, so captures are compared, not only matches.
//! Compile errors are compared by message.
//!
//! The cases are a hand-written table (manual examples and probes) and a
//! deterministic pseudo-random generator over the supported grammar.
//! The test is skipped (with a message) when `sed` is not GNU sed 4.9.
//!
//! `RIPGSED_REGEX_CASES=N` changes the number of random cases;
//! `RIPGSED_REGEX_SEED=S` the seed.

use std::io::Write;
use std::process::{Command, Stdio};

use ripgsed::regex::{Captures, Locale, Regex, Syntax};

#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    ere: bool,
    posix: bool,
    posixly_correct: bool,
    icase: bool,
    multiline: bool,
    utf8: bool,
}

impl Flags {
    fn parse(s: &str) -> Flags {
        let mut f = Flags::default();
        for c in s.chars() {
            match c {
                'E' => f.ere = true,
                'P' => f.posix = true,
                'C' => f.posixly_correct = true,
                'I' => f.icase = true,
                'M' => f.multiline = true,
                'U' => f.utf8 = true,
                _ => panic!("bad flag {c}"),
            }
        }
        f
    }

    fn describe(&self) -> String {
        let mut s = String::new();
        for (on, c) in [
            (self.ere, 'E'),
            (self.posix, 'P'),
            (self.posixly_correct, 'C'),
            (self.icase, 'I'),
            (self.multiline, 'M'),
            (self.utf8, 'U'),
        ] {
            if on {
                s.push(c);
            }
        }
        if s.is_empty() { "-".into() } else { s }
    }

    fn syntax(&self) -> Syntax {
        Syntax {
            extended: self.ere,
            posix: self.posix,
            posixly_correct: self.posixly_correct,
            icase: self.icase,
            multiline: self.multiline,
            locale: if self.utf8 { Locale::Utf8 } else { Locale::C },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Records(Vec<Vec<u8>>),
    Error(String),
}

fn replacement(groups: usize) -> Vec<u8> {
    let mut r = b"<&>".to_vec();
    for g in 1..=groups.min(9) {
        r.extend_from_slice(format!("[\\{g}]").as_bytes());
    }
    r
}

fn pick_delimiter(pattern: &[u8]) -> Option<u8> {
    b"/,#%@!;~\"'=:_".iter().copied().find(|d| !pattern.contains(d))
}

/// Runs GNU sed. `groups` is the number of groups to show.
fn run_sed(flags: Flags, pattern: &[u8], groups: usize, inputs: &[Vec<u8>]) -> Option<Outcome> {
    let delim = pick_delimiter(pattern)?;
    let mut script = vec![b's', delim];
    for &b in pattern {
        if b == b'\n' {
            script.extend_from_slice(b"\\n");
        } else {
            script.push(b);
        }
    }
    script.push(delim);
    script.extend_from_slice(&replacement(groups));
    script.push(delim);
    script.push(b'g');
    if flags.icase {
        script.push(b'I');
    }
    if flags.multiline {
        script.push(b'M');
    }
    let mut cmd = Command::new("sed");
    // GNU sed's M flag anchors at the record delimiter, so a case with M
    // runs one record at a time as a whole input joined with N.
    if flags.multiline {
        if inputs.len() != 1 {
            let mut recs = Vec::new();
            for i in inputs {
                match run_sed(flags, pattern, groups, std::slice::from_ref(i))? {
                    Outcome::Records(mut r) => recs.append(&mut r),
                    e @ Outcome::Error(_) => return Some(e),
                }
            }
            return Some(Outcome::Records(recs));
        }
        cmd.arg("-e").arg(":a").arg("-e").arg("$!{N;ba;}");
    } else {
        cmd.arg("-z");
    }
    if flags.ere {
        cmd.arg("-E");
    }
    if flags.posix {
        cmd.arg("--posix");
    }
    cmd.arg("-e");
    use std::os::unix::ffi::OsStrExt;
    cmd.arg(std::ffi::OsStr::from_bytes(&script));
    cmd.env_remove("POSIXLY_CORRECT");
    if flags.posixly_correct {
        cmd.env("POSIXLY_CORRECT", "1");
    }
    cmd.env("LC_ALL", if flags.utf8 { "C.utf8" } else { "C" });
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("run sed");
    let mut data = Vec::new();
    for i in inputs {
        data.extend_from_slice(i);
        data.push(if flags.multiline { b'\n' } else { 0 });
    }
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&data);
    });
    let out = child.wait_with_output().expect("sed output");
    writer.join().unwrap();
    if out.status.code().is_none() {
        // GNU sed crashed (it does on some back-reference patterns).
        return None;
    }
    if !out.status.success() || !out.stderr.is_empty() {
        let text = String::from_utf8_lossy(&out.stderr).trim_end().to_string();
        let msg = match text.find(": ") {
            Some(_) if text.starts_with("sed: -e expression") => {
                // sed: -e expression #1, char N: MESSAGE
                let after = &text["sed: -e expression".len()..];
                after.split_once(": ").map(|(_, m)| m.to_string()).unwrap_or(text.clone())
            }
            Some(k) => text[k + 2..].to_string(),
            None => text.clone(),
        };
        return Some(Outcome::Error(msg));
    }
    if flags.multiline {
        let mut o = out.stdout;
        if o.last() == Some(&b'\n') {
            o.pop();
        }
        return Some(Outcome::Records(vec![o]));
    }
    let mut recs: Vec<Vec<u8>> = out.stdout.split(|&b| b == 0).map(|s| s.to_vec()).collect();
    if recs.last().is_some_and(|r| r.is_empty()) {
        recs.pop();
    }
    Some(Outcome::Records(recs))
}

/// The engine through a reference `s///g` loop.
fn run_engine(re: &Regex, input: &[u8]) -> Vec<u8> {
    let groups = re.group_count().min(9);
    let mut out = Vec::new();
    let mut pos = 0;
    let mut copied = 0;
    let mut last_end: Option<usize> = None;
    let mut caps = Captures::default();
    while pos <= input.len() && re.captures_at(input, pos, &mut caps) {
        let (s, e) = caps.whole();
        if s == e && Some(s) == last_end {
            // No empty match right after the previous match.
            if s >= input.len() {
                break;
            }
            pos = s + 1;
            last_end = None;
            continue;
        }
        last_end = Some(e);
        out.extend_from_slice(&input[copied..s]);
        out.push(b'<');
        out.extend_from_slice(&input[s..e]);
        out.push(b'>');
        for g in 1..=groups {
            out.push(b'[');
            if let Some((a, b)) = caps.get(g) {
                out.extend_from_slice(&input[a..b]);
            }
            out.push(b']');
        }
        copied = e;
        if s == e {
            if s >= input.len() {
                break;
            }
            out.push(input[s]);
            copied = s + 1;
            pos = s + 1;
        } else {
            pos = e;
        }
    }
    if copied < input.len() {
        out.extend_from_slice(&input[copied..]);
    }
    out
}

fn engine_outcome(flags: Flags, pattern: &[u8], inputs: &[Vec<u8>]) -> (Outcome, usize) {
    match Regex::compile(pattern, &flags.syntax()) {
        Err(e) => (Outcome::Error(e.message), 0),
        Ok(re) => {
            let recs = inputs.iter().map(|i| run_engine(&re, i)).collect();
            (Outcome::Records(recs), re.group_count())
        }
    }
}

struct Report {
    total: usize,
    failures: Vec<String>,
}

fn show(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        match c {
            b'\n' => s.push_str("\\n"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s
}

fn check(report: &mut Report, flags: Flags, pattern: &[u8], inputs: &[Vec<u8>]) {
    // The empty regexp and modifiers under --posix are sed-level matters.
    if pattern.is_empty() || (flags.posix && (flags.icase || flags.multiline)) {
        return;
    }
    let (mine, groups) = engine_outcome(flags, pattern, inputs);
    let Some(theirs) = run_sed(flags, pattern, groups, inputs) else {
        return;
    };
    report.total += 1;
    if mine == theirs {
        return;
    }
    let mut msg = format!("flags {} pattern `{}`:", flags.describe(), show(pattern));
    match (&mine, &theirs) {
        (Outcome::Records(a), Outcome::Records(b)) => {
            for (k, input) in inputs.iter().enumerate() {
                let (x, y) = (a.get(k), b.get(k));
                if x != y {
                    msg.push_str(&format!(
                        "\n    input `{}`: engine `{}`, sed `{}`",
                        show(input),
                        x.map(|v| show(v)).unwrap_or_default(),
                        y.map(|v| show(v)).unwrap_or_default()
                    ));
                }
            }
        }
        _ => {
            let fmt = |o: &Outcome| match o {
                Outcome::Error(e) => format!("error `{e}`"),
                Outcome::Records(r) => r.iter().map(|x| format!("`{}`", show(x))).collect::<Vec<_>>().join(" "),
            };
            msg.push_str(&format!("\n    engine {}\n    sed    {}", fmt(&mine), fmt(&theirs)));
        }
    }
    report.failures.push(msg);
}

fn gnu_sed_49() -> bool {
    match Command::new("sed").arg("--version").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().next() == Some("sed (GNU sed) 4.9"),
        Err(_) => false,
    }
}

/// Hand-written cases: (flags, pattern, inputs separated by `|`).
/// `\n` in inputs is a newline.
const TABLE: &[(&str, &str, &str)] = &[
    // Leftmost-longest and alternation.
    ("", "a\\|ab", "abc|ab|a|xab|babc"),
    ("", "ab\\|a", "abc|ab"),
    ("", "x*\\|b", "abc|babc"),
    ("E", "a|ab|abc", "abcd|xabc"),
    ("E", "(a|ab)(c|bcd)(d*)", "abc|abcd"),
    ("E", "(a|ab)(bc|c)?", "abc|ab|aab"),
    ("E", "(a*)(ab)*(b*)", "abc|abcd|ab|aab|abab|abbb"),
    ("E", "(a*)+", "aa|b|"),
    ("E", "(a*)*", "b|aab|aa"),
    ("E", "(a|ab)*c", "ababc|abac|aabc"),
    ("E", "(ab|a)(bc|c)?", "abc"),
    ("E", "(a*)(ab)*b", "aabb"),
    ("E", "(.*)(.*)", "abc"),
    ("E", "(a?)*", "aa"),
    ("E", "((a)|b)*", "ab|ba"),
    ("E", "((a)|(b))*", "ab|ba"),
    ("E", "((x)|(y))+", "xyxy|x|y"),
    ("E", "(a|b)*", "ab"),
    ("E", "(a*)*(b)", "ab"),
    ("E", "(a+|b)*", "ab|aabbc"),
    ("E", "(aa|a)(a?)", "aaa"),
    ("E", "(a|aa)*", "aaa"),
    ("E", "(a|aa)(a|aa)", "aaa|aaaa"),
    ("E", "(a|b*)+", "a"),
    ("E", "(a|b*)*", "a"),
    ("E", "(b*|a)*", "a"),
    ("E", "((a)|b*)*", "a"),
    ("E", "(a|ab){2}", "aab"),
    ("E", "(a){0}b", "ab"),
    ("E", "(a?){3}", "aa"),
    ("E", "(a?){2,3}", "a"),
    ("E", "(a*){2}", "aaa"),
    ("E", "(x)?(x)?y", "xy"),
    ("E", "(a?)(a*)(a?)", "aaa|aaaa|a|b"),
    ("E", "(|a)*", "aa"),
    ("E", "(a|)*", "aa"),
    ("E", "(|a)+", "aa"),
    ("E", "(a|b|)+", "bab|xay|abba|c"),
    // Empty matches with g.
    ("", "x*", "abc|bbb"),
    ("", "b*", "abc|bbb|xaxbx"),
    ("", "\\(ab\\)*", "abc|ababc"),
    ("E", "(a|)", "abc|bbb|xaxbx|ababc"),
    // Back-references.
    ("E", "(a*)\\1", "aaaa|aaa"),
    ("E", "(a*)\\1b", "aaab"),
    ("E", "(a|ab)\\1", "abab"),
    ("E", "(a*)*\\1", "aab"),
    ("E", "(a*)+\\1", "aaaa"),
    ("E", "(.)\\1*", "aaab"),
    ("E", "((a)|b)\\2", "bb|aa"),
    ("E", "(a)?b\\1", "b|ab|aba"),
    ("E", "(a*)b\\1+", "aabaaaa"),
    ("E", "(a)?(b)\\1?c", "bc|abc|abac"),
    ("E", "(a)*b\\1*", "bc|abc|aabaa|bb"),
    ("E", "(a|(b))+\\2", "bb|ab|abb"),
    ("E", "((a)|b)+\\2", "aab|bab|abb|abba|aa"),
    ("E", "(a|b)*\\1", "aab|bab|abb|abba|aa|baa"),
    ("E", "(a*)*\\1b", "aab|bab|abb|ab"),
    ("E", "(x)|(y)\\2", "xyy|yyx"),
    ("E", "((a)|(b))\\3", "bb|aa"),
    ("E", "(a|b)(\\1)", "aa|ba"),
    ("", "\\(.\\)\\(.\\)\\2\\1", "abba|xyyx|abc"),
    ("", "^\\(.*\\)\\1$", "abab|abcabc|aa|abc"),
    ("I", "\\(a\\)\\1", "aA|Aa|aa"),
    ("EI", "(ab)\\1", "abAB|aBAb"),
    // Errors.
    ("", "a**", ""),
    ("", "\\(a\\)**", ""),
    ("", "a\\{1\\}\\{2\\}", ""),
    ("", "a*\\{2\\}", ""),
    ("", "a\\?*", ""),
    ("", "\\{2\\}", ""),
    ("", "\\(\\{2\\}a\\)", ""),
    ("E", "*a", ""),
    ("E", "a|*b", ""),
    ("E", "(*a)", ""),
    ("E", "^*", ""),
    ("E", "a$*", ""),
    ("E", "\\<*", ""),
    ("E", "a{1", ""),
    ("E", "a{1,2", ""),
    ("E", "a{x}", ""),
    ("E", "a{}", ""),
    ("E", "{", ""),
    ("E", "a{,}", "aaa"),
    ("", "a\\{,2\\}", "aaa"),
    ("", "a\\{2,1\\}", ""),
    ("", "a\\{32767\\}", "aa"),
    ("", "a\\{32768\\}", ""),
    ("", "a\\{0,32768\\}", ""),
    ("E", "a{99999999999}", ""),
    ("", "\\(a", ""),
    ("", "a\\)", ""),
    ("E", "(a", ""),
    ("E", "a)", ""),
    ("E", "(a))", ""),
    ("P", "a\\)", "a)"),
    ("EP", "a)", "a)"),
    ("", "\\(a\\)\\2", ""),
    ("E", "(a)|b\\1", ""),
    ("E", "a(.)|b\\1", ""),
    ("E", "(a\\1)", ""),
    ("", "[z-a]", ""),
    ("", "[0-9-a]", ""),
    ("", "[a-c-e]", ""),
    ("", "[[:alpha:]-z]", ""),
    ("", "[a-[:alpha:]]", ""),
    ("", "[[=a=]-z]", ""),
    ("", "[[:foo:]]", ""),
    ("", "[[:ALPHA:]]", ""),
    ("", "[[.space.]]", ""),
    ("", "[[.ab.]]", ""),
    ("", "[[..]]", ""),
    ("", "[[==]]", ""),
    ("", "[[=ab=]]", ""),
    ("", "[:alpha:]", ""),
    ("", "[:a:]", ""),
    ("", "[^:alpha:]", ""),
    ("", "[[:digit:]:alpha:]", "a1:x"),
    ("", "[:a]", "a:b:c"),
    ("", "[:]", "a:b"),
    // Brackets.
    ("", "[]]", "a]b]c"),
    ("", "[]a]", "a]b]c"),
    ("", "[^]a]", "a]b]c"),
    ("", "[]-a]", "^_`|]]"),
    ("", "[-a]", "a-b-c"),
    ("", "[a-]", "a-b-c"),
    ("", "[+--]", ",+-./0"),
    ("", "[--/]", ",+-./0"),
    ("", "[\\]", "a\\b"),
    ("", "[\\]]", "\\]|]"),
    ("", "[a^]", "a^b"),
    ("", "[^^]", "^^a"),
    ("", "[[.a.]-[.c.]]", "abcd"),
    ("", "[[.-.]]", "a-b"),
    ("", "[[.].]]", "a]b"),
    ("", "[[=a=]b]", "abcABC"),
    ("", "[[:alpha:]_]\\+", "Ab 12 c_d-e"),
    ("", "[^[:space:][:punct:]]\\+", "Hello, World! 42"),
    ("I", "[^a-c]\\+", "abcdABCDxX|xyzXYZ"),
    ("I", "[[:upper:]]", "aBc|ABC|abc"),
    ("I", "[b-d]x*", "abcdABCDxX"),
    ("M", "[^x]*", "a\nb"),
    ("", "[^x]*", "a\nb"),
    // Anchors.
    ("", "b\\|^a", "aab|bab|ba"),
    ("", "a$\\|b", "aab|bab|ba"),
    ("", "\\(a\\|^b\\)c", "abca|bca"),
    ("", "\\(^a\\)", "aab|ab"),
    ("", "x\\(^a\\)", "xa|x^a"),
    ("", "\\(a$\\)b", "ab"),
    ("", "a^b", "a^b"),
    ("", "a$b", "a$b"),
    ("", "^*", "*abc|abc"),
    ("", "^**", "**a|a"),
    ("", "\\(*a\\)", "*a|a*a"),
    ("", "\\(^*\\)", "*a|a*a"),
    ("", "*a", "*a"),
    ("", "**", "**a|a"),
    ("", "a\\|*b", "a*b|*b"),
    ("", "^^", "^^a|a"),
    ("", "$$", "a$$|a"),
    ("", "^$", "|a"),
    ("", "\\+", "+a|a+"),
    ("", "\\?", "?a"),
    ("", "a\\{1\\}\\+", "aaa"),
    ("", "a*\\+", "aaab"),
    ("", "a\\?\\?b", "aab"),
    ("", "\\b*", "ab cd"),
    ("", "\\<*", "ab cd"),
    ("", "\\b\\+", "ab+cd"),
    ("", "a$*", "a$$|a"),
    ("E", "a^b", "a^b"),
    ("E", "(^a|b$)", "aab|ab"),
    ("E", "x|^a", "xaxb"),
    ("E", "a**", "aaa|b"),
    ("E", "a+*", "aaa|b"),
    ("E", "a?*", "aaa"),
    ("E", "a{2}*", "aaaab|b"),
    ("E", "(ab){2}*", "abababab"),
    ("E", "(a*)**b", "aab|b"),
    // Word assertions and GNU escapes.
    ("", "\\b", "foo bar| a  b |x|...|foo.foo,foo_foo|"),
    ("", "\\B", "foo bar| a  b |x|...|"),
    ("", "\\<", "foo bar| a  b |x|..."),
    ("", "\\>", "foo bar| a  b |x|..."),
    ("", "\\<\\>", "ab cd"),
    ("", "\\w\\+", "Hello, World_42!"),
    ("", "\\W\\+", "Hello, World_42!"),
    ("", "\\s\\+", "a b\tc"),
    ("", "\\S\\+", "a b\tc"),
    ("", "\\`a", "aba|bab"),
    ("", "a\\'", "aba|bab"),
    ("M", "\\`.", "a\nb"),
    ("M", ".\\'", "a\nb"),
    ("M", "^.", "ab\ncd"),
    ("M", ".$", "ab\ncd"),
    ("M", "^$", "a\n\nb"),
    ("M", "a.b", "a\nb"),
    ("", "a.b", "a\nb"),
    ("M", "\\W", "a\nb"),
    ("", "\\b", "a\nb"),
    ("P", "\\w\\W\\s\\S\\b\\B\\<\\>", "wWsSbB<> x"),
    ("P", "a\\+b\\?c\\|d", "a+b?c|d"),
    ("P", "\\`", "a`b"),
    ("EP", "\\w|\\b|\\<", "wWsSbB<> x"),
    ("C", "a\\+\\w", "aab"),
    // Case-insensitive.
    ("I", "abc", "xabcxABCx|aBc"),
    ("EI", "cat|dog", "Cat DOG cAt"),
    ("I", "\\w", "aBc 12"),
];

fn table_cases(report: &mut Report) {
    for &(flags, pattern, inputs) in TABLE {
        let flags = Flags::parse(flags);
        let inputs: Vec<Vec<u8>> = inputs.split('|').map(|s| s.as_bytes().to_vec()).collect();
        check(report, flags, pattern.as_bytes(), &inputs);
    }
}

/// xorshift64*: deterministic and dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

struct Gen<'r> {
    rng: &'r mut Rng,
    ere: bool,
    gnu: bool,
    groups: usize,
    budget: usize,
}

impl Gen<'_> {
    fn op(&self, ere: &str, bre: &str) -> String {
        if self.ere { ere.to_string() } else { bre.to_string() }
    }

    fn regex(&mut self, depth: usize) -> String {
        let mut s = self.branch(depth);
        while self.rng.chance(18) && self.budget > 0 {
            self.budget -= 1;
            s.push_str(&self.op("|", "\\|"));
            s.push_str(&self.branch(depth));
        }
        s
    }

    fn branch(&mut self, depth: usize) -> String {
        let n = self.rng.below(4) as usize;
        let mut s = String::new();
        for _ in 0..n {
            if self.budget == 0 {
                break;
            }
            self.budget -= 1;
            s.push_str(&self.piece(depth));
        }
        s
    }

    fn piece(&mut self, depth: usize) -> String {
        let mut s = self.atom(depth);
        if self.rng.chance(35) {
            s.push_str(&self.quant());
            if self.rng.chance(10) {
                s.push_str(&self.quant());
            }
        }
        s
    }

    fn quant(&mut self) -> String {
        let (m, n) = (self.rng.below(3), self.rng.below(3));
        match self.rng.below(7) {
            0 | 1 => "*".into(),
            2 => self.op("+", "\\+"),
            3 => self.op("?", "\\?"),
            4 => self.op(&format!("{{{m}}}"), &format!("\\{{{m}\\}}")),
            5 => self.op(&format!("{{{m},}}"), &format!("\\{{{m},\\}}")),
            _ => {
                let (lo, hi) = (m.min(n), m.max(n));
                self.op(&format!("{{{lo},{hi}}}"), &format!("\\{{{lo},{hi}\\}}"))
            }
        }
    }

    fn atom(&mut self, depth: usize) -> String {
        let r = self.rng.below(100);
        match r {
            0..=34 => self.rng.pick(&["a", "b", "c", "a", "b", " "]).to_string(),
            35..=41 => ".".into(),
            42..=53 => self.bracket(),
            54..=69 if depth < 3 => {
                self.groups += 1;
                let inner = self.regex(depth + 1);
                format!("{}{}{}", self.op("(", "\\("), inner, self.op(")", "\\)"))
            }
            70..=75 => self.rng.pick(&["^", "$"]).to_string(),
            76..=83 if self.groups > 0 => {
                let k = 1 + self.rng.below(self.groups as u64) as usize;
                format!("\\{}", k.min(9))
            }
            84..=93 if self.gnu || self.rng.chance(30) => {
                self.rng.pick(&["\\w", "\\W", "\\s", "\\S", "\\b", "\\B", "\\<", "\\>", "\\`", "\\'"]).to_string()
            }
            _ => self.rng.pick(&["\\.", "\\*", "a", "b", "-", "\\n"]).to_string(),
        }
    }

    fn bracket(&mut self) -> String {
        let mut s = String::from("[");
        if self.rng.chance(30) {
            s.push('^');
        }
        if self.rng.chance(10) {
            s.push(']');
        }
        let n = 1 + self.rng.below(3);
        for _ in 0..n {
            let e = *self.rng.pick(&[
                "a", "b", "c", "a-c", "b-c", "[:alpha:]", "[:space:]", "[:upper:]", "[:punct:]", "[.a.]", "[=b=]",
                " ", "A",
            ]);
            s.push_str(e);
        }
        if self.rng.chance(10) {
            s.push('-');
        }
        s.push(']');
        s
    }
}

fn random_input(rng: &mut Rng, utf8: bool) -> Vec<u8> {
    let n = rng.below(11) as usize;
    let mut v = Vec::new();
    for _ in 0..n {
        let alphabet: &[&[u8]] = if utf8 {
            &[b"a", b"b", b"c", b"A", b" ", b"\n", "é".as_bytes(), "É".as_bytes(), "中".as_bytes(), b"\xff", b"_"]
        } else {
            &[b"a", b"b", b"c", b"a", b"b", b"A", b"B", b" ", b"-", b"\n", b"_", b"*"]
        };
        v.extend_from_slice(rng.pick(alphabet));
    }
    v
}

fn random_cases(report: &mut Report, count: usize, seed: u64) {
    let mut rng = Rng(seed | 1);
    for _ in 0..count {
        let ere = rng.chance(50);
        let posix = rng.chance(12);
        let flags = Flags {
            ere,
            posix,
            posixly_correct: false,
            icase: !posix && rng.chance(15),
            multiline: !posix && rng.chance(15),
            utf8: false,
        };
        let mut g = Gen { rng: &mut rng, ere, gnu: !posix, groups: 0, budget: 8 };
        let pattern = g.regex(0);
        let inputs: Vec<Vec<u8>> = (0..6).map(|_| random_input(&mut rng, flags.utf8)).collect();
        check(report, flags, pattern.as_bytes(), &inputs);
    }
}

#[test]
fn regex_matches_gnu_sed() {
    if !gnu_sed_49() {
        eprintln!("skipping regex differential test: `sed --version` is not GNU sed 4.9");
        return;
    }
    let count = std::env::var("RIPGSED_REGEX_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    let seed = std::env::var("RIPGSED_REGEX_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0x5eed_1234_u64);
    let mut report = Report { total: 0, failures: Vec::new() };
    if let Ok(path) = std::env::var("RIPGSED_REGEX_ONLY") {
        // Investigation mode: cases from a file, one per line:
        // FLAGS<TAB>PATTERN<TAB>INPUT|INPUT... (FLAGS `-` for none).
        let text = std::fs::read_to_string(path).expect("read case file");
        for line in text.lines().filter(|l| !l.is_empty()) {
            let mut parts = line.splitn(3, '\t');
            let flags = parts.next().unwrap();
            let flags = Flags::parse(if flags == "-" { "" } else { flags });
            let pattern = parts.next().unwrap_or("");
            let inputs: Vec<Vec<u8>> =
                parts.next().unwrap_or("").split('|').map(|s| s.replace("\\n", "\n").into_bytes()).collect();
            check(&mut report, flags, pattern.as_bytes(), &inputs);
        }
        for f in &report.failures {
            eprintln!("{f}");
        }
        eprintln!("{} of {} cases differ", report.failures.len(), report.total);
        return;
    }
    table_cases(&mut report);
    random_cases(&mut report, count, seed);
    if !report.failures.is_empty() {
        let shown: Vec<&String> = report.failures.iter().take(40).collect();
        panic!(
            "{} of {} cases differ from GNU sed:\n{}",
            report.failures.len(),
            report.total,
            shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
        );
    }
    eprintln!("{} regex cases agree with GNU sed", report.total);
}

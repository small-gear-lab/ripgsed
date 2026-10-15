//! Differential fuzzing: random scripts and inputs run through GNU sed and through ripgsed;
//! stdout, stderr, and the exit status must agree. A fixed seed makes failures reproducible.
//!
//! `RIPGSED_FUZZ_EXEC_CASES=5000 cargo test --release --test execution_fuzz` runs more cases and
//! `RIPGSED_FUZZ_EXEC_SEED=7` picks another seed. The test is skipped when the `sed` on `PATH`
//! is not GNU sed 4.9.

use std::io::Write;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

const REGEXES_BRE: &[&str] = &[
    "a", "b", "ab", "a*", "a\\+", "a\\?", "[ab]", "[^a]", "[a-c]*", ".", ".*", "^a", "b$", "^$", "\\(a\\)", "\\(a*\\)b",
    "\\(a\\|b\\)\\1", "a\\{2\\}", "a\\{1,\\}", "[[:digit:]]", "[[:alpha:]]\\+", "\\bb", "\\<a", "x\\|y", "\\(.\\)\\1", " ",
    "[0-9]\\+", "\\w\\+", "\\s", "a.b", "[ab]*c", "\\n", "^\\(.*\\)$", "\\(a\\|ab\\)\\(c\\|bcd\\)\\?", "[[:upper:]]",
    "[^[:space:]]*", "\\`a", "c\\'", "a\\{0,1\\}b", "\\(b*\\)*", "\\(ab\\)*\\1", "[]a]", "[a-]", "\\Bb", "a\\b", "é", ".\\{2\\}",
];
const REGEXES_ERE: &[&str] = &[
    "a", "ab", "a*", "a+", "a?", "[ab]", "[^a]", ".*", "^a", "b$", "(a)", "(a|b)\\1", "a{2}", "a{1,}", "(a*)b", "x|y",
    "(.)\\1", "[[:digit:]]+", "\\w+", "(a|ab)(c|bcd)?", "^(.*)$", "()", "a|", "[ab]+c?", "(a*)*", "(a|b)*c", "((a)|(b))+", "a{0}b", "a{,2}",
    "(ab|a)(bc|c)?", "\\<a", "b\\>", "x*", "(^|b)a", "a($|b)", "[[:alnum:]_]+", ".{2,3}",
];
const REPLACEMENTS: &[&str] = &["X", "&", "[&]", "\\1", "<\\1>", "", "\\n", "\\U&", "\\L&", "\\u&", "a&b", "\\&", "\\\\"];
const TEXTS: &[&str] = &["hello", "x y", "", "a\\tb", "  spaced", "end"];
const LINES: &[&str] = &[
    "a", "b", "ab", "abc", "aab", "ba", "abab", "x", "y", "1", "42", "a b", "foo bar", "", "aaa", "bcd", "a1b2", "xyz", " a ", "A",
    "B", "abcabc", "é", "日本",
];

fn regex(rng: &mut Rng, extended: bool) -> String {
    rng.pick(if extended { REGEXES_ERE } else { REGEXES_BRE }).to_string()
}

fn address(rng: &mut Rng, extended: bool) -> String {
    match rng.below(14) {
        0 => rng.below(6).max(1).to_string(),
        1 => "$".into(),
        2 => format!("/{}/", regex(rng, extended)),
        3 => format!("/{}/I", regex(rng, extended)),
        4 => format!("{},{}", 1 + rng.below(3), 1 + rng.below(5)),
        5 => format!("/{}/,/{}/", regex(rng, extended), regex(rng, extended)),
        6 => format!("{},+{}", 1 + rng.below(3), rng.below(3)),
        7 => format!("{}~{}", rng.below(3), rng.below(4)),
        8 => format!("0,/{}/", regex(rng, extended)),
        9 => format!("{},~{}", 1 + rng.below(4), 1 + rng.below(4)),
        10 => format!("{},$", 1 + rng.below(3)),
        11 => format!("/{}/,{}", regex(rng, extended), 1 + rng.below(5)),
        12 => "//".into(),
        _ => format!("{}", 1 + rng.below(4)),
    }
}

fn command(rng: &mut Rng, extended: bool, depth: usize) -> String {
    let addr = if rng.chance(2) {
        let a = address(rng, extended);
        if rng.chance(5) { format!("{a}!") } else { a }
    } else {
        String::new()
    };
    let body = match rng.below(27) {
        0 => "p".into(),
        1 => "d".into(),
        2 => "D".into(),
        3 => "n".into(),
        4 => "N".into(),
        5 => "g".into(),
        6 => "G".into(),
        7 => "h".into(),
        8 => "H".into(),
        9 => "x".into(),
        10 => "P".into(),
        11 => "z".into(),
        12 => "=".into(),
        13 => "l".into(),
        14 | 15 | 16 => {
            let flags = rng.pick(&["", "g", "p", "2", "gp", "I", "2g", "w /dev/null", "M", "Ig", "3", "gI"]);
            format!("s/{}/{}/{}", regex(rng, extended), rng.pick(REPLACEMENTS), flags)
        }
        17 => "y/abc/xyz/".into(),
        18 => format!("a {}", rng.pick(TEXTS)),
        19 => format!("i {}", rng.pick(TEXTS)),
        20 => format!("c {}", rng.pick(TEXTS)),
        21 => format!("{}", rng.pick(&["q", "q5", "Q", "Q3"])),
        22 if depth < 2 => {
            let inner: Vec<String> = (0..1 + rng.below(3)).map(|_| command(rng, extended, depth + 1)).collect();
            format!("{{{}}}", inner.join(";"))
        }
        23 => format!("t{}", rng.pick(&["", " end"])),
        24 => format!("b{}", rng.pick(&["", " end"])),
        25 => format!("{} {}", rng.pick(&["r", "R", "w", "W"]), rng.pick(&["r.txt", "out1.txt", "out2.txt"])),
        _ => "p".into(),
    };
    // Commands that read the rest of the line must end it with a newline.
    format!("{addr}{body}")
}

fn script(rng: &mut Rng, extended: bool) -> String {
    let n = 1 + rng.below(5);
    let mut parts: Vec<String> = (0..n).map(|_| command(rng, extended, 0)).collect();
    if parts.iter().any(|p| p.contains(" end")) {
        parts.push(":end".into());
    }
    // `a`, `i`, `c`, and `w` take the rest of the line, so join with newlines.
    parts.join("\n")
}

fn input(rng: &mut Rng) -> Vec<u8> {
    let n = rng.below(7);
    let mut out = Vec::new();
    for i in 0..n {
        out.extend_from_slice(rng.pick(LINES).as_bytes());
        if i + 1 < n || !rng.chance(4) {
            out.push(b'\n');
        }
    }
    out
}

struct Run {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<i32>,
    timed_out: bool,
}

/// All files under `dir`, with their contents (for comparing what `w` and `-i` leave behind).
fn tree(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                out.insert(entry.file_name().to_string_lossy().into_owned(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    out
}

fn fresh_dir(dir: &Path, files: &[(&str, &[u8])]) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
}

fn run(program: &Path, args: &[String], stdin: &[u8], cwd: &Path, locale: &str) -> Run {
    let mut child = Command::new(program)
        .arg0("sed")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("LC_ALL", locale)
        .env("RIPGSED_GLIBC_COMPAT", "1")
        .envs(["RIPGSED_JOBS", "RIPGSED_BLOCK"].iter().filter_map(|k| std::env::var(k).ok().map(|v| (*k, v))))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut pipe = child.stdin.take().unwrap();
    let data = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&data);
    });
    let start = Instant::now();
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        if start.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            break true;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let output = child.wait_with_output().unwrap();
    let _ = writer.join();
    Run { stdout: output.stdout, stderr: output.stderr, status: output.status.code(), timed_out }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ripgsed-fuzz-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn random_scripts_agree_with_gnu_sed() {
    let version = Command::new("sed").arg("--version").output();
    let is_gnu_4_9 = version
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").ends_with("(GNU sed) 4.9"))
        .unwrap_or(false);
    if !is_gnu_4_9 {
        eprintln!("skipped: the sed on PATH is not GNU sed 4.9");
        return;
    }
    let cases: usize = std::env::var("RIPGSED_FUZZ_EXEC_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let seed: u64 = std::env::var("RIPGSED_FUZZ_EXEC_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);

    // Both programs run as `sed`, so that messages carry the same prefix.
    let work = temp_dir("w");
    let work_ours = temp_dir("o");
    let bin = temp_dir("b");
    let gnu = bin.join("gnu").join("sed");
    let ours = bin.join("ours").join("sed");
    std::fs::create_dir_all(gnu.parent().unwrap()).unwrap();
    std::fs::create_dir_all(ours.parent().unwrap()).unwrap();
    symlink(Command::new("which").arg("sed").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap(), &gnu).unwrap();
    symlink(env!("CARGO_BIN_EXE_ripgsed"), &ours).unwrap();

    let mut failures = Vec::new();
    for case in 0..cases {
        let extended = rng.chance(3);
        let quiet = rng.chance(3);
        let script = script(&mut rng, extended);
        let data = input(&mut rng);
        let mut args: Vec<String> = Vec::new();
        if quiet {
            args.push("-n".into());
        }
        if extended {
            args.push("-E".into());
        }
        if rng.chance(8) {
            args.push("-s".into());
        }
        if rng.chance(12) {
            args.push("-z".into());
        }
        if rng.chance(25) {
            args.push("--posix".into());
        }
        // Sometimes the input is two files (and sometimes edited in place) instead of stdin.
        let use_files = rng.chance(4);
        let in_place = use_files && rng.chance(3);
        if in_place {
            args.push("-i".into());
        }
        args.push("--".into());
        args.push(script.clone());
        let f2 = input(&mut rng);
        let r_txt = input(&mut rng);
        let files: [(&str, &[u8]); 3] = [("f1", &data), ("f2", &f2), ("r.txt", &r_txt)];
        fresh_dir(&work, &files);
        fresh_dir(&work_ours, &files);
        let stdin: &[u8] = if use_files { b"" } else { &data };
        if use_files {
            args.push("f1".into());
            args.push("f2".into());
        }

        let locale = if rng.chance(3) { "C.utf8" } else { "C" };
        let want = run(&gnu, &args, stdin, &work, locale);
        if want.timed_out {
            continue;
        }
        let got = run(&ours, &args, stdin, &work_ours, locale);
        let same = !got.timed_out
            && want.stdout == got.stdout
            && want.stderr == got.stderr
            && want.status == got.status
            && tree(&work) == tree(&work_ours);
        if !same {
            let (tw, tg) = (tree(&work), tree(&work_ours));
            let mut file_diff = String::new();
            for name in tw.keys().chain(tg.keys()).collect::<std::collections::BTreeSet<_>>() {
                if tw.get(name) != tg.get(name) {
                    file_diff.push_str(&format!(
                        "\n  file {name}: want {:?} got {:?}",
                        tw.get(name).map(|b| String::from_utf8_lossy(b).into_owned()),
                        tg.get(name).map(|b| String::from_utf8_lossy(b).into_owned()),
                    ));
                }
            }
            failures.push(format!(
                "case {case} ({locale}): sed {} {:?}\n  input {:?}\n  want status {:?}: {:?} / {:?}\n  got  status {:?}{}: {:?} / {:?}{file_diff}",
                args[..args.len() - if use_files { 3 } else { 1 }].join(" "),
                script,
                String::from_utf8_lossy(&data),
                want.status,
                String::from_utf8_lossy(&want.stdout),
                String::from_utf8_lossy(&want.stderr),
                got.status,
                if got.timed_out { " (timed out)" } else { "" },
                String::from_utf8_lossy(&got.stdout),
                String::from_utf8_lossy(&got.stderr),
            ));
            if failures.len() >= 8 {
                break;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&work_ours);
    let _ = std::fs::remove_dir_all(&bin);
    assert!(failures.is_empty(), "{} divergences from GNU sed (seed {seed}):\n{}", failures.len(), failures.join("\n"));
}

//! Differential tests: run every fixture against ripgsed and compare the
//! result with the output recorded from GNU sed (see tests/README.md).
//!
//! A fixture whose covered compatibility items are all `unsupported` is
//! pending: it is run and reported, but its failure does not fail the test.
//! A fixture that covers any `partial` or `verified` item must pass.
//!
//! `tests/passing.txt` is a ratchet: every fixture listed there must pass, even
//! if its matrix items are still `unsupported`. Progress is recorded by adding
//! fixtures to it, and a fixture that passes stops being allowed to regress.
//!
//! Environment:
//! - `RIPGSED_FIXTURES=name1,name2` runs only the named fixtures.
//! - `RIPGSED_SHOW_PENDING=1` also lists pending fixtures that already pass.
//! - `RIPGSED_SHOW_FAILURES=1` prints the differences of pending fixtures that fail.
//! - `RIPGSED_UPDATE_PASSING=1` rewrites `tests/passing.txt` with every fixture
//!   that passes (only valid for a full run).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(15);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Compatibility item id -> status, from compat/sed.tsv.
fn load_matrix() -> BTreeMap<String, String> {
    let text = fs::read_to_string(root().join("compat/sed.tsv")).expect("compat/sed.tsv");
    let mut map = BTreeMap::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(cols.len() == 6, "bad matrix row: {line}");
        map.insert(cols[0].to_string(), cols[3].to_string());
    }
    map
}

/// Relative path -> contents for every file under `dir`. A symbolic link is
/// recorded as `\0link:<target>` (never followed, so dangling links are fine),
/// and anything that is neither a file, a directory, nor a link is recorded as
/// `\0special` (never opened, so a FIFO cannot block the runner).
fn read_tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(rel, format!("\0link:{}", target.display()).into_bytes());
            } else if meta.is_dir() {
                walk(base, &path, out);
            } else if meta.is_file() {
                out.insert(rel, fs::read(&path).unwrap_or_default());
            } else {
                out.insert(rel, b"\0special".to_vec());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn copy_tree(from: &Path, to: &Path) {
    for (rel, bytes) in read_tree(from) {
        let dest = to.join(&rel);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        if let Some(target) = bytes.strip_prefix(b"\0link:") {
            symlink(String::from_utf8_lossy(target).to_string(), dest).unwrap();
        } else {
            fs::write(dest, bytes).unwrap();
        }
    }
}

struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: i32,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn run_fixture(dir: &Path, scratch: &Path) -> Result<Outcome, String> {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let work = scratch.join(&name);
    let cwd = work.join("cwd");
    let bin = work.join("bin");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&bin).unwrap();
    if dir.join("files").is_dir() {
        copy_tree(&dir.join("files"), &cwd);
    }
    // Run as plain `sed` so that messages carry the same `sed:` prefix as GNU sed.
    symlink(env!("CARGO_BIN_EXE_ripgsed"), bin.join("sed")).unwrap();

    let mut cmd = Command::new("sh");
    cmd.arg(dir.join("cmd"))
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", "/nonexistent")
        .env("LC_ALL", "C");
    if let Ok(text) = fs::read_to_string(dir.join("env")) {
        for line in text.lines().filter(|l| !l.is_empty()) {
            let (k, v) = line.split_once('=').ok_or(format!("{name}: bad env line {line:?}"))?;
            cmd.env(k, v);
        }
    }
    cmd.stdin(match fs::File::open(dir.join("stdin")) {
        Ok(f) => Stdio::from(f),
        Err(_) => Stdio::null(),
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("{name}: spawn: {e}"))?;
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        out_pipe.read_to_end(&mut b).ok();
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        err_pipe.read_to_end(&mut b).ok();
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s.code().unwrap_or(-1);
        }
        if start.elapsed() > TIMEOUT {
            child.kill().ok();
            child.wait().ok();
            return Err(format!("{name}: timed out after {TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(Outcome {
        stdout: out_t.join().unwrap(),
        stderr: err_t.join().unwrap(),
        status,
        files: read_tree(&cwd),
    })
}

fn expected(dir: &Path) -> Outcome {
    let e = dir.join("expected");
    let status = fs::read_to_string(e.join("status"))
        .map(|s| s.trim().parse().expect("expected/status"))
        .unwrap_or(0);
    let files = if e.join("files").is_dir() {
        read_tree(&e.join("files"))
    } else {
        read_tree(&dir.join("files"))
    };
    Outcome {
        stdout: fs::read(e.join("stdout")).unwrap_or_default(),
        stderr: fs::read(e.join("stderr")).unwrap_or_default(),
        status,
        files,
    }
}

fn show(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() > 400 { format!("{:?}…", &s[..400]) } else { format!("{s:?}") }
}

fn compare(want: &Outcome, got: &Outcome) -> Vec<String> {
    let mut diffs = Vec::new();
    if want.stdout != got.stdout {
        diffs.push(format!("stdout\n    want {}\n    got  {}", show(&want.stdout), show(&got.stdout)));
    }
    if want.stderr != got.stderr {
        diffs.push(format!("stderr\n    want {}\n    got  {}", show(&want.stderr), show(&got.stderr)));
    }
    if want.status != got.status {
        diffs.push(format!("status want {} got {}", want.status, got.status));
    }
    if want.files != got.files {
        let names: BTreeSet<_> = want.files.keys().chain(got.files.keys()).collect();
        for n in names {
            if want.files.get(n) != got.files.get(n) {
                diffs.push(format!(
                    "file {}\n    want {:?}\n    got  {:?}",
                    n.display(),
                    want.files.get(n).map(|b| show(b)),
                    got.files.get(n).map(|b| show(b)),
                ));
            }
        }
    }
    diffs
}

enum Verdict {
    Pass,
    PendingPass,
    PendingFail(String),
    Fail(String),
}

fn load_ratchet() -> BTreeSet<String> {
    fs::read_to_string(root().join("tests/passing.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn evaluate(
    dir: &Path,
    scratch: &Path,
    matrix: &BTreeMap<String, String>,
    ratchet: &BTreeSet<String>,
) -> Verdict {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let covers = fs::read_to_string(dir.join("covers")).unwrap_or_default();
    let ids: Vec<&str> = covers.split_whitespace().collect();
    if ids.is_empty() {
        return Verdict::Fail(format!("{name}: `covers` is empty"));
    }
    let mut pending = !ratchet.contains(&name);
    for id in &ids {
        match matrix.get(*id).map(String::as_str) {
            None => return Verdict::Fail(format!("{name}: unknown compatibility item {id}")),
            Some("unsupported") => {}
            Some(_) => pending = false,
        }
    }
    let diffs = match run_fixture(dir, scratch) {
        Ok(got) => compare(&expected(dir), &got),
        Err(e) => vec![e],
    };
    match (diffs.is_empty(), pending) {
        (true, false) => Verdict::Pass,
        (true, true) => Verdict::PendingPass,
        (false, true) => Verdict::PendingFail(format!("{name}\n  {}", diffs.join("\n  "))),
        (false, false) => Verdict::Fail(format!("{name}\n  {}", diffs.join("\n  "))),
    }
}

#[test]
fn fixtures_match_gnu_sed() {
    use std::sync::{Arc, Mutex};

    let matrix = Arc::new(load_matrix());
    let ratchet = Arc::new(load_ratchet());
    let only: Option<Vec<String>> = std::env::var("RIPGSED_FIXTURES")
        .ok()
        .map(|v| v.split(',').map(str::to_string).collect());
    let show_pending = std::env::var("RIPGSED_SHOW_PENDING").is_ok_and(|v| v == "1");
    let show_failures = std::env::var("RIPGSED_SHOW_FAILURES").is_ok_and(|v| v == "1");

    let mut dirs: Vec<PathBuf> = fs::read_dir(root().join("tests/fixtures"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            only.as_ref().is_none_or(|o| o.contains(&name))
        })
        .collect();
    dirs.sort();

    let scratch = std::env::temp_dir().join(format!("ripgsed-differential-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();

    let queue = Arc::new(Mutex::new(dirs.into_iter()));
    let results: Arc<Mutex<Vec<(String, Verdict)>>> = Arc::new(Mutex::new(Vec::new()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let (queue, results, matrix, ratchet, scratch) =
                (queue.clone(), results.clone(), matrix.clone(), ratchet.clone(), scratch.clone());
            std::thread::spawn(move || {
                loop {
                    let next = queue.lock().unwrap().next();
                    let Some(dir) = next else { break };
                    let name = dir.file_name().unwrap().to_string_lossy().to_string();
                    let verdict = evaluate(&dir, &scratch, &matrix, &ratchet);
                    results.lock().unwrap().push((name, verdict));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let _ = fs::remove_dir_all(&scratch);

    let mut results = Arc::try_unwrap(results).ok().unwrap().into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let (mut passed, mut pending_ok, mut pending_fail) = (0, 0, 0);
    let mut failures = Vec::new();
    let mut promotable = Vec::new();
    let mut all_passing = Vec::new();
    for (name, verdict) in results {
        match verdict {
            Verdict::Pass => {
                passed += 1;
                all_passing.push(name);
            }
            Verdict::PendingPass => {
                pending_ok += 1;
                all_passing.push(name.clone());
                promotable.push(name);
            }
            Verdict::PendingFail(msg) => {
                pending_fail += 1;
                if show_failures {
                    eprintln!("pending failure: {msg}");
                }
            }
            Verdict::Fail(msg) => failures.push(msg),
        }
    }

    eprintln!(
        "fixtures: {passed} passed, {} failed, {pending_ok} pending but passing, {pending_fail} pending",
        failures.len()
    );
    if show_pending && !promotable.is_empty() {
        eprintln!("pending fixtures that already pass: {}", promotable.join(", "));
    }
    if std::env::var("RIPGSED_UPDATE_PASSING").is_ok_and(|v| v == "1") {
        assert!(only.is_none(), "RIPGSED_UPDATE_PASSING needs a full run");
        let mut text = String::from("# Fixtures that must pass. Updated with RIPGSED_UPDATE_PASSING=1 cargo test.\n");
        for name in &all_passing {
            text.push_str(name);
            text.push('\n');
        }
        fs::write(root().join("tests/passing.txt"), text).unwrap();
    }
    assert!(failures.is_empty(), "fixture failures:\n{}", failures.join("\n"));
}

/// Every `verified` matrix item must be covered by at least one fixture.
#[test]
fn verified_items_have_fixtures() {
    let matrix = load_matrix();
    let mut covered = BTreeSet::new();
    for entry in fs::read_dir(root().join("tests/fixtures")).unwrap() {
        let dir = entry.unwrap().path();
        if let Ok(text) = fs::read_to_string(dir.join("covers")) {
            covered.extend(text.split_whitespace().map(str::to_string));
        }
    }
    let missing: Vec<_> = matrix
        .iter()
        .filter(|(id, status)| status.as_str() == "verified" && !covered.contains(*id))
        .map(|(id, _)| id.as_str())
        .collect();
    assert!(missing.is_empty(), "verified items without fixtures: {missing:?}");
}

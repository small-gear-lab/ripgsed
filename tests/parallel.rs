//! The parallel executor must give byte-for-byte the output of the sequential run, whatever
//! the chunk size and the number of workers: chunk boundaries inside and between files,
//! records without a final terminator, empty files, and line numbers across chunks.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

fn run(jobs: &str, block: &str, args: &[&str], files: &[&std::path::Path]) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    let out = Command::new(BIN)
        .args(args)
        .args(files)
        .env("RIPGSED_JOBS", jobs)
        .env("RIPGSED_BLOCK", block)
        .env("LC_ALL", "C")
        .output()
        .expect("run ripgsed");
    (out.stdout, out.stderr, out.status.code())
}

fn lines(n: usize, tail_newline: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 1..=n {
        out.extend_from_slice(format!("line {i} alpha beta {}", i * 7 % 13).as_bytes());
        if i < n || tail_newline {
            out.push(b'\n');
        }
    }
    out
}

#[test]
fn parallel_output_equals_sequential_output() {
    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let empty = dir.join("empty.txt");
    std::fs::write(&a, lines(500, false)).unwrap();
    std::fs::write(&b, lines(37, true)).unwrap();
    std::fs::write(&empty, b"").unwrap();

    let scripts: &[&[&str]] = &[
        &["s/alpha/X/g"],
        &["-n", "/7$/p"],
        &["/line 1/d"],
        &["="],
        &["-n", "5~7{=;p}"],
        &["y/abc/xyz/;s/[0-9]\\+/<&>/2"],
        &["a added"],
        &["i inserted"],
        &["-E", "s/(a)(l)/\\2\\1/;t;s/$/ no/"],
        &["-z", "s/\\n/|/g"],
        &["l 20"],
        &["$d"],
        &["F"],
        &["-n", "10,20p"],
        &["-n", "2,5p;300,~7p"],
        &["/line 3/,/line 5/d"],
        &["-n", "/line 10 /,/line 400 /p"],
        &["/9 /,/zzz/s/alpha/X/"],
        &["/7$/,+2s/a/b/"],
        &["2~50,+3d"],
        &["/line 2/,$d"],
        &["5,3p"],
        &["/line 4[0-9] /,/line 4[0-9] /!s/a/b/"],
        &["0,/alpha/s/line/L/"],
        &["20,30c\\changed"],
        &["/line 1[0-9] /,/line 3[0-9] /{s/a/b/;/line 2/,/line 22 /d}"],
        &["/alpha/s//X/g"],
        &["s/line/L/;s//M/2"],
        &["/[0-9]$/!s/a/b/g"],
        &["-n", "/beta/{s/a/b/;s//c/;p}"],
        &["-n", "2{F;p}"],
        &["$!d"],
        &["-n", "$p"],
        &["$s/$/ END/"],
        &["$!s/alpha/X/;$a last"],
        &["-n", "$!{/7$/p};${=;p}"],
        &["$q5"],
        &["-z", "$d"],
    ];
    for script in scripts {
        let mut args: Vec<&str> = script[..script.len() - 1].to_vec();
        args.push(script[script.len() - 1]);
        for files in [vec![a.as_path()], vec![a.as_path(), b.as_path()], vec![a.as_path(), empty.as_path(), b.as_path()], vec![b.as_path(), a.as_path()], vec![a.as_path(), empty.as_path()], vec![b.as_path(), empty.as_path(), empty.as_path()], vec![empty.as_path(), a.as_path(), empty.as_path()]] {
            let want = run("1", "1048576", &args, &files);
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096"), ("5", "100000")] {
                let got = run(jobs, block, &args, &files);
                assert_eq!(got, want, "script {args:?} files {files:?} jobs {jobs} block {block}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_falls_back_to_the_sequential_messages() {
    let (out, err, status) = run("4", "16", &["s/a/b/", "/nonexistent/file"], &[]);
    assert!(out.is_empty());
    assert_eq!(String::from_utf8_lossy(&err), "ripgsed: can't read /nonexistent/file: No such file or directory\n");
    assert_eq!(status, Some(2));
}

/// A reader that stops after the first bytes (as `head -c` does) must end the whole parallel run:
/// the worker threads and the reader notice that the writer is gone and the process exits.
#[test]
fn parallel_run_ends_when_the_output_is_closed() {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-close-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let big = dir.join("big.txt");
    std::fs::write(&big, lines(200_000, true)).unwrap();

    let mut child = Command::new(BIN)
        .args(["s/alpha/X/g"])
        .arg(&big)
        .env("RIPGSED_JOBS", "4")
        .env("RIPGSED_BLOCK", "4096")
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run ripgsed");
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 16];
    stdout.read_exact(&mut first).unwrap();
    drop(stdout);

    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            child.kill().ok();
            child.wait().ok();
            panic!("the run did not end after its output was closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `q` and `Q` stop at the first record that reaches them, whatever chunk it is in: the output
/// before it and the exit status are those of the sequential run, and the chunks after it are dropped.
#[test]
fn quit_gives_the_sequential_output_and_status() {
    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-quit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let empty = dir.join("empty.txt");
    std::fs::write(&a, lines(500, false)).unwrap();
    std::fs::write(&b, lines(37, true)).unwrap();
    std::fs::write(&empty, b"").unwrap();

    let scripts: &[&[&str]] = &[
        &["5q"],
        &["300q7"],
        &["300Q7"],
        &["/line 250 /q3"],
        &["-n", "/line 77 /{p;q}"],
        &["s/alpha/X/;/line 4[0-9] /q"],
        &["/line 4[0-9] /q5;/line 2[0-9] /q9"],
        &["500q"],
        &["501q4"],
        &["2i before\n2a after\n2q"],
        &["37{s/$/!/;q}"],
        &["-z", "2q"],
    ];
    for args in scripts {
        for files in [vec![a.as_path()], vec![a.as_path(), b.as_path()], vec![a.as_path(), empty.as_path(), b.as_path()], vec![b.as_path(), a.as_path()], vec![a.as_path(), empty.as_path()], vec![b.as_path(), empty.as_path(), empty.as_path()], vec![empty.as_path(), a.as_path(), empty.as_path()]] {
            let want = run("1", "1048576", args, &files);
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096"), ("5", "100000")] {
                let got = run(jobs, block, args, &files);
                assert_eq!(got, want, "script {args:?} files {files:?} jobs {jobs} block {block}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read error in a chunk after the one where `q` happens is never reported.
#[test]
fn quit_before_a_later_read_error_is_not_an_error() {
    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-quit-err-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    std::fs::write(&a, lines(500, true)).unwrap();
    let missing = dir.join("missing.txt");
    let want = run("1", "1048576", &["3q"], &[a.as_path(), missing.as_path()]);
    assert_eq!(want.2, Some(0));
    // (A named file that cannot be read keeps the run sequential, so this is the same run twice.)
    assert_eq!(run("4", "64", &["3q"], &[a.as_path(), missing.as_path()]), want);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Like GNU sed, a run that quits leaves a seekable standard input just after the last record it used,
/// so the next process reads the rest.
#[test]
fn quit_leaves_standard_input_after_the_last_record_used() {
    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-quit-stdin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    std::fs::write(&a, lines(500, true)).unwrap();
    for script in ["5q", "300q", "300Q", "/line 123 /q"] {
        let command = format!("'{BIN}' '{script}'; echo ---; cat");
        let outputs: Vec<Vec<u8>> = [("1", "1048576"), ("4", "64"), ("3", "1000"), ("8", "4096")]
            .iter()
            .map(|(jobs, block)| {
                let out = Command::new("sh")
                    .args(["-c", &command])
                    .stdin(std::fs::File::open(&a).unwrap())
                    .env("RIPGSED_JOBS", jobs)
                    .env("RIPGSED_BLOCK", block)
                    .env("LC_ALL", "C")
                    .output()
                    .expect("run sh");
                out.stdout
            })
            .collect();
        for got in &outputs[1..] {
            assert_eq!(got, &outputs[0], "script {script}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files written by `w`, `W` and `s///w` get the bytes the sequential run gives them, in the same order.
#[test]
fn files_written_are_the_same_as_in_the_sequential_run() {
    let dir = std::env::temp_dir().join(format!("ripgsed-parallel-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let empty = dir.join("empty.txt");
    let out1 = dir.join("out1.txt");
    let out2 = dir.join("out2.txt");
    std::fs::write(&a, lines(500, false)).unwrap();
    std::fs::write(&b, lines(37, true)).unwrap();
    std::fs::write(&empty, b"").unwrap();
    let (o1, o2) = (out1.display().to_string(), out2.display().to_string());

    let scripts: Vec<Vec<String>> = vec![
        vec!["-n".into(), format!("/7$/w {o1}")],
        vec![format!("/1 /w {o1}")],
        vec!["-n".into(), format!("/line 4/W {o1}")],
        vec![format!("s/alpha/X/w {o1}")],
        vec![format!("s/line 1/L/gw {o1}\n/9 /w {o2}\n/8 /w {o1}")],
        vec![format!("/[0-9]$/{{p;w {o1}\n}}")],
        vec![format!("$w {o1}")],
        vec![format!("/line 2/,/line 3/w {o1}")],
        vec![format!("{}q;w {o1}", 120)],
    ];
    let read = |p: &std::path::Path| std::fs::read(p).unwrap_or_default();
    for script in &scripts {
        let args: Vec<&str> = script.iter().map(String::as_str).collect();
        for files in [vec![a.as_path()], vec![a.as_path(), b.as_path()], vec![a.as_path(), empty.as_path(), b.as_path()], vec![b.as_path(), a.as_path()]] {
            let _ = (std::fs::remove_file(&out1), std::fs::remove_file(&out2));
            let want = (run("1", "1048576", &args, &files), read(&out1), read(&out2));
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096")] {
                let _ = (std::fs::remove_file(&out1), std::fs::remove_file(&out2));
                let got = (run(jobs, block, &args, &files), read(&out1), read(&out2));
                assert_eq!(got, want, "script {args:?} files {files:?} jobs {jobs} block {block}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

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
    ];
    for script in scripts {
        let mut args: Vec<&str> = script[..script.len() - 1].to_vec();
        args.push(script[script.len() - 1]);
        for files in [vec![a.as_path()], vec![a.as_path(), b.as_path()], vec![a.as_path(), empty.as_path(), b.as_path()], vec![b.as_path(), a.as_path()]] {
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

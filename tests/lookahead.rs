//! The record that is read ahead to know whether a line is the last (`$`) is held where it was read, and moves to a
//! queue when more than one record is looked at (`$-N`). These scripts switch between the two, on a file, on several
//! files, on a pipe, and with `-s`; the expected output is worked out by hand and is the same for GNU sed where it
//! accepts the script (the ones with `$-1` are an extension).

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

fn run_files(args: &[&str], files: &[&str]) -> String {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ripgsed-lookahead-{}-{call}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let path = dir.join(format!("f{i}"));
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect();
    let out = Command::new(BIN)
        .args(args)
        .args(&paths)
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1")
        .env("RIPGSED_TAIL", "0")
        .env("RIPGSED_HEAD", "0")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn run_pipe(args: &[&str], input: &str) -> String {
    let mut child = Command::new(BIN)
        .args(args)
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1")
        .env("RIPGSED_TAIL", "0")
        .env("RIPGSED_HEAD", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const FIVE: &str = "1\n2\n3\n4\n5\n";

#[test]
fn the_last_line_and_the_one_before_it_in_one_script() {
    for script in ["$p;$-1p", "$-1p;$p"] {
        assert_eq!(run_files(&["-n", script], &[FIVE]), "4\n5\n", "{script}");
        assert_eq!(run_pipe(&["-n", script], FIVE), "4\n5\n", "{script} pipe");
    }
}

#[test]
fn the_last_line_with_n_and_big_n_reading_on() {
    // `n` reads the next line, so only the odd lines are looked at, and 5 is the last of them.
    assert_eq!(run_files(&["-n", "$p;n"], &[FIVE]), "5\n");
    assert_eq!(run_pipe(&["-n", "$p;n"], FIVE), "5\n");
    // Four lines: the cycles are 1+2 and 3+4, and no cycle starts on the last line.
    assert_eq!(run_files(&["-n", "$p;n"], &["1\n2\n3\n4\n"]), "");
    // `$-1` joins the two last lines.
    assert_eq!(run_files(&["-n", "$-1{N;p}"], &[FIVE]), "4\n5\n");
    assert_eq!(run_files(&["-n", "$!N;$p"], &[FIVE]), "5\n");
}

#[test]
fn the_last_line_of_each_file_with_s_and_of_the_input_without_it() {
    let (a, b) = ("1\n2\n3\n", "4\n5\n");
    assert_eq!(run_files(&["-s", "-n", "$p;$-1p"], &[a, b]), "2\n3\n4\n5\n");
    assert_eq!(run_files(&["-s", "-n", "$p"], &[a, b]), "3\n5\n");
    assert_eq!(run_files(&["-n", "$p"], &[a, b]), "5\n");
    assert_eq!(run_files(&["-n", "$-1p;$p"], &[a, b]), "4\n5\n");
    // The last file is empty: the last line of the input is the last of the first.
    assert_eq!(run_files(&["-n", "$p"], &[a, ""]), "3\n");
    assert_eq!(run_files(&["-n", "$p"], &["", a, ""]), "3\n");
}

#[test]
fn a_last_line_without_a_terminator_is_kept_as_it_is() {
    assert_eq!(run_files(&["-n", "$p"], &["1\n2"]), "2");
    assert_eq!(run_pipe(&["-n", "$p"], "1\n2"), "2");
    assert_eq!(run_files(&["-n", "$-1p;$p"], &["1\n2\n3"]), "2\n3");
    assert_eq!(run_files(&["-s", "-n", "$p"], &["1\n2", "3\n4"]), "2\n4");
}

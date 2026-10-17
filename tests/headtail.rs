//! A script that does nothing between its first lines and its last (`analysis::head_tail_plan`) reads those two ends of
//! a file and nothing else. The answer must be that of the run that reads it all, and so must what is left of
//! standard input.
//!
//! Every program that runs here has its output collected, so what it can write is small, and the ones that read a
//! file of gigabytes run under `ulimit -v` (see AGENTS.md).

use std::io::{Seek, SeekFrom, Write};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

type Run = (Vec<u8>, Vec<u8>, Option<i32>);

/// A run with the reading of the two ends as it is (`plan`) or turned off.
fn run(plan: bool, args: &[&str], file: &std::path::Path) -> Run {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .arg(file)
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1");
    if plan {
        command.env("RIPGSED_TAIL_MIN_BYTES", "0");
    } else {
        command.env("RIPGSED_TAIL", "0").env("RIPGSED_HEAD", "0");
    }
    let out = command.output().expect("run ripgsed");
    (out.stdout, out.stderr, out.status.code())
}

fn lines(count: usize, last_terminated: bool, separator: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 1..=count {
        out.extend_from_slice(format!("line {i} alpha").as_bytes());
        if i < count || last_terminated {
            out.push(separator);
        }
    }
    out
}

#[test]
fn the_two_ends_give_the_answer_of_the_whole_file() {
    let dir = std::env::temp_dir().join(format!("ripgsed-headtail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    // The ones that read the two ends, and the ones that must not.
    let scripts: &[&[&str]] = &[
        &["-n", "1p;$p"],
        &["-n", "1,2p;$p"],
        &["-n", "2p;$-1,$p"],
        &["-n", "1,2p;$-2,$p"],
        &["-n", "3{p;p};$-1p"],
        &["-n", "2{s/a/b/;p};$p"],
        &["-n", "1p;$,-2p"],
        &["-n", "/1/d;1p;$p"],
        &["-n", "1p;$p;$w out.txt"],
        &["-z", "-n", "1p;$p"],
        &["-z", "-n", "1,2p;$-1,$p"],
        &["1,2!d;$!d"],
        &["1p;$p"],
        &["-n", "1p;$="],
        &["-n", "2,$p"],
        &["-n", "1p;/3/p;$p"],
        &["-n", "1p;$d"],
        &["-n", "0~2p;$p"],
        &["-n", "1{N;p};$p"],
        &["-n", "1p;$p;q"],
        &["-s", "-n", "1p;$p"],
    ];
    for separator in [b'\n', 0u8] {
        for count in 0..=15usize {
            for terminated in [true, false] {
                std::fs::write(&file, lines(count, terminated, separator)).unwrap();
                for script in scripts {
                    // (-z reads NUL-separated records: the files made with NUL are for it.)
                    if (separator == 0) != script.contains(&"-z") {
                        continue;
                    }
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let want = run(false, script, &file);
                    let want_out = std::fs::read(dir.join("out.txt")).ok();
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let got = run(true, script, &file);
                    let got_out = std::fs::read(dir.join("out.txt")).ok();
                    assert_eq!(
                        (got, got_out),
                        (want, want_out),
                        "script {script:?} on {count} records, last terminated {terminated}, separator {separator}"
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file with two lines, a hole of 6 GiB (one record of zeros) and two lines: the memory that the run is allowed does
/// not hold the hole, so a run that reads between the ends fails. (What the run can write is a line or two.)
fn file_with_a_hole(path: &std::path::Path) {
    let size: u64 = 6 << 30;
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"a\nb\n").unwrap();
    f.set_len(size).unwrap();
    f.seek(SeekFrom::Start(size - 5)).unwrap();
    f.write_all(b"\nc\nd\n").unwrap();
}

#[test]
fn only_the_two_ends_of_a_huge_file_are_read() {
    let dir = std::env::temp_dir().join(format!("ripgsed-headtail-sparse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    file_with_a_hole(&file);
    for (args, want) in [
        (&["-n", "1p;$p"][..], "a\nd\n"),
        (&["-n", "1,2p;$-1,$p"][..], "a\nb\nc\nd\n"),
        (&["-n", "2p;$p"][..], "b\nd\n"),
    ] {
        // 1.5 GB of address space: the hole, as one record, does not fit.
        let out = Command::new("sh")
            .args(["-c", "ulimit -v 1500000; exec \"$0\" \"$@\""])
            .arg(BIN)
            .args(args)
            .arg(&file)
            .env("LC_ALL", "C")
            .output()
            .expect("run sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            want,
            "args {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(0), "args {args:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What `sh` shows of standard input that is a file: the output of the run, and what is left of the file for the next
/// command, of which at most 100,000 bytes are shown (a run that read to the end leaves nothing).
fn through_stdin(
    plan: bool,
    before: &str,
    args: &[&str],
    file: &std::path::Path,
) -> (Vec<u8>, Option<i32>) {
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            &format!("{before} \"$0\" \"$@\"; echo \"--- left:\"; head -c 100000"),
        ])
        .arg(BIN)
        .args(args)
        .stdin(std::fs::File::open(file).unwrap())
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1");
    if plan {
        command.env("RIPGSED_TAIL_MIN_BYTES", "0");
    } else {
        command.env("RIPGSED_TAIL", "0").env("RIPGSED_HEAD", "0");
    }
    let out = command.output().expect("run sh");
    (out.stdout, out.status.code())
}

#[test]
fn standard_input_that_is_a_file_is_left_at_its_end_as_before() {
    let dir = std::env::temp_dir().join(format!("ripgsed-headtail-stdin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    let scripts: &[&[&str]] = &[
        &["-n", "1p;$p"],
        &["-n", "2p;$-1,$p"],
        &["-n", "1,2p;$p"],
        &["-n", "p"],
    ];
    for count in [0usize, 1, 2, 3, 6, 11] {
        for terminated in [true, false] {
            std::fs::write(&file, lines(count, terminated, b'\n')).unwrap();
            // From the start of the file, and from the second line (the first of the records is then the one after it).
            for before in ["", "read first;"] {
                for script in scripts {
                    assert_eq!(
                        through_stdin(true, before, script, &file),
                        through_stdin(false, before, script, &file),
                        "script {script:?} on {count} records, last terminated {terminated}, after `{before}`"
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_two_ends_of_a_huge_file_on_standard_input_are_read() {
    let dir = std::env::temp_dir().join(format!(
        "ripgsed-headtail-stdin-sparse-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    file_with_a_hole(&file);
    // (What is left of the file is shown with `head -c`: if the run did not leave standard input at the end, the next
    // command would otherwise write gigabytes into the memory of this test.)
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -v 1500000; \"$0\" -n '1p;$p'; echo \"--- left:\"; head -c 100",
        ])
        .arg(BIN)
        .stdin(std::fs::File::open(&file).unwrap())
        .env("LC_ALL", "C")
        .output()
        .expect("run sh");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "a\nd\n--- left:\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

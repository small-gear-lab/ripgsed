//! A script that does nothing after its last line number (`analysis::head_limit`) stops reading there. The answer
//! must be that of the run that reads to the end, and so must what is left of standard input.
//!
//! Every program that runs here has its output collected, so what it can write is small, and the ones that read a
//! file of gigabytes run under `ulimit -v` (see AGENTS.md).

use std::io::{Seek, SeekFrom, Write};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

type Left = (Vec<u8>, Vec<u8>, Option<i32>);

/// The output, the messages and the status of a run, with the stop at the limit as it is or turned off.
fn run(stop: bool, args: &[&str], files: &[&std::path::Path], dir: &std::path::Path) -> Left {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .args(files)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1");
    if !stop {
        command.env("RIPGSED_HEAD", "0");
    }
    let out = command.output().expect("run ripgsed");
    let stderr =
        String::from_utf8_lossy(&out.stderr).replace(dir.to_string_lossy().as_ref(), "DIR");
    (out.stdout, stderr.into_bytes(), out.status.code())
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
fn the_first_records_give_the_answer_of_the_whole_input() {
    let dir = std::env::temp_dir().join(format!("ripgsed-head-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    // The ones that stop at the limit, and the ones that must not.
    let scripts: &[&[&str]] = &[
        &["-n", "3p"],
        &["-n", "2,4p"],
        &["-n", "4,2p"],
        &["-n", "1p;3p"],
        &["-n", "2{p;q}"],
        &["-n", "3q7"],
        &["3q7"],
        &["1,3!d"],
        &["3!d"],
        &["-n", "2{s/a/b/;p}"],
        &["-n", "2,3p;/1/d"],
        &["-n", "2w out.txt"],
        &["-n", "5{=;p}"],
        &["-z", "-n", "2p"],
        &["-z", "1,2!d"],
        &["-n", "$p"],
        &["3p"],
        &["-n", "/1/p"],
        &["-n", "2,/3/p"],
        &["-n", "0~2p"],
        &["-n", "2{N;p}"],
        &["-n", "2,+1p"],
        &["-s", "-n", "2p"],
    ];
    for separator in [b'\n', 0u8] {
        for (first, second) in [
            (0, 0),
            (1, 0),
            (3, 0),
            (7, 0),
            (12, 0),
            (2, 3),
            (5, 2),
            (0, 6),
            (4, 9),
        ] {
            for terminated in [true, false] {
                std::fs::write(&a, lines(first, true, separator)).unwrap();
                std::fs::write(&b, lines(second, terminated, separator)).unwrap();
                let files: Vec<&std::path::Path> = if second == 0 && first > 0 {
                    vec![a.as_path()]
                } else {
                    vec![a.as_path(), b.as_path()]
                };
                for script in scripts {
                    // (-z reads NUL-separated records: the files made with NUL are for it.)
                    if (separator == 0) != script.contains(&"-z") {
                        continue;
                    }
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let want = run(false, script, &files, &dir);
                    let want_out = std::fs::read(dir.join("out.txt")).ok();
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let got = run(true, script, &files, &dir);
                    let got_out = std::fs::read(dir.join("out.txt")).ok();
                    assert_eq!(
                        (got, got_out),
                        (want, want_out),
                        "script {script:?} on files of {first} and {second} records, last terminated {terminated}, separator {separator}"
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that has its first lines and then a hole of 6 GiB, which is one record of zeros: the memory that the run is
/// allowed does not hold it, so a run that reads on after the last line number it needs fails. (What the run can
/// write is the line or two that it prints.)
#[test]
fn a_huge_file_is_read_only_as_far_as_the_script_needs() {
    let dir = std::env::temp_dir().join(format!("ripgsed-head-sparse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    let mut f = std::fs::File::create(&file).unwrap();
    f.write_all(b"x\ny\nz\n").unwrap();
    f.set_len(6 << 30).unwrap();
    drop(f);
    for (args, want) in [
        (&["-n", "2p"][..], "y\n"),
        (&["-n", "1,2p"][..], "x\ny\n"),
        (&["1,2!d"][..], "x\ny\n"),
        (&["-n", "2{p;q}"][..], "y\n"),
        (&["-n", "1p;3p"][..], "x\nz\n"),
        // (A quit that comes early: the threads would read ahead of it, so it is run on one.)
        (&["1q"][..], "x\n"),
        (&["2q"][..], "x\ny\n"),
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
/// command, of which at most `limit` bytes are shown (a run that read to the end leaves nothing).
fn through_stdin(
    stop: bool,
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
    if !stop {
        command.env("RIPGSED_HEAD", "0");
    }
    let out = command.output().expect("run sh");
    (out.stdout, out.status.code())
}

#[test]
fn standard_input_that_is_a_file_is_left_at_its_end_as_before() {
    let dir = std::env::temp_dir().join(format!("ripgsed-head-stdin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    let scripts: &[&[&str]] = &[
        &["-n", "3p"],
        &["1,2!d"],
        &["-n", "2,4p"],
        &["-n", "2{p;q}"],
        &["-n", "p"],
    ];
    for count in [0usize, 1, 2, 3, 5, 9] {
        for terminated in [true, false] {
            std::fs::write(&file, lines(count, terminated, b'\n')).unwrap();
            // From the start of the file, and from the second line.
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
fn a_huge_file_on_standard_input_is_read_only_as_far_as_the_script_needs() {
    let dir =
        std::env::temp_dir().join(format!("ripgsed-head-stdin-sparse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    let mut f = std::fs::File::create(&file).unwrap();
    f.write_all(b"x\ny\nz\n").unwrap();
    f.set_len(6 << 30).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    drop(f);
    // (What is left of the file is shown with `head -c`: if the run did not leave standard input at the end, the next
    // command would otherwise write gigabytes into the memory of this test.)
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -v 1500000; \"$0\" -n 2p; echo \"--- left:\"; head -c 100",
        ])
        .arg(BIN)
        .stdin(std::fs::File::open(&file).unwrap())
        .env("LC_ALL", "C")
        .output()
        .expect("run sh");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "y\n--- left:\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Where the stop does not apply, the run is the usual one and says what it always said.
#[test]
fn what_cannot_be_stopped_early_is_read_as_before() {
    let dir = std::env::temp_dir().join(format!("ripgsed-head-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (a, missing) = (dir.join("a.txt"), dir.join("missing.txt"));
    std::fs::write(&a, lines(6, true, b'\n')).unwrap();
    // A file after the one that holds the line is not there: the usual run says so, and has the status for it.
    for args in [&["-n", "2p"][..], &["1,2!d"][..]] {
        assert_eq!(
            run(true, args, &[&a, &missing], &dir),
            run(false, args, &[&a, &missing], &dir),
            "{args:?}"
        );
    }
    let (_, message, status) = run(true, &["-n", "2p"], &[&a, &missing], &dir);
    assert!(
        String::from_utf8_lossy(&message).contains("can't read"),
        "{}",
        String::from_utf8_lossy(&message)
    );
    assert_eq!(status, Some(2));
    // A directory, and a pipe.
    assert_eq!(
        run(true, &["-n", "2p"], &[&dir], &dir),
        run(false, &["-n", "2p"], &[&dir], &dir)
    );
    for stop in [true, false] {
        let mut child = Command::new(BIN)
            .args(["-n", "2p"])
            .env("LC_ALL", "C")
            .env("RIPGSED_HEAD", if stop { "1" } else { "0" })
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("run ripgsed");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&lines(6, true, b'\n'))
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "line 2 alpha\n");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

//! A script that does nothing for the records before the last few (`analysis::tail_window`) runs on the last
//! records of a file, read from its end. The answer must be that of the run on the whole file.

use std::io::{Seek, SeekFrom, Write};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

/// The output, the messages and the status of a run, with the search from the end of the file forced
/// (`forced`: for every file, however small) or turned off.
fn run(forced: bool, args: &[&str], file: &std::path::Path) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .arg(file)
        // (a file that `w` writes is made next to the input, where the test reads it)
        .current_dir(file.parent().expect("a file in a directory"))
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1");
    if forced {
        command.env("RIPGSED_TAIL_MIN_BYTES", "0");
    } else {
        command.env("RIPGSED_TAIL", "0");
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
fn the_last_records_give_the_answer_of_the_whole_file() {
    let dir = std::env::temp_dir().join(format!("ripgsed-tail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    // The ones that the search from the end can take, and the ones that it must leave alone.
    let scripts: &[&[&str]] = &[
        &["-n", "$p"],
        &["$!d"],
        &["-n", "$-2,$p"],
        &["$-2,$!d"],
        &["-n", "$,-3p"],
        &["-n", "$-3,$-1{p;p}"],
        &["$!d;s/a/b/;$a end"],
        &["-n", "$p;$q3"],
        &["$-1!d;$q5"],
        &["-n", "$-9{s/a/b/;p}"],
        &["-n", "$-1,/alpha/p"],
        &["-n", "$-1,+3p"],
        &["-s", "-n", "$p"],
        &["-n", "$p;$w out.txt"],
        &["-z", "-n", "$p"],
        &["-n", "$d"],
        &["-n", "p"],
        &["$d"],
        &["-n", "$p;1p"],
        &["-n", "/1/p;$p"],
        &["-n", "$=;$p"],
        &["-n", "2,3p;$p"],
        &["1!G;h;$!d"],
        &["$!N;P;D"],
        &["-n", "/1/,/3/p;$p"],
        &["-n", "x;$p"],
        &["-n", "$p;b"],
        &["-n", "2q;$p"],
    ];
    for separator in [b'\n', 0u8] {
        for count in 0..=12usize {
            for terminated in [true, false] {
                std::fs::write(&file, lines(count, terminated, separator)).unwrap();
                for script in scripts {
                    let args = script.to_vec();
                    // (-z reads NUL-separated records: the files made with NUL are for it.)
                    if (separator == 0) != args.contains(&"-z") {
                        continue;
                    }
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let want = run(false, &args, &file);
                    let want_out = std::fs::read(dir.join("out.txt")).ok();
                    let _ = std::fs::remove_file(dir.join("out.txt"));
                    let got = run(true, &args, &file);
                    let got_out = std::fs::read(dir.join("out.txt")).ok();
                    assert_eq!(
                        (got, got_out),
                        (want, want_out),
                        "script {args:?} on {count} records, last terminated {terminated}, separator {separator}"
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that is a hole but for its last lines: only a run that reads from the end can do it within the memory
/// that it is allowed (the whole file is one record of 6 GiB), so this is the proof that the search is used, and
/// that it is used by default.
#[test]
fn only_the_end_of_a_huge_file_is_read() {
    let dir = std::env::temp_dir().join(format!("ripgsed-tail-sparse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    let size: u64 = 6 << 30;
    for (end, args, want) in [
        (&b"x\ny\nz\n"[..], &["-n", "$p"][..], "z\n"),
        (&b"x\ny\nz\n"[..], &["$!d"][..], "z\n"),
        (&b"x\ny\nz\n"[..], &["-n", "$-1,$p"][..], "y\nz\n"),
        (&b"x\ny\nz"[..], &["-n", "$p"][..], "z"),
    ] {
        let mut f = std::fs::File::create(&file).unwrap();
        f.set_len(size).unwrap();
        f.seek(SeekFrom::Start(size - end.len() as u64)).unwrap();
        f.write_all(end).unwrap();
        drop(f);
        // 1.5 GB of address space: the file, as one record, does not fit.
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

/// Where the search from the end does not apply, the run is the usual one and says what it always said.
#[test]
fn what_cannot_be_searched_from_the_end_is_read_as_before() {
    let dir = std::env::temp_dir().join(format!("ripgsed-tail-fallback-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, lines(5, true, b'\n')).unwrap();
    std::fs::write(&b, lines(3, true, b'\n')).unwrap();
    let run_with = |args: &[&str],
                    files: &[&std::path::Path],
                    stdin: Option<&std::path::Path>,
                    forced: bool| {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .args(files)
            .env("LC_ALL", "C")
            .env("RIPGSED_JOBS", "1");
        if forced {
            command.env("RIPGSED_TAIL_MIN_BYTES", "0");
        } else {
            command.env("RIPGSED_TAIL", "0");
        }
        if let Some(path) = stdin {
            command.stdin(std::fs::File::open(path).unwrap());
        }
        let out = command.output().expect("run ripgsed");
        (
            out.stdout,
            String::from_utf8_lossy(&out.stderr).replace(dir.to_string_lossy().as_ref(), "DIR"),
            out.status.code(),
        )
    };
    // Two files are one stream of records: the last ones are in the second.
    assert_eq!(
        run_with(&["-n", "$p"], &[&a, &b], None, true),
        run_with(&["-n", "$p"], &[&a, &b], None, false)
    );
    assert_eq!(
        run_with(&["-n", "$p"], &[&a, &b], None, true).0,
        b"line 3 alpha\n"
    );
    // A file that is not there, a directory, and standard input.
    let missing = dir.join("missing.txt");
    assert_eq!(
        run_with(&["-n", "$p"], &[&missing], None, true),
        run_with(&["-n", "$p"], &[&missing], None, false)
    );
    assert_eq!(
        run_with(&["-n", "$p"], &[&dir], None, true),
        run_with(&["-n", "$p"], &[&dir], None, false)
    );
    assert_eq!(
        run_with(&["-n", "$p"], &[], Some(&a), true),
        run_with(&["-n", "$p"], &[], Some(&a), false)
    );
    // An empty file.
    let empty = dir.join("empty.txt");
    std::fs::write(&empty, b"").unwrap();
    assert_eq!(
        run_with(&["-n", "$p"], &[&empty], None, true),
        run_with(&["-n", "$p"], &[&empty], None, false)
    );
    // Editing in place and the trace keep the whole file.
    let _ = std::fs::copy(&a, dir.join("c.txt"));
    let c = dir.join("c.txt");
    assert_eq!(run_with(&["-i", "$!d"], &[&c], None, true).2, Some(0));
    assert_eq!(std::fs::read_to_string(&c).unwrap(), "line 5 alpha\n");
    assert_eq!(
        run_with(&["--debug", "-n", "$p"], &[&a], None, true),
        run_with(&["--debug", "-n", "$p"], &[&a], None, false)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What `sh` shows of standard input that is a file: the output of the run, and what is left of the file for
/// the next command (a run that reads to the end leaves nothing, as one that is cut short would leave something).
fn through_stdin(
    forced: bool,
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
    if forced {
        command.env("RIPGSED_TAIL_MIN_BYTES", "0");
    } else {
        command.env("RIPGSED_TAIL", "0");
    }
    let out = command.output().expect("run sh");
    (out.stdout, out.status.code())
}

#[test]
fn standard_input_that_is_a_file_is_read_from_its_end_too() {
    let dir = std::env::temp_dir().join(format!("ripgsed-tail-stdin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    let scripts: &[&[&str]] = &[
        &["-n", "$p"],
        &["$!d"],
        &["-n", "$-2,$p"],
        &["-n", "$p;$q3"],
        &["-n", "p"],
    ];
    for count in 0..=7usize {
        for terminated in [true, false] {
            std::fs::write(&file, lines(count, terminated, b'\n')).unwrap();
            // From the start of the file, and from the second line (what was read before is not part of it).
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
fn only_the_end_of_a_huge_file_on_standard_input_is_read() {
    // (What is left of the file is shown with `head -c`: if the run did not read to the end, the next command would
    // otherwise write gigabytes into the memory of this test.)
    let dir =
        std::env::temp_dir().join(format!("ripgsed-tail-stdin-sparse-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.txt");
    let size: u64 = 6 << 30;
    let mut f = std::fs::File::create(&file).unwrap();
    f.set_len(size).unwrap();
    f.seek(SeekFrom::Start(size - 6)).unwrap();
    f.write_all(b"x\ny\nz\n").unwrap();
    drop(f);
    let out = Command::new("sh")
        .args([
            "-c",
            "ulimit -v 1500000; \"$0\" -n '$p'; echo \"--- left:\"; head -c 100",
        ])
        .arg(BIN)
        .stdin(std::fs::File::open(&file).unwrap())
        .env("LC_ALL", "C")
        .output()
        .expect("run sh");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "z\n--- left:\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

//! The parallel executor must give byte-for-byte the output of the sequential run, whatever
//! the chunk size and the number of workers: chunk boundaries inside and between files,
//! records without a final terminator, empty files, and line numbers across chunks.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

fn run(
    jobs: &str,
    block: &str,
    args: &[&str],
    files: &[&std::path::Path],
) -> (Vec<u8>, Vec<u8>, Option<i32>) {
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
        for files in [
            vec![a.as_path()],
            vec![a.as_path(), b.as_path()],
            vec![a.as_path(), empty.as_path(), b.as_path()],
            vec![b.as_path(), a.as_path()],
            vec![a.as_path(), empty.as_path()],
            vec![b.as_path(), empty.as_path(), empty.as_path()],
            vec![empty.as_path(), a.as_path(), empty.as_path()],
        ] {
            let want = run("1", "1048576", &args, &files);
            for (jobs, block) in [
                ("2", "1"),
                ("3", "50"),
                ("4", "333"),
                ("8", "4096"),
                ("5", "100000"),
            ] {
                let got = run(jobs, block, &args, &files);
                assert_eq!(
                    got, want,
                    "script {args:?} files {files:?} jobs {jobs} block {block}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_falls_back_to_the_sequential_messages() {
    let (out, err, status) = run("4", "16", &["s/a/b/", "/nonexistent/file"], &[]);
    assert!(out.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&err),
        "ripgsed: can't read /nonexistent/file: No such file or directory\n"
    );
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
        for files in [
            vec![a.as_path()],
            vec![a.as_path(), b.as_path()],
            vec![a.as_path(), empty.as_path(), b.as_path()],
            vec![b.as_path(), a.as_path()],
            vec![a.as_path(), empty.as_path()],
            vec![b.as_path(), empty.as_path(), empty.as_path()],
            vec![empty.as_path(), a.as_path(), empty.as_path()],
        ] {
            let want = run("1", "1048576", args, &files);
            for (jobs, block) in [
                ("2", "1"),
                ("3", "50"),
                ("4", "333"),
                ("8", "4096"),
                ("5", "100000"),
            ] {
                let got = run(jobs, block, args, &files);
                assert_eq!(
                    got, want,
                    "script {args:?} files {files:?} jobs {jobs} block {block}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read error in a chunk after the one where `q` happens is never reported.
#[test]
fn quit_before_a_later_read_error_is_not_an_error() {
    let dir =
        std::env::temp_dir().join(format!("ripgsed-parallel-quit-err-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    std::fs::write(&a, lines(500, true)).unwrap();
    let missing = dir.join("missing.txt");
    let want = run("1", "1048576", &["3q"], &[a.as_path(), missing.as_path()]);
    assert_eq!(want.2, Some(0));
    // (A named file that cannot be read keeps the run sequential, so this is the same run twice.)
    assert_eq!(
        run("4", "64", &["3q"], &[a.as_path(), missing.as_path()]),
        want
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Like GNU sed, a run that quits leaves a seekable standard input just after the last record it used,
/// so the next process reads the rest.
#[test]
fn quit_leaves_standard_input_after_the_last_record_used() {
    let dir = std::env::temp_dir().join(format!(
        "ripgsed-parallel-quit-stdin-{}",
        std::process::id()
    ));
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
        for files in [
            vec![a.as_path()],
            vec![a.as_path(), b.as_path()],
            vec![a.as_path(), empty.as_path(), b.as_path()],
            vec![b.as_path(), a.as_path()],
        ] {
            let _ = (std::fs::remove_file(&out1), std::fs::remove_file(&out2));
            let want = (run("1", "1048576", &args, &files), read(&out1), read(&out2));
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096")] {
                let _ = (std::fs::remove_file(&out1), std::fs::remove_file(&out2));
                let got = (run(jobs, block, &args, &files), read(&out1), read(&out2));
                assert_eq!(
                    got, want,
                    "script {args:?} files {files:?} jobs {jobs} block {block}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// With `-s` each file is a stream of its own: its line numbers, its ranges and its `$` are the file's, whatever
/// chunk the line is in.
#[test]
fn separate_files_are_streams_of_their_own_in_parallel_too() {
    let dir =
        std::env::temp_dir().join(format!("ripgsed-parallel-separate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let empty = dir.join("empty.txt");
    std::fs::write(&a, lines(500, false)).unwrap();
    std::fs::write(&b, lines(37, true)).unwrap();
    std::fs::write(&empty, b"").unwrap();

    let scripts: &[&[&str]] = &[
        &["-s", "$d"],
        &["-s", "-n", "$p"],
        &["-s", "1d"],
        &["-s", "-n", "2,4p;$-1,$p"],
        &["-s", "-n", "1F;$F"],
        &["-s", "-n", "$=;1="],
        &["-s", "/line 3/,/line 5/d"],
        &["-s", "-n", "/line 30 /,/line 2[0-9][0-9] /p"],
        &["-s", "/line 3[0-9] /,$d"],
        &["-s", "0~100s/alpha/X/"],
        &["-s", "$,-2d"],
        &["-s", "$!s/a/b/;$a end"],
        &["-s", "-n", "/line 1[0-9] /,+3p"],
        &["-s", "20q3"],
        &["-s", "-n", "$-2,$p;4q"],
    ];
    for args in scripts {
        for files in [
            vec![a.as_path(), b.as_path()],
            vec![b.as_path(), a.as_path()],
            vec![a.as_path(), empty.as_path(), b.as_path(), empty.as_path()],
            vec![b.as_path(), b.as_path(), a.as_path()],
        ] {
            let want = run("1", "1048576", args, &files);
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096")] {
                let got = run(jobs, block, args, &files);
                assert_eq!(
                    got, want,
                    "script {args:?} files {files:?} jobs {jobs} block {block}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What `-i` leaves behind: the standard output, the status, and for every name the bytes and the mode of the
/// file (a file that is not there is `None`).
type Left = (
    (Vec<u8>, Vec<u8>, Option<i32>),
    Vec<(String, Option<(Vec<u8>, u32)>)>,
);

fn edit_in_place(jobs: &str, block: &str, args: &[&str], names: &[&str], extra: &[&str]) -> Left {
    use std::os::unix::fs::{PermissionsExt, symlink};
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ripgsed-parallel-inplace-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), lines(500, false)).unwrap();
    std::fs::write(dir.join("b.txt"), lines(37, true)).unwrap();
    std::fs::write(dir.join("c.txt"), lines(120, true)).unwrap();
    std::fs::write(dir.join("empty.txt"), b"").unwrap();
    std::fs::set_permissions(dir.join("b.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
    symlink("c.txt", dir.join("link.txt")).unwrap();
    let out = Command::new(BIN)
        .args(args)
        .args(names)
        .env("RIPGSED_JOBS", jobs)
        .env("RIPGSED_BLOCK", block)
        .env("LC_ALL", "C")
        .current_dir(&dir)
        .output()
        .expect("run ripgsed");
    let mut seen: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    seen.sort();
    let left = seen
        .into_iter()
        .map(|name| {
            let path = dir.join(&name);
            let state = std::fs::symlink_metadata(&path).ok().map(|m| {
                let bytes = if m.file_type().is_symlink() {
                    std::fs::read_link(&path)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                        .into_bytes()
                } else {
                    std::fs::read(&path).unwrap_or_default()
                };
                (bytes, m.permissions().mode())
            });
            (name, state)
        })
        .collect();
    let _ = extra;
    let _ = std::fs::remove_dir_all(&dir);
    // (The messages name the directory the files were in: it is not the same twice.)
    let stderr = String::from_utf8_lossy(&out.stderr)
        .replace(dir.to_string_lossy().as_ref(), "DIR")
        .into_bytes();
    ((out.stdout, stderr, out.status.code()), left)
}

#[test]
fn files_edited_in_place_come_out_as_in_the_sequential_run() {
    let name_sets: &[&[&str]] = &[
        &["a.txt", "b.txt", "c.txt"],
        &["c.txt", "empty.txt", "a.txt"],
        &["link.txt", "a.txt"],
    ];
    let scripts: &[&[&str]] = &[
        &["-i", "s/alpha/X/g"],
        &["-i.bak", "s/alpha/X/g"],
        &["-i", "$d"],
        &["-i", "1d;$a end"],
        &["-i", "2,4d;$-1,$s/a/b/"],
        &["-i", "/line 3/,/line 5/d"],
        &["-i", "-n", "/7$/p"],
        &["-i", "-e", "1F;=", "-e", "$,-2d"],
        &["-i", "20q"],
        &["-i", "/line 12 /q5"],
        &["-i", "--follow-symlinks", "s/line/L/"],
        &["-i", "-n", "2{p;p}"],
        &["-i", "/9 /w out.txt"],
        &["-i", "s/alpha/X/w out.txt"],
    ];
    for names in name_sets {
        for args in scripts {
            let want = edit_in_place("1", "1048576", args, names, &[]);
            for (jobs, block) in [("2", "1"), ("3", "50"), ("4", "333"), ("8", "4096")] {
                let got = edit_in_place(jobs, block, args, names, &[]);
                assert_eq!(
                    got, want,
                    "script {args:?} files {names:?} jobs {jobs} block {block}"
                );
            }
        }
    }
}

/// A file named twice is read the second time as the first left it, and a file that the script writes is read
/// as it is written: those runs stay sequential, with the same result.
#[test]
fn a_file_that_is_read_after_it_is_written_is_not_read_ahead() {
    let twice = ["a.txt", "b.txt", "a.txt"];
    let want = edit_in_place("1", "1048576", &["-i", "1d"], &twice, &[]);
    for (jobs, block) in [("2", "1"), ("4", "333")] {
        assert_eq!(
            edit_in_place(jobs, block, &["-i", "1d"], &twice, &[]),
            want,
            "jobs {jobs} block {block}"
        );
    }
    // (500 lines minus two firsts)
    let a = want
        .1
        .iter()
        .find(|(name, _)| name == "a.txt")
        .unwrap()
        .1
        .as_ref()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&a.0).lines().count(), 498);
    let into_input = ["-i", "s/alpha/X/w b.txt"];
    let want = edit_in_place("1", "1048576", &into_input, &["a.txt", "b.txt"], &[]);
    for (jobs, block) in [("2", "1"), ("4", "333")] {
        assert_eq!(
            edit_in_place(jobs, block, &into_input, &["a.txt", "b.txt"], &[]),
            want,
            "jobs {jobs} block {block}"
        );
    }
}

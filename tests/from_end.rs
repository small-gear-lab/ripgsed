//! `$-N` is the line with N lines after it (`$-0` is `$`): an address that GNU sed 4.9 rejects and that
//! ripgsed accepts. The expected output is made here from a model of the lines, not by another sed.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

/// The lines of the input, one number per line, so that the output says which lines were selected.
fn numbered(count: usize, from: usize) -> String {
    (from..from + count).map(|i| format!("{i}\n")).collect()
}

/// The 1-based numbers of the lines the script selects among `n` lines (the model).
fn model(kind: &str, n: i64, k: i64, j: i64) -> Vec<i64> {
    let all = 1..=n;
    match kind {
        // `$-K`
        "single" => all.filter(|&i| i == n - k && n - k >= 1).collect(),
        // `$-K,$`
        "to_end" => all.filter(|&i| i >= (n - k).max(1)).collect(),
        // `2,$-K`: a range that starts on line 2 and ends on line N-K, or on line 2 if that is not after it
        "from_2" => {
            if n < 2 {
                return vec![];
            }
            let end = (n - k).max(2);
            all.filter(|&i| i >= 2 && i <= end).collect()
        }
        // `$-K,$-J` with K > J
        "both" => {
            if n < 1 {
                return vec![];
            }
            let start = (n - k).max(1);
            let end = (n - j).max(start);
            all.filter(|&i| i >= start && i <= end).collect()
        }
        _ => unreachable!(),
    }
}

fn script(kind: &str, k: i64, j: i64) -> String {
    match kind {
        "single" => format!("$-{k}p"),
        "to_end" => format!("$-{k},$p"),
        "from_2" => format!("2,$-{k}p"),
        "both" => format!("$-{k},$-{j}p"),
        _ => unreachable!(),
    }
}

fn run(args: &[&str], files: &[&std::path::Path], parallel: bool) -> (String, String, Option<i32>) {
    let mut command = Command::new(BIN);
    command.args(args).args(files).env("LC_ALL", "C");
    if parallel {
        command.env("RIPGSED_JOBS", "3").env("RIPGSED_BLOCK", "7");
    } else {
        command.env("RIPGSED_JOBS", "1");
    }
    let out = command.output().expect("run ripgsed");
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code())
}

fn lines_of(selected: &[i64], first: i64) -> String {
    selected.iter().map(|i| format!("{}\n", i - 1 + first)).collect()
}

#[test]
fn an_address_counted_back_from_the_end_selects_the_line_the_model_says() {
    let dir = std::env::temp_dir().join(format!("ripgsed-from-end-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    for parallel in [false, true] {
        for n in 0..=9i64 {
            std::fs::write(&file, numbered(n as usize, 1)).unwrap();
            for kind in ["single", "to_end", "from_2", "both"] {
                for k in 0..=4i64 {
                    for j in 0..=k {
                        // (`j` is the second count of `$-K,$-J`, which has to be the smaller one.)
                        if (kind == "both" && j >= k) || (kind != "both" && j > 0) {
                            continue;
                        }
                        let text = script(kind, k, j);
                        let want = lines_of(&model(kind, n, k, j), 1);
                        let got = run(&["-n", &text], &[&file], parallel);
                        assert_eq!((got.0, got.2), (want, Some(0)), "script {text} on {n} lines, parallel {parallel}: {}", got.1);
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn deleting_and_negating_with_an_address_counted_back() {
    let dir = std::env::temp_dir().join(format!("ripgsed-from-end-neg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    std::fs::write(&file, numbered(6, 1)).unwrap();
    for parallel in [false, true] {
        assert_eq!(run(&["$-2d"], &[&file], parallel).0, "1\n2\n3\n5\n6\n");
        assert_eq!(run(&["$-2!d"], &[&file], parallel).0, "4\n");
        assert_eq!(run(&["$-1,$d"], &[&file], parallel).0, "1\n2\n3\n4\n");
        assert_eq!(run(&["$-0d"], &[&file], parallel).0, "1\n2\n3\n4\n5\n");
        assert_eq!(run(&["-n", "$-2{=;p}"], &[&file], parallel).0, "4\n4\n");
        assert_eq!(run(&["-n", "$-3,$-1{p;p}"], &[&file], parallel).0, "3\n3\n4\n4\n5\n5\n");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_count_goes_across_files_and_stops_at_the_end_of_each_one_with_s() {
    let dir = std::env::temp_dir().join(format!("ripgsed-from-end-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    let empty = dir.join("empty.txt");
    std::fs::write(&a, numbered(3, 1)).unwrap();
    std::fs::write(&b, numbered(4, 4)).unwrap();
    std::fs::write(&empty, b"").unwrap();
    for parallel in [false, true] {
        // One stream of 7 lines: the last three are 5, 6 and 7.
        assert_eq!(run(&["-n", "$-2,$p"], &[&a, &b], parallel).0, "5\n6\n7\n");
        assert_eq!(run(&["-n", "$-2,$p"], &[&a, &empty, &b, &empty], parallel).0, "5\n6\n7\n");
        // With -s each file counts for itself.
        assert_eq!(run(&["-s", "-n", "$-1,$p"], &[&a, &b], parallel).0, "2\n3\n6\n7\n");
        assert_eq!(run(&["-s", "-n", "$-0p"], &[&a, &empty, &b], parallel).0, "3\n7\n");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_count_that_is_not_a_number_is_still_the_error_of_gnu_sed() {
    let (out, err, status) = run(&["-n", "$-p"], &[], false);
    assert!(out.is_empty());
    assert!(err.contains("unknown command: `-'"), "{err}");
    assert_eq!(status, Some(1));
}

/// `addr,-N` is the first address and the N lines before it, as `addr,+N` is the first address and the N lines
/// after it: `10,-3` is `7,10`, and `-0` is one line.
#[test]
fn a_range_counted_back_from_its_first_address() {
    let dir = std::env::temp_dir().join(format!("ripgsed-from-end-back-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    for parallel in [false, true] {
        for n in 0..=9i64 {
            std::fs::write(&file, numbered(n as usize, 1)).unwrap();
            for m in 1..=8i64 {
                for back in 0..=4i64 {
                    let extra = back;
                    let want: Vec<i64> = (1..=n).filter(|&i| i >= (m - extra).max(1) && i <= m).collect();
                    let text = format!("{m},-{back}p");
                    let got = run(&["-n", &text], &[&file], parallel);
                    assert_eq!((got.0, got.2), (lines_of(&want, 1), Some(0)), "script {text} on {n} lines, parallel {parallel}: {}", got.1);
                }
            }
            for back in 0..=4i64 {
                let text = format!("$,-{back}p");
                let got = run(&["-n", &text], &[&file], parallel);
                let extra = back;
                assert_eq!(got.0, lines_of(&model("to_end", n, extra, 0), 1), "script {text} on {n} lines, parallel {parallel}");
                for k in 1..=3i64 {
                    let text = format!("$-{k},-{back}p");
                    let got = run(&["-n", &text], &[&file], parallel);
                    assert_eq!(got.0, lines_of(&model("both", n, k + extra, k), 1), "script {text} on {n} lines, parallel {parallel}");
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_range_counted_back_needs_a_line_number_to_count_from() {
    for (text, message) in [
        ("/x/,-2p", "invalid usage of -N after a regexp or a step as first address"),
        ("1~2,-2p", "invalid usage of -N after a regexp or a step as first address"),
        ("0,-2p", "invalid usage of line address 0"),
        ("2,-p", "unexpected `,'"),
    ] {
        let (out, err, status) = run(&["-n", text], &[], false);
        assert!(out.is_empty(), "{text}: {out}");
        assert!(err.contains(message), "{text}: {err}");
        assert_eq!(status, Some(1), "{text}");
    }
}

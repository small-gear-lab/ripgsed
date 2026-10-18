//! The program names itself when it is run under its own name (the fixtures run it as `sed`).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

#[test]
fn version_names_the_project() {
    let out = Command::new(BIN)
        .arg("--version")
        .env("LC_ALL", "C")
        .output()
        .expect("run ripgsed");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text, format!("ripgsed {}\n", env!("CARGO_PKG_VERSION")));
}

/// The grep mode is chosen by the name the program runs under, or by a first argument `--grep`.
mod grep_mode {
    use std::io::Write;
    use std::os::unix::fs::symlink;
    use std::process::{Command, Stdio};

    const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

    fn run_as(name: &str, args: &[&str], input: &str) -> (String, String, i32) {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ripgsed-cli-{}-{}-{}", std::process::id(), n, name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join(name);
        symlink(BIN, &link).unwrap();
        let mut child = Command::new(&link)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run ripgsed");
        // (A command that fails at once has closed the pipe before the write: not an error.)
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    #[test]
    fn the_names_choose_the_patterns() {
        assert_eq!(
            run_as("grep", &["a+"], "a+\naa\n"),
            ("a+\n".to_string(), String::new(), 0)
        );
        assert_eq!(
            run_as("egrep", &["a+"], "a+\naa\n"),
            ("a+\naa\n".to_string(), String::new(), 0)
        );
        assert_eq!(
            run_as("fgrep", &["a."], "a.\nab\n"),
            ("a.\n".to_string(), String::new(), 0)
        );
        assert_eq!(
            run_as("rgrep", &["b"], "a\nb\n"),
            ("b\n".to_string(), String::new(), 0)
        );
    }

    #[test]
    fn the_messages_say_grep_whatever_the_name() {
        for name in ["grep", "egrep", "fgrep"] {
            let (out, err, status) = run_as(name, &["-Z"], "");
            assert_eq!(out, "");
            assert!(
                err.starts_with("Usage: grep [OPTION]... PATTERNS [FILE]..."),
                "{name}: {err}"
            );
            assert_eq!(status, 2);
        }
        let (_, err, status) = run_as("egrep", &["-F", "x"], "x\n");
        assert_eq!(err, "grep: conflicting matchers specified\n");
        assert_eq!(status, 2);
    }

    #[test]
    fn the_first_argument_grep_chooses_the_mode_under_the_own_name() {
        let (out, err, status) = run_as("ripgsed", &["--grep", "-n", "b"], "a\nb\n");
        assert_eq!((out.as_str(), err.as_str(), status), ("2:b\n", "", 0));
        // Without it, the program is sed.
        let (out, _, _) = run_as("ripgsed", &["-n", "2p"], "a\nb\n");
        assert_eq!(out, "b\n");
    }

    /// A pattern that PCRE2 gives up on (its match limit) is an error, never "no match".
    #[test]
    fn an_exceeded_pcre_limit_is_an_error_of_grep_not_no_match() {
        let input = format!("{}!\n", "a".repeat(60));
        let (out, err, status) = run_as("grep", &["-P", "^(a|aa)+$"], &input);
        assert_eq!((out.as_str(), status), ("", 2));
        assert_eq!(
            err,
            "grep: (standard input): exceeded PCRE's backtracking limit\n"
        );
        // Lines before the one that fails are selected, and the status is 2 all the same.
        let input = format!("a\n{}!\nb\n", "a".repeat(60));
        let (out, err, status) = run_as("grep", &["-P", "^(a|aa)+$"], &input);
        assert_eq!((out.as_str(), status), ("a\n", 2));
        assert!(err.contains("exceeded PCRE's backtracking limit"), "{err}");
        // The status of an error wins over -q as well.
        let input = format!("{}!\n", "a".repeat(60));
        let (_, _, status) = run_as("grep", &["-qP", "^(a|aa)+$"], &input);
        assert_eq!(status, 2);
    }

    /// GNU grep ends the whole run at the file it gave up on: the files after it are not looked at,
    /// the count of that file is not printed, and -s does not silence the message.
    #[test]
    fn an_exceeded_pcre_limit_ends_the_run_at_that_file() {
        let dir = std::env::temp_dir().join(format!("ripgsed-cli-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.txt"), "aa\nb\n").unwrap();
        std::fs::write(dir.join("deep.txt"), format!("{}!\n", "a".repeat(60))).unwrap();
        std::fs::write(dir.join("after.txt"), "a\n").unwrap();
        let run = |args: &[&str]| {
            let out = Command::new(BIN)
                .arg("--grep")
                .args(args)
                .current_dir(&dir)
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            (
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
                out.status.code().unwrap(),
            )
        };
        let (out, err, status) = run(&["-P", "^(a|aa)+$", "ok.txt", "deep.txt", "after.txt"]);
        assert_eq!((out.as_str(), status), ("ok.txt:aa\n", 2));
        assert_eq!(err, "grep: deep.txt: exceeded PCRE's backtracking limit\n");
        let (out, _, status) = run(&["-cP", "^(a|aa)+$", "ok.txt", "deep.txt", "after.txt"]);
        assert_eq!((out.as_str(), status), ("ok.txt:1\n", 2));
        let (out, err, status) = run(&["-sP", "^(a|aa)+$", "deep.txt"]);
        assert_eq!((out.as_str(), status), ("", 2));
        assert!(err.contains("exceeded PCRE's backtracking limit"), "{err}");
        // With threads the run ends in the same place.
        let out = Command::new(BIN)
            .arg("--grep")
            .args(["-rP", "^(a|aa)+$", "."])
            .current_dir(&dir)
            .env("LC_ALL", "C")
            .env("RIPGSED_JOBS", "4")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .contains("deep.txt: exceeded PCRE's backtracking limit")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_exit_status_is_one_without_a_selected_line() {
        let (out, err, status) = run_as("grep", &["zzz"], "a\n");
        assert_eq!((out.as_str(), err.as_str(), status), ("", "", 1));
    }
}

/// `-P`: the regular expressions of the script are PCRE2 ones (the library of the system).
mod sed_pcre {
    use std::io::Write;
    use std::process::{Command, Stdio};

    const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

    fn sed(args: &[&str], input: &str) -> (String, String, i32) {
        let mut child = Command::new(BIN)
            .args(args)
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run ripgsed");
        // (A command that fails at once has closed the pipe before the write: not an error.)
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        let out = child.wait_with_output().unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    #[test]
    fn perl_syntax_in_substitution_and_address() {
        assert_eq!(
            sed(&["-P", "s/(\\d+)\\s(?=bar)/<\\1>/"], "foo123 bar\n").0,
            "foo<123>bar\n"
        );
        assert_eq!(
            sed(&["--perl-regexp", "-n", "/^(?i)hello/p"], "abc\nHello\n").0,
            "Hello\n"
        );
        assert_eq!(
            sed(
                &["--perl-regexp", "s/\\b(\\w)(\\w*)/\\U\\1\\E\\2/g"],
                "ab cd\n"
            )
            .0,
            "Ab Cd\n"
        );
    }

    #[test]
    fn flags_and_unset_groups() {
        assert_eq!(sed(&["-P", "s/(x)?c/[\\1]/"], "abc\n").0, "ab[]\n");
        assert_eq!(sed(&["-P", "N;s/a$/X/M"], "a\nb\n").0, "X\nb\n");
        assert_eq!(sed(&["-P", "N;s/a$/X/"], "a\nb\n").0, "a\nb\n");
        assert_eq!(sed(&["-P", "s/é/E/I"], "É\n").0, "E\n");
    }

    #[test]
    fn an_exceeded_limit_is_an_error_of_the_run_not_no_match() {
        let input = format!("{}!\n", "a".repeat(60));
        for script in ["s/^(a|aa)+$/X/", "/^(a|aa)+$/d"] {
            let (out, err, status) = sed(&["-P", script], &input);
            assert_eq!(status, 4, "{script}: {err}");
            assert_eq!(out, "", "{script}");
            assert!(
                err.starts_with("ripgsed: ") && err.contains("exceeded PCRE's backtracking limit"),
                "{script}: {err}"
            );
        }
        // A script that stays within the limits is not touched.
        assert_eq!(sed(&["-P", "s/^(a|aa)+$/X/"], "aaaa\n").0, "X\n");
    }

    #[test]
    fn errors_are_reported_like_other_regexp_errors() {
        let (_, err, status) = sed(&["-P", "s/(/x/"], "a\n");
        assert_eq!(status, 1);
        assert!(
            err.starts_with("ripgsed: -e expression #1, char 6: "),
            "{err}"
        );
        let (_, err, status) = sed(&["-P", "s/a/\\2/"], "a\n");
        assert_eq!(status, 1);
        assert!(
            err.contains("invalid reference \\2 on `s' command's RHS"),
            "{err}"
        );
    }
}

/// `--posix` turns off the GNU extensions, and `-P` (PCRE2 regular expressions) is one that GNU sed does not even
/// have: the two together are a usage error, in whichever order they come, and the last of `-P` and `-E` wins as it
/// always did.
mod posix_and_perl {
    use std::io::Write;
    use std::process::{Command, Stdio};

    const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

    fn run(args: &[&str], input: &str) -> (String, String, Option<i32>) {
        let mut child = Command::new(BIN)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run ripgsed");
        // (A run that is refused at once does not read its input: the pipe may be closed by then.)
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        let out = child.wait_with_output().unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code(),
        )
    }

    #[test]
    fn posix_with_perl_is_a_usage_error_in_either_order() {
        for args in [
            &["--posix", "-P", "s/\\d/X/"][..],
            &["-P", "--posix", "s/\\d/X/"],
            &["--posix", "-P", "-e", "s/\\d/X/"],
            &["--posix", "--perl-regexp", "-e", "s/\\d/X/"],
            &["-Pn", "--posix", "-e", "/\\d/p"],
            &["-P", "-e", "s/\\d/X/", "--posix"],
        ] {
            let (out, err, code) = run(args, "a1\n");
            assert_eq!(
                (out.as_str(), code),
                ("", Some(1)),
                "{args:?}: stderr {err:?}"
            );
            assert!(
                err.contains("-P") && err.contains("--posix"),
                "{args:?}: stderr {err:?}"
            );
        }
    }

    #[test]
    fn perl_and_posix_each_alone_and_with_extended_still_work() {
        assert_eq!(run(&["-P", "s/\\d/X/"], "a1\n").0, "aX\n");
        assert_eq!(run(&["--posix", "s/[0-9]/X/"], "a1\n").0, "aX\n");
        assert_eq!(run(&["--posix", "-E", "s/[0-9]+/X/"], "a12\n").0, "aX\n");
        // The last of -P and -E is the one that counts, so `-P -E` is ERE and `--posix -P -E` is allowed.
        assert_eq!(
            run(&["--posix", "-P", "-E", "s/[0-9]+/X/"], "a12\n").0,
            "aX\n"
        );
    }
}

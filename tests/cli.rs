//! The program names itself when it is run under its own name (the fixtures run it as `sed`).

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

#[test]
fn version_names_the_project() {
    let out = Command::new(BIN).arg("--version").env("LC_ALL", "C").output().expect("run ripgsed");
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
        let dir = std::env::temp_dir().join(format!("ripgsed-cli-{}-{}-{}", std::process::id(), n, name));
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
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
    }

    #[test]
    fn the_names_choose_the_patterns() {
        assert_eq!(run_as("grep", &["a+"], "a+\naa\n"), ("a+\n".to_string(), String::new(), 0));
        assert_eq!(run_as("egrep", &["a+"], "a+\naa\n"), ("a+\naa\n".to_string(), String::new(), 0));
        assert_eq!(run_as("fgrep", &["a."], "a.\nab\n"), ("a.\n".to_string(), String::new(), 0));
        assert_eq!(run_as("rgrep", &["b"], "a\nb\n"), ("b\n".to_string(), String::new(), 0));
    }

    #[test]
    fn the_messages_say_grep_whatever_the_name() {
        for name in ["grep", "egrep", "fgrep"] {
            let (out, err, status) = run_as(name, &["-Z"], "");
            assert_eq!(out, "");
            assert!(err.starts_with("Usage: grep [OPTION]... PATTERNS [FILE]..."), "{name}: {err}");
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
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
    }

    #[test]
    fn perl_syntax_in_substitution_and_address() {
        assert_eq!(sed(&["-P", "s/(\\d+)\\s(?=bar)/<\\1>/"], "foo123 bar\n").0, "foo<123>bar\n");
        assert_eq!(sed(&["--perl-regexp", "-n", "/^(?i)hello/p"], "abc\nHello\n").0, "Hello\n");
        assert_eq!(sed(&["--perl-regexp", "s/\\b(\\w)(\\w*)/\\U\\1\\E\\2/g"], "ab cd\n").0, "Ab Cd\n");
    }

    #[test]
    fn flags_and_unset_groups() {
        assert_eq!(sed(&["-P", "s/(x)?c/[\\1]/"], "abc\n").0, "ab[]\n");
        assert_eq!(sed(&["-P", "N;s/a$/X/M"], "a\nb\n").0, "X\nb\n");
        assert_eq!(sed(&["-P", "N;s/a$/X/"], "a\nb\n").0, "a\nb\n");
        assert_eq!(sed(&["-P", "s/é/E/I"], "É\n").0, "E\n");
    }

    #[test]
    fn errors_are_reported_like_other_regexp_errors() {
        let (_, err, status) = sed(&["-P", "s/(/x/"], "a\n");
        assert_eq!(status, 1);
        assert!(err.starts_with("ripgsed: -e expression #1, char 6: "), "{err}");
        let (_, err, status) = sed(&["-P", "s/a/\\2/"], "a\n");
        assert_eq!(status, 1);
        assert!(err.contains("invalid reference \\2 on `s' command's RHS"), "{err}");
    }
}

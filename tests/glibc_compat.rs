//! `--glibc-compat` (and `RIPGSED_GLIBC_COMPAT`) reproduce the quirks of the C library's regex
//! matcher; without them the match is the one the definition gives (D10).

use std::io::Write;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

fn run(args: &[&str], env: Option<&str>, input: &str) -> String {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .env("LC_ALL", "C")
        .env_remove("RIPGSED_GLIBC_COMPAT");
    if let Some(v) = env {
        cmd.env("RIPGSED_GLIBC_COMPAT", v);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run ripgsed");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
}

#[test]
fn the_definition_is_the_default() {
    assert_eq!(run(&["-E", "s/(a+)+\\1/<&>/"], None, "aaaa\n"), "<aaaa>\n");
    assert_eq!(run(&["s/b*\\B/<&>/"], None, "ab\n"), "a<>b\n");
    assert_eq!(
        run(&["-E", "s/(a*){2}\\1/<&>/"], None, "aaaaaa\n"),
        "<aaaaaa>\n"
    );
}

#[test]
fn the_option_reproduces_glibc() {
    assert_eq!(
        run(&["--glibc-compat", "-E", "s/(a+)+\\1/<&>/"], None, "aaaa\n"),
        "<aa>aa\n"
    );
    assert_eq!(
        run(&["--glibc-compat", "s/b*\\B/<&>/"], None, "ab\n"),
        "ab<>\n"
    );
    assert_eq!(
        run(
            &["--glibc-compat", "-E", "s/(a*){2}\\1/<&>/"],
            None,
            "aaaaaa\n"
        ),
        "aaaaaa\n"
    );
}

#[test]
fn the_environment_variable_does_the_same() {
    assert_eq!(
        run(&["-E", "s/(a+)+\\1/<&>/"], Some("1"), "aaaa\n"),
        "<aa>aa\n"
    );
    assert_eq!(
        run(&["-E", "s/(a+)+\\1/<&>/"], Some("0"), "aaaa\n"),
        "<aaaa>\n"
    );
    assert_eq!(
        run(&["-E", "s/(a+)+\\1/<&>/"], Some(""), "aaaa\n"),
        "<aaaa>\n"
    );
}

/// GNU sed cuts the path of a relative link target that is too long for a fixed buffer; that is
/// a bug, so without `--glibc-compat` the whole path is used (the fixture `cmd-059` holds the
/// GNU result and runs with the option).
#[test]
fn a_long_symlink_chain_is_followed_in_full_by_default() {
    let dir = std::env::temp_dir().join(format!("ripgsed-glibc-compat-{}", std::process::id()));
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let _ = std::os::unix::fs::symlink(BIN, bin.join("sed"));
    let script = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cmd-059-probe-follow-symlink/cmd"
    ))
    .unwrap();
    let run = |compat: bool| {
        let work = dir.join(if compat { "on" } else { "off" });
        std::fs::create_dir_all(&work).unwrap();
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&script)
            .current_dir(&work)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("LC_ALL", "C");
        cmd.env_remove("RIPGSED_GLIBC_COMPAT");
        if compat {
            cmd.env("RIPGSED_GLIBC_COMPAT", "1");
        }
        String::from_utf8_lossy(&cmd.output().unwrap().stdout).into_owned()
    };
    assert_eq!(run(false), "sed exit=0\nnew\n");
    assert_eq!(run(true), "sed exit=4\nold\n");
    let _ = std::fs::remove_dir_all(&dir);
}

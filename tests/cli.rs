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

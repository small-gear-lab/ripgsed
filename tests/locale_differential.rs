//! Compares ripgsed with GNU sed 4.9 in UTF-8 locales other than `C.UTF-8` on the first 16,384
//! code points (the whole range is `tools/locale-diff.py` without `--max-cp`, see the
//! description there): the answers of bracket expressions, classes, `\w`, the `I` flag and the
//! case conversions must be the same for every character.
//!
//! The test needs GNU sed 4.9 as `sed`, `python3`, and `localedef` with the definitions in
//! /usr/share/i18n (Debian and Ubuntu: the package `locales`). When one is missing the test says
//! so and passes, as a machine without them cannot produce the reference answers either.

use std::process::Command;

fn have(program: &str, arg: &str) -> Option<String> {
    let out = Command::new(program).arg(arg).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn bracket_classes_and_case_rules_follow_the_c_library() {
    let reference = have("sed", "--version").unwrap_or_default();
    if !reference.starts_with("sed (GNU sed) 4.9") {
        eprintln!("skipped: the reference sed is not GNU sed 4.9");
        return;
    }
    if have("python3", "--version").is_none() || have("localedef", "--version").is_none() || !std::path::Path::new("/usr/share/i18n/locales/tr_TR").exists() {
        eprintln!("skipped: python3, localedef or /usr/share/i18n is missing");
        return;
    }
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/locale-diff.py");
    let out = Command::new("python3")
        .args(["-I", script, "--ours", env!("CARGO_BIN_EXE_ripgsed"), "--max-cp", "0x4000", "--quick", "--locales", "en_US.UTF-8,tr_TR.UTF-8,sv_SE.UTF-8"])
        .output()
        .expect("run tools/locale-diff.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
}

/// The same for the other character sets (EUC-JP, Shift_JIS, Big5, ISO-8859-1, KOI8-R: every byte
/// and the two-byte sequences of some first bytes) and for random patterns in company
/// (`tools/locale-fuzz.py`).
#[test]
fn other_character_sets_follow_the_c_library() {
    let reference = have("sed", "--version").unwrap_or_default();
    if !reference.starts_with("sed (GNU sed) 4.9") {
        eprintln!("skipped: the reference sed is not GNU sed 4.9");
        return;
    }
    if have("python3", "--version").is_none() || have("localedef", "--version").is_none() || !std::path::Path::new("/usr/share/i18n/charmaps").exists() {
        eprintln!("skipped: python3, localedef or /usr/share/i18n is missing");
        return;
    }
    let locales = "ja_JP.eucJP,ja_JP.SJIS,zh_CN.GB18030,zh_TW.BIG5,de_DE.ISO-8859-1,ru_RU.KOI8-R";
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/locale-diff.py");
    let out = Command::new("python3")
        .args(["-I", script, "--ours", env!("CARGO_BIN_EXE_ripgsed"), "--quick", "--sample", "--locales", locales])
        .output()
        .expect("run tools/locale-diff.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    // With --glibc-compat the quirks of the library that ripgsed can reproduce are exact: the abort
    // of a case conversion to a character the set cannot write, and the pairs of bytes of an
    // unassigned code that a negated bracket expression takes under `I`.
    let out = Command::new("python3")
        .args(["-I", script, "--ours", env!("CARGO_BIN_EXE_ripgsed"), "--quick", "--sample", "--glibc-compat", "--locales", "ja_JP.SJIS,zh_CN.GBK"])
        .output()
        .expect("run tools/locale-diff.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/locale-fuzz.py");
    let out = Command::new("python3")
        .args(["-I", script, "--ours", env!("CARGO_BIN_EXE_ripgsed"), "--cases", "60", "--invalid", "--locales", locales])
        .output()
        .expect("run tools/locale-fuzz.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}{}", String::from_utf8_lossy(&out.stderr));
}

//! A quiet script whose commands are all for lines that are ahead (a line number, or `first~step`) skips the lines
//! between them without running a cycle. `-s` does not skip, and for one file it numbers the lines as the run
//! without it does: so the two must print the same.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ripgsed");

fn lines(count: usize, last_terminated: bool, separator: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 1..=count {
        out.extend_from_slice(format!("line {i}").as_bytes());
        if i < count || last_terminated {
            out.push(separator);
        }
    }
    out
}

fn run(args: &[&str], file: &std::path::Path) -> (Vec<u8>, Vec<u8>, Option<i32>) {
    let out = Command::new(BIN)
        .args(args)
        .arg(file)
        .env("LC_ALL", "C")
        .env("RIPGSED_JOBS", "1")
        .env("RIPGSED_HEAD", "0")
        .env("RIPGSED_TAIL", "0")
        .output()
        .expect("run ripgsed");
    (out.stdout, out.stderr, out.status.code())
}

#[test]
fn skipping_to_the_next_step_prints_what_running_every_cycle_prints() {
    let dir = std::env::temp_dir().join(format!("ripgsed-skip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    let scripts = [
        "0~1p",
        "0~2p",
        "0~7p",
        "1~3p",
        "2~5p",
        "3~0p",
        "5~1p",
        "10~4p",
        "40~7p",
        "41~3p",
        "0~3{p;=}",
        "1~2p;3~3p",
        "2p;0~5p",
        "0~4p;9p",
        "0~4{N;p}",
        "0~3p;$p",
        "0~3,5p",
        "1~4F",
        "0~4=",
        "2~3l",
        "0~9{p;p}",
        "0~100p",
    ];
    for separator in [b'\n', 0u8] {
        for count in [0usize, 1, 2, 3, 6, 7, 8, 13, 40, 41, 99] {
            for terminated in [true, false] {
                std::fs::write(&file, lines(count, terminated, separator)).unwrap();
                for script in scripts {
                    let z: &[&str] = if separator == 0 { &["-z"] } else { &[] };
                    let mut skipping = z.to_vec();
                    skipping.extend(["-n", script]);
                    let mut every_cycle = z.to_vec();
                    every_cycle.extend(["-s", "-n", script]);
                    assert_eq!(
                        run(&skipping, &file),
                        run(&every_cycle, &file),
                        "script {script} on {count} records, last terminated {terminated}, separator {separator}"
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

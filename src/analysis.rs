//! Static analysis: can a script be run on chunks of records independently?
//!
//! A script is *record-local* when what it does to one record never depends on any other
//! record and it has no effect outside the output stream. Such a script can run on chunks in
//! any order (and in parallel) as long as the outputs are put back in input order. The test
//! is conservative: anything not proven record-local runs sequentially.

use crate::runtime::Options;
use crate::syntax::{Addr, Addr2, Command, Kind, Reach, Script, Selector};

/// Why a script cannot run in parallel.
#[derive(Debug, PartialEq, Eq)]
pub enum Reason {
    /// Names the construct that carries state across records or touches the outside world.
    Construct(&'static str),
}

/// `Ok(())` if the script is record-local under these options, else the first reason it is not.
pub fn record_local(script: &Script, options: &Options) -> Result<(), Reason> {
    if options.debug {
        return Err(Reason::Construct("--debug"));
    }
    // (`-s` and `-i` are fine: the files are independent, and a chunk is told when it starts one.)
    if options.unbuffered {
        return Err(Reason::Construct("-u"));
    }
    for (index, command) in script.commands.iter().enumerate() {
        command_local(&script.commands, index, command, options)?;
    }
    Ok(())
}

/// How many records after the current one the script has to see: one for `$`, N+1 for `$-N` (the most of
/// the addresses that look ahead).
pub fn lookahead(script: &Script) -> u64 {
    let mut most = 1u64;
    for command in &script.commands {
        let mut count = |n: &u64| most = most.max(n.saturating_add(1));
        match &command.selector {
            Selector::Single(Addr::FromEnd(n)) => count(n),
            Selector::Range(first, last) => {
                if let Addr::FromEnd(n) = first {
                    count(n);
                }
                if let Addr2::FromEnd(n) = last {
                    count(n);
                }
            }
            _ => {}
        }
    }
    most
}

fn command_local(
    commands: &[Command],
    index: usize,
    command: &Command,
    options: &Options,
) -> Result<(), Reason> {
    // `F` names the file as the sequential run opens it, which is the resolved path under `--follow-symlinks`.
    if matches!(command.kind, Kind::FileName) && options.follow_symlinks {
        return Err(Reason::Construct("F with --follow-symlinks"));
    }
    match command.reach() {
        Reach::OtherRecords("an empty regexp") if empty_regexp_is_known(commands, index) => Ok(()),
        reach => match reach.construct() {
            None => Ok(()),
            Some(name) => Err(Reason::Construct(name)),
        },
    }
}

/// Whether the regexp that an empty one in the command stands for is the same on every record: the
/// one the command's own address has just tried, or the one of the `s` right before it when that
/// always runs (control reaches a command only from the one before it, as labels are commands too).
fn empty_regexp_is_known(commands: &[Command], index: usize) -> bool {
    let command = &commands[index];
    if matches!(&command.selector, Selector::Single(Addr::Regex(p)) if p.regex.is_some()) {
        return true;
    }
    if !matches!(command.selector, Selector::All) || command.negate {
        return false;
    }
    let Some(before) = index.checked_sub(1).map(|i| &commands[i]) else {
        return false;
    };
    matches!(before.selector, Selector::All)
        && !before.negate
        && matches!(&before.kind, Kind::Substitute(sub) if sub.pattern.regex.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Chunk, ChunkSource, ParseOptions, parse};

    fn local(script: &str) -> bool {
        let chunk = Chunk {
            source: ChunkSource::Expression(1),
            text: script.as_bytes().to_vec(),
        };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        record_local(&script, &Options::default()).is_ok()
    }

    #[test]
    fn record_local_scripts() {
        for s in [
            "s/a/b/g",
            "/x/d",
            "y/abc/xyz/",
            "2~3p",
            "5s/a/b/",
            "s/a/b/;ta;s/c/d/;:a",
            "=",
            "/a/{s/a/b/;p}",
            "a text",
            "l",
            "q",
            "5q3",
            "/a/Q",
            "/a/{s/a/b/;q}",
            "$p",
            "$!d",
            "$s/a/b/",
            "$!{s/a/b/;p}",
            "$q",
            "$-2p",
            "$-1,$d",
            "2,$-3p",
            "$-2,$-1s/a/b/",
            "F",
            "3{F;d}",
            "1,3p",
            "/a/,/b/d",
            "/a/,$d",
            "0,/a/d",
            "2,+3s/a/b/",
            "/a/,~4p",
            "2~3,5p",
            "/a/,/b/!d",
            "/a/,/b/c\\text",
            "/a/s//b/",
            "/a/!s//b/",
            "s/a/b/;s//c/",
            "$!{s/a/b/;s//c/}",
            "/a/w out",
            "/a/W out",
            "s/a/b/w out",
            "/a/{s/b/c/w out\n}",
        ] {
            assert!(local(s), "{s} should be record-local");
        }
    }

    #[test]
    fn scripts_with_cross_record_state_are_not() {
        for s in [
            "N",
            "x",
            "G",
            "h",
            "n",
            "D",
            "s//x/",
            "/a/{s//x/}",
            "1s/a/b/;s//c/",
            "/a/,/b/s//x/",
            "//,/b/p",
            "/a/,//p",
            ":a;s//x/",
            "//p",
            "$!N",
            "w /dev/stdout",
            "/a/w /dev/stderr",
            "s/a/b/w /dev/stdout",
            "R f",
            "r f",
            "0r f",
            "$r f",
            "/a/r f",
            "r /dev/stdin",
            "e",
            "s/a/b/e",
        ] {
            assert!(!local(s), "{s} must not be record-local");
        }
    }

    #[test]
    fn the_file_name_follows_the_symlinks_only_in_the_sequential_run() {
        let chunk = Chunk {
            source: ChunkSource::Expression(1),
            text: b"F".to_vec(),
        };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        let options = Options {
            follow_symlinks: true,
            ..Options::default()
        };
        assert_eq!(
            record_local(&script, &options),
            Err(Reason::Construct("F with --follow-symlinks"))
        );
    }

    #[test]
    fn the_script_asks_to_see_as_far_ahead_as_its_addresses_look() {
        let ahead = |text: &str| {
            let chunk = Chunk {
                source: ChunkSource::Expression(1),
                text: text.as_bytes().to_vec(),
            };
            lookahead(&parse(&[(chunk, ParseOptions::default())]).expect("parses"))
        };
        assert_eq!(ahead("p"), 1);
        assert_eq!(ahead("$p"), 1);
        assert_eq!(ahead("$-0p"), 1);
        assert_eq!(ahead("$-2p"), 3);
        assert_eq!(ahead("2,$-4p"), 5);
        assert_eq!(ahead("$-3,$-1p;$-9d"), 10);
    }

    #[test]
    fn files_that_are_separate_or_edited_in_place_run_chunk_by_chunk_too() {
        let chunk = Chunk {
            source: ChunkSource::Expression(1),
            text: b"$!s/a/b/;2,4p".to_vec(),
        };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        let separate = Options {
            separate: true,
            ..Options::default()
        };
        let in_place = Options {
            in_place: Some(String::new()),
            separate: true,
            ..Options::default()
        };
        assert_eq!(record_local(&script, &separate), Ok(()));
        assert_eq!(record_local(&script, &in_place), Ok(()));
        // What is written at once stays out.
        let unbuffered = Options {
            unbuffered: true,
            ..Options::default()
        };
        assert_eq!(
            record_local(&script, &unbuffered),
            Err(Reason::Construct("-u"))
        );
    }
}

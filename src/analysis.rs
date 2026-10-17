//! Static analysis: can a script be run on chunks of records independently?
//!
//! A script is *record-local* when what it does to one record never depends on any other
//! record and it has no effect outside the output stream. Such a script can run on chunks in
//! any order (and in parallel) as long as the outputs are put back in input order. The test
//! is conservative: anything not proven record-local runs sequentially.

use crate::runtime::Options;
use crate::syntax::{Addr, Command, Kind, Reach, Script, Selector};

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
    if options.in_place.is_some() {
        return Err(Reason::Construct("-i"));
    }
    if options.separate {
        return Err(Reason::Construct("-s"));
    }
    if options.unbuffered {
        return Err(Reason::Construct("-u"));
    }
    for (index, command) in script.commands.iter().enumerate() {
        command_local(&script.commands, index, command, options)?;
    }
    Ok(())
}

fn command_local(commands: &[Command], index: usize, command: &Command, options: &Options) -> Result<(), Reason> {
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
    let Some(before) = index.checked_sub(1).map(|i| &commands[i]) else { return false };
    matches!(before.selector, Selector::All)
        && !before.negate
        && matches!(&before.kind, Kind::Substitute(sub) if sub.pattern.regex.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{parse, Chunk, ChunkSource, ParseOptions};

    fn local(script: &str) -> bool {
        let chunk = Chunk { source: ChunkSource::Expression(1), text: script.as_bytes().to_vec() };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        record_local(&script, &Options::default()).is_ok()
    }

    #[test]
    fn record_local_scripts() {
        for s in ["s/a/b/g", "/x/d", "y/abc/xyz/", "2~3p", "5s/a/b/", "s/a/b/;ta;s/c/d/;:a", "=", "/a/{s/a/b/;p}", "a text", "l", "q", "5q3", "/a/Q", "/a/{s/a/b/;q}", "$p", "$!d", "$s/a/b/", "$!{s/a/b/;p}", "$q", "F", "3{F;d}", "1,3p", "/a/,/b/d", "/a/,$d", "0,/a/d", "2,+3s/a/b/", "/a/,~4p", "2~3,5p", "/a/,/b/!d", "/a/,/b/c\\text", "/a/s//b/", "/a/!s//b/", "s/a/b/;s//c/", "$!{s/a/b/;s//c/}", "/a/w out", "/a/W out", "s/a/b/w out", "/a/{s/b/c/w out\n}"] {
            assert!(local(s), "{s} should be record-local");
        }
    }

    #[test]
    fn scripts_with_cross_record_state_are_not() {
        for s in ["N", "x", "G", "h", "n", "D", "s//x/", "/a/{s//x/}", "1s/a/b/;s//c/", "/a/,/b/s//x/", "//,/b/p", "/a/,//p", ":a;s//x/", "//p", "$!N", "w /dev/stdout", "/a/w /dev/stderr", "s/a/b/w /dev/stdout", "R f", "r f", "0r f", "$r f", "/a/r f", "r /dev/stdin", "e", "s/a/b/e"] {
            assert!(!local(s), "{s} must not be record-local");
        }
    }

    #[test]
    fn the_file_name_follows_the_symlinks_only_in_the_sequential_run() {
        let chunk = Chunk { source: ChunkSource::Expression(1), text: b"F".to_vec() };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        let options = Options { follow_symlinks: true, ..Options::default() };
        assert_eq!(record_local(&script, &options), Err(Reason::Construct("F with --follow-symlinks")));
    }
}

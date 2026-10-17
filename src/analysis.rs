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

/// How many records at the end of the input are all that the script needs, if the records before them leave
/// nothing behind: nothing is printed or written for them, and nothing changes that a later record sees. Such a
/// script gives the same answer on the last records alone as on the whole input, and a file that can be
/// searched can be read from its end. `None` if that cannot be shown.
///
/// What is shown is about a record that has more records after it than any address counts (so every address
/// counted from the end is false for it): the commands that can run for it are followed from the first, and each
/// has to be one that leaves nothing, until the record is deleted or the script ends (and then it must be quiet).
pub fn tail_window(script: &Script, options: &Options) -> Option<u64> {
    if options.debug || options.unbuffered || options.in_place.is_some() || options.follow_symlinks
    {
        return None;
    }
    let commands = &script.commands;
    // The records of the window are not numbered as they are in the whole input, and what an empty regexp stands
    // for, or the state of a range that starts on a regexp, comes from the records before.
    if !commands
        .iter()
        .all(|command| tail_selector_is_fine(&command.selector) && tail_kind_is_fine(&command.kind))
    {
        return None;
    }
    let quiet = options.quiet || script.quiet;
    let mut pc = 0;
    while pc < commands.len() {
        let command = &commands[pc];
        let run = match early(&command.selector) {
            Early::Yes => !command.negate,
            Early::No => command.negate,
            Early::Maybe => true,
        };
        let sure = !matches!(early(&command.selector), Early::Maybe);
        if !run {
            // (A block that is not entered is jumped over.)
            pc = match command.kind {
                Kind::BlockStart(end) => end + 1,
                _ => pc + 1,
            };
            continue;
        }
        match &command.kind {
            // The record is gone: nothing more happens for it.
            Kind::Delete if sure => return Some(lookahead(script)),
            Kind::Delete => {}
            Kind::BlockStart(_)
            | Kind::BlockEnd
            | Kind::Label(_)
            | Kind::Zap
            | Kind::Translate(_)
            | Kind::Version => {}
            Kind::Substitute(sub)
                if !sub.eval && sub.write.is_none() && !sub.print_before && !sub.print_after => {}
            // A record that is printed, written, read from, kept for later or that reads another record.
            _ => return None,
        }
        pc += 1;
    }
    // The end of the script: the record is printed, unless the script is quiet.
    quiet.then(|| lookahead(script))
}

/// What an address says about a record that is far from the end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Early {
    Yes,
    No,
    /// It depends on what the record holds.
    Maybe,
}

fn early(selector: &Selector) -> Early {
    match selector {
        Selector::All => Early::Yes,
        Selector::Single(Addr::Last | Addr::FromEnd(_))
        | Selector::Range(Addr::Last | Addr::FromEnd(_), _) => Early::No,
        Selector::Single(Addr::Null) => Early::Yes,
        // (The other addresses are not let through by `tail_selector_is_fine`, or are a regexp.)
        _ => Early::Maybe,
    }
}

/// Whether what the selector says is the same on the records of the window as on the whole input.
fn tail_selector_is_fine(selector: &Selector) -> bool {
    let regexp_is_written = |pattern: &crate::syntax::Pattern| pattern.regex.is_some();
    match selector {
        Selector::All => true,
        Selector::Single(Addr::Last | Addr::FromEnd(_) | Addr::Null) => true,
        Selector::Single(Addr::Regex(pattern)) => regexp_is_written(pattern),
        // The line numbers, or the steps of them, are those of the whole input.
        Selector::Single(Addr::Line(_) | Addr::Step { .. }) => false,
        // A range that starts before the window may be going on when the window starts. One that starts in it
        // starts from nothing, and ends on what it holds, or on the number of lines after its start.
        Selector::Range(Addr::Last | Addr::FromEnd(_), end) => match end {
            Addr2::Last | Addr2::FromEnd(_) | Addr2::Plus(_) => true,
            Addr2::Regex(pattern) => regexp_is_written(pattern),
            Addr2::Line(_) | Addr2::Step { .. } | Addr2::Multiple(_) => false,
        },
        Selector::Range(..) => false,
    }
}

/// Whether the command does the same for a record of the window as for the same record of the whole input.
fn tail_kind_is_fine(kind: &Kind) -> bool {
    match kind {
        // The number of the line.
        Kind::LineNumber => false,
        // What an empty regexp stands for.
        Kind::Substitute(sub) => sub.pattern.regex.is_some(),
        _ => true,
    }
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

    fn window(text: &str, quiet: bool) -> Option<u64> {
        let chunk = Chunk {
            source: ChunkSource::Expression(1),
            text: text.as_bytes().to_vec(),
        };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        tail_window(
            &script,
            &Options {
                quiet,
                ..Options::default()
            },
        )
    }

    #[test]
    fn scripts_that_do_nothing_before_the_end_need_only_the_last_records() {
        // -n and only addresses counted from the end: nothing happens before them.
        assert_eq!(window("$p", true), Some(1));
        assert_eq!(window("$-2,$p", true), Some(3));
        assert_eq!(window("$-3,$-1{p;p}", true), Some(4));
        assert_eq!(window("$p;$q3", true), Some(1));
        assert_eq!(window("$-9{s/a/b/;p}", true), Some(10));
        assert_eq!(window("$,-4p", true), Some(5));
        // A first command that deletes every record that is not in the window.
        assert_eq!(window("$!d", false), Some(1));
        assert_eq!(window("$-2,$!d", false), Some(3));
        assert_eq!(window("$!d;s/a/b/;$a end", false), Some(1));
        assert_eq!(window("$-1!d;$q5", false), Some(2));
        assert_eq!(
            window("$!d;p;=", false),
            None,
            "= needs the number of the line"
        );
        // A regexp does not need to know where the line is.
        assert_eq!(window("$!d;/a/s/b/c/", false), Some(1));
    }

    #[test]
    fn scripts_that_do_something_before_the_end_need_all_the_records() {
        // What is printed, or written, or kept, before the end.
        assert_eq!(window("p", true), None);
        assert_eq!(
            window("$d", false),
            None,
            "the records before the end are printed"
        );
        assert_eq!(
            window("$p", false),
            None,
            "and so are they here, without -n"
        );
        assert_eq!(window("/a/p;$p", true), None);
        // (A record that is deleted, or that nothing reaches, leaves nothing behind.)
        assert_eq!(window("/a/d;$p", true), Some(1));
        assert_eq!(window("s/a/b/w out;$p", true), None);
        assert_eq!(window("s/a/b/p;$p", true), None);
        assert_eq!(window("$!{p}", true), None);
        assert_eq!(window("i x", true), None);
        assert_eq!(window("/a/w out;$p", true), None);
        // Line numbers, in an address or printed.
        assert_eq!(window("2p;$p", true), None);
        assert_eq!(window("$=", true), None);
        assert_eq!(window("0~3p;$p", true), None);
        assert_eq!(window("$-2,5p", true), None);
        assert_eq!(window("/a/,~4p;$p", true), None);
        // State that goes from record to record.
        assert_eq!(window("1!G;h;$!d", false), None);
        assert_eq!(window("/a/,/b/s/x/y/;$p", true), None);
        assert_eq!(window("$!N;P;D", false), None);
        assert_eq!(window("x;$p", true), None);
        assert_eq!(window(":a;$p;ba", true), None);
        assert_eq!(window("s//x/;$p", true), None);
        // Quitting before the end.
        assert_eq!(window("2q;$p", true), None);
        assert_eq!(window("/a/q;$p", true), None);
    }

    #[test]
    fn the_options_that_write_as_it_goes_or_edit_in_place_keep_the_whole_file() {
        let chunk = Chunk {
            source: ChunkSource::Expression(1),
            text: b"$p".to_vec(),
        };
        let script = parse(&[(chunk, ParseOptions::default())]).expect("parses");
        let quiet = Options {
            quiet: true,
            ..Options::default()
        };
        assert_eq!(tail_window(&script, &quiet), Some(1));
        for options in [
            Options {
                in_place: Some(String::new()),
                separate: true,
                ..quiet.clone()
            },
            Options {
                unbuffered: true,
                ..quiet.clone()
            },
            Options {
                debug: true,
                ..quiet.clone()
            },
            Options {
                follow_symlinks: true,
                ..quiet.clone()
            },
        ] {
            assert_eq!(tail_window(&script, &options), None);
        }
    }
}

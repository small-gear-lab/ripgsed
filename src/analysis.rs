//! Static analysis: can a script be run on chunks of records independently?
//!
//! A script is *record-local* when what it does to one record never depends on any other
//! record and it has no effect outside the output stream. Such a script can run on chunks in
//! any order (and in parallel) as long as the outputs are put back in input order. The test
//! is conservative: anything not proven record-local runs sequentially.

use crate::runtime::Options;
use crate::syntax::{Addr, Addr2, Command, Kind, Script, Selector};

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
    for command in &script.commands {
        command_local(command)?;
    }
    Ok(())
}

fn command_local(command: &Command) -> Result<(), Reason> {
    match &command.selector {
        None => {}
        Some(Selector::Single(a)) => addr_local(a)?,
        Some(Selector::Range(..)) => return Err(Reason::Construct("an address range")),
    }
    match &command.kind {
        // Everything below acts on the current record only.
        Kind::BlockStart(_)
        | Kind::BlockEnd
        | Kind::Label(_)
        | Kind::Branch { .. }
        | Kind::Append(_)
        | Kind::Insert(_)
        | Kind::Change(_)
        | Kind::Delete
        | Kind::Print
        | Kind::PrintFirst
        | Kind::Zap
        | Kind::LineNumber
        | Kind::List(_)
        | Kind::Translate(_)
        | Kind::Version => Ok(()),
        Kind::Substitute(sub) => {
            if sub.pattern.regex.is_none() {
                // `s//x/` reuses the regexp last used, which depends on the records before.
                Err(Reason::Construct("an empty regexp"))
            } else if sub.write.is_some() {
                Err(Reason::Construct("the w flag of s"))
            } else if sub.eval {
                Err(Reason::Construct("the e flag of s"))
            } else {
                Ok(())
            }
        }
        Kind::DeleteFirst | Kind::Next | Kind::NextAppend => Err(Reason::Construct("D, n or N")),
        Kind::Get | Kind::GetAppend | Kind::Hold | Kind::HoldAppend | Kind::Exchange => Err(Reason::Construct("the hold space")),
        Kind::Quit(_) | Kind::QuitSilent(_) => Err(Reason::Construct("q or Q")),
        Kind::FileName => Err(Reason::Construct("F")),
        Kind::ReadFile(_) | Kind::ReadLine(_) | Kind::WriteFile(_) | Kind::WriteFirstLine(_) => {
            Err(Reason::Construct("a file command"))
        }
        Kind::Execute(_) => Err(Reason::Construct("e")),
        Kind::Removed(_) => Err(Reason::Construct("L")),
    }
}

fn addr_local(addr: &Addr) -> Result<(), Reason> {
    match addr {
        Addr::Line(0) => Err(Reason::Construct("address 0")),
        Addr::Line(_) | Addr::Step { .. } => Ok(()),
        // `$` needs to know where the whole input ends.
        Addr::Last => Err(Reason::Construct("$")),
        Addr::Regex(p) if p.regex.is_none() => Err(Reason::Construct("an empty regexp")),
        Addr::Regex(_) => Ok(()),
    }
}

#[allow(dead_code)]
fn unused(_: &Addr2) {}

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
        for s in ["s/a/b/g", "/x/d", "y/abc/xyz/", "2~3p", "5s/a/b/", "s/a/b/;ta;s/c/d/;:a", "=", "/a/{s/a/b/;p}", "a text", "l"] {
            assert!(local(s), "{s} should be record-local");
        }
    }

    #[test]
    fn scripts_with_cross_record_state_are_not() {
        for s in ["N", "$p", "1,3p", "/a/,/b/d", "x", "G", "h", "n", "D", "q", "s//x/", "w out", "r f", "0,/a/d", "F", "s/a/b/w x"] {
            assert!(!local(s), "{s} must not be record-local");
        }
    }
}

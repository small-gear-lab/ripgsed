//! `--debug`: the canonical form of a program and the annotations of its execution.

use crate::syntax::{
    Addr, Addr2, CaseConversion, Command, Kind, Pattern, ReplacementPart, Script, Selector, Substitute,
};

/// The program in canonical form, one command per line, with two spaces of indentation
/// per open block.
pub fn program(script: &Script, utf8: bool) -> Vec<u8> {
    let mut out = b"SED PROGRAM:\n".to_vec();
    let mut depth = 1usize;
    for command in &script.commands {
        if matches!(command.kind, Kind::Version) {
            // `v` leaves no command behind in GNU sed.
            continue;
        }
        if matches!(command.kind, Kind::BlockEnd) {
            depth = depth.saturating_sub(1);
        }
        out.extend(std::iter::repeat_n(b' ', depth * 2));
        out.extend_from_slice(&command_text(command, utf8));
        out.push(b'\n');
        if matches!(command.kind, Kind::BlockStart(_)) {
            depth += 1;
        }
    }
    out
}

/// One command as GNU sed's debug output writes it: address, `!`, a space, the command.
pub fn command_text(command: &Command, utf8: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(selector) = &command.selector {
        selector_text(selector, &mut out);
        if command.negate {
            out.push(b'!');
        }
        out.push(b' ');
    } else if command.negate {
        out.push(b'!');
    }
    kind_text(&command.kind, utf8, &mut out);
    out
}

fn selector_text(selector: &Selector, out: &mut Vec<u8>) {
    match selector {
        Selector::Single(a) => addr_text(a, out),
        Selector::Range(a1, a2) => {
            addr_text(a1, out);
            out.push(b',');
            match a2 {
                Addr2::Line(n) => out.extend_from_slice(n.to_string().as_bytes()),
                Addr2::Last => out.push(b'$'),
                Addr2::Regex(p) => pattern_text(p, out),
                Addr2::Step { first, step } => out.extend_from_slice(format!("{first}~{step}").as_bytes()),
                Addr2::Plus(0) | Addr2::Multiple(0) => out.extend_from_slice(b"[ADDR-NULL]"),
                Addr2::Plus(n) => out.extend_from_slice(format!("+{n}").as_bytes()),
                Addr2::Multiple(n) => out.extend_from_slice(format!("~{n}").as_bytes()),
            }
        }
    }
}

fn addr_text(addr: &Addr, out: &mut Vec<u8>) {
    match addr {
        Addr::Line(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Addr::Last => out.push(b'$'),
        Addr::Null => out.extend_from_slice(b"[ADDR-NULL]"),
        Addr::Step { first, step: 0 } => out.extend_from_slice(first.to_string().as_bytes()),
        Addr::Step { first, step } => out.extend_from_slice(format!("{first}~{step}").as_bytes()),
        Addr::Regex(p) => pattern_text(p, out),
    }
}

/// `/regexp/` with its flags.
fn pattern_text(p: &Pattern, out: &mut Vec<u8>) {
    out.push(b'/');
    push_regex_source(&p.source, out);
    out.push(b'/');
    if p.icase {
        out.push(b'I');
    }
    if p.multiline {
        out.push(b'M');
    }
}

fn kind_text(kind: &Kind, utf8: bool, out: &mut Vec<u8>) {
    let simple = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
    match kind {
        Kind::BlockStart(_) => simple(out, "{"),
        Kind::BlockEnd => simple(out, "}"),
        Kind::Label(name) => {
            out.push(b':');
            out.extend_from_slice(name);
        }
        Kind::Branch { when, label, .. } => {
            simple(
                out,
                match when {
                    crate::syntax::BranchWhen::Always => "b",
                    crate::syntax::BranchWhen::IfSubstituted => "t",
                    crate::syntax::BranchWhen::IfNotSubstituted => "T",
                },
            );
            if !label.is_empty() {
                out.push(b' ');
                out.extend_from_slice(label);
            }
        }
        Kind::Append(text) => text_command(out, b'a', text),
        Kind::Insert(text) => text_command(out, b'i', text),
        Kind::Change(text) => text_command(out, b'c', text),
        Kind::Delete => simple(out, "d"),
        Kind::DeleteFirst => simple(out, "D"),
        Kind::Next => simple(out, "n"),
        Kind::NextAppend => simple(out, "N"),
        Kind::Print => simple(out, "p"),
        Kind::PrintFirst => simple(out, "P"),
        Kind::Get => simple(out, "g"),
        Kind::GetAppend => simple(out, "G"),
        Kind::Hold => simple(out, "h"),
        Kind::HoldAppend => simple(out, "H"),
        Kind::Exchange => simple(out, "x"),
        Kind::Zap => simple(out, "z"),
        Kind::LineNumber => simple(out, "="),
        Kind::List(width) => {
            simple(out, "l");
            if let Some(w) = width {
                out.extend_from_slice(format!(" {w}").as_bytes());
            }
        }
        Kind::Removed(width) => {
            simple(out, "L");
            if let Some(w) = width {
                out.extend_from_slice(format!(" {w}").as_bytes());
            }
        }
        Kind::Quit(code) => exit_command(out, b'q', *code),
        Kind::QuitSilent(code) => exit_command(out, b'Q', *code),
        Kind::FileName => simple(out, "F"),
        Kind::ReadFile(name) | Kind::ReadFileNow(name) => file_command(out, b'r', name),
        Kind::ReadLine(name) => file_command(out, b'R', name),
        Kind::WriteFile(name) => {
            out.push(b'w');
            out.extend_from_slice(name);
        }
        Kind::WriteFirstLine(name) => {
            out.push(b'W');
            out.extend_from_slice(name);
        }
        Kind::Execute(command) => {
            // The command's text keeps its own newline; the listing adds another.
            out.extend_from_slice(b"e ");
            out.extend_from_slice(command);
        }
        Kind::Translate(pairs) => {
            out.extend_from_slice(b"y/");
            if utf8 {
                for (from, _) in pairs {
                    out.extend_from_slice(from);
                }
                out.push(b'/');
                for (_, to) in pairs {
                    out.extend_from_slice(to);
                }
            } else {
                // A single-byte locale keeps a table: the bytes that change, in byte order.
                let mut table: [Option<u8>; 256] = [None; 256];
                for (from, to) in pairs {
                    if let (Some(&f), Some(&t)) = (from.first(), to.first()) {
                        table[f as usize] = Some(t);
                    }
                }
                let changed: Vec<(u8, u8)> =
                    (0..=255u8).filter_map(|b| table[b as usize].filter(|&t| t != b).map(|t| (b, t))).collect();
                out.extend(changed.iter().map(|&(f, _)| f));
                out.push(b'/');
                out.extend(changed.iter().map(|&(_, t)| t));
            }
            out.push(b'/');
        }
        Kind::Substitute(sub) => substitute_text(sub, out),
        Kind::Version => unreachable!("`v` is skipped by program() and never run in debug mode"),
    }
}

/// `a\text` followed by the text's own newline (the listing adds one more).
fn text_command(out: &mut Vec<u8>, letter: u8, text: &[u8]) {
    out.push(letter);
    out.push(b'\\');
    out.extend_from_slice(text);
}

fn exit_command(out: &mut Vec<u8>, letter: u8, code: Option<i32>) {
    out.push(letter);
    if let Some(code) = code {
        out.extend_from_slice(format!(" {code}").as_bytes());
    }
}

fn file_command(out: &mut Vec<u8>, letter: u8, name: &[u8]) {
    out.push(letter);
    out.push(b' ');
    out.extend_from_slice(name);
}

fn substitute_text(sub: &Substitute, out: &mut Vec<u8>) {
    out.extend_from_slice(b"s/");
    push_regex_source(&sub.pattern.source, out);
    out.push(b'/');
    replacement_text(&sub.replacement, out);
    out.push(b'/');
    if sub.pattern.icase {
        out.push(b'i');
    }
    if sub.pattern.multiline {
        out.push(b'm');
    }
    if sub.global {
        out.push(b'g');
    }
    if sub.eval {
        out.push(b'e');
    }
    if sub.print_before || sub.print_after {
        out.push(b'p');
    }
    if sub.occurrence > 0 {
        out.extend_from_slice(sub.occurrence.to_string().as_bytes());
    }
    if let Some(name) = &sub.write {
        out.push(b'w');
        out.extend_from_slice(name);
    }
}

/// The replacement as GNU sed lists it: the literal text as it is, and the case conversions the
/// way its list of segments gives them. A conversion ends the segment before it, and so does a
/// reference. A segment shows only what changes from the one before, a conversion that comes
/// last shows nothing, and a one-shot `\u` or `\l` ends after the reference it applies to.
fn replacement_text(parts: &[ReplacementPart], out: &mut Vec<u8>) {
    // (continuous conversion: `U` or `L`, one-shot conversion: `u` or `l`)
    type State = (Option<u8>, Option<u8>);
    let mut current: State = (None, None);
    let mut shown: State = (None, None);
    let mut prefix: Vec<u8> = Vec::new();

    fn close(current: State, shown: &mut State, prefix: &mut Vec<u8>, reference: Option<&[u8]>, out: &mut Vec<u8>) {
        if *shown != current {
            if current == (None, None) {
                out.extend_from_slice(b"\\E");
            } else {
                if shown.0 != current.0 || (shown.1.is_some() && current.1.is_none()) {
                    if let Some(c) = current.0 {
                        out.extend_from_slice(&[b'\\', c]);
                    }
                }
                if shown.1 != current.1 {
                    if let Some(c) = current.1 {
                        out.extend_from_slice(&[b'\\', c]);
                    }
                }
            }
            *shown = current;
        }
        out.append(prefix);
        if let Some(r) = reference {
            out.extend_from_slice(r);
        }
    }

    for part in parts {
        match part {
            ReplacementPart::Literal(text) => prefix.extend_from_slice(text),
            ReplacementPart::Group(n) => {
                let reference = if *n == 0 { b"&".to_vec() } else { format!("\\{n}").into_bytes() };
                close(current, &mut shown, &mut prefix, Some(&reference), out);
                current.1 = None;
            }
            ReplacementPart::Case(c) => {
                close(current, &mut shown, &mut prefix, None, out);
                match c {
                    CaseConversion::Upper => current = (Some(b'U'), None),
                    CaseConversion::Lower => current = (Some(b'L'), None),
                    CaseConversion::Stop => current = (None, None),
                    CaseConversion::UpperNext => current.1 = Some(b'u'),
                    CaseConversion::LowerNext => current.1 = Some(b'l'),
                }
            }
        }
    }
    if !prefix.is_empty() {
        close(current, &mut shown, &mut prefix, None, out);
    }
}

/// The regexp as the listing shows it: the text GNU sed keeps, written one byte at a time as
/// `escape` does, with a slash escaped as well.
fn push_regex_source(source: &[u8], out: &mut Vec<u8>) {
    for &b in source {
        if b == b'/' {
            out.extend_from_slice(b"\\/");
        } else {
            push_char(b, out);
        }
    }
}

/// One byte as the trace writes it: the usual escapes for control characters, a backslash
/// doubled, and every other byte outside ASCII as the octal of its signed value (GNU sed does
/// not look at the locale here).
pub fn push_char(b: u8, out: &mut Vec<u8>) {
    match b {
        0x07 => out.extend_from_slice(b"\\a"),
        b'\t' => out.extend_from_slice(b"\\t"),
        b'\n' => out.extend_from_slice(b"\\n"),
        0x0b => out.extend_from_slice(b"\\v"),
        0x0c => out.extend_from_slice(b"\\f"),
        b'\r' => out.extend_from_slice(b"\\r"),
        b'\\' => out.extend_from_slice(b"\\\\"),
        0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\o{b:03o}").as_bytes()),
        0x80..=0xff => out.extend_from_slice(format!("\\o{:o}", b as i8 as i32 as u32).as_bytes()),
        _ => out.push(b),
    }
}

/// A pattern or hold space as the trace shows it.
pub fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        push_char(b, &mut out);
    }
    out
}

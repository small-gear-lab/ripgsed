//! `--debug`: the canonical form of a program and the annotations of its execution.

use crate::syntax::{
    Addr, Addr2, CaseConversion, Command, Kind, Pattern, ReplacementPart, Script, Selector, Substitute,
};

/// The program in canonical form, one command per line, with two spaces of indentation
/// per open block.
pub fn program(script: &Script) -> Vec<u8> {
    let mut out = b"SED PROGRAM:\n".to_vec();
    let mut depth = 1usize;
    for command in &script.commands {
        if matches!(command.kind, Kind::BlockEnd) {
            depth = depth.saturating_sub(1);
        }
        out.extend(std::iter::repeat_n(b' ', depth * 2));
        out.extend_from_slice(&command_text(command));
        out.push(b'\n');
        if matches!(command.kind, Kind::BlockStart(_)) {
            depth += 1;
        }
    }
    out
}

/// One command as GNU sed's debug output writes it: address, `!`, a space, the command.
pub fn command_text(command: &Command) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(selector) = &command.selector {
        selector_text(selector, &mut out);
        if command.negate {
            out.push(b'!');
        }
        out.push(b' ');
    } else if command.negate {
        out.extend_from_slice(b"! ");
    }
    kind_text(&command.kind, &mut out);
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

fn kind_text(kind: &Kind, out: &mut Vec<u8>) {
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
        Kind::Quit(code) => exit_command(out, b'q', *code),
        Kind::QuitSilent(code) => exit_command(out, b'Q', *code),
        Kind::FileName => simple(out, "F"),
        Kind::ReadFile(name) => file_command(out, b'r', name),
        Kind::ReadLine(name) => file_command(out, b'R', name),
        Kind::WriteFile(name) => file_command(out, b'w', name),
        Kind::WriteFirstLine(name) => file_command(out, b'W', name),
        Kind::Execute(command) => {
            out.push(b'e');
            if !command.is_empty() {
                out.push(b' ');
                out.extend_from_slice(command);
            }
        }
        Kind::Translate(pairs) => {
            out.extend_from_slice(b"y/");
            for (from, _) in pairs {
                out.extend_from_slice(from);
            }
            out.push(b'/');
            for (_, to) in pairs {
                out.extend_from_slice(to);
            }
            out.push(b'/');
        }
        Kind::Substitute(sub) => substitute_text(sub, out),
        Kind::Version => simple(out, "v"),
    }
}

/// `a\text` followed by the text's own newline (the listing adds one more).
fn text_command(out: &mut Vec<u8>, letter: u8, text: &[u8]) {
    out.push(letter);
    out.push(b'\\');
    out.extend_from_slice(text);
}

fn exit_command(out: &mut Vec<u8>, letter: u8, code: i32) {
    out.push(letter);
    if code != 0 {
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
    for part in &sub.replacement {
        match part {
            ReplacementPart::Literal(text) => {
                for &b in text {
                    if matches!(b, b'\\' | b'&') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
            }
            ReplacementPart::Group(0) => out.push(b'&'),
            ReplacementPart::Group(n) => out.extend_from_slice(format!("\\{n}").as_bytes()),
            ReplacementPart::Case(c) => out.extend_from_slice(match c {
                CaseConversion::Upper => b"\\U",
                CaseConversion::Lower => b"\\L",
                CaseConversion::UpperNext => b"\\u",
                CaseConversion::LowerNext => b"\\l",
                CaseConversion::Stop => b"\\E",
            }),
        }
    }
    out.push(b'/');
    if sub.occurrence > 1 {
        out.extend_from_slice(sub.occurrence.to_string().as_bytes());
    }
    if sub.global {
        out.push(b'g');
    }
    if sub.print_before || sub.print_after {
        out.push(b'p');
    }
    if sub.pattern.icase {
        out.push(b'i');
    }
    if sub.pattern.multiline {
        out.push(b'm');
    }
    if sub.eval {
        out.push(b'e');
    }
    if let Some(name) = &sub.write {
        out.push(b'w');
        out.extend_from_slice(name);
    }
}

/// The regexp as the listing shows it: every backslash doubled.
fn push_regex_source(source: &[u8], out: &mut Vec<u8>) {
    for &b in source {
        if b == b'\\' {
            out.push(b'\\');
        }
        out.push(b);
    }
}

/// A pattern or hold space as the trace shows it: a newline is written `\n`.
pub fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\n' => out.extend_from_slice(b"\\n"),
            other => out.push(other),
        }
    }
    out
}

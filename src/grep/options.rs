//! The command line of the grep mode: the options of GNU grep and what they set.

use std::io::Read;

/// How the patterns are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Basic,
    Extended,
    Fixed,
    Perl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryFiles {
    Binary,
    Text,
    WithoutMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Directories {
    Read,
    Recurse,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Devices {
    /// Devices named on the command line are read, those met while recursing are skipped.
    ReadCommandLine,
    Read,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListFiles {
    No,
    Matching,
    NonMatching,
}

/// Everything the command line decided.
#[derive(Debug, Clone)]
pub struct Options {
    pub kind: Kind,
    /// The patterns, one per line of every `-e` and `-f`.
    pub patterns: Vec<Vec<u8>>,
    pub ignore_case: bool,
    pub word: bool,
    pub line: bool,
    pub invert: bool,
    pub null_data: bool,
    pub no_messages: bool,
    pub quiet: bool,
    pub max_count: Option<u64>,
    pub byte_offset: bool,
    pub line_number: bool,
    pub line_buffered: bool,
    /// `Some(true)` for `-H`, `Some(false)` for `-h`.
    pub with_filename: Option<bool>,
    pub label: Option<Vec<u8>>,
    pub only_matching: bool,
    pub count: bool,
    pub list_files: ListFiles,
    pub binary_files: BinaryFiles,
    pub directories: Directories,
    pub devices: Devices,
    pub dereference_recursive: bool,
    /// `--include` (true) and `--exclude` (false) in the order they were given.
    pub file_patterns: Vec<(bool, Vec<u8>)>,
    pub exclude_dir: Vec<Vec<u8>>,
    pub initial_tab: bool,
    pub null: bool,
    /// Context lines before and after (`None`: not asked for).
    pub before: Option<u64>,
    pub after: Option<u64>,
    /// `Some(None)` for `--no-group-separator`.
    pub group_separator: Option<Option<Vec<u8>>>,
    pub color: bool,
    pub files: Vec<Vec<u8>>,
}

/// What the command line asks for.
pub enum Parsed {
    Run(Box<Options>),
    /// Print this text on standard output and end with this status.
    Print(String, i32),
    /// The command line is wrong: the messages are printed, and this is the status.
    Error(i32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arg {
    No,
    Required,
    Optional,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opt {
    After,
    Before,
    Context,
    BasicRegexp,
    ExtendedRegexp,
    FixedStrings,
    PerlRegexp,
    Matcher,
    Regexp,
    File,
    IgnoreCase,
    NoIgnoreCase,
    Word,
    Line,
    NullData,
    NoMessages,
    Invert,
    Version,
    Help,
    MaxCount,
    ByteOffset,
    LineNumber,
    LineBuffered,
    WithFilename,
    NoFilename,
    Label,
    OnlyMatching,
    Quiet,
    BinaryFiles,
    Text,
    BinaryWithoutMatch,
    Directories,
    Devices,
    Recursive,
    DereferenceRecursive,
    Include,
    Exclude,
    ExcludeFrom,
    ExcludeDir,
    FilesWithoutMatch,
    FilesWithMatches,
    Count,
    InitialTab,
    Null,
    GroupSeparator,
    NoGroupSeparator,
    Color,
    Binary,
    ObsoleteIgnoreCase,
    UnixByteOffsets,
    Digit(u8),
}

const LONG_OPTIONS: &[(&str, Arg, Opt)] = &[
    ("basic-regexp", Arg::No, Opt::BasicRegexp),
    ("extended-regexp", Arg::No, Opt::ExtendedRegexp),
    ("fixed-regexp", Arg::No, Opt::FixedStrings),
    ("fixed-strings", Arg::No, Opt::FixedStrings),
    ("perl-regexp", Arg::No, Opt::PerlRegexp),
    ("after-context", Arg::Required, Opt::After),
    ("before-context", Arg::Required, Opt::Before),
    ("binary-files", Arg::Required, Opt::BinaryFiles),
    ("byte-offset", Arg::No, Opt::ByteOffset),
    ("context", Arg::Required, Opt::Context),
    ("color", Arg::Optional, Opt::Color),
    ("colour", Arg::Optional, Opt::Color),
    ("count", Arg::No, Opt::Count),
    ("dereference-recursive", Arg::No, Opt::DereferenceRecursive),
    ("devices", Arg::Required, Opt::Devices),
    ("directories", Arg::Required, Opt::Directories),
    ("exclude", Arg::Required, Opt::Exclude),
    ("exclude-from", Arg::Required, Opt::ExcludeFrom),
    ("exclude-dir", Arg::Required, Opt::ExcludeDir),
    ("file", Arg::Required, Opt::File),
    ("files-with-matches", Arg::No, Opt::FilesWithMatches),
    ("files-without-match", Arg::No, Opt::FilesWithoutMatch),
    ("group-separator", Arg::Required, Opt::GroupSeparator),
    ("help", Arg::No, Opt::Help),
    ("include", Arg::Required, Opt::Include),
    ("ignore-case", Arg::No, Opt::IgnoreCase),
    ("no-ignore-case", Arg::No, Opt::NoIgnoreCase),
    ("initial-tab", Arg::No, Opt::InitialTab),
    ("label", Arg::Required, Opt::Label),
    ("line-buffered", Arg::No, Opt::LineBuffered),
    ("line-number", Arg::No, Opt::LineNumber),
    ("line-regexp", Arg::No, Opt::Line),
    ("max-count", Arg::Required, Opt::MaxCount),
    ("no-filename", Arg::No, Opt::NoFilename),
    ("no-group-separator", Arg::No, Opt::NoGroupSeparator),
    ("no-messages", Arg::No, Opt::NoMessages),
    ("null", Arg::No, Opt::Null),
    ("null-data", Arg::No, Opt::NullData),
    ("only-matching", Arg::No, Opt::OnlyMatching),
    ("quiet", Arg::No, Opt::Quiet),
    ("recursive", Arg::No, Opt::Recursive),
    ("regexp", Arg::Required, Opt::Regexp),
    ("invert-match", Arg::No, Opt::Invert),
    ("silent", Arg::No, Opt::Quiet),
    ("text", Arg::No, Opt::Text),
    ("binary", Arg::No, Opt::Binary),
    ("unix-byte-offsets", Arg::No, Opt::UnixByteOffsets),
    ("version", Arg::No, Opt::Version),
    ("with-filename", Arg::No, Opt::WithFilename),
    ("word-regexp", Arg::No, Opt::Word),
];

const SHORT_OPTIONS: &[(char, Arg, Opt)] = &[
    ('A', Arg::Required, Opt::After),
    ('B', Arg::Required, Opt::Before),
    ('C', Arg::Required, Opt::Context),
    ('D', Arg::Required, Opt::Devices),
    ('E', Arg::No, Opt::ExtendedRegexp),
    ('F', Arg::No, Opt::FixedStrings),
    ('G', Arg::No, Opt::BasicRegexp),
    ('H', Arg::No, Opt::WithFilename),
    ('I', Arg::No, Opt::BinaryWithoutMatch),
    ('L', Arg::No, Opt::FilesWithoutMatch),
    ('P', Arg::No, Opt::PerlRegexp),
    ('R', Arg::No, Opt::DereferenceRecursive),
    ('T', Arg::No, Opt::InitialTab),
    ('U', Arg::No, Opt::Binary),
    ('V', Arg::No, Opt::Version),
    ('X', Arg::Required, Opt::Matcher),
    ('a', Arg::No, Opt::Text),
    ('b', Arg::No, Opt::ByteOffset),
    ('c', Arg::No, Opt::Count),
    ('d', Arg::Required, Opt::Directories),
    ('e', Arg::Required, Opt::Regexp),
    ('f', Arg::Required, Opt::File),
    ('h', Arg::No, Opt::NoFilename),
    ('i', Arg::No, Opt::IgnoreCase),
    ('l', Arg::No, Opt::FilesWithMatches),
    ('m', Arg::Required, Opt::MaxCount),
    ('n', Arg::No, Opt::LineNumber),
    ('o', Arg::No, Opt::OnlyMatching),
    ('q', Arg::No, Opt::Quiet),
    ('r', Arg::No, Opt::Recursive),
    ('s', Arg::No, Opt::NoMessages),
    ('u', Arg::No, Opt::UnixByteOffsets),
    ('v', Arg::No, Opt::Invert),
    ('w', Arg::No, Opt::Word),
    ('x', Arg::No, Opt::Line),
    ('y', Arg::No, Opt::ObsoleteIgnoreCase),
    ('Z', Arg::No, Opt::Null),
    ('z', Arg::No, Opt::NullData),
];

/// Whether the release of GNU grep that is followed has the option (see `GNU_GREP_3_12`).
fn available(opt: Opt) -> bool {
    !(super::GNU_GREP_3_12 && opt == Opt::UnixByteOffsets)
}

/// Resolves `--name`, accepting an unambiguous prefix as getopt_long does.
fn find_long(name: &str, program: &str) -> Result<(&'static str, Arg, Opt), String> {
    if let Some(&(n, a, o)) = LONG_OPTIONS
        .iter()
        .filter(|e| available(e.2))
        .find(|(n, ..)| *n == name)
    {
        return Ok((n, a, o));
    }
    let candidates: Vec<_> = LONG_OPTIONS
        .iter()
        .filter(|e| available(e.2))
        .filter(|(n, ..)| n.starts_with(name))
        .collect();
    // Names that stand for the same option are not ambiguous.
    let mut distinct: Vec<&(&str, Arg, Opt)> = Vec::new();
    for c in &candidates {
        if !distinct.iter().any(|d| d.2 == c.2 && d.1 == c.1) {
            distinct.push(c);
        }
    }
    match distinct.as_slice() {
        [] => Err(format!("{program}: unrecognized option '--{name}'")),
        [(n, a, o)] => Ok((n, *a, *o)),
        _ => {
            let list: Vec<String> = candidates
                .iter()
                .map(|(n, ..)| format!("'--{n}'"))
                .collect();
            Err(format!(
                "{program}: option '--{name}' is ambiguous; possibilities: {}",
                list.join(" ")
            ))
        }
    }
}

pub fn usage_lines(program: &str) -> String {
    format!(
        "Usage: {program} [OPTION]... PATTERNS [FILE]...\nTry '{program} --help' for more information.\n"
    )
}

fn bad_usage(message: Option<String>, program: &str) -> Parsed {
    if let Some(m) = message {
        eprintln!("{m}");
    }
    eprint!("{}", usage_lines(program));
    Parsed::Error(2)
}

/// Reads a number as GNU grep reads a context length: all digits, no sign.
fn context_length(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(text.parse::<u64>().unwrap_or(u64::MAX))
}

/// Parses the arguments (without the program name). `default_kind` is what the name the
/// program runs under stands for (`egrep` is `-E`).
pub fn parse(args: Vec<Vec<u8>>, program: &str, default_kind: Kind) -> Parsed {
    let mut o = Options {
        kind: default_kind,
        patterns: Vec::new(),
        ignore_case: false,
        word: false,
        line: false,
        invert: false,
        null_data: false,
        no_messages: false,
        quiet: false,
        max_count: None,
        byte_offset: false,
        line_number: false,
        line_buffered: false,
        with_filename: None,
        label: None,
        only_matching: false,
        count: false,
        list_files: ListFiles::No,
        binary_files: BinaryFiles::Binary,
        directories: Directories::Read,
        devices: Devices::ReadCommandLine,
        dereference_recursive: false,
        file_patterns: Vec::new(),
        exclude_dir: Vec::new(),
        initial_tab: false,
        null: false,
        before: None,
        after: None,
        group_separator: None,
        color: false,
        files: Vec::new(),
    };
    let posixly_correct = std::env::var_os("POSIXLY_CORRECT").is_some();
    let mut have_pattern = false;
    let mut operands: Vec<Vec<u8>> = Vec::new();
    // `egrep` and `fgrep` are `grep -E` and `grep -F`: another matcher is a conflict.
    let mut matcher_set: Option<Kind> = (default_kind != Kind::Basic).then_some(default_kind);
    let mut default_context: Option<u64> = None;
    let mut show_help = false;
    let mut show_version = false;
    let mut color_when: Option<String> = None;
    let mut digit_args: Option<(usize, String)> = None;
    let mut status_error = false;

    let mut args = args.into_iter().enumerate();
    let mut options_done = false;
    // The text of an argument as a string for the option machinery (bytes that are not text are
    // kept in the values that need them).
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();

    macro_rules! usage {
        ($msg:expr) => {
            return bad_usage(Some($msg), program)
        };
    }

    while let Some((index, arg)) = args.next() {
        if options_done || arg == b"-" || !arg.starts_with(b"-") {
            if posixly_correct {
                options_done = true;
            }
            operands.push(arg);
            continue;
        }
        if arg == b"--" {
            options_done = true;
            continue;
        }
        let mut pending: Vec<(Opt, Option<Vec<u8>>, String)> = Vec::new();
        if arg.starts_with(b"--") {
            let long = text(&arg[2..]);
            let (name, inline) = match long.split_once('=') {
                Some((n, _)) => {
                    let split = 2 + n.len() + 1;
                    (n.to_string(), Some(arg[split.min(arg.len())..].to_vec()))
                }
                None => (long.clone(), None),
            };
            let (full, kind, opt) = match find_long(&name, program) {
                Ok(r) => r,
                Err(m) => usage!(m),
            };
            let value = match (kind, inline) {
                (Arg::No, Some(_)) => usage!(format!(
                    "{program}: option '--{full}' doesn't allow an argument"
                )),
                (Arg::Required, None) => match args.next() {
                    Some((_, v)) => Some(v),
                    None => usage!(format!("{program}: option '--{full}' requires an argument")),
                },
                (_, v) => v,
            };
            pending.push((opt, value, format!("--{full}")));
        } else {
            let letters = &arg[1..];
            let mut i = 0;
            while i < letters.len() {
                let c = letters[i] as char;
                i += 1;
                if c.is_ascii_digit() {
                    pending.push((Opt::Digit(c as u8 - b'0'), None, format!("-{c}")));
                    continue;
                }
                let Some(&(_, kind, opt)) = SHORT_OPTIONS
                    .iter()
                    .filter(|e| available(e.2))
                    .find(|(l, ..)| *l == c)
                else {
                    if super::GNU_GREP_3_12 && c == 'u' {
                        // GNU grep 3.12 still lists `u` among the short options and no longer handles
                        // it, so it gives the usage without saying that the option is invalid.
                        return bad_usage(None, program);
                    }
                    usage!(format!("{program}: invalid option -- '{c}'"));
                };
                let value = match kind {
                    Arg::No => None,
                    Arg::Optional | Arg::Required => {
                        let rest = letters[i..].to_vec();
                        i = letters.len();
                        if !rest.is_empty() {
                            Some(rest)
                        } else if kind == Arg::Required {
                            match args.next() {
                                Some((_, v)) => Some(v),
                                None => usage!(format!(
                                    "{program}: option requires an argument -- '{c}'"
                                )),
                            }
                        } else {
                            None
                        }
                    }
                };
                pending.push((opt, value, format!("-{c}")));
            }
        }
        for (opt, value, _name) in pending {
            let value_text = value.as_deref().map(text).unwrap_or_default();
            match opt {
                Opt::Digit(d) => {
                    // Digits of one argument make one number: -12 is 12; a digit in another
                    // argument starts a new number.
                    let mut entry = digit_args
                        .take()
                        .filter(|(i, _)| *i == index)
                        .unwrap_or((index, String::new()));
                    entry.1.push((b'0' + d) as char);
                    match context_length(&entry.1) {
                        Some(n) => default_context = Some(n),
                        None => {
                            eprintln!("{program}: {}: invalid context length argument", entry.1);
                            return Parsed::Error(2);
                        }
                    }
                    digit_args = Some(entry);
                }
                Opt::After | Opt::Before | Opt::Context => {
                    let Some(n) = context_length(&value_text) else {
                        eprintln!("{program}: {value_text}: invalid context length argument");
                        return Parsed::Error(2);
                    };
                    match opt {
                        Opt::After => o.after = Some(n),
                        Opt::Before => o.before = Some(n),
                        _ => default_context = Some(n),
                    }
                }
                Opt::BasicRegexp | Opt::ExtendedRegexp | Opt::FixedStrings | Opt::PerlRegexp => {
                    let kind = match opt {
                        Opt::BasicRegexp => Kind::Basic,
                        Opt::ExtendedRegexp => Kind::Extended,
                        Opt::FixedStrings => Kind::Fixed,
                        _ => Kind::Perl,
                    };
                    set_matcher(&mut matcher_set, kind);
                }
                Opt::Matcher => {
                    let kind = match value_text.as_str() {
                        "grep" => Kind::Basic,
                        "egrep" | "awk" | "gawk" | "posixawk" => Kind::Extended,
                        "fgrep" => Kind::Fixed,
                        "perl" => Kind::Perl,
                        other => {
                            eprintln!("{program}: invalid matcher {other}");
                            return Parsed::Error(2);
                        }
                    };
                    set_matcher(&mut matcher_set, kind);
                }
                Opt::Regexp => {
                    have_pattern = true;
                    add_patterns(&mut o.patterns, value.as_deref().unwrap_or_default());
                }
                Opt::File => {
                    have_pattern = true;
                    let name = value_text.clone();
                    let bytes = if name == "-" {
                        let mut buf = Vec::new();
                        std::io::stdin().read_to_end(&mut buf).map(|_| buf)
                    } else {
                        std::fs::read(crate::io::bytes_path(value.as_deref().unwrap_or_default()))
                    };
                    match bytes {
                        Ok(mut b) => {
                            if !b.is_empty() {
                                // The patterns of the file are its lines; a last line without a
                                // newline counts too.
                                if b.last() == Some(&b'\n') {
                                    b.pop();
                                }
                                add_patterns(&mut o.patterns, &b);
                            }
                        }
                        Err(e) => {
                            eprintln!("{program}: {name}: {}", crate::io::strerror(&e));
                            return Parsed::Error(2);
                        }
                    }
                }
                Opt::IgnoreCase | Opt::ObsoleteIgnoreCase => o.ignore_case = true,
                Opt::NoIgnoreCase => o.ignore_case = false,
                Opt::Word => o.word = true,
                Opt::Line => o.line = true,
                Opt::NullData => o.null_data = true,
                Opt::NoMessages => o.no_messages = true,
                Opt::Invert => o.invert = true,
                Opt::Version => show_version = true,
                Opt::Help => show_help = true,
                Opt::MaxCount => {
                    let t = value_text.trim_start();
                    let (neg, digits) = match t.strip_prefix('-') {
                        Some(d) => (true, d),
                        None => (false, t.strip_prefix('+').unwrap_or(t)),
                    };
                    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                        eprintln!("{program}: invalid max count");
                        return Parsed::Error(2);
                    }
                    o.max_count = if neg {
                        None
                    } else {
                        Some(digits.parse().unwrap_or(u64::MAX))
                    };
                }
                Opt::ByteOffset => o.byte_offset = true,
                Opt::LineNumber => o.line_number = true,
                Opt::LineBuffered => o.line_buffered = true,
                Opt::WithFilename => o.with_filename = Some(true),
                Opt::NoFilename => o.with_filename = Some(false),
                Opt::Label => o.label = value,
                Opt::OnlyMatching => o.only_matching = true,
                Opt::Quiet => o.quiet = true,
                Opt::BinaryFiles => {
                    o.binary_files = match value_text.as_str() {
                        "binary" => BinaryFiles::Binary,
                        "text" => BinaryFiles::Text,
                        "without-match" => BinaryFiles::WithoutMatch,
                        _ => {
                            eprintln!("{program}: unknown binary-files type");
                            return Parsed::Error(2);
                        }
                    };
                }
                Opt::Text => o.binary_files = BinaryFiles::Text,
                Opt::BinaryWithoutMatch => o.binary_files = BinaryFiles::WithoutMatch,
                Opt::Directories => {
                    o.directories = match value_text.as_str() {
                        "read" => Directories::Read,
                        "recurse" => Directories::Recurse,
                        "skip" => Directories::Skip,
                        other => {
                            eprintln!("{program}: invalid argument '{other}' for '--directories'");
                            eprintln!(
                                "Valid arguments are:\n  - 'read'\n  - 'recurse'\n  - 'skip'"
                            );
                            eprint!("{}", usage_lines(program));
                            return Parsed::Error(1);
                        }
                    };
                }
                Opt::Devices => {
                    o.devices = match value_text.as_str() {
                        "read" => Devices::Read,
                        "skip" => Devices::Skip,
                        _ => {
                            eprintln!("{program}: unknown devices method");
                            return Parsed::Error(2);
                        }
                    };
                }
                Opt::Recursive => {
                    o.directories = Directories::Recurse;
                    o.dereference_recursive = false;
                }
                Opt::DereferenceRecursive => {
                    o.directories = Directories::Recurse;
                    o.dereference_recursive = true;
                }
                Opt::Include => o.file_patterns.push((true, value.unwrap_or_default())),
                Opt::Exclude => o.file_patterns.push((false, value.unwrap_or_default())),
                Opt::ExcludeFrom => {
                    let name = value_text.clone();
                    match std::fs::read(crate::io::bytes_path(value.as_deref().unwrap_or_default()))
                    {
                        Ok(b) => {
                            for line in b.split(|&c| c == b'\n') {
                                if !line.is_empty() {
                                    o.file_patterns.push((false, line.to_vec()));
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("{program}: {name}: {}", crate::io::strerror(&e));
                            return Parsed::Error(2);
                        }
                    }
                }
                Opt::ExcludeDir => {
                    let mut pattern = value.unwrap_or_default();
                    while pattern.len() > 1 && pattern.last() == Some(&b'/') {
                        pattern.pop();
                    }
                    o.exclude_dir.push(pattern);
                }
                Opt::FilesWithoutMatch => o.list_files = ListFiles::NonMatching,
                Opt::FilesWithMatches => o.list_files = ListFiles::Matching,
                Opt::Count => o.count = true,
                Opt::InitialTab => o.initial_tab = true,
                Opt::Null => o.null = true,
                Opt::GroupSeparator => o.group_separator = Some(Some(value.unwrap_or_default())),
                Opt::NoGroupSeparator => o.group_separator = Some(None),
                Opt::Color => color_when = Some(value_text),
                Opt::Binary => {}
                // (An option of GNU grep 3.11 and earlier; 3.12 does not have it.)
                Opt::UnixByteOffsets if !super::GNU_GREP_3_12 => {
                    eprintln!("{program}: warning: --unix-byte-offsets (-u) is obsolete");
                }
                Opt::UnixByteOffsets => unreachable!("rejected when the options are read"),
            }
            if status_error {
                return Parsed::Error(2);
            }
        }
    }
    let _ = &mut status_error;

    if show_version {
        return Parsed::Print(version_text(program), 0);
    }
    if show_help {
        return Parsed::Print(help_text(program), 0);
    }
    if let Some(kind) = matcher_set {
        o.kind = kind;
    }
    if !have_pattern {
        if operands.is_empty() {
            eprint!("{}", usage_lines(program));
            return Parsed::Error(2);
        }
        let first = operands.remove(0);
        add_patterns(&mut o.patterns, &first);
    }
    o.files = operands;
    if o.quiet {
        // -q says nothing, whatever -l and -L ask for.
        o.list_files = ListFiles::No;
    }
    if o.before.is_none() {
        o.before = default_context;
    }
    if o.after.is_none() {
        o.after = default_context;
    }
    if let Some(when) = color_when {
        o.color = match when.as_str() {
            "" | "auto" | "tty" | "if-tty" => color_auto(),
            "always" | "yes" | "force" => true,
            "never" | "no" | "none" => false,
            _ => {
                // GNU grep 3.11 shows the help text for a WHEN it does not know.
                return Parsed::Print(help_text(program), 0);
            }
        };
    }
    Parsed::Run(Box::new(o))
}

fn color_auto() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::env::var("TERM").is_ok_and(|t| t != "dumb")
}

fn set_matcher(slot: &mut Option<Kind>, kind: Kind) {
    // Two different matchers are a mistake; the same one twice is not.
    if let Some(old) = *slot {
        if old != kind {
            eprintln!("grep: conflicting matchers specified");
            std::process::exit(2);
        }
    }
    *slot = Some(kind);
}

/// Adds the patterns of a text: one for each line of it.
fn add_patterns(patterns: &mut Vec<Vec<u8>>, text: &[u8]) {
    for line in text.split(|&b| b == b'\n') {
        patterns.push(line.to_vec());
    }
}

pub fn version_text(program: &str) -> String {
    let _ = program;
    format!(
        "grep (ripgsed) {}\nLicense: MIT OR Apache-2.0.\nBehavior target: GNU grep 3.11 and later (experimental, see https://github.com/small-gear-lab/ripgsed)\n",
        env!("CARGO_PKG_VERSION")
    )
}

pub fn help_text(program: &str) -> String {
    format!(
        "Usage: {program} [OPTION]... PATTERNS [FILE]...
Search for PATTERNS in each FILE.
Example: {program} -i 'hello world' menu.h main.c
PATTERNS can contain multiple patterns separated by newlines.

Pattern selection and interpretation:
  -E, --extended-regexp     PATTERNS are extended regular expressions
  -F, --fixed-strings       PATTERNS are strings
  -G, --basic-regexp        PATTERNS are basic regular expressions
  -P, --perl-regexp         PATTERNS are Perl regular expressions
  -e, --regexp=PATTERNS     use PATTERNS for matching
  -f, --file=FILE           take PATTERNS from FILE
  -i, --ignore-case         ignore case distinctions in patterns and data
      --no-ignore-case      do not ignore case distinctions (default)
  -w, --word-regexp         match only whole words
  -x, --line-regexp         match only whole lines
  -z, --null-data           a data line ends in 0 byte, not newline

Miscellaneous:
  -s, --no-messages         suppress error messages
  -v, --invert-match        select non-matching lines
  -V, --version             display version information and exit
      --help                display this help text and exit

Output control:
  -m, --max-count=NUM       stop after NUM selected lines
  -b, --byte-offset         print the byte offset with output lines
  -n, --line-number         print line number with output lines
      --line-buffered       flush output on every line
  -H, --with-filename       print file name with output lines
  -h, --no-filename         suppress the file name prefix on output
      --label=LABEL         use LABEL as the standard input file name prefix
  -o, --only-matching       show only nonempty parts of lines that match
  -q, --quiet, --silent     suppress all normal output
      --binary-files=TYPE   assume that binary files are TYPE;
                            TYPE is 'binary', 'text', or 'without-match'
  -a, --text                equivalent to --binary-files=text
  -I                        equivalent to --binary-files=without-match
  -d, --directories=ACTION  how to handle directories;
                            ACTION is 'read', 'recurse', or 'skip'
  -D, --devices=ACTION      how to handle devices, FIFOs and sockets;
                            ACTION is 'read' or 'skip'
  -r, --recursive           like --directories=recurse
  -R, --dereference-recursive  likewise, but follow all symlinks
      --include=GLOB        search only files that match GLOB (a file pattern)
      --exclude=GLOB        skip files that match GLOB
      --exclude-from=FILE   skip files that match any file pattern from FILE
      --exclude-dir=GLOB    skip directories that match GLOB
  -L, --files-without-match  print only names of FILEs with no selected lines
  -l, --files-with-matches  print only names of FILEs with selected lines
  -c, --count               print only a count of selected lines per FILE
  -T, --initial-tab         make tabs line up (if needed)
  -Z, --null                print 0 byte after FILE name

Context control:
  -B, --before-context=NUM  print NUM lines of leading context
  -A, --after-context=NUM   print NUM lines of trailing context
  -C, --context=NUM         print NUM lines of output context
  -NUM                      same as --context=NUM
      --group-separator=SEP  print SEP on line between matches with context
      --no-group-separator  do not print separator for matches with context
      --color[=WHEN],
      --colour[=WHEN]       use markers to highlight the matching strings;
                            WHEN is 'always', 'never', or 'auto'
  -U, --binary              do not strip CR characters at EOL (MSDOS/Windows)

When FILE is '-', read standard input.  With no FILE, read '.' if
recursive, '-' otherwise.  With fewer than two FILEs, assume -h.
Exit status is 0 if any line is selected, 1 otherwise;
if any error occurs and -q is not given, the exit status is 2.
"
    )
}

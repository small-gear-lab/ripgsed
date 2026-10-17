//! Unit tests of the engine. Expected values were observed with GNU sed 4.9.

use super::*;

fn syn(ere: bool) -> Syntax {
    // The expected values were observed with GNU sed, quirks included.
    Syntax {
        extended: ere,
        glibc_compat: true,
        ..Syntax::default()
    }
}

/// Applies `s/pat/<&>[\1]...[\n]/g` the way GNU sed does and returns the result.
fn sub_g(pat: &str, input: &str, s: &Syntax) -> String {
    let re = Regex::compile(pat.as_bytes(), s).unwrap_or_else(|e| panic!("{pat}: {e}"));
    let hay = input.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0;
    let mut copied = 0;
    let mut caps = Captures::default();
    let mut last_end = None;
    while pos <= hay.len() && re.captures_at(hay, pos, &mut caps) {
        let (s0, e0) = caps.whole();
        if s0 == e0 && Some(s0) == last_end {
            // GNU sed does not take an empty match right after a match.
            if s0 >= hay.len() {
                break;
            }
            pos = s0 + 1;
            last_end = None;
            continue;
        }
        last_end = Some(e0);
        out.extend_from_slice(&hay[copied..s0]);
        out.push(b'<');
        out.extend_from_slice(&hay[s0..e0]);
        out.push(b'>');
        for g in 1..=re.group_count() {
            out.push(b'[');
            if let Some((a, b)) = caps.get(g) {
                out.extend_from_slice(&hay[a..b]);
            }
            out.push(b']');
        }
        copied = e0;
        if s0 == e0 {
            if s0 < hay.len() {
                out.push(hay[s0]);
            }
            copied = s0 + 1;
            pos = s0 + 1;
        } else {
            pos = e0;
        }
    }
    if copied < hay.len() {
        out.extend_from_slice(&hay[copied..]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn err(pat: &str, s: &Syntax) -> String {
    match Regex::compile(pat.as_bytes(), s) {
        Ok(_) => "ok".to_string(),
        Err(e) => e.message,
    }
}

#[test]
fn literal_match() {
    let re = Regex::compile(b"old", &Syntax::default()).unwrap();
    let mut caps = Captures::default();
    assert!(re.captures_at(b"an old one", 0, &mut caps));
    assert_eq!(caps.whole(), (3, 6));
    assert!(!re.captures_at(b"an old one", 4, &mut caps));
}

#[test]
fn leftmost_longest_and_subexpressions() {
    let e = syn(true);
    assert_eq!(sub_g("(a|ab)(c|bcd)(d*)", "abcd", &e), "<abcd>[a][bcd][]");
    assert_eq!(sub_g("(a*)+", "aa", &e), "<aa>[aa]");
    assert_eq!(sub_g("(a|ab)*c", "ababc", &e), "<ababc>[ab]");
    assert_eq!(sub_g("(a*)(ab)*b", "aabb", &e), "<aabb>[a][ab]");
    assert_eq!(sub_g("((a)|b)*", "ab", &e), "<ab>[b][a]");
    assert_eq!(sub_g("(a?){3}", "aa", &e), "<aa>[]");
    assert_eq!(sub_g("(a?){2,3}", "a", &e), "<a>[a]");
    assert_eq!(sub_g("(a|aa)(a|aa)", "aaa", &e), "<aaa>[a][aa]");
    assert_eq!(sub_g("(x)?(x)?y", "xy", &e), "<xy>[x][]");
    assert_eq!(sub_g("a|ab", "a|abc", &syn(false)), "<a|ab>c");
}

#[test]
fn alternation_longest_bre() {
    assert_eq!(sub_g("a\\|ab", "abc", &syn(false)), "<ab>c");
}

#[test]
fn empty_matches_global() {
    assert_eq!(sub_g("x*", "abc", &syn(false)), "<>a<>b<>c<>");
    assert_eq!(sub_g("b*", "abc", &syn(false)), "<>a<b>c<>");
}

#[test]
fn backreferences() {
    let e = syn(true);
    assert_eq!(sub_g("(a*)\\1", "aaaa", &e), "<aaaa>[aa]");
    assert_eq!(sub_g("(a*)\\1b", "aaab", &e), "a<aab>[a]");
    assert_eq!(sub_g("(a*)*\\1", "aab", &e), "<aa>[a]b<>[]");
    assert_eq!(sub_g("(.)\\1*", "aaab", &e), "<aaa>[a]<b>[b]");
    assert_eq!(sub_g("((a)|b)\\2", "bb", &e), "bb");
}

#[test]
fn backreference_to_repeated_group() {
    let e = syn(true);
    // A bounded repetition `{m,n}` is seen by a back-reference only after `m` or `n` iterations.
    assert_eq!(sub_g("(a){1,3}\\1", "aaa", &e), "<aa>[a]a");
    assert_eq!(sub_g("(a){1,3}\\1", "aaaa", &e), "<aaaa>[a]");
    assert_eq!(sub_g("(a){0,2}\\1", "aa", &e), "aa");
    assert_eq!(sub_g("(a){0,2}\\1", "aaa", &e), "<aaa>[a]");
    // A copy of the group is not seen where another copy could also end.
    assert_eq!(sub_g("(a+)+\\1", "aaaa", &e), "<aa>[a]<aa>[a]");
    assert_eq!(sub_g("(a+)+\\1b", "aaab", &e), "a<aab>[a]");
    assert_eq!(sub_g("(a|aa){2}\\1", "aaa", &e), "aaa");
    assert_eq!(sub_g("(a|aa){2}\\1", "aaaa", &e), "<aaaa>[a]");
    assert_eq!(sub_g("(a|b|ab){2}\\1", "aabab", &e), "<aabab>[ab]");
    assert_eq!(sub_g("(a|b|ab){2,}\\1", "aabab", &e), "aabab");
    assert_eq!(sub_g("a?(a|b){2}\\1", "aabb", &e), "<aabb>[b]");
    // The same for a group that can be empty.
    assert_eq!(sub_g("(a*)+\\1", "aaaa", &e), "<aaaa>[aa]");
    assert_eq!(sub_g("(a*)+\\1", "aaa", &e), "<aa>[a]a<>[]");
    assert_eq!(sub_g("(a*){2}\\1", "aaaaaa", &e), "aaaaaa");
}

#[test]
fn backreference_to_group_with_empty_iteration() {
    let e = syn(true);
    // An empty last iteration of a loop is seen as an empty group, and the group is reported
    // with its earlier value.
    assert_eq!(sub_g("(a?)*\\1", "a", &e), "<a>[a]");
    assert_eq!(sub_g("(a|)*\\1", "aa", &e), "<aa>[a]");
    assert_eq!(sub_g("(a*)*\\1b", "ab", &e), "<ab>[]");
    assert_eq!(sub_g("(a?)+b\\1", "aab", &e), "<aab>[a]");
    // ... unless another copy of the group could also end there.
    assert_eq!(sub_g("(a?)+b\\1", "ab", &e), "a<b>[]");
    assert_eq!(sub_g("(a?)+\\1", "ab", &e), "<>[]a<>[]b<>[]");
    // A refused empty group at the start of the search ends the search.
    assert_eq!(sub_g("(a?){2}\\1", "aaa", &e), "<aaa>[a]");
    assert_eq!(sub_g("(a?){2}\\1", "baaa", &e), "baaa");
    assert_eq!(sub_g("(a?){2}\\1b", "baaab", &e), "baaab");
    assert_eq!(sub_g("(a?){2}\\1|b", "abab", &e), "abab");
    assert_eq!(sub_g("(b?)*\\1b", "abb", &e), "a<bb>[b]");
}

#[test]
fn compile_errors() {
    let b = syn(false);
    let e = syn(true);
    assert_eq!(err("a**", &b), "Invalid preceding regular expression");
    assert_eq!(err("a**", &e), "ok");
    assert_eq!(err("\\{2\\}", &b), "Invalid preceding regular expression");
    assert_eq!(err("*a", &e), "Invalid preceding regular expression");
    assert_eq!(err("*a", &b), "ok");
    assert_eq!(err("a\\{32768\\}", &b), "Regular expression too big");
    assert_eq!(err("a\\{2,1\\}", &b), "Invalid content of \\{\\}");
    assert_eq!(err("a{1", &e), "Unmatched \\{");
    assert_eq!(err("a(.)|b\\1", &e), "Invalid back reference");
    assert_eq!(err("((a)|(b))\\3", &e), "ok");
    assert_eq!(err("(a", &e), "Unmatched ( or \\(");
    assert_eq!(err("a)", &e), "Unmatched ) or \\)");
    assert_eq!(err("[z-a]", &b), "Invalid range end");
    assert_eq!(err("[[:foo:]]", &b), "Invalid character class name");
    assert_eq!(err("[[.ab.]]", &b), "Invalid collation character");
    assert_eq!(
        err("[:alpha:]", &b),
        "character class syntax is [[:space:]], not [:space:]"
    );
    assert_eq!(err("a\\", &b), "Trailing backslash");
}

#[test]
fn anchors_and_words() {
    let b = syn(false);
    assert_eq!(sub_g("\\b", "ab cd", &b), "<>ab<> <>cd<>");
    assert_eq!(sub_g("a^b", "a^b", &b), "<a^b>");
    assert_eq!(sub_g("^*", "*a", &b), "<*>a");
    let m = Syntax {
        multiline: true,
        ..b
    };
    assert_eq!(sub_g("^.", "ab\ncd", &m), "<a>b\n<c>d");
}

#[test]
fn no_exponential_blowup() {
    let hay = "a".repeat(5000);
    let re = Regex::compile(b"(a*)*b", &syn(true)).unwrap();
    let mut caps = Captures::default();
    assert!(!re.captures_at(hay.as_bytes(), 0, &mut caps));
    let re = Regex::compile(b"\\(a*\\)*\\1b", &syn(false)).unwrap();
    assert!(!re.captures_at(&hay.as_bytes()[..40], 0, &mut caps));
}

#[test]
fn address_semantics() {
    let m = Syntax {
        multiline: true,
        ..syn(false)
    };
    let re = Regex::compile(b"\\`b", &m).unwrap();
    assert!(re.is_match(b"a\nb\nc"));
    let mut caps = Captures::default();
    assert!(!re.captures_at(b"a\nb\nc", 0, &mut caps));
    let re = Regex::compile(b"b$(.)*", &syn(true)).unwrap();
    assert!(re.is_match(b"ab\nbb"));
    assert!(re.captures_at(b"ab\nbb", 0, &mut caps));
    assert_eq!(caps.whole(), (4, 5));
}

#[test]
fn null_data() {
    let z = Syntax {
        multiline: true,
        null_data: true,
        ..syn(false)
    };
    assert_eq!(sub_g("^.", "ab\ncd", &z), "<a>b\ncd");
    assert_eq!(sub_g("^.", "ab\0cd", &z), "<a>b\0<c>d");
    assert_eq!(sub_g("..", "b\n_ A", &z), "b\n<_ >A");
}

#[test]
fn utf8() {
    let u = Syntax {
        locale: Locale::Utf8,
        ..syn(false)
    };
    assert_eq!(sub_g(".", "é中a", &u), "<é><中><a>");
    assert_eq!(sub_g("\\w\\+", "é中x «x»", &u), "<é中x> «<x>»");
    let ui = Syntax { icase: true, ..u };
    assert_eq!(sub_g("ß", "ß ẞ", &ui), "<ß> ẞ");
    assert_eq!(sub_g("ǆ", "Ǆǅǆ", &ui), "<Ǆ><ǅ><ǆ>");
    assert_eq!(err("[à-é]", &u), "Invalid collation character");
    // Raw bytes match inside a character; `.` does not match invalid bytes.
    let re = Regex::compile(b"\xa9", &u).unwrap();
    let mut caps = Captures::default();
    assert!(re.captures_at("é".as_bytes(), 0, &mut caps));
    assert_eq!(caps.whole(), (1, 2));
    let re = Regex::compile(b"a.b", &u).unwrap();
    assert!(!re.captures_at(b"a\xffb", 0, &mut caps));
}

#[test]
fn long_lines_are_fast() {
    let mut hay = "ab".repeat(20000);
    hay.push('c');
    let mut caps = Captures::default();
    for (pat, ere) in [
        ("(a|b)*c", true),
        ("\\(.*\\)c", false),
        ("([ab]+)+x", true),
        ("[a-z]*$", false),
    ] {
        let re = Regex::compile(pat.as_bytes(), &syn(ere)).unwrap();
        re.captures_at(hay.as_bytes(), 0, &mut caps);
    }
}

/// The DFA scans and the NFA simulation find the same matches.
#[test]
fn dfa_agrees_with_simulation() {
    let atoms = [
        "a",
        "b",
        "c",
        ".",
        "[ab]",
        "[^a]",
        "a*",
        "b+",
        "c?",
        "(a|b)",
        "(ab|a)",
        "x*",
        "^",
        "$",
        "\\b",
        "\\<",
        "\\>",
        "\\w",
        "\\W",
        "a{2}",
        "[a-c]*",
        "(a*)b",
        "()",
        "(abc|c)",
        "(a*c|b)",
        "(ab|bca|c)",
        "(a)(b)",
        "(a(b))c",
        "((a)b)(c)",
        "()a",
        ".(b)",
    ];
    let hays = [
        "", "a", "ab", "abc", "aab b", "cab ab", "xx a_b", "b a", "  ab", "abcabc", "a b",
        "bbbaaa", "ca",
    ];
    let mut seed = 0x2545f4914f6cdd1du64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for round in 0..4000 {
        let ere = round % 2 == 0;
        let k = 1 + next(4);
        let mut pat = String::new();
        for _ in 0..k {
            let a = atoms[next(atoms.len())];
            pat.push_str(&if ere {
                a.to_string()
            } else {
                a.replace('(', "\\(")
                    .replace(')', "\\)")
                    .replace('|', "\\|")
                    .replace('{', "\\{")
                    .replace('}', "\\}")
                    .replace('+', "\\+")
                    .replace('?', "\\?")
            });
        }
        let mut s = syn(ere);
        s.icase = next(5) == 0;
        let Ok(mut re) = Regex::compile(pat.as_bytes(), &s) else {
            continue;
        };
        for hay in hays {
            for start in 0..=hay.len() {
                re.use_dfa = true;
                let mut a = Captures::default();
                let ra = re.captures_at(hay.as_bytes(), start, &mut a);
                let ea = re.is_match(hay.as_bytes());
                re.use_dfa = false;
                let fixed = re.fixed.take();
                let mut b = Captures::default();
                let rb = re.captures_at(hay.as_bytes(), start, &mut b);
                let eb = re.is_match(hay.as_bytes());
                re.fixed = fixed;
                assert_eq!(
                    (ra, &a.groups, ea),
                    (rb, &b.groups, eb),
                    "pattern {pat:?} ere={ere} hay {hay:?} start {start}"
                );
            }
        }
    }
}

#[test]
fn dfa_is_used_for_plain_patterns() {
    let re = Regex::compile(b"[0-9]+", &syn(true)).unwrap();
    let mut caps = Captures::default();
    assert!(re.captures_at(b"ab 123", 0, &mut caps));
    assert_eq!(caps.whole(), (3, 6));
    assert!(matches!(*re.dfa.borrow(), dfa::DfaSlot::Ready(_)));
}

/// The same, in a UTF-8 locale, on text with multibyte characters and invalid bytes.
#[test]
fn dfa_agrees_with_simulation_in_utf8() {
    let atoms = [
        "a",
        "é",
        "中",
        ".",
        "[^a]",
        "[é中a]",
        "[^é]",
        "[[:alpha:]]",
        "[[:upper:]]",
        "[[:punct:]]",
        "[a-é]",
        "(é|a)",
        "é*",
        "中+",
        ".*",
        "x?",
        "^",
        "$",
        "(é)(a)",
        "(中)",
        "[^[:space:]]",
        "\\w",
        "\\W",
        "\\<",
    ];
    let hays: [&[u8]; 14] = [
        b"",
        b"a",
        "é".as_bytes(),
        "aéa".as_bytes(),
        "中中é a".as_bytes(),
        b"\xff",
        b"a\xc3",
        b"\xe4\xb8a",
        "Éa é".as_bytes(),
        "x中Y".as_bytes(),
        b"\xc3\xa9\xc3",
        "é!é".as_bytes(),
        "ÿ中\u{1f600}a".as_bytes(),
        b"\xed\xa0\x80a",
    ];
    let mut seed = 0x9e3779b97f4a7c15u64;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for round in 0..3000 {
        let k = 1 + next(3);
        let mut pat = String::new();
        for _ in 0..k {
            pat.push_str(atoms[next(atoms.len())]);
        }
        let mut s = syn(round % 2 == 0);
        s.locale = Locale::Utf8;
        s.icase = next(8) == 0;
        let pat = if round % 2 == 0 {
            pat
        } else {
            pat.replace('(', "\\(").replace(')', "\\)")
        };
        let Ok(mut re) = Regex::compile(pat.as_bytes(), &s) else {
            continue;
        };
        for hay in hays {
            for start in 0..=hay.len() {
                re.use_dfa = true;
                let mut a = Captures::default();
                let ra = re.captures_at(hay, start, &mut a);
                let ea = re.is_match(hay);
                re.use_dfa = false;
                let fixed = re.fixed.take();
                let mut b = Captures::default();
                let rb = re.captures_at(hay, start, &mut b);
                let eb = re.is_match(hay);
                re.fixed = fixed;
                assert_eq!(
                    (ra, &a.groups, ea),
                    (rb, &b.groups, eb),
                    "pattern {pat:?} hay {hay:?} start {start}"
                );
            }
        }
    }
}

/// Without `glibc_compat` the match is the one the definition gives; with it, the C library's.
#[test]
fn glibc_quirks_are_opt_in() {
    fn span(pat: &str, text: &str, compat: bool) -> Option<(usize, usize)> {
        let s = Syntax {
            extended: true,
            glibc_compat: compat,
            ..Syntax::default()
        };
        let re = Regex::compile(pat.as_bytes(), &s).unwrap();
        let mut caps = Captures::default();
        re.captures_at(text.as_bytes(), 0, &mut caps)
            .then(|| caps.whole())
    }
    // Back-references to a variable-width group in a repetition: the longest match by default.
    for (pat, text, definition, glibc) in [
        ("(a+)+\\1", "aaaa", Some((0, 4)), Some((0, 2))),
        ("(a+)+\\1b", "aaab", Some((0, 4)), Some((1, 4))),
        ("(a*)+\\1", "aaa", Some((0, 3)), Some((0, 2))),
        ("(a*){2}\\1", "aaaaaa", Some((0, 6)), None),
        (
            "(ab*){1,9}\\1",
            "abbbabbabbbbb",
            Some((0, 10)),
            Some((4, 10)),
        ),
        // `\B` after a leading repetition: the empty match where `\B` holds, not after the run.
        ("b*\\B", "ab", Some((1, 1)), Some((2, 2))),
    ] {
        assert_eq!(
            span(pat, text, false),
            definition,
            "default: {pat} on {text}"
        );
        assert_eq!(
            span(pat, text, true),
            glibc,
            "glibc_compat: {pat} on {text}"
        );
    }
}

fn utf8_syntax(ere: bool) -> Syntax {
    Syntax {
        locale: Locale::Utf8,
        ..syn(ere)
    }
}

/// Whether the DFA path and the simulation path give the same answer for every start.
fn assert_dfa_agrees(pat: &[u8], s: &Syntax, hay: &[u8]) {
    let mut re =
        Regex::compile(pat, s).unwrap_or_else(|e| panic!("{}: {e}", String::from_utf8_lossy(pat)));
    for start in 0..=hay.len() {
        re.use_dfa = true;
        let mut a = Captures::default();
        let ra = re.captures_at(hay, start, &mut a);
        let ea = re.is_match(hay);
        re.use_dfa = false;
        let fixed = re.fixed.take();
        let mut b = Captures::default();
        let rb = re.captures_at(hay, start, &mut b);
        let eb = re.is_match(hay);
        re.fixed = fixed;
        assert_eq!(
            (ra, &a.groups, ea),
            (rb, &b.groups, eb),
            "pattern {:?} hay {:?} start {start}",
            String::from_utf8_lossy(pat),
            String::from_utf8_lossy(hay)
        );
    }
}

/// The regexp parser takes the escapes that the sed parser has normally converted already.
#[test]
fn the_regex_parser_accepts_gnu_escapes() {
    let b = syn(false);
    for (pat, text, want) in [
        ("a\\tb", "a\tb", "<a\tb>"),
        ("a\\ab", "a\u{7}b", "<a\u{7}b>"),
        ("a\\fb", "a\u{c}b", "<a\u{c}b>"),
        ("a\\vb", "a\u{b}b", "<a\u{b}b>"),
        ("a\\rb", "a\rb", "<a\rb>"),
        ("a\\x41b", "aAb", "<aAb>"),
        ("a\\d066b", "aBb", "<aBb>"),
        ("a\\o103b", "aCb", "<aCb>"),
        ("\\cA", "\u{1}", "<\u{1}>"),
        ("\\ca", "\u{1}", "<\u{1}>"),
        // Without digits the letter stands for itself.
        ("a\\xg", "axg", "<axg>"),
    ] {
        assert_eq!(sub_g(pat, text, &b), want, "{pat:?}");
    }
    assert_eq!(err("a\\c", &b), "ok");
}

#[test]
fn an_invalid_byte_in_a_bracket_expression() {
    let u = utf8_syntax(false);
    // The byte alone matches nothing; as the end of a range it is the character of that number.
    assert_eq!(sub_g("[\u{fffd}]", "a", &u), "a");
    let pat = |p: &[u8]| Regex::compile(p, &u).map(|_| ()).map_err(|e| e.message);
    assert_eq!(pat(b"[\xff]"), Ok(()));
    assert_eq!(pat(b"[\xffa]"), Ok(()));
    assert_eq!(pat(b"[a-\xff]"), Ok(()));
    assert_eq!(pat(b"[\xff-\xff]"), Ok(()));
    assert_eq!(pat(b"[\xff-z]"), Err("Invalid range end".to_string()));
    assert_eq!(
        pat("[é-\u{ff}]".as_bytes()),
        Err("Invalid collation character".to_string())
    );
    assert_eq!(pat(b"[[:alpha:]-z]"), Err("Invalid range end".to_string()));
    assert_eq!(pat(b"[a-c-e]"), Err("Invalid range end".to_string()));
    assert_eq!(pat(b"[a-c-]"), Ok(()));
    assert_eq!(
        pat(b"[[:alpha]"),
        Err("Unmatched [, [^, [:, [., or [=".to_string())
    );
    assert_eq!(pat(b"[a-[:alpha:]]"), Err("Invalid range end".to_string()));
    assert_eq!(pat(b"[a-[=b=]]"), Err("Invalid range end".to_string()));
    // Matching: the range a-\xff holds ASCII letters and Latin-1 characters, never the invalid byte.
    let re = Regex::compile(b"[a-\xff]", &u).unwrap();
    let mut caps = Captures::default();
    assert!(re.captures_at("é".as_bytes(), 0, &mut caps));
    assert!(!re.captures_at(b"\xff", 0, &mut caps));
    let re = Regex::compile(b"[\xff]", &u).unwrap();
    assert!(!re.captures_at(b"\xff", 0, &mut caps));
    assert!(!re.captures_at("\u{ff}".as_bytes(), 0, &mut caps));
}

#[test]
fn a_regex_that_is_too_big() {
    let e = syn(true);
    assert_eq!(
        err("((a{1000}){1000}){10}", &e),
        "Regular expression too big"
    );
}

#[test]
fn small_api_corners() {
    let e = syn(true);
    // An empty pattern, and one with only an empty group, take no literal shortcut.
    for pat in ["", "()", "(|)"] {
        let re = Regex::compile(pat.as_bytes(), &e).unwrap();
        let mut caps = Captures::default();
        assert!(re.captures_at(b"abc", 1, &mut caps), "{pat:?}");
        assert_eq!(caps.whole(), (1, 1));
        // A start past the end finds nothing.
        assert!(!re.captures_at(b"abc", 4, &mut caps));
    }
    let error = Regex::compile(b"a\\{", &syn(false)).unwrap_err();
    assert_eq!(error.to_string(), "Unmatched \\{");
    assert_eq!(format!("{error}"), error.message);
}

#[test]
fn dfa_slot_is_a_cache() {
    let re = Regex::compile(b"[0-9]+", &syn(true)).unwrap();
    assert!(re.is_match(b"x 12"));
    assert!(format!("{re:?}").contains("DfaSlot::Ready"));
    // A clone starts without automata and builds its own.
    let copy = re.clone();
    assert!(format!("{copy:?}").contains("DfaSlot::Unbuilt"));
    assert!(copy.is_match(b"x 12"));
    // Back-references are out of the DFA's reach.
    let with = Regex::compile(b"(a)\\1", &syn(true)).unwrap();
    let mut slot = dfa::DfaSlot::default();
    assert!(
        slot.get(&with.prog, &with.node, false, false, b'\n')
            .is_none()
    );
    assert!(format!("{slot:?}").contains("DfaSlot::Disabled"));
    assert!(
        slot.get(&with.prog, &with.node, false, false, b'\n')
            .is_none()
    );
}

/// A pattern with more than 2000 DFA states: the DFA gives up and the simulation answers.
#[test]
fn dfa_gives_up_on_too_many_states() {
    let mut hay = Vec::new();
    let mut x = 12345u32;
    for _ in 0..30000 {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        hay.push(if (x >> 16) & 1 == 0 { b'a' } else { b'b' });
    }
    hay.extend_from_slice(b"a");
    hay.extend_from_slice(b"abbababbabaab");
    hay.push(b'c');
    for pat in ["[ab]*a[ab]{13}c", "a[ab]{14}c", "[ab]{13}a[ab]{2}c$"] {
        let re = Regex::compile(pat.as_bytes(), &syn(true)).unwrap();
        let mut caps = Captures::default();
        let found = re.captures_at(&hay, 0, &mut caps);
        assert_eq!(re.is_match(&hay), found, "{pat}");
        let mut slow = re.clone();
        slow.use_dfa = false;
        let mut other = Captures::default();
        assert_eq!(slow.captures_at(&hay, 0, &mut other), found, "{pat}");
        assert_eq!(caps, other, "{pat}");
    }
}

/// Starts before the earliest end are tried one by one within a budget; running out of it falls
/// back to the simulation.
#[test]
fn dfa_gives_up_when_many_starts_fail_slowly() {
    let mut hay = vec![b'a'; 3000];
    hay.push(b'c');
    assert_dfa_agrees(b"a*b|c", &syn(true), &hay);
}

#[test]
fn dfa_stops_at_bytes_it_cannot_follow() {
    // Case folding keeps a UTF-8 pattern from being spelled out as bytes: the DFA stops before
    // the first non-ASCII byte and the search is redone by the simulation.
    let mut s = utf8_syntax(true);
    s.icase = true;
    for hay in ["aé", "éa", "a éa", "xxa é"] {
        assert_dfa_agrees(b"a", &s, hay.as_bytes());
        assert_dfa_agrees(b"a+b?", &s, hay.as_bytes());
    }
}

#[test]
fn utf8_sets_in_the_dfa() {
    // Word assertions and large or empty sets are not spelled out as bytes.
    let mut hays: Vec<&[u8]> = vec![
        b"",
        b"a b",
        "é a ü".as_bytes(),
        "中文 abc".as_bytes(),
        b"\xff a",
    ];
    hays.push("\u{1f600}x".as_bytes());
    for pat in [
        "\\bé",
        "\\<a",
        "a\\>",
        "\\Bb",
        "[[:graph:]]+",
        "[[:alpha:][:punct:][:digit:]]+",
        "[^a]",
        "[^é]+",
        "\\W+",
        "[\u{ff}]",
        "[\u{ff}-\u{ff}]",
        "[a-\u{ff}]+",
        "[^[:alpha:]]",
        ".",
    ] {
        for ere in [true, false] {
            let s = utf8_syntax(ere);
            if Regex::compile(pat.as_bytes(), &s).is_err() {
                continue;
            }
            for hay in &hays {
                assert_dfa_agrees(pat.as_bytes(), &s, hay);
            }
        }
    }
    // `[\xff]` holds no character at all.
    let s = utf8_syntax(false);
    let re = Regex::compile(b"[\xff]", &s).unwrap();
    assert!(!re.is_match(b"a\xff"));
    assert_dfa_agrees(b"[\xff]", &s, b"a\xff");
}

#[test]
fn multiline_negated_sets_in_utf8() {
    let mut s = utf8_syntax(true);
    s.multiline = true;
    for pat in ["[^a]+", "[^é]", "[^\\n]", ".", "[^a]b$"] {
        for hay in ["xx\nyy", "é\né", "ab\nab\n", "\n"] {
            assert_dfa_agrees(pat.as_bytes(), &s, hay.as_bytes());
        }
    }
}

#[test]
fn case_insensitive_back_references_in_utf8() {
    let mut s = utf8_syntax(true);
    s.icase = true;
    s.glibc_compat = false;
    let cases: [(&str, &[u8]); 6] = [
        ("(é+)\\1", "éÉéÉ".as_bytes()),
        ("(.)\\1", "ǅǆ".as_bytes()),
        ("(a.)\\1", b"a\xffA\xff"),
        ("(a.)\\1", b"a\xffa\xfe"),
        ("(a.)\\1", b"a\xc3A\xc3"),
        ("(a.)\\1", "aéAÉ".as_bytes()),
    ];
    for (pat, hay) in cases {
        let re = Regex::compile(pat.as_bytes(), &s).unwrap();
        let mut caps = Captures::default();
        let _ = re.captures_at(hay, 0, &mut caps);
    }
    // The ASCII fold in the C locale, and no fold at all.
    let mut c = syn(true);
    c.icase = true;
    assert_eq!(sub_g("(a+)\\1", "aAaA", &c), "<aAaA>[aA]");
    assert_eq!(sub_g("(a+)\\1", "aAaA", &syn(true)), "aAaA");
}

/// A very long match of a big program is searched with a hashed memo and without tables; the
/// limit is lowered here so that a short input takes that path.
#[test]
fn large_programs_use_a_hashed_memo() {
    for (pat, hay) in [
        ("(a+)(b*)c", "xaabbc"),
        ("(a|ab)(c|bcd)(d*)", "abcd"),
        ("(a*)*b", "aab"),
        ("((a)|b)+c", "abac"),
    ] {
        let e = syn(true);
        let re = Regex::compile(pat.as_bytes(), &e).unwrap();
        let mut normal = Captures::default();
        let found = re.captures_at(hay.as_bytes(), 0, &mut normal);
        exec::TEST_BIG_TABLE_BITS.with(|c| c.set(1));
        let mut big = Captures::default();
        let found_big = re.captures_at(hay.as_bytes(), 0, &mut big);
        exec::TEST_BIG_TABLE_BITS.with(|c| c.set(1 << 27));
        assert_eq!((found, normal), (found_big, big), "{pat}");
    }
}

/// A thread that started later than the best match is dropped when the best one is found.
#[test]
fn later_starts_are_dropped_once_a_match_is_found() {
    for (pat, hay, want) in [
        ("ab|bcd", "abcd", "<ab>cd"),
        ("abc|b", "abc", "<abc>"),
        ("a.c|bcd|cde", "abcde", "<abc>de"),
    ] {
        assert_eq!(sub_g(pat, hay, &syn(true)), want, "{pat}");
        assert_dfa_agrees(pat.as_bytes(), &syn(true), hay.as_bytes());
        let mut re = Regex::compile(pat.as_bytes(), &syn(true)).unwrap();
        re.use_dfa = false;
        let mut caps = Captures::default();
        assert!(re.captures_at(hay.as_bytes(), 0, &mut caps));
    }
}

/// A repetition of an empty-matching group meets the loop guard in the back-reference search.
#[test]
fn empty_iterations_with_back_references() {
    for (pat, hay) in [
        ("(a*)*\\1", "aab"),
        ("(|a)*\\1", "aab"),
        ("(()|a)+\\1", "aab"),
        ("(a?)*\\1b", "aab"),
        ("((a*)*)*\\1x", "aax"),
        ("(a*|b)*\\1c", "abc"),
    ] {
        for glibc in [false, true] {
            let s = Syntax {
                glibc_compat: glibc,
                ..syn(true)
            };
            let re = Regex::compile(pat.as_bytes(), &s).unwrap();
            let mut caps = Captures::default();
            let _ = re.captures_at(hay.as_bytes(), 0, &mut caps);
        }
    }
}

#[test]
fn back_reference_after_invalid_bytes_with_case_folding() {
    let mut s = utf8_syntax(true);
    s.icase = true;
    for hay in [
        &b"a\xc3\xa9a\xc3"[..],
        b"a\xc3\xa9a\xc3\xa8",
        "aéAÉ".as_bytes(),
        b"a\xc3\xa9A",
    ] {
        let re = Regex::compile(b"(a.)\\1", &s).unwrap();
        let mut caps = Captures::default();
        let found = re.captures_at(hay, 0, &mut caps);
        assert_eq!(found, hay == "aéAÉ".as_bytes(), "{hay:?}");
    }
}

#[test]
fn the_dfa_leaves_unspellable_sets_to_the_simulation() {
    let s = utf8_syntax(true);
    let hay = "aé中 \u{1f600}b".repeat(3);
    // The sets are too large to be written out as byte sequences all at once.
    for pat in [
        "[[:print:]][[:graph:]][[:alpha:]]+",
        "[[:alpha:]][[:alpha:]][[:alpha:]]",
        "[[:print:]]+[[:alnum:]]+[[:graph:]]+",
    ] {
        assert_dfa_agrees(pat.as_bytes(), &s, hay.as_bytes());
    }
    let many = "[[:alpha:]]".repeat(12);
    assert_dfa_agrees(
        many.as_bytes(),
        &s,
        "aéb中c\u{1f600}dé".repeat(2).as_bytes(),
    );
    // Case folding leaves a set to the simulation; so does an invalid byte next to a match.
    let mut f = utf8_syntax(true);
    f.icase = true;
    for pat in ["[a-c]", "[[:alpha:]]+", "é"] {
        for hay in ["aé", "é", "xéa", "bÉ"] {
            assert_dfa_agrees(pat.as_bytes(), &f, hay.as_bytes());
        }
        assert_dfa_agrees(pat.as_bytes(), &f, b"a\xff");
    }
}

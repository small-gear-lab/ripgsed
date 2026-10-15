//! Unit tests of the engine. Expected values were observed with GNU sed 4.9.

use super::*;

fn syn(ere: bool) -> Syntax {
    Syntax { extended: ere, ..Syntax::default() }
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
    assert_eq!(err("[:alpha:]", &b), "character class syntax is [[:space:]], not [:space:]");
    assert_eq!(err("a\\", &b), "Trailing backslash");
}

#[test]
fn anchors_and_words() {
    let b = syn(false);
    assert_eq!(sub_g("\\b", "ab cd", &b), "<>ab<> <>cd<>");
    assert_eq!(sub_g("a^b", "a^b", &b), "<a^b>");
    assert_eq!(sub_g("^*", "*a", &b), "<*>a");
    let m = Syntax { multiline: true, ..b };
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
    let m = Syntax { multiline: true, ..syn(false) };
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
    let z = Syntax { multiline: true, null_data: true, ..syn(false) };
    assert_eq!(sub_g("^.", "ab\ncd", &z), "<a>b\ncd");
    assert_eq!(sub_g("^.", "ab\0cd", &z), "<a>b\0<c>d");
    assert_eq!(sub_g("..", "b\n_ A", &z), "b\n<_ >A");
}

#[test]
fn utf8() {
    let u = Syntax { locale: Locale::Utf8, ..syn(false) };
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
    for (pat, ere) in [("(a|b)*c", true), ("\\(.*\\)c", false), ("([ab]+)+x", true), ("[a-z]*$", false)] {
        let re = Regex::compile(pat.as_bytes(), &syn(ere)).unwrap();
        re.captures_at(hay.as_bytes(), 0, &mut caps);
    }
}

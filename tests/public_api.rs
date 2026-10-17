//! The public interface only: `Script`, `Edit`, `Address`, `Pattern`, `Run`, `Error`.
//! The behaviour of the commands is the business of the fixtures; these tests check that the
//! ways to make a script give the same script, and that the settings of `Run` reach the run.

use ripgsed::{Address, Edit, Encoding, ErrorKind, Pattern, Run, Script};

fn run(script: &Script, input: &str) -> String {
    String::from_utf8(
        Run::new()
            .encoding(Encoding::Bytes)
            .apply(script, input.as_bytes())
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_script_written_in_sed_runs() {
    let script = Script::parse("s/a/b/;2q").unwrap();
    assert_eq!(run(&script, "a\na\na\n"), "b\nb\n");
}

#[test]
fn to_string_parses_back_into_an_equal_script() {
    for text in [
        "s/a/b/g",
        "1,3d",
        "$!{p;d}",
        "/x/I,+2s/o/0/2p",
        "y/abc/xyz/",
        "1~2a\\\nhello",
    ] {
        let script: Script = text.parse().unwrap();
        let again = Script::parse(&script.to_string()).unwrap();
        assert_eq!(script, again, "{text}");
    }
}

#[test]
fn a_builder_script_equals_the_script_written_by_hand() {
    let built = Edit::new()
        .when(Address::lines(1..=3))
        .delete()
        .substitute(Pattern::basic("foo"), "bar")
        .global()
        .build()
        .unwrap();
    assert_eq!(built.to_string(), "1,3d\ns/foo/bar/g");
    assert_eq!(built, Script::parse(&built.to_string()).unwrap());
    let input = "a\nb\nc\nfoo foo\nfoo\n";
    assert_eq!(run(&built, input), "bar bar\nbar\n");
    assert_eq!(
        run(&built, input),
        run(&Script::parse("1,3d;s/foo/bar/g").unwrap(), input)
    );
}

#[test]
fn addresses_of_every_shape() {
    let cases: Vec<(Address, &str, &str)> = vec![
        (Address::line(2), "a\nb\nc\n", "a\nX\nc\n"),
        (Address::last(), "a\nb\nc\n", "a\nb\nX\n"),
        (Address::step(1, 2), "a\nb\nc\nd\n", "X\nb\nX\nd\n"),
        (
            Address::matching(Pattern::basic("b+")),
            "b+\nbb\n",
            "X\nbb\n",
        ),
        (
            Address::matching(Pattern::basic("^b\\+$").ignore_case()),
            "BB\nab\n",
            "X\nab\n",
        ),
        (Address::lines(2..=3), "a\nb\nc\nd\n", "a\nX\nX\nd\n"),
        (
            Address::range(Address::line(1), Address::matching(Pattern::basic("c"))),
            "a\nb\nc\nd\n",
            "X\nX\nX\nd\n",
        ),
        (
            Address::following(Address::line(2), 1),
            "a\nb\nc\nd\n",
            "a\nX\nX\nd\n",
        ),
        (
            Address::until_multiple_of(Address::line(2), 3),
            "a\nb\nc\nd\n",
            "a\nX\nX\nd\n",
        ),
        (Address::lines(2..=3).not(), "a\nb\nc\nd\n", "X\nb\nc\nX\n"),
        (Address::line(2).not().not(), "a\nb\n", "a\nX\n"),
        (Address::all(), "a\nb\n", "X\nX\n"),
        (Address::all().not(), "a\nb\n", "a\nb\n"),
    ];
    for (address, input, expected) in cases {
        let text = format!("{address:?}");
        let script = Edit::new()
            .when(address)
            .substitute(Pattern::basic(".*"), "X")
            .build()
            .unwrap();
        assert_eq!(run(&script, input), expected, "{text}");
    }
}

#[test]
fn text_commands_keep_backslashes_and_lines() {
    let script = Edit::new()
        .when(Address::line(1))
        .append("two\\three\nfour")
        .build()
        .unwrap();
    assert_eq!(run(&script, "one\n"), "one\ntwo\\three\nfour\n");
    let script = Edit::new()
        .when(Address::line(1))
        .insert("  lead")
        .build()
        .unwrap();
    assert_eq!(run(&script, "one\n"), "  lead\none\n");
}

#[test]
fn replacements_and_patterns_with_slashes() {
    let script = Edit::new()
        .substitute(Pattern::basic("/usr/"), "/opt/")
        .build()
        .unwrap();
    assert_eq!(run(&script, "/usr/bin\n"), "/opt/bin\n");
    // `&` and `\1` mean what they mean in sed ...
    let script = Edit::new()
        .substitute(Pattern::basic("\\(a\\)b"), "[&|\\1]")
        .build()
        .unwrap();
    assert_eq!(run(&script, "ab\n"), "[ab|a]\n");
    // ... unless the text is literal.
    let script = Edit::new()
        .substitute_literal(Pattern::basic("a"), "[&|\\1/]")
        .build()
        .unwrap();
    assert_eq!(run(&script, "a\n"), "[&|\\1/]\n");
    let script = Edit::new()
        .substitute_literal(Pattern::basic("a"), "x\ny")
        .build()
        .unwrap();
    assert_eq!(run(&script, "a\n"), "x\ny\n");
}

#[test]
fn substitute_flags_and_translate() {
    let script = Edit::new()
        .substitute(Pattern::basic("a"), "b")
        .occurrence(2)
        .and_print()
        .build()
        .unwrap();
    assert_eq!(script.to_string(), "s/a/b/2p");
    let script = Edit::new().translate("a/\\", "x-_").build().unwrap();
    assert_eq!(run(&script, "a/\\\n"), "x-_\n");
}

#[test]
fn blocks_and_raw_commands() {
    let script = Edit::new()
        .when(Address::matching(Pattern::basic("keep")))
        .block(|b| b.print().substitute(Pattern::basic("keep"), "kept"))
        .when(Address::last())
        .raw("G")
        .build()
        .unwrap();
    assert_eq!(run(&script, "keep\nx\n"), "keep\nkept\nx\n\n");
}

#[test]
fn quit_and_the_exit_status() {
    let script = Edit::new().when(Address::line(2)).quit(7).build().unwrap();
    let mut out = Vec::new();
    let outcome = Run::new()
        .run(&script, &b"a\nb\nc\n"[..], &mut out)
        .unwrap();
    assert_eq!(outcome.exit_status, 7);
    assert_eq!(out, b"a\nb\n");
}

#[test]
fn the_settings_of_run_reach_the_run() {
    let script = Script::parse("p").unwrap();
    assert_eq!(
        String::from_utf8(Run::new().quiet(true).apply(&script, b"a\n").unwrap()).unwrap(),
        "a\n"
    );
    let script = Script::parse("s/^/>/").unwrap();
    assert_eq!(
        Run::new()
            .null_data(true)
            .apply(&script, b"a\0b\0")
            .unwrap(),
        b">a\0>b\0"
    );
    let script = Script::parse("l").unwrap();
    assert_eq!(
        Run::new()
            .quiet(true)
            .line_length(5)
            .apply(&script, b"abcdefgh\n")
            .unwrap(),
        b"abcd\\\nefgh$\n"
    );
}

#[test]
fn encoding_changes_what_a_dot_matches() {
    let script = Script::parse("s/./X/g").unwrap();
    let input = "\u{3042}\u{3044}\n".as_bytes();
    assert_eq!(
        Run::new()
            .encoding(Encoding::Utf8)
            .apply(&script, input)
            .unwrap(),
        b"XX\n"
    );
    assert_eq!(
        Run::new()
            .encoding(Encoding::Bytes)
            .apply(&script, input)
            .unwrap(),
        b"XXXXXX\n"
    );
}

#[test]
fn sandbox_rejects_commands_that_touch_files() {
    let script = Script::parse("w /dev/null").unwrap();
    let error = Run::new().sandbox(true).apply(&script, b"a\n").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Syntax);
    assert!(
        error
            .message()
            .contains("e/r/w commands disabled in sandbox mode"),
        "{error}"
    );
}

#[test]
fn mistakes_are_reported_with_the_message_of_sed() {
    let error = Script::parse("s/a/b").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Syntax);
    assert_eq!(
        error.to_string(),
        "-e expression #1, char 5: unterminated `s' command"
    );
    assert_eq!(error.exit_status(), 1);
    // An address count the command does not take is the parser's check, in the builder too.
    let error = Edit::new()
        .when(Address::lines(1..=2))
        .quit(0)
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "-e expression #1, char 4: command only uses one address"
    );
}

#[test]
fn builder_mistakes_are_caught_when_the_script_is_built() {
    let error = Edit::new().global().build().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Build);
    let error = Edit::new()
        .substitute(Pattern::basic("a"), "b")
        .delete()
        .global()
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Build);
    let error = Edit::new().when(Address::line(1)).build().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Build);
    let error = Edit::new()
        .substitute(Pattern::basic("a"), "b")
        .substitute(Pattern::extended("a+"), "b")
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Build);
    let error = Edit::new()
        .when(Address::range(Address::lines(1..=2), Address::line(5)))
        .delete()
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Build);
}

#[test]
fn an_extended_script_is_marked_and_parses_back_extended() {
    let script = Edit::new()
        .substitute(Pattern::extended("(a|b)+"), "<&>")
        .build()
        .unwrap();
    assert_eq!(script.dialect(), ripgsed::Dialect::Extended);
    assert_eq!(run(&script, "abab c\n"), "<abab> c\n");
    let again = Script::parse_extended(&script.to_string()).unwrap();
    assert_eq!(script, again);
}

#[test]
fn a_failing_writer_is_an_error_of_the_run() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let script = Script::parse("p").unwrap();
    let error = Run::new().run(&script, &b"a\n"[..], Broken).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Run);
}

#[test]
fn the_output_is_written_while_the_input_is_read() {
    // A reader that gives lines one at a time and a writer that records when it was written to.
    struct Slow(Vec<&'static [u8]>);
    impl std::io::Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop() {
                Some(chunk) => {
                    buf[..chunk.len()].copy_from_slice(chunk);
                    Ok(chunk.len())
                }
                None => Ok(0),
            }
        }
    }
    let script = Script::parse("s/a/b/").unwrap();
    let mut out = Vec::new();
    Run::new()
        .run(&script, Slow(vec![b"a\n", b"a\n"]), &mut out)
        .unwrap();
    assert_eq!(out, b"b\nb\n");
}

fn big_input() -> Vec<u8> {
    let mut input = Vec::new();
    for i in 0..300_000u32 {
        input.extend_from_slice(format!("line {i} alpha beta\n").as_bytes());
    }
    input.extend_from_slice(b"no newline at the end");
    input
}

#[test]
fn threads_give_the_same_bytes() {
    let input = big_input();
    assert!(input.len() > 4 << 20);
    for text in [
        "s/alpha/ALPHA/g",
        "/9$/d",
        "s/\\([a-z]*\\) \\([0-9]*\\)/\\2 \\1/",
        "y/abc/xyz/;p",
    ] {
        let script = Script::parse(text).unwrap();
        let one = Run::new()
            .encoding(Encoding::Bytes)
            .apply(&script, &input)
            .unwrap();
        let many = Run::new()
            .encoding(Encoding::Bytes)
            .jobs(4)
            .apply(&script, &input)
            .unwrap();
        assert!(one == many, "{text}");
        let mut out = Vec::new();
        Run::new()
            .encoding(Encoding::Bytes)
            .jobs(0)
            .run_parallel(&script, &input[..], &mut out)
            .unwrap();
        assert!(one == out, "{text}");
    }
}

#[test]
fn a_script_that_cannot_be_spread_runs_on_one_thread() {
    // `$` and `N` need to know what comes next, and `q` stops the whole run.
    let input = big_input();
    for text in ["$d", "N;P;D", "5q3", "$!{h;d};G"] {
        let script = Script::parse(text).unwrap();
        let one = Run::new().apply(&script, &input).unwrap();
        let many = Run::new().jobs(4).apply(&script, &input).unwrap();
        assert!(one == many, "{text}");
    }
    let script = Script::parse("5q3").unwrap();
    let outcome = Run::new()
        .jobs(4)
        .run_parallel(&script, &big_input()[..], Vec::new())
        .unwrap();
    assert_eq!(outcome.exit_status, 3);
}

#[test]
fn a_perl_script_uses_pcre2_and_parses_back_perl() {
    let script = match Edit::new()
        .when(Address::matching(Pattern::perl("^(?i)a/b")))
        .substitute(Pattern::perl(r"(\d+)(?=px)"), r"<\1>")
        .build()
    {
        Ok(script) => script,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(script.dialect(), ripgsed::Dialect::Perl);
    assert_eq!(
        run(&script, "A/b 12px\nA/b 3em\nx 4px\n"),
        "A/b <12>px\nA/b 3em\nx 4px\n"
    );
    assert_eq!(script, Script::parse_perl(&script.to_string()).unwrap());
    let mixed = Edit::new()
        .substitute(Pattern::perl("a"), "b")
        .substitute(Pattern::extended("a"), "b")
        .build()
        .unwrap_err();
    assert_eq!(mixed.kind(), ErrorKind::Build);
}

#[test]
fn an_exceeded_pcre_limit_is_an_error_of_the_run() {
    let script = Script::parse_perl("s/^(a|aa)+$/X/").unwrap();
    let input = format!("{}!\n", "a".repeat(60));
    let error = Run::new()
        .encoding(Encoding::Bytes)
        .apply(&script, input.as_bytes())
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Run);
    assert_eq!(error.exit_status(), 4);
    assert!(
        error
            .message()
            .contains("exceeded PCRE's backtracking limit"),
        "{}",
        error.message()
    );
    // The same script on a line within the limits.
    assert_eq!(run(&script, "aaaa\n"), "X\n");
}

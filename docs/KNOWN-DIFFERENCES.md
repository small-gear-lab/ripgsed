# Known differences from GNU sed 4.9

ripgsed behaves like GNU sed 4.9: same output, same error messages in the C locale, same exit status (D4 in [DECISIONS.md](DECISIONS.md)). Where GNU sed's behavior is plainly a bug of the C library's regular expression matcher, ripgsed gives the result the definition gives by default, and reproduces the bug only with `--glibc-compat` (D10). The option exists for moving existing GNU sed assets that must keep producing exactly the same output; it is not needed otherwise. This page lists the differences and what the option reproduces.

## Intentional differences

- `--version` and `--help` print ripgsed's own text. The first line of the usage message and the `sed:` lines of option errors are the same as GNU sed's.
- Only the `C` locale and UTF-8 locales are implemented. Other multibyte encodings are read as bytes.
- Memory use is higher: input is read in blocks of 256 KiB, and parallel runs keep several blocks in flight.

## glibc behavior reproduced with `--glibc-compat`

GNU sed uses the regular expression matcher of the C library. Where that matcher does something no specification asks for, `--glibc-compat` (or `RIPGSED_GLIBC_COMPAT=1` in the environment) makes ripgsed do the same, so that scripts give the same output. Without it, the match is the one the definition gives (leftmost-longest, back-references as defined). The differential tests and fixtures run with the option, because they hold GNU sed's results:

- In a pattern that starts with a repetition of a single character (`b*`, `[ab]*`, `(a*)*`, `a*\+`) followed by `\B`, a match that is empty at a position where the repetition could consume a character is reported after that character. `echo ab | sed 's/b*\B/<&>/'` prints `ab<>`, not `a<>b`. It does not apply at the start of the buffer, and not to `\b`, `\<`, or `\>`. Fixture: `rgx-039`.
- A back-reference to a group inside a repetition does not see every value that POSIX allows. A bounded repetition `X{m,n}` (`?` is `{0,1}`; `+`, `*`, and `{m,}` are not bounded) is expanded into copies of the group, and the group is seen only after `m` or after `n` iterations: `echo aaa | sed -E 's/(a){1,3}\1/<&>/'` prints `<aa>a`, not `<aaa>`. And the value of a copy is not seen at a position where another copy of the group could also end, if that copy is an earlier one that starts no later or a later one that starts later: `(a+)+\1` on `aaaa` matches only `aa`, `(a|aa){2}\1` does not match `aaa` but matches `aaaa`, and `(a*){2}\1` never matches. Two exceptions: when the body of the group cannot be empty, a copy inside an optional copy of the repetition is not hindered by a mandatory copy that starts where it does; when it can be empty, a mandatory copy is not hindered by a later optional one. Where a copy could start is found by walking the pattern from the start of the match without looking at back-references, for matches up to 256 bytes and groups with up to 16 copies. Fixture: `rgx-040`.
- When the group is the whole body of a loop (`*`, `+`, `{m,}`) and can be empty, a last iteration that is empty is seen by a back-reference as an empty group, although the group is reported with the value of the last non-empty iteration: `echo a | sed -E 's/(a?)*\1/<&>[\1]/'` prints `<a>[a]`, and `(a*)*\1b` matches all of `ab`. The copy rule above applies to that empty iteration too, so `(a?)+b\1` matches `aab` entirely but, on `ab`, only the `b`. If the body of the group is itself a repetition (`(a*)*`), the group is reported empty, and among the paths that reach the match end the first one without an empty iteration after a non-empty one gives the registers (`(a*)*\1` prints `<aa>[a]` on `aa`). Fixtures: `rgx-041`, `rgx-012-backref-group-in-star-iteration`.
- When GNU sed's first attempt of a search fails because the back-reference refuses an empty group at the start of the match, the whole search fails, even if a later position would match: `(a?){2}\1` does not match `baaa` (it matches `aaa`), `(a?){2}\1b` does not match `baaab`. A refused empty group that is not at the start does not do this (`b(a?){2}\1` matches the `baaa` of `bbaaa`). Fixture: `rgx-041`.
- A relative link target that makes the path too long: GNU sed keeps the joined path in a buffer of fixed size and cuts it (the limit is 121 bytes, 185 when the directory part is longer, and the directory part alone when that is longer still, measured on GNU sed 4.9), so `--follow-symlinks` then reads or edits a different path (`couldn't edit ...: not a regular file`). By default the whole path is used. Fixture: `cmd-059`.
- Always on, not an option: character classes (`[[:alpha:]]`, `\w`, ...) in UTF-8 locales follow glibc's `C.utf8` tables, which are generated from the C library (`tools/gen-classes.py`) and so follow its Unicode version, not the one in Rust's standard library.

## glibc behavior that is not reproduced, even with `--glibc-compat`

These are cases where glibc's matcher gives a result that disagrees with the leftmost-longest rule or with the documented meaning of a back-reference. About 0.06% of random patterns in the differential test (`tests/regex_differential.rs`) hit one of them; ordinary patterns do not. ripgsed returns the result the definition gives.

| pattern (ERE) | text | glibc | ripgsed |
| --- | --- | --- | --- |
| `a?(a+)+\1` | `aaaaa` | match `aaaaa`, `\1` is `aa` | match `aaa`, `\1` is `a` |
| `a?(a?)+\1` | `aa` | match `aa`, `\1` is empty | match `aa`, `\1` is `a` |
| `((a?)*)x\1` | `x` | no match | match `x` |

The first is a back-reference to a group inside a repetition where something before the repetition can be empty, and the copy rule is not enough: with `a?` taking one `a` the value is seen for every length of the group, with the `a?` empty only for the shortest. In the second, `\1` matched through one path and the groups are reported from another. The third is a group that contains a loop of a group that can be empty, to which GNU sed's answers are no match or garbage. A group of fixed width (`(ab)+\1`) behaves as specified. Captures under stacked quantifiers (`((b?)+*|c){1}`) and `\B` after a repetition that does not start the pattern can also differ. The matrix entries are RGX-009 and RGX-012 in `compat/sed.tsv`.

With `-z`, a `$` at the start of a pattern that goes on with a negated bracket expression can hold before a newline that the match then consumes: `echo` of `a_`, a newline, and ` aAcB` with `-z -E 's/$[b-c]*[^ c]*/<&>/g'` matches the newline in GNU sed, and does not in ripgsed (a `$` that is not at the end of the record has no reason to hold there). Seen once in about 2,300 random patterns.

## UTF-8 cases of the C library that are not reproduced

- A byte sequence that is structured like a UTF-8 character but is not one (a surrogate such as `ED A0 80`, or a code point above U+10FFFF such as `F4 90 80 80`) is one character for glibc in some places (`.*`, `\W`, `[^a]`, `\b`) and not in others. ripgsed treats it as invalid bytes everywhere.
- With the `I` flag, glibc matches a pattern character against the input in one direction only: `s/В/X/I` does not match `U+1C80` although both fold to the same capital, and `[µ]` matches `Μ` and `μ`. ripgsed folds literals by the capital letter in both directions and brackets by the two cases of each character.
- A text of `a`, `i`, or `c` that ends in `\c` takes the newline GNU sed appends as its control character (`a x\c` writes `xJ`).
- A range that starts with an invalid collating symbol and ends with a class (`[[.space.]-[:alpha:]]`) reports `Invalid collation character`, GNU sed `Invalid range end`.
- Reading a directory that a symbolic link points to with `--follow-symlinks` names the target in the read error; GNU sed 4.9 built from the release tarball names the link, the packaged one the target (ripgsed names the target).

## Reporting the glibc cases upstream

The back-reference and `\B` cases above reproduce with plain `regcomp`/`regexec` (`REG_EXTENDED`), without sed:

```c
regcomp(&re, "(a+)+\\1", REG_EXTENDED);   /* "aaaa": reports [0,2), the longest is [0,4) */
regcomp(&re, "(a*){2}\\1", REG_EXTENDED); /* "aaaaaa": no match, [0,6) is one */
regcomp(&re, "b*\\B", REG_EXTENDED);      /* "ab": reports the empty match at 2, not at 1 */
```

Observed with glibc 2.43 (Ubuntu 2.43-2ubuntu2.4). A report to the glibc bug tracker (component `regex`) should say that POSIX asks for the leftmost-longest match and that `\B` must hold at the position of the match.

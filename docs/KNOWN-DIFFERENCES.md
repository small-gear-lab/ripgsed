# Known differences from GNU sed 4.9

ripgsed aims to behave exactly like GNU sed 4.9 (D4 in [DECISIONS.md](DECISIONS.md)): same output, same error messages in the C locale, same exit status. This page lists where it does not, and the glibc behavior it copies even though that behavior looks like a bug.

## Intentional differences

- `--version` and `--help` print ripgsed's own text. The first line of the usage message and the `sed:` lines of option errors are the same as GNU sed's.
- Only the `C` locale and UTF-8 locales are implemented. Other multibyte encodings are read as bytes.
- Memory use is higher: input is read in blocks of 256 KiB, and parallel runs keep several blocks in flight.

## glibc behavior that is reproduced

GNU sed uses the regular expression matcher of the C library. Where that matcher does something no specification asks for, ripgsed does the same, so that scripts give the same output:

- In a pattern that starts with a repetition of a single character (`b*`, `[ab]*`, `(a*)*`, `a*\+`) followed by `\B`, a match that is empty at a position where the repetition could consume a character is reported after that character. `echo ab | sed 's/b*\B/<&>/'` prints `ab<>`, not `a<>b`. It does not apply at the start of the buffer, and not to `\b`, `\<`, or `\>`. Fixture: `rgx-039`.
- Character classes (`[[:alpha:]]`, `\w`, ...) in UTF-8 locales follow glibc's `C.utf8` tables, which are generated from the C library (`tools/gen-classes.py`) and so follow its Unicode version, not the one in Rust's standard library.

## glibc behavior that is not reproduced

These are cases where glibc's matcher gives a result that disagrees with the leftmost-longest rule or with the documented meaning of a back-reference. About 0.2% of random patterns in the differential test (`tests/regex_differential.rs`) hit one of them; ordinary patterns do not. ripgsed returns the result the definition gives.

| pattern (ERE) | text | glibc | ripgsed |
| --- | --- | --- | --- |
| `(a+)+\1` | `aaaa` | match `aa` | match `aaaa` |
| `(a+)+\1b` | `aaab` | match `aab` (from offset 1) | match `aaab` |
| `(a*)+\1` | `aaa` | match `aa` | match `aaa` |
| `(a*){2}\1` | `aaaaaa` | no match | match `aaaaaa` |

All four are back-references to a variable-width group inside a repetition. A fixed-width group (`(ab)+\1`) and `(a*)*\1b` behave as specified. Captures under stacked quantifiers (`((b?)+*|c){1}`) and `\B` after a repetition that does not start the pattern can also differ. The matrix entries are RGX-009 and RGX-012 in `compat/sed.tsv`, and the pending fixture is `rgx-012-backref-group-in-star-iteration`.

## Reporting the glibc cases upstream

The four rows above reproduce with plain `regcomp`/`regexec` (`REG_EXTENDED`), without sed:

```c
regcomp(&re, "(a+)+\\1", REG_EXTENDED);   /* "aaaa": reports [0,2), the longest is [0,4) */
regcomp(&re, "(a*){2}\\1", REG_EXTENDED); /* "aaaaaa": no match, [0,6) is one */
regcomp(&re, "b*\\B", REG_EXTENDED);      /* "ab": reports the empty match at 2, not at 1 */
```

Observed with glibc 2.43 (Ubuntu 2.43-2ubuntu2.4). A report to the glibc bug tracker (component `regex`) should say that POSIX asks for the leftmost-longest match and that `\B` must hold at the position of the match.

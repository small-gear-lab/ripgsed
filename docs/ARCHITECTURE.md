# Architecture

Background and rationale are in [DESIGN.md](DESIGN.md). This file records the current layout and policies; it is not a settled specification.

## Layout

One crate (lib + bin). No workspace split until dependency boundaries settle.

| Module | Responsible for | Not responsible for |
| --- | --- | --- |
| `syntax` | tokenizing, AST, source spans, syntax diagnostics (`parser`) | execution, file writes |
| `regex` | the regular expression engine: BRE/ERE dialects, leftmost-longest matching, captures | sed syntax, substitution |
| `locale` | the locale of the run, decided by the C library; the few library calls the matcher and `\U \L` make (`towupper`, `iswalnum`, `regcomp`/`regexec` for bracket expressions) | the tables of `C.utf8` (`regex::classes_table`) |
| `runtime` | cycle, pattern/hold space, command evaluation, `s` and `y`, `l` | CLI argument parsing |
| `io` | record reading, output streams, `w` files, in-place editing, standard input | script syntax |
| `debug` | the canonical program text and the execution trace of `--debug` | running the script |
| `cli` | option parsing, mode selection, exit status | a second implementation of the transformation logic |
| `analysis` | deciding statically whether a script is record-local (can run on chunks independently) | running the script |
| `parallel`, `stream` | the ordered chunk stream, workers, and the ordered merge of their output | script semantics |
| `search` | byte and substring search (SSE2/AVX2 with scalar fallbacks) | regular expressions |

## The regex engine

A search goes through up to three stages, each cheaper than the next and each producing exactly what the NFA simulation would:

1. **Literal prefilter** (`regex::literal`). A pattern that is only a literal is answered by a substring search; a pattern that requires a literal (possibly spread over plain groups) rejects a haystack that lacks it before any automaton runs.
2. **Lazy DFAs** (`regex::dfa`), used when the pattern has no back-reference and the haystack has no delimiter or newline byte (in a UTF-8 locale, characters are spelled out as UTF-8 byte sequences so the DFAs run on bytes; patterns with case folding or word assertions are limited to ASCII haystacks). A forward unanchored DFA finds the earliest match end; a reverse DFA (built from the mirrored pattern) finds the leftmost start of a match ending there; earlier starts, whose matches must end later, are tried with a forward anchored DFA inside a work budget; and the forward anchored DFA gives the longest end. States are built on demand (at most 2000; beyond that the search falls back). Looking for the earliest end first keeps repeated searches on one long line linear.
3. **NFA simulation and backtracking** (`regex::exec`). The reference: it handles back-references, non-ASCII input, and everything the DFAs decline, and it assigns the sub-matches on the span the DFAs found.

The DFA path is tested against the simulation on random patterns and haystacks (`regex::tests`), and by the fixtures and the execution fuzz test, which run with it enabled.

## Locales

`locale::from_env` calls `setlocale(LC_ALL, "")` once, before anything else, and reads the character set of `LC_CTYPE` and the names of `LC_CTYPE` and `LC_COLLATE` back. The result is one of five `regex::Locale` values, and every place that reads characters asks `Locale::is_chars` (text is read as characters), `is_utf8`, or `is_lib` (the library answers) rather than comparing:

| `Locale` | when | what a character is, what a set contains |
| --- | --- | --- |
| `C` | byte-wise: `C`, `POSIX`, a name that is not installed, a character set other than UTF-8 | one byte; ASCII classes |
| `Utf8` | `LC_CTYPE` is `C.UTF-8` and `LC_COLLATE` is `C`, `POSIX` or `C.UTF-8` | one UTF-8 character; the tables of `C.utf8` built in (`classes_table`, made by `tools/gen-classes.py`); ranges by code point; no call into the library while matching |
| `Libc` | any other UTF-8 locale | one UTF-8 character; the questions below go to the C library |
| `Mb` | a multibyte character set other than UTF-8 (EUC-JP, Shift_JIS, GBK, GB18030, Big5, EUC-KR, EUC-TW) | one character read with `mbrtowc`; the questions below go to the C library |
| `Sb` | a single-byte character set other than ASCII (ISO-8859-x, KOI8-R, Windows-125x) | one byte; a byte set per bracket expression, filled by asking the C library about the 256 bytes |

In `Libc` the parser builds a `CharSet` with `lib` set whenever the answer depends on the locale, and `CharSet::matches_char` asks the library:

- a bracket expression that has a range, a class, an equivalence class or a collating symbol, or that is read under `I`, is compiled as a whole by `regcomp` (the error codes become GNU sed's messages: `Invalid range end`, `Invalid collation character`, ...) and a character is tested with `regexec` on its own. Answers are remembered (ASCII in two bit masks, the rest in a table), so a text costs one library call per distinct character. This is what GNU sed does, so ranges follow the collation of `LC_COLLATE`, and classes and case follow `LC_CTYPE`, with nothing to model;
- a pattern with a bracket expression of single characters and ranges between ASCII digits is also compiled a second time with those ranges read as code points, which GNU sed's first matcher does; a record that this reading does not match is not matched (`Regex::filter`), and the match inside a record that passes is the library's (`echo 3٣ | sed 's/[0-9]/N/g'` prints `NN`, `echo ٣ | sed 's/[0-9]/N/'` prints `٣`);
- under `I`, a literal letter or a bracket expression of single characters does not take U+1C80..U+1C88 unless the pattern writes the character (an observed rule of GNU sed's first matcher);
- `\w`, `\W`, `\s`, `\S` are `[_[:alnum:]]` and `[[:space:]]` for the library; the word assertions use `iswalnum`;
- a literal letter under `I` is compiled by `regcomp` with `REG_ICASE` and asked the same way (in `tr_TR.UTF-8` `i` goes with `İ`, and `I` with `ı`); back-references under `I` and `\U \L \u \l` use `towupper`/`towlower`.

A set with `lib` is opaque to the DFA (`expand_utf8` refuses it), so such a pattern runs the DFA on ASCII haystacks and the NFA simulation on the rest; the default paths are unaffected.

**Other character sets.** `regcomp` and `iswalnum` work on `wchar_t`, which is Unicode in glibc for every character set, so the questions above are the same for `Mb` and `Libc`; what differs is reading and writing a character.

- `locale::mb_decode` (`mbrtowc`) and `locale::mb_encode` (`wcrtomb`) are the only places that know the encoding. `Searcher::dec` and the parser's `dec` replace `decode_utf8` (for `Mb`), `LibSet` encodes the character it is asked about with `wcrtomb`, and `\U \L` convert through the wide character and write it back. A byte that `mbrtowc` refuses, or accepts but the library's own matcher does not take for `.` (0x80 in GBK, found at start-up), is an invalid byte, as in UTF-8.
- A multibyte match starts only at character boundaries (`Program::char_starts` is always on) and every step consumes whole characters, so the second byte of a character is never matched alone. `Searcher::dec_before` finds the character before a position by reading the line forward from a known boundary (`bhint`): in EUC-JP a second byte is also a valid first byte, so backward decoding is not an option in any of the sets. The `s` loop steps one character, not one byte, after an empty match in `Mb`.
- A pattern can hold a byte of 0x80 or more that is not a whole character (`s/\xa4./X/`, the lead byte of a two-byte character), and with `--glibc-compat` a bracket expression can take the first byte of a character alone. The match then ends inside the character. `Regex::tails` says that this can happen: `Searcher::dec` reads a character at a position inside another one only from the bytes left of it (as glibc does, which decodes the whole line from its start), and a search that starts inside a character goes on at the next one.
- `locale::ascii_bytes_are_characters` (measured with `mbrtowc` at start-up: bytes 0..127 are ASCII, and no multibyte character has an ASCII byte) says whether the DFAs and the literal search may look at ASCII bytes. EUC-JP and EUC-KR pass; Shift_JIS (0x5c, 0x7e), GBK and Big5 (second bytes 0x40..0x7e) do not, so there the DFAs are off and only `required` literals (a necessary condition, correct whenever ASCII bytes are ASCII) are used. `expand_utf8` is never used for `Mb`.
- `Sb` has no decoder: bytes are characters. The bracket expression is compiled by `regcomp` as a whole and the byte set is filled by asking about each byte; the DFA and the literal search run as in the `C` locale on those byte sets, and the word characters of 0x80..0xff come from a table filled at start-up (`locale::word_byte_high`).
- The script is read character by character where GNU sed does (`Parser::multibyte_tail`, `locale::script_char_len` in `normalize_escapes`) so that a second byte that is `\`, `[`, `]` or the delimiter stays in its character; the replacement is read byte by byte, also as GNU sed does.

## Policies

- The CLI and the Rust API share one path (parse, then run). Record-local scripts on large regular files run in parallel over an ordered chunk stream; everything else runs on the sequential VM.
- Unsupported syntax is a compile-time error and is never silently reinterpreted.
- Input is held as bytes. The record terminator is kept separate from the record body.

## Provenance and licensing

- GNU sed and GNU grep sources live in `reference/` (untracked) for reading behavior only. Code is not copied from them.
- Expected test output is generated by running GNU sed as a command (see `tests/README.md` in the repository).
- The project is licensed under `MIT OR Apache-2.0`. Nothing derived from GPL code may be added, because that would conflict with the license.

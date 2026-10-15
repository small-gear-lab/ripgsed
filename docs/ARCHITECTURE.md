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

`locale::from_env` calls `setlocale(LC_ALL, "")` once, before anything else, and reads the character set of `LC_CTYPE` and the names of `LC_CTYPE` and `LC_COLLATE` back. The result is one of three `regex::Locale` values, and every place that reads characters asks `Locale::is_utf8` rather than comparing:

| `Locale` | when | what a character is, what a set contains |
| --- | --- | --- |
| `C` | byte-wise: `C`, `POSIX`, a name that is not installed, a character set other than UTF-8 | one byte; ASCII classes |
| `Utf8` | `LC_CTYPE` is `C.UTF-8` and `LC_COLLATE` is `C`, `POSIX` or `C.UTF-8` | one UTF-8 character; the tables of `C.utf8` built in (`classes_table`, made by `tools/gen-classes.py`); ranges by code point; no call into the library while matching |
| `Libc` | any other UTF-8 locale | one UTF-8 character; the questions below go to the C library |

In `Libc` the parser builds a `CharSet` with `lib` set whenever the answer depends on the locale, and `CharSet::matches_char` asks the library:

- a bracket expression that has a range, a class, an equivalence class or a collating symbol, or that is read under `I`, is compiled as a whole by `regcomp` (the error codes become GNU sed's messages: `Invalid range end`, `Invalid collation character`, ...) and a character is tested with `regexec` on its own. Answers are remembered (ASCII in two bit masks, the rest in a table), so a text costs one library call per distinct character. This is what GNU sed does, so ranges follow the collation of `LC_COLLATE`, and classes and case follow `LC_CTYPE`, with nothing to model;
- a pattern with a bracket expression of single characters and ranges between ASCII digits is also compiled a second time with those ranges read as code points, which GNU sed's first matcher does; a record that this reading does not match is not matched (`Regex::filter`), and the match inside a record that passes is the library's (`echo 3٣ | sed 's/[0-9]/N/g'` prints `NN`, `echo ٣ | sed 's/[0-9]/N/'` prints `٣`);
- under `I`, a literal letter or a bracket expression of single characters does not take U+1C80..U+1C88 unless the pattern writes the character (an observed rule of GNU sed's first matcher);
- `\w`, `\W`, `\s`, `\S` are `[_[:alnum:]]` and `[[:space:]]` for the library; the word assertions use `iswalnum`;
- a literal letter under `I` is compiled by `regcomp` with `REG_ICASE` and asked the same way (in `tr_TR.UTF-8` `i` goes with `İ`, and `I` with `ı`); back-references under `I` and `\U \L \u \l` use `towupper`/`towlower`.

A set with `lib` is opaque to the DFA (`expand_utf8` refuses it), so such a pattern runs the DFA on ASCII haystacks and the NFA simulation on the rest; the default paths are unaffected.

**Adding other character sets** (EUC-JP, Shift_JIS, GBK, Big5, EUC-KR, and single-byte sets such as ISO-8859-x, KOI8-R, Windows-125x) does not change the questions: the library already answers them for any locale (`regcomp` and `iswalnum` work on `wchar_t`, which is Unicode in glibc for every character set). What is new is reading and writing a character. A locale would carry a decoder (`mbrtowc` for a character at a position, and the reverse for `\U` output, `y`, and the DFA's byte sequences, which cannot be spelled out for a stateful or non-self-synchronizing set, so those patterns use the simulation), and a single-byte set is a byte set whose class and case tables are filled by calling the library for the 256 bytes. `Locale::is_utf8` and the places that match on `Locale` (`regex::parse`, `regex::exec`, `runtime::case`, `runtime::translate`, the delimiter check in `syntax::parser`) are the places to extend.

## Policies

- The CLI and the Rust API share one path (parse, then run). Record-local scripts on large regular files run in parallel over an ordered chunk stream; everything else runs on the sequential VM.
- Unsupported syntax is a compile-time error and is never silently reinterpreted.
- Input is held as bytes. The record terminator is kept separate from the record body.

## Provenance and licensing

- GNU sed and GNU grep sources live in `reference/` (untracked) for reading behavior only. Code is not copied from them.
- Expected test output is generated by running GNU sed as a command (see `tests/README.md` in the repository).
- The project is licensed under `MIT OR Apache-2.0`. Nothing derived from GPL code may be added, because that would conflict with the license.

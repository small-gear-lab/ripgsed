# Decisions

Decisions that have been made, with their reasons. Proposals and open questions stay in [DESIGN.md](DESIGN.md). A decision can be revised; record the change here instead of silently dropping it.

## D1. License: `MIT OR Apache-2.0`

- The project is an independent implementation, licensed `MIT OR Apache-2.0`, the usual dual license in the Rust ecosystem.
- Nothing derived from GPL code is added. GNU sed and GNU grep are read only as references and are never copied (see the provenance policy in DESIGN.md).

## D2. Input representation: bytes

- Input, pattern space, hold space, and output are byte sequences, not `String`. Full GNU sed compatibility requires accepting invalid UTF-8 and NUL.
- The record terminator (LF or NUL) is kept separate from the record body. A missing final newline is preserved, never normalized.
- P0 targets the `C` locale only: byte-wise behavior matching GNU sed under `LC_ALL=C`. Multibyte, character-wise behavior (for example `.` matching one character) is a later stage and is not claimed before then.
- Input is read through buffered streams. Memory grows with the length of a single record, not with the size of the file.

## D3. Regular expressions sit behind an engine abstraction

- The core never calls a regex library directly. It uses an engine interface (see "Engine abstraction" in DESIGN.md), so that the project's own engine (D7) and other engines such as PCRE2 as an opt-in extension can sit beside each other.
- The engine is chosen explicitly (by syntax dialect and options) and never switched silently when a construct is unsupported.

## D4. Full GNU sed compatibility is the minimum requirement

- ripgsed is not released or described as a replacement for `sed` until it is fully compatible with GNU sed: the same stdout, exit status, and file effects for any valid GNU sed invocation. Only the version and help text differ on purpose (see DESIGN.md).
- Milestones such as P0 are an order of work, not a smaller product. A subset is never treated as "done" for the purpose of this requirement.
- The scope is therefore the whole of GNU sed, taken from its manual: every command, option, flag, escape, address form, and special case. The compatibility matrix lists all of them from the start, each marked unsupported, partial, or verified, and unsupported items are compile-time errors.
- The scope explicitly includes multibyte and UTF-8 locale behavior (D2 only orders the work: `C` locale first) and `--debug`, whose canonical program listing and execution annotations are compared with GNU sed like any other output.
- Compatibility is verified only by differential tests against GNU sed (see tests/README.md in the repository), never by assertion.
- The grep mode aims at the same level for GNU grep, after the sed mode.

## D5. Exit status and error messages match GNU sed exactly

- Exit statuses are identical to GNU sed in every case, including the documented 0, 1, 2, and 4 and the code given to `q` and `Q`.
- Error and warning messages written to stderr are identical to GNU sed's messages in the `C` locale, word for word, including the location parts such as `-e expression #1, char 3:`.
- The only part that follows the invoked name is the program prefix: `sed:` when run as `sed`, `ripgsed:` when run as `ripgsed`. This is a consequence of the deliberate identification difference, not an exception to compatibility.
- Messages translated through gettext for other languages belong to the locale items in the compatibility matrix and come after the English messages.
- Differential tests compare stderr as well as stdout and the exit status.

## D6. Compatibility target: GNU sed 4.9

- The reference for behavior and for generated test expectations is GNU sed 4.9, the latest stable release at the time of writing.
- Behavior that changed in newer GNU sed versions is not followed silently. It is recorded as a separate item in the compatibility matrix, and the target version is revisited as a decision.
- The version line printed by `--version` states this target (see DESIGN.md).

## D7. The regular expression engine is written from scratch

- No third-party regex library is used as a stopgap. GNU sed's BRE and ERE, with POSIX leftmost-longest matching, back-references, and the GNU extensions, are implemented by the project's own engine from the first release.
- The dialects include the `--posix` and `POSIXLY_CORRECT` variants of BRE and ERE, not only the default GNU behavior, and the locale (D2 orders the work: `C` locale first, then UTF-8 and other multibyte locales).
- The specification is the observable behavior of GNU sed 4.9 (D6), including where it deviates from POSIX. Differential tests against GNU sed decide that behavior.
- The engine sits behind the interface of D3, so it can be tested, benchmarked, and compared against other engines in isolation.

## D8. Platforms and toolchain

- Linux is the first supported platform. The differential tests rely on a POSIX `sh`, symbolic links, and GNU sed 4.9 as the reference, so they run on Unix systems only. Other platforms come after the core is verified.
- The minimum supported Rust version is 1.85, the first release with edition 2024, and it is declared in `Cargo.toml`. It is raised only when a feature is needed, with a note here.
- The project has one third-party dependency, `pcre2-sys` for `-P` (D14); otherwise none (see D7 for regular expressions). A dependency needs a reason recorded here.

## D9. Speed is a core goal: beat GNU sed 4.9, measured

- Being faster than GNU sed 4.9 is a goal of the project, together with full compatibility (D4). Compatibility comes first: no optimization may change any output, so every change must keep all differential fixtures and the execution fuzz test passing.
- "Faster" is defined by `tools/bench.sh`: fixed, deterministic workloads (literal and regex substitution, address matching, `y`, multi-line cycles, one huge line), compared with GNU sed 4.9 on the same machine, with output equality checked by hash. A claim names the machine, the build, and the numbers; an unmeasured claim is not made.
- The work is ordered by expected gain: the regex search (literal prefilters, a DFA for patterns without back-references), allocation and I/O overhead, then automatic parallel execution of scripts proven record-local by static analysis (DESIGN.md), with a sequential fallback for everything else.
- The first release is not called a replacement for `sed` before it is both fully compatible and faster on the benchmark set, or the gap is stated honestly next to the numbers.

## D10. Full compatibility, with the C library's regex bugs only on request

- Compatibility with GNU sed 4.9 (D4) has three levels. (1) Documented and intended behavior is always the same as GNU sed's. (2) Behavior that is plainly a bug is, by default, the corrected behavior: the result the definition gives. (3) The bug itself is reproduced only with `--glibc-compat` (or `RIPGSED_GLIBC_COMPAT=1`), for moving existing GNU sed assets that must keep producing exactly the same output.
- A behavior counts as a bug only if at least one of these holds: it contradicts the GNU sed manual or POSIX; the upstream project (glibc, GNU sed) acknowledges it as a defect; a later version fixes it. Anything else is treated as behavior and stays in level 1.
- The first cases are the glibc regular expression matcher's quirks: the empty match before `\B` after a leading repetition, and the incomplete back-reference search for groups inside repetitions (docs/KNOWN-DIFFERENCES.md). The character class tables of `C.utf8` are an implementation of the locale, not a quirk, and are always on.
- The differential tests and fixtures compare with GNU sed, so they run with the option on. The default side is covered by tests that state the result of the definition. The compatibility matrix (`compat/sed.tsv`) records an item as verified when it matches GNU sed with the option on.
- The option name avoids the long-option prefixes GNU sed uses (`--regex...` would make the abbreviations of `--regexp-extended` ambiguous).

## D11. Locales are the C library's

- GNU sed asks the C library about the locale (`setlocale`, `towupper`, `iswalnum`, the matcher of `regcomp`), so ripgsed asks the same library. The locale of a run is decided by `setlocale(LC_ALL, "")`: the priority of `LC_ALL`, `LC_CTYPE`/`LC_COLLATE` and `LANG`, the spelling of the names, `LOCPATH`, and the failure of a name that is not installed (every category stays `C`) are the library's, not a copy of its rules.
- Only `LC_CTYPE` and `LC_COLLATE` are kept switched on, because they are the only categories the matcher reads. The messages of the C library (`strerror`) and the formats stay as they are in the `C` locale.
- `C` and glibc's `C.UTF-8` keep their own tables and byte-wise or code-point rules and never call the library while matching, so the speed of the default paths does not depend on the library (D9).
  - Every other UTF-8 locale takes the answers of the library: `iswalnum` for `\w` and the word assertions, `towupper`/`towlower` for `\U \L \u \l` and for back-references under `I`, and the matcher of `regcomp`/`regexec`, which GNU sed also uses, for what a bracket expression contains (ranges by the collation of the locale, classes, equivalence classes, collating symbols) and for what a letter under `I` stands for.
  - The collation of a locale is not available through `wcscoll` alone: the order glibc uses for the end points of a range is not the order of `wcscoll` (`[a-z]` does not take `A` in `en_US.UTF-8`, though `wcscoll` puts `A` between `a` and `b`), and a bracket expression of a locale was checked against GNU sed for every code point (`tools/locale-diff.py`).
- Two rules are GNU sed's own, observed on the real program, and not the library's.
  - GNU sed runs every pattern through a first matcher (its own DFA) before the library's: it reads a bracket expression that has only single characters and ranges between ASCII digits by code point, and a line it does not match is not matched at all.
  - Where the match is in a line that passes, the library decides.
  - So `echo ٣ | sed 's/[0-9]/N/'` prints `٣`, and `echo 3٣ | sed 's/[0-9]/N/g'` prints `NN` (`٣` is in `[0-9]` of `en_US.UTF-8` by collation). ripgsed compiles such a pattern twice and asks the code-point reading first (`Regex::filter`).
  - The second rule: under `I`, a literal letter or a bracket expression of single characters does not take the small Cyrillic variants U+1C80..U+1C88 unless the pattern writes the variant itself (`в` does not match `ᲀ`).
- Case rules and collation are specification, not bugs (D10): they are always the locale's. The quirks of the matcher that show only with some locales are listed in docs/KNOWN-DIFFERENCES.md.
- Character sets other than UTF-8 use the same library calls with two more kinds of `Locale`.
  - `Mb` (EUC-JP, Shift_JIS, GBK, Big5, EUC-KR, and any other multibyte set) reads and writes a character with `mbrtowc` and `wcrtomb`; `wchar_t` is Unicode in glibc for every set, so `towupper`, `iswalnum` and `regcomp` need no change, and a bracket expression is compiled from the bytes of the pattern and asked the character in the set's own encoding.
  - `Sb` (ISO-8859-x, KOI8-R, Windows-125x and any single-byte set) keeps bytes as characters and fills a byte set for every bracket expression, `\w`, and letter under `I` by asking the library about the 256 bytes; case conversion goes through the wide character (`towupper` of `btowc`, then `wctob`, and 0xFF when there is no such byte, which is what GNU sed prints).
  - The locale is found with `nl_langinfo(CODESET)` and `MB_CUR_MAX` and is not guessed from its name.
- Three facts of a multibyte set decide what stays fast: whether the bytes 0..127 are the ASCII characters (Shift_JIS maps 0x5c and 0x7e to the yen sign and the overline), and whether every byte of a multibyte character is 0x80 or more (EUC-JP, EUC-KR; not Shift_JIS, GBK, Big5, whose second bytes are ASCII).
  - They are measured at start-up with `mbrtowc`, not assumed from the name.
  - Where both hold, the DFAs and the literal searches that need an ASCII byte to be a character of its own run on ASCII text as they do in UTF-8 (the DFA gives up at the first byte of 0x80 or more and the simulation takes over); in the other sets only the necessary literal is used and the DFAs are off, and the simulation steps by characters, starts matches at character boundaries, and finds the character before a position by reading the line forward (a second byte can look like a first byte, so reading backward is not possible).
- GNU sed reads a script character by character in a multibyte set (a second byte that is a backslash, a bracket or the delimiter belongs to its character in a regular expression, a `y` string and the text around them), but the replacement of `s` byte by byte.
  - Both are reproduced.
  - A lead byte that asks for more bytes cannot delimit a command (`delimiter character is not a single-byte character`).
  - Bytes that are not a character of the set are kept, never match `.` or a bracket expression, and stop `\U \L` the way an invalid UTF-8 byte does.
- Every character and every sequence of two bytes of each set was compared with GNU sed 4.9 (`tools/locale-diff.py`, then random patterns with `tools/locale-fuzz.py`); the differences that remain are listed in docs/KNOWN-DIFFERENCES.md.

## D12. The grep mode follows GNU grep 3.x

- The project also behaves like GNU grep: the same output, messages and status for the same command line (D4 and D5 for grep). The mode is chosen by the name the program runs under (`grep`, `egrep`, `fgrep`, `rgrep`) or by a first argument `--grep`. The sed mode is unchanged.
- The reference is GNU grep 3.11 with PCRE2 10.42, the releases that the CI runner has, as the sed mode's is GNU sed 4.9 (D6).
  - Their versions are recorded in `tests/reference-grep.json`, and CI follows the latest runner image (`ubuntu-latest`) and compares that record: a runner that brings other releases makes the `fixtures` job fail, and that is a change to look at, not to hide.
  - `tools/ref-grep.sh` unpacks the grep and PCRE2 of a release (checked against the signed archive index) and `GREP_DIR=... tools/gen-fixtures.sh` records the expected results with it, so the fixtures do not depend on the machine's grep.
  - Where 3.12 differs, `GNU_GREP_3_12` in `src/grep/mod.rs` chooses; docs/GREP.md has the steps for the day the runner has 3.12.
- The regular expression engine is the same engine (D7), asked to read the syntax of GNU grep (`Syntax::grep`) and, where GNU grep's DFA and the C library's matcher differ, both readings (`Syntax::regex_semantic`). The locale rules are the sed mode's (D11).
- `-P` is the PCRE2 library. It was first opened with `dlopen`, so that the crate had no dependency and no build requirement (D1, D8); D14 replaced that with the crate `pcre2-sys`. The sources of GNU grep were not read, as those of GNU sed were not: the behavior was learned by running it, and the code that implements it is written here.
- GNU grep's behavior that is plainly a bug follows the same rule as GNU sed's (D10): the shorter-match search of `-w -o` that measures from where the search began is reproduced because the fixtures and the random tests show it as the result of GNU grep and nothing in the manual contradicts it.

## D13. The library interface is small

- The public interface is `Script` (sed text, or built with methods), `Edit` with `Address` and `Pattern` (the builder), `Run` (the settings of a run, also with threads), `Outcome` and `Error`. Every other module is `#[doc(hidden)]` and may change in any release.
- The builder writes sed text and the parser of the command line reads it: there is one parser and one VM, a script made with methods is the script written by hand, and a mistake has the message of GNU sed. A command is an address and an operation, and no address is the address that selects every line.
- It does not claim to beat plain Rust on a trivial job: one literal replacement is faster with `str::replace`. What it gives is what GNU sed means by a script, checked against GNU sed 4.9, and threads where the script allows them (`docs/GREP.md` and the README give the measured numbers).

## D14. `-P`: PCRE2 patterns in the sed mode

- The dependency is `pcre2-sys` (`Unlicense OR MIT`, by the author of ripgrep, with 4 million downloads), the bindings that carry the source of PCRE2 (BSD-3-Clause WITH PCRE2-exception) and build it with a C compiler.
  - This costs the build a C compiler and the crate a dependency (and `cc`, `pkg-config` and `libc` at build time), and was chosen over the own pure-Rust engine (weeks of work, differences to chase) and over opening the library of the system with `dlopen` (the first version: no dependency, but `-P` fails where the library is missing, and the library's version is whatever the machine has).
  - The people who use a grep and sed like these also use `rg` and `fd` and build with `cargo`, which has a C compiler at hand anyway.
  - A pure-Rust engine can replace it later behind the same `src/pcre.rs`.
  - The low-level bindings are used, not the crate `pcre2`, because `-P` needs `DOLLAR_ENDONLY` and `MATCH_INVALID_UTF` and patterns that are bytes.
- `-P` / `--perl-regexp` is the spelling of grep's option, and GNU sed leaves `-P` unused, so a script that works with GNU sed is not affected. `-E` after it switches back.
- It uses the library of grep's `-P` (`src/pcre.rs`, D12), through the crate `pcre2-sys`.
  - A `Regex` made by `Regex::compile_pcre` carries the compiled pattern and answers `is_match`, `captures_at` and `group_count` with it; the commands, the replacement, the empty regexp (the last one is reused) and the `I` and `M` flags are sed's.
  - `I` is caseless and `M` is multiline; without `M`, `$` matches only at the end of the pattern space (`DOLLAR_ENDONLY`).
  - In a UTF-8 locale the pattern and the text are UTF-8 (invalid text is not an error) and `\w`, `\b` and the classes know Unicode.
- The pattern is PCRE2's, so `.` does not match a newline unless `(?s)`, `\d` is a digit (not sed's `\dNNN`), and `\n`, `\t`, `\xHH` are the library's. A delimiter inside a bracket expression must be escaped, as in Perl: the sed reading of `[/]` is not used. `M` uses the newline the library was built with (a line feed), also under `-z`.
- The library has the same: `Pattern::perl` and `Script::parse_perl`, with `Script::dialect` (`Dialect::{Basic, Extended, Perl}`) in place of the flag for extended patterns. A script uses one dialect, and mixing them is an error when it is built; 
- There is no GNU sed to compare with, so the tests state the library's answers (`tests/cli.rs`) and `--debug` prints the pattern as it was written.

## D15. Addresses counted from the end: `$-N` and `addr,-N`

- GNU sed 4.9 rejects both (`unknown command: `-'`, `unexpected `,'`), so a script that works with GNU sed is not affected, as with `-P` (D14). There is no switch for them: the spelling is what asks. The cost is that a script that uses them stops on GNU sed.
- `$-N` is the line with N lines after it (`$-0` is `$`). It is an address and the end of a range (`addr,$-N`). With `-s` and `-i` it counts in its file.
- `addr,-N` is `addr` and the N lines before it, as `addr,+N` is `addr` and the N lines after it: `10,-3` is `7,10`, and `-0` is one line, as `+0` is. `addr` has to be a line number, `$` or `$-N`: after a regexp or a step the lines before it have gone by, and it is an error.
- A bare negative line number (`-3,$`) is not accepted: the command line would read it as options before the script is read.
- `$,-9!d` keeps the last 10 lines, as `tail -10` does, with a lookahead of N records in the input (`RecordSource::records_after`). The chunk executor is told how many records follow a chunk, up to what the script asks to see (`analysis::lookahead`), so these scripts run in parallel too.
- GNU sed has nothing to compare with, so `tests/from_end.rs` states the answers from a model of the lines, and not a fixture. The fixture `adr-019-invalid-address-forms` no longer has `2,-1p`, which GNU sed rejects and ripgsed accepts.

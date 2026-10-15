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
- The project has no third-party dependencies so far (see D7 for regular expressions). A dependency needs a reason recorded here.

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
- `C` and glibc's `C.UTF-8` keep their own tables and byte-wise or code-point rules and never call the library while matching, so the speed of the default paths does not depend on the library (D9). Every other UTF-8 locale takes the answers of the library: `iswalnum` for `\w` and the word assertions, `towupper`/`towlower` for `\U \L \u \l` and for back-references under `I`, and the matcher of `regcomp`/`regexec`, which GNU sed also uses, for what a bracket expression contains (ranges by the collation of the locale, classes, equivalence classes, collating symbols) and for what a letter under `I` stands for. The collation of a locale is not available through `wcscoll` alone: the order glibc uses for the end points of a range is not the order of `wcscoll` (`[a-z]` does not take `A` in `en_US.UTF-8`, though `wcscoll` puts `A` between `a` and `b`), and a bracket expression of a locale was checked against GNU sed for every code point (`tools/locale-diff.py`).
- Two rules are GNU sed's own, observed on the real program, and not the library's. GNU sed runs every pattern through a first matcher (its own DFA) before the library's: it reads a bracket expression that has only single characters and ranges between ASCII digits by code point, and a line it does not match is not matched at all. Where the match is in a line that passes, the library decides. So `echo ٣ | sed 's/[0-9]/N/'` prints `٣`, and `echo 3٣ | sed 's/[0-9]/N/g'` prints `NN` (`٣` is in `[0-9]` of `en_US.UTF-8` by collation). ripgsed compiles such a pattern twice and asks the code-point reading first (`Regex::filter`). The second rule: under `I`, a literal letter or a bracket expression of single characters does not take the small Cyrillic variants U+1C80..U+1C88 unless the pattern writes the variant itself (`в` does not match `ᲀ`).
- Case rules and collation are specification, not bugs (D10): they are always the locale's. The quirks of the matcher that show only with some locales are listed in docs/KNOWN-DIFFERENCES.md.
- Character sets other than UTF-8 are not implemented yet: such a locale is read as bytes, as `C` is. Adding them means another kind of `Locale` that decodes a character with `mbrtowc` and leaves every question to the same library calls (docs/ARCHITECTURE.md).

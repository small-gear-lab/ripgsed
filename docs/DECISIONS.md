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

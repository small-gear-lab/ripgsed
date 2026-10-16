# ripgsed

[日本語](README.ja.md)

A fast GNU sed 4.9 in Rust, as a command and as a library. The same program is also a grep.

> **Status: experimental (0.1).** The aim is a drop-in replacement for GNU sed (and a grep). What is claimed below is what the tests verify. The library interface may change before 1.0, and the program is only tested on Linux.

The name says what it is: a fast gsed (GNU sed).

## What you get

- **A sed that behaves like GNU sed 4.9.** The sed language, its own regular expression engine (BRE, ERE, `--posix`), `-i`, `-z`, `-s`, `--debug`, `w`/`r`/`R`/`W`, and the C locale and every locale of the C library (UTF-8, EUC-JP, Shift_JIS, GBK, GB18030, Big5, EUC-KR, EUC-TW, ISO-8859-x, KOI8-R, Windows-125x).
- **Faster than GNU sed**, and much faster on large files, because scripts that are proven to work on one line at a time run on several threads (see Performance).
- **A grep**, when it runs as `grep`, `egrep` or `fgrep` (or with `--grep`): GNU grep 3.x, including `-P` and a search of directory trees on several threads.
- **Perl-style patterns in sed**: `ripgsed -P 's/(\d+)(?=px)/<$1>/g'` (PCRE2).
- **A Rust library** with the same sed language, and a builder for scripts.

## How compatibility is checked

- 261 behaviors are listed in the compatibility matrix (from the GNU sed manual and from differences the tests found). Each one is verified against the real GNU sed 4.9 by 1,889 differential fixtures: [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).
- Random differential tests of regular expressions and of whole scripts agree with GNU sed in about 99.9% of the cases.
- The fixtures execute 95.7% of the lines of GNU sed's own code, and the tests of ripgsed execute 99% of its lines.
- Compatibility is only claimed where fixtures verify it.

The known differences are in [docs/KNOWN-DIFFERENCES.md](docs/KNOWN-DIFFERENCES.md). They are bugs of the C library's regular expression matcher, and by default ripgsed gives the result the definition gives. `--glibc-compat` (or `RIPGSED_GLIBC_COMPAT=1`) reproduces GNU sed's result for the rare script that needs exactly that (D10).

## Install

```sh
cargo install ripgsed          # needs Rust 1.85 or later and a C compiler (for PCRE2, see -P)
ln -s "$(command -v ripgsed)" ~/.local/bin/sed    # as a sed ...
ln -s "$(command -v ripgsed)" ~/.local/bin/grep   # ... and as a grep (also egrep, fgrep)
```

The program takes its mode from the name it runs under, so the links are all there is to it; `ripgsed --grep ...` is the grep without a link. In a script that has to run the same with any sed, call `ripgsed` by its name.

It is developed and tested on Linux (Ubuntu). It asks the C library about locales, and other systems are untested. Editor integration is not implemented.

## Performance

`tools/bench.sh` compares ripgsed with GNU sed 4.9 on deterministic workloads (1,000,000 log-like lines, output equality checked by hash).

- Times are medians of 7 runs, in seconds, on an Intel Core i7-1260P (16 threads), Linux, release build.
- The locale is `LC_ALL=C.UTF-8`, a UTF-8 locale and the harder case. `BENCH_LOCALE=ja_JP.UTF-8 tools/bench.sh` measures a locale that goes through the C library: the Japanese workloads that use `\w` take about twice as long there and are still faster than GNU sed.
- "Auto" is the default mode: scripts proven record-local by static analysis run in parallel on large regular files.

| workload | GNU sed | ripgsed, 1 thread | ratio | ripgsed, auto | ratio |
|---|---:|---:|---:|---:|---:|
| `s/alpha/X/g` | 0.42 | 0.27 | 0.64 | 0.05 | 0.12 |
| `-E s/[0-9]+/N/g` | 1.39 | 0.85 | 0.61 | 0.16 | 0.12 |
| `s/[a-z]*a$/END/` | 1.94 | 1.25 | 0.64 | 0.23 | 0.12 |
| `/ERROR/d` | 0.18 | 0.13 | 0.72 | 0.04 | 0.22 |
| `-n /user=beta.*id=1/p` | 0.16 | 0.13 | 0.81 | 0.03 | 0.19 |
| `y/abc/xyz/` | 1.88 | 1.52 | 0.81 | 0.26 | 0.14 |
| `s/\(a\)\(l\)/\2\1/g` | 0.46 | 0.28 | 0.61 | 0.06 | 0.13 |
| `$!N;P;D` (not parallelizable) | 0.23 | 0.21 | 0.91 | 0.21 | 0.91 |
| `-n 1000000p` (2,000,000 lines) | 0.06 | 0.05 | 0.83 | 0.02 | 0.33 |
| one 20 MB line, `s/alpha/X/g` | 0.13 | 0.06 | 0.46 | 0.08 | 0.62 |
| Japanese text (500,000 lines), `s/ユーザー/U/g` | 0.22 | 0.14 | 0.64 | 0.03 | 0.14 |
| Japanese text, `-E s/[0-9]+/N/g` | 1.30 | 0.53 | 0.41 | 0.10 | 0.08 |
| Japanese text, `s/\w\+/<&>/g` | 2.65 | 0.79 | 0.30 | 0.25 | 0.09 |

- **Memory** use is higher than GNU sed's: 4 to 70 MiB against 2 MiB, and 172 MiB for the Japanese `\w` workload (one 20 MB line takes about 60 MiB in both), because input is read in blocks.
- **Sequential runs**: a script that reads other records, uses the hold space, or touches files runs sequentially. The sequential path is faster than GNU sed on every workload above.

## As a grep

`ln -s ripgsed grep` (also `egrep`, `fgrep`), or `ripgsed --grep ...`: the same options, output, messages and exit status as GNU grep, in the C locale and in the locales of the C library.

- **Checked**: 83 items of [compat/grep.tsv](compat/grep.tsv) are verified by 89 fixtures made with GNU grep 3.11 and PCRE2 10.42 (what the CI runner has). The four things that 3.12 changes are one switch in the source, see [docs/GREP.md](docs/GREP.md). Random patterns and random option combinations agree with GNU grep in all but about one case in five hundred.
- **Input** is searched as it is read: a pipe as it arrives, and a file larger than memory is fine.
- **Directory trees** are searched on several threads, with the output in the order of one thread. A tree of 1,000 files (71 MB): `-rc ERROR` in 0.023 s against GNU grep's 0.103 s.
- **One thread, 57 MB of log lines**, ripgsed against the machine's GNU grep (3.12), as a ratio of the time (above 1 is slower): a literal 1.0 to 1.2 (`-c` and `-F` the same; `-i` faster, a rare literal 0.7), `-o` 0.7, `-w` 0.4, `\w` in Japanese text 0.2. Slower: an alternation with a repetition about 1.9, a back-reference 2.4. The table and the differences are in [docs/GREP.md](docs/GREP.md).
- **`-P`** is PCRE2, built with the program.

## Perl-style patterns: `-P`

`ripgsed -P 's/(\d+)(?=px)/<$1>/g'`, or `--perl-regexp` (the spelling of grep's `-P`; GNU sed has no such option).

- The regular expressions of the script, in addresses and in `s`, are PCRE2 ones. The library is built with the program, through the crate `pcre2-sys`, so a C compiler is needed to build it.
- The commands, the flags `I` and `M`, the replacement (`\1`, `&`, `\U`) and the empty regexp are sed's. Only what the pattern means is PCRE2's.
- The library has it too (`Pattern::perl`, `Script::parse_perl`).
- There is no GNU sed to compare with, so the results are those of the library, tested in `tests/cli.rs` (D14).

## As a library

A script is a list of commands, each an address and an operation (no address means every line). Write it as sed text, or build it with methods; both give the same `Script`, and a `Run` applies it to a reader, a writer, or bytes.

```rust
use ripgsed::{Address, Edit, Pattern, Run, Script};

// As sed text ...
let script = Script::parse("2,3d;s/foo/bar/g")?;
// ... or with methods. The builder writes sed text and the same parser reads it, so a mistake
// (a range for `q`, an unknown label) gets the message of GNU sed.
let same = Edit::new()
    .when(Address::lines(2..=3)).delete()
    .substitute(Pattern::basic("foo"), "bar").global()
    .build()?;

let out = Run::new().jobs(0).apply(&same, b"foo\nfoo\nfoo\nfoo foo\n")?;
assert_eq!(out, b"bar\nbar bar\n");
```

The commands without a method yet (the hold space, `n` `N` `D` `P`, `r` `w`, labels) go in with `Edit::raw("1!G;h;$!d")`. `Run::jobs` spreads a script whose lines are independent over threads, with the same output byte for byte; `Run::glibc_compat` reproduces the quirks of the C library's matcher.

Do not expect it to beat plain Rust on a trivial job. Measured on 55.6 MB of log-like lines:

- `str::replace` for a literal is about 2.7 times faster than the same `s/alpha/X/g` through ripgsed on one thread.
- `regex::Regex::replace_all` is about 1.7 to 2.3 times faster than `s/[0-9]+/N/g`.
- A small input (100 lines) through a script that is already built takes about twice as long as the same work with a built `regex::Regex`.

What the library gives is what GNU sed means by a script (the addresses and ranges, the `s` flags, the case conversions, the locale rules), checked against GNU sed 4.9, and threads when the script allows them. For one literal replacement, use `str::replace`.

## Method and what is next

Full GNU sed 4.9 compatibility, verified by differential tests (D4 in [docs/DECISIONS.md](docs/DECISIONS.md)); the grep mode (D12), `-P` (D14) and the library interface (D13) follow the same method. Automatic parallelism and the DFA-based regex search are in the sed mode, and the files of a tree are searched in parallel in the grep mode. Next: editor integration, a faster path for the slow grep cases, and a pure-Rust PCRE2 engine behind `src/pcre.rs` if the C build turns out to be a burden.

- [docs/DESIGN.md](docs/DESIGN.md): goals, semantics, open decisions
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): current module layout and policies
- [tests/README.md](tests/README.md): how fixtures are generated

## Documentation

Docs are written in English and built with Sphinx in English and Japanese. The HTML is built by CI and published to GitHub Pages; it is not checked in. The Japanese translation lives in `docs/locale/ja/LC_MESSAGES/*.po`, and the Japanese Markdown generated from it is checked in under [`docs/ja/`](docs/ja/index.md) so it can be read directly on GitHub. `README.ja.md` is built the same way, from `docs/locale/ja/LC_MESSAGES/readme.po` (its code blocks from `docs/locale/ja/readme-code.json`).

```sh
tools/build-docs.sh          # build HTML for both languages and regenerate docs/ja
tools/build-docs.sh update   # refresh the .po files from the English source first
```

## Development

```sh
cargo build
cargo test              # the fixtures are in tests/fixtures; CI runs this in the debug build
tools/gen-fixtures.sh   # regenerate expected outputs; requires GNU sed (and GNU grep for the grep fixtures)
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Unless you state otherwise, any contribution you intentionally submit for inclusion in this project is licensed as above, without additional terms or conditions.

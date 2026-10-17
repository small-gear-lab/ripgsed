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
- **Addresses counted from the end**: `ripgsed '$,-9!d'` keeps the last 10 lines, as `tail -10` does, and `$-2,$p` prints the last three. GNU sed rejects these spellings (D15).
- **Reads only what a script needs**: `sed -n '$p' file` reads the end of the file, `sed -n '1,10p'` stops after ten lines, `sed -n '1p;$p'` reads the two ends, and `sed -n '0~1000p'` counts the lines between the thousandth ones, when the input is a file (D16).  GNU sed reads all of the input in each of these cases.
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

It is developed and tested on Linux (Ubuntu, glibc), where the compatibility with GNU sed 4.9 is verified. On macOS (Apple Silicon, CI) it builds, the tests of the program itself pass, and the 1,879 fixtures that apply there pass (the ones about the locales of glibc, `/dev/full` and `/proc`, and a few of the way Linux opens `/dev/stdin`, are left out); the locale is `C` or UTF-8 by name, and the other multibyte character sets are read as bytes. The standard sed of macOS is BSD sed, and GNU sed comes as `gsed` (`brew install gnu-sed`): ripgsed is a faster `gsed`, so link it under that name (`ln -s ~/.cargo/bin/ripgsed ~/.local/bin/gsed`). Editor integration is not implemented.

## Performance

`tools/bench.sh` compares ripgsed with GNU sed 4.9 on deterministic workloads (1,000,000 log-like lines, output equality checked by hash).

- Times are medians of 5 runs, in seconds, on an Intel Core i7-1260P (16 threads), Linux, release build.
- The locale is `LC_ALL=C.UTF-8`, a UTF-8 locale and the harder case. `BENCH_LOCALE=ja_JP.UTF-8 tools/bench.sh` measures a locale that goes through the C library: the Japanese workloads that use `\w` take about twice as long there and are still faster than GNU sed.
- "Auto" is the default mode: scripts proven record-local by static analysis run in parallel on large regular files, with `-s` and `-i` too (each file is a stream of its own, and the files are edited on several threads).

| workload | GNU sed | ripgsed, 1 thread | ratio | ripgsed, auto | ratio |
|---|---:|---:|---:|---:|---:|
| `s/alpha/X/g` | 0.22 | 0.16 | 0.73 | 0.04 | 0.18 |
| `-E s/[0-9]+/N/g` | 0.83 | 0.52 | 0.63 | 0.11 | 0.13 |
| `s/[a-z]*a$/END/` | 1.17 | 0.74 | 0.63 | 0.16 | 0.14 |
| `/ERROR/d` | 0.12 | 0.08 | 0.67 | 0.03 | 0.25 |
| `-n /user=beta.*id=1/p` | 0.10 | 0.08 | 0.80 | 0.02 | 0.20 |
| `y/abc/xyz/` | 1.01 | 0.09 | 0.09 | 0.03 | 0.03 |
| `y/a-z/A-Z/` (26 letters) | 2.78 | 0.09 | 0.03 | 0.03 | 0.01 |
| `s/\(a\)\(l\)/\2\1/g` | 0.28 | 0.17 | 0.61 | 0.03 | 0.11 |
| `$!N;P;D` (not parallelizable) | 0.15 | 0.12 | 0.80 | 0.14 | 0.93 |
| `-n 1000000p` (2,000,000 lines) | 0.05 | 0.00 | 0.00 | 0.00 | 0.00 |
| one 20 MB line, `s/alpha/X/g` | 0.10 | 0.05 | 0.50 | 0.06 | 0.60 |
| Japanese text (500,000 lines), `s/ユーザー/U/g` | 0.16 | 0.09 | 0.56 | 0.02 | 0.12 |
| Japanese text, `-E s/[0-9]+/N/g` | 0.79 | 0.31 | 0.39 | 0.06 | 0.08 |
| Japanese text, `s/\w\+/<&>/g` | 1.60 | 0.49 | 0.31 | 0.18 | 0.11 |
| `500000q` (quits half way) | 0.04 | 0.03 | 0.75 | 0.01 | 0.25 |
| `/NEVERMATCH/q` (never quits) | 0.11 | 0.09 | 0.82 | 0.03 | 0.27 |
| `$!s/alpha/X/` | 0.18 | 0.16 | 0.89 | 0.03 | 0.17 |
| `/ERROR/,/WARN/s/alpha/X/` (short ranges) | 0.19 | 0.15 | 0.79 | 0.10 | 0.53 |
| `/id=1234[0-9] /,/id=5678[0-9] /s/alpha/X/` (long ranges) | 0.18 | 0.14 | 0.78 | 0.10 | 0.56 |
| `0~1000F` | 0.07 | 0.06 | 0.86 | 0.02 | 0.29 |
| `/ERROR/w file` | 0.13 | 0.10 | 0.77 | 0.03 | 0.23 |
| `s/alpha/X/w file` | 0.19 | 0.17 | 0.89 | 0.04 | 0.21 |
| `-s '$!s/alpha/X/'` (2 files) | 0.36 | 0.32 | 0.89 | 0.06 | 0.17 |
| `-s -n '$p'` (2 files) | 0.07 | 0.08 | 1.14 | 0.03 | 0.43 |
| `-s /ERROR/,/WARN/s/alpha/X/` (2 files) | 0.37 | 0.31 | 0.84 | 0.23 | 0.62 |
| `-i s/alpha/X/g` (4 files) | 0.87 | 0.69 | 0.79 | 0.15 | 0.17 |
| `-i '$!s/alpha/X/'` (4 files) | 0.73 | 0.65 | 0.89 | 0.15 | 0.21 |
| `-i /ERROR/,/WARN/s/alpha/X/` (4 files) | 0.76 | 0.68 | 0.89 | 0.52 | 0.68 |
| `-i.bak '1d;$d'` (4 files) | 0.30 | 0.29 | 0.97 | 0.10 | 0.33 |
| `-n '$p'` (last line) | 0.03 | 0.00 | 0.00 | 0.00 | 0.00 |
| `$!d` (last line) | 0.03 | 0.00 | 0.00 | 0.00 | 0.00 |
| `-n 1,10p` (first lines) | 0.03 | 0.00 | 0.00 | 0.00 | 0.00 |
| `-n 500000p` (half way) | 0.03 | 0.00 | 0.00 | 0.00 | 0.00 |
| `-n '1p;$p'` (both ends) | 0.03 | 0.00 | 0.00 | 0.00 | 0.00 |
| `10q` (quits at once) | 0.00 | 0.00 | - | 0.00 | - |
| `-n 0~1000p` (every 1000th line) | 0.03 | 0.02 | 0.67 | 0.02 | 0.67 |
| `-n 2~7p` (every 7th line) | 0.04 | 0.02 | 0.50 | 0.02 | 0.50 |

- **Memory** use is higher than GNU sed's: 4 to 169 MiB against 2 MiB (the ranges and `-i` take the most), and 177 MiB for the Japanese `\w` workload (one 20 MB line takes about 60 MiB in both), because input is read in blocks.
- **Sequential runs**: a script that reads other records (`n`, `N`, `D`), uses the hold space, or reads files or runs commands (`r`, `R`, `e`) runs sequentially. On one thread the sequential path is faster than GNU sed on the workloads above, except `-s -n '$p'` on two files (1.14; 0.07 s for GNU sed, 0.08 s for ripgsed, and 0.43 with the threads). A ratio of 0.00 means a time below 0.005 s: those scripts read only the head or the tail of the file. The output of every workload was checked equal to GNU sed's.

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

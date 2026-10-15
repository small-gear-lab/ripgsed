# ripgsed

A stream editor in Rust that keeps the sed language but separates syntax, semantics, and execution strategy.

**Status: compatible with GNU sed 4.9, and faster.** ripgsed implements the sed language and its own regular expression engine (BRE/ERE, `--posix`, the C locale and every locale of the C library: UTF-8, EUC-JP, Shift_JIS, GBK, GB18030, Big5, EUC-KR, EUC-TW, and the single-byte sets such as ISO-8859-x, KOI8-R and Windows-125x (collation order of ranges, classes, case rules), `-i`, `--debug`, `-z`, `-s`, `w`/`r`/`R`/`W`, ...). Every behavior listed in the compatibility matrix (261 items, from the GNU sed manual and from differences the tests found) is verified against the real GNU sed 4.9 by 1,880+ differential fixtures, and random differential tests of regular expressions and of whole scripts agree with it on about 99.9% of the cases. The fixtures exercise 95.7% of the lines of GNU sed 4.9's own code; ripgsed's tests execute 99% of its lines. Compatibility is only claimed where fixtures verify it (see [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) and `tools/compat-summary.sh`). The known differences are listed in [docs/KNOWN-DIFFERENCES.md](docs/KNOWN-DIFFERENCES.md): they are bugs of the C library's regular expression matcher, and by default ripgsed gives the result the definition gives; `--glibc-compat` (or `RIPGSED_GLIBC_COMPAT=1`) reproduces GNU sed's result for the rare script that needs exactly that (D10). The project is not released yet: there is no published crate, and the grep mode, PCRE2, and editor integration are not implemented.

## Performance

`tools/bench.sh` compares ripgsed with GNU sed 4.9 on deterministic workloads (1,000,000 log-like lines, output equality checked by hash). Times are medians of 7 runs in seconds on an Intel Core i7-1260P (16 threads), Linux, release build, `LC_ALL=C.UTF-8` (a UTF-8 locale, the harder case; `BENCH_LOCALE=ja_JP.UTF-8 tools/bench.sh` measures a locale that goes through the C library, where the Japanese workloads that use `\w` take about twice as long and are still faster than GNU sed). "Auto" is the default mode: scripts proven record-local by static analysis run in parallel on large regular files.

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

Memory use is higher than GNU sed's (4 to 70 MiB against 2 MiB, and 172 MiB for the Japanese `\w` workload; one 20 MB line takes about 60 MiB in both), because input is read in blocks. A script that reads other records, uses the hold space, or touches files runs sequentially; the sequential path is faster than GNU sed on every workload above.

## Plan

The first milestone is full GNU sed 4.9 compatibility, verified by differential tests (D4 in [docs/DECISIONS.md](docs/DECISIONS.md)). Automatic parallelism and the DFA-based regex search are in; PCRE2, the grep mode, and editor integration come after compatibility is complete.

- [docs/DESIGN.md](docs/DESIGN.md): goals, semantics, open decisions
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): current module layout and policies
- [tests/README.md](tests/README.md): how fixtures are generated

## Documentation

Docs are written in English and built with Sphinx in English and Japanese. The HTML is built by CI and published to GitHub Pages; it is not checked in. The Japanese translation lives in `docs/locale/ja/LC_MESSAGES/*.po`, and the Japanese Markdown generated from it is checked in under [`docs/ja/`](docs/ja/index.md) so it can be read directly on GitHub.

```sh
tools/build-docs.sh          # build HTML for both languages and regenerate docs/ja
tools/build-docs.sh update   # refresh the .po files from the English source first
```

## Development

```sh
cargo build
tools/gen-fixtures.sh   # regenerate expected outputs; requires GNU sed
```

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Unless you state otherwise, any contribution you intentionally submit for inclusion in this project is licensed as above, without additional terms or conditions.

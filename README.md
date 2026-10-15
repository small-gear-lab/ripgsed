# ripgsed

A stream editor in Rust that keeps the sed language but separates syntax, semantics, and execution strategy.

**Status: work in progress.** The sed language and its own regular expression engine are implemented (BRE/ERE, `--posix`, C and UTF-8 locales, `-i`, `--debug`, `-z`, `-s`, `w`/`r`/`R`/`W`, ...) and checked against GNU sed 4.9 by 1,700+ differential fixtures plus random differential tests. Compatibility is only claimed where fixtures verify it (see [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) and `tools/compat-summary.sh`); one fixture is known to differ (a glibc quirk with a back-reference to a group inside a repetition).

## Performance

`tools/bench.sh` compares ripgsed with GNU sed 4.9 on deterministic workloads (1,000,000 log-like lines, output equality checked by hash). Times are medians of 7 runs in seconds on an Intel Core i7-1260P (16 threads), Linux, release build, `LC_ALL=C`. "Auto" is the default mode: scripts proven record-local by static analysis run in parallel on large regular files.

| workload | GNU sed | ripgsed, 1 thread | ratio | ripgsed, auto | ratio |
|---|---:|---:|---:|---:|---:|
| `s/alpha/X/g` | 0.21 | 0.16 | 0.76 | 0.03 | 0.14 |
| `-E s/[0-9]+/N/g` | 0.93 | 0.50 | 0.54 | 0.11 | 0.12 |
| `s/[a-z]*a$/END/` | 1.31 | 0.95 | 0.73 | 0.23 | 0.18 |
| `/ERROR/d` | 0.16 | 0.14 | 0.88 | 0.06 | 0.38 |
| `-n /user=beta.*id=1/p` | 0.12 | 0.09 | 0.75 | 0.02 | 0.17 |
| `y/abc/xyz/` | 1.20 | 0.90 | 0.75 | 0.18 | 0.15 |
| `s/\(a\)\(l\)/\2\1/g` | 0.31 | 0.30 | 0.97 | 0.05 | 0.16 |
| `$!N;P;D` (not parallelizable) | 0.17 | 0.12 | 0.71 | 0.13 | 0.76 |
| `-n 1000000p` (2,000,000 lines) | 0.04 | 0.06 | 1.50 | 0.01 | 0.25 |
| one 20 MB line, `s/alpha/X/g` | 0.09 | 0.04 | 0.44 | 0.05 | 0.56 |

Memory use is higher than GNU sed's (tens of MiB against about 3 MiB), because input is read in blocks. A script that reads other records, uses the hold space, or touches files runs sequentially; the sequential path is not slower than GNU sed on the workloads above except for plain line selection by number.

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

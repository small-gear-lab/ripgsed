# ripgsed

A stream editor in Rust that keeps the sed language but separates syntax, semantics, and execution strategy.

**Status: work in progress.** The sed language and its own regular expression engine are implemented (BRE/ERE, `--posix`, C and UTF-8 locales, `-i`, `--debug`, `-z`, `-s`, `w`/`r`/`R`/`W`, ...) and checked against GNU sed 4.9 by 1,700+ differential fixtures plus random differential tests. Compatibility is only claimed where fixtures verify it (see [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) and `tools/compat-summary.sh`); the few known differences, all of them glibc regular expression quirks, are listed in [docs/KNOWN-DIFFERENCES.md](docs/KNOWN-DIFFERENCES.md).

## Performance

`tools/bench.sh` compares ripgsed with GNU sed 4.9 on deterministic workloads (1,000,000 log-like lines, output equality checked by hash). Times are medians of 7 runs in seconds on an Intel Core i7-1260P (16 threads), Linux, release build, `LANG=ja_JP.UTF-8` (a UTF-8 locale, the harder case). "Auto" is the default mode: scripts proven record-local by static analysis run in parallel on large regular files.

| workload | GNU sed | ripgsed, 1 thread | ratio | ripgsed, auto | ratio |
|---|---:|---:|---:|---:|---:|
| `s/alpha/X/g` | 0.31 | 0.22 | 0.71 | 0.05 | 0.16 |
| `-E s/[0-9]+/N/g` | 1.21 | 0.66 | 0.55 | 0.14 | 0.12 |
| `s/[a-z]*a$/END/` | 1.70 | 1.04 | 0.61 | 0.22 | 0.13 |
| `/ERROR/d` | 0.16 | 0.12 | 0.75 | 0.03 | 0.19 |
| `-n /user=beta.*id=1/p` | 0.14 | 0.11 | 0.79 | 0.03 | 0.21 |
| `y/abc/xyz/` | 1.43 | 1.06 | 0.74 | 0.22 | 0.15 |
| `s/\(a\)\(l\)/\2\1/g` | 0.40 | 0.22 | 0.55 | 0.04 | 0.10 |
| `$!N;P;D` (not parallelizable) | 0.20 | 0.18 | 0.90 | 0.18 | 0.90 |
| `-n 1000000p` (2,000,000 lines) | 0.06 | 0.04 | 0.67 | 0.02 | 0.33 |
| one 20 MB line, `s/alpha/X/g` | 0.12 | 0.05 | 0.42 | 0.07 | 0.58 |
| Japanese text (500,000 lines), `s/ユーザー/U/g` | 0.18 | 0.12 | 0.67 | 0.03 | 0.17 |
| Japanese text, `-E s/[0-9]+/N/g` | 4.47 | 0.40 | 0.09 | 0.07 | 0.02 |
| Japanese text, `s/\w\+/<&>/g` | 5.63 | 0.68 | 0.12 | 0.21 | 0.04 |

Memory use is higher than GNU sed's (tens of MiB against about 3 MiB), because input is read in blocks. A script that reads other records, uses the hold space, or touches files runs sequentially; the sequential path is faster than GNU sed on every workload above.

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

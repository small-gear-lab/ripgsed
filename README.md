# ripgsed

A stream editor in Rust that keeps the sed language but separates syntax, semantics, and execution strategy.

**Status: work in progress.** The sed language is implemented (parser, sequential VM, options including `-i` and `--debug`) and checked against GNU sed 4.9 by 1,700+ differential fixtures; the regular expression engine is being written. Compatibility is only claimed where fixtures verify it (see [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) and `tools/compat-summary.sh`). There are no performance numbers.

## Plan

The first milestone is full GNU sed 4.9 compatibility, verified by differential tests (D4 in [docs/DECISIONS.md](docs/DECISIONS.md)). Parallelism, PCRE2, the grep mode, and editor integration come only after that.

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

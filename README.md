# ripgsed

A stream editor in Rust that keeps the sed language but separates syntax, semantics, and execution strategy.

**Status: early design stage. Nothing is implemented yet**; `ripgsed` currently exits with "not implemented". There are no performance numbers and no compatibility claims.

## Plan

The first milestone (P0) is a sequential reference implementation of a small subset (literal `s/old/new/g`, LF-delimited, stdin to stdout), verified by differential tests against GNU sed. Parallelism, PCRE2, and editor integration come only after that.

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

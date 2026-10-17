# AGENTS.md

Guidance for coding agents (and people) working in this repository. The project is described in
[README.md](README.md); the decisions behind it are in [docs/DECISIONS.md](docs/DECISIONS.md) (D1 to D14) and the
layout in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). This file only says how to work here.

## What this is

ripgsed is a GNU sed 4.9 compatible stream editor in Rust (a command and a library). The same binary is a GNU grep
(mode chosen by the program name `grep`, `egrep`, `fgrep`, `rgrep`, or by `--grep` as the first argument), and `-P`
reads the regular expressions with PCRE2 (`pcre2-sys`). It is experimental (0.1.x), published on crates.io.

## Rules that must not be broken

- **Do not read or copy the source of GNU sed, GNU grep or glibc** (GPL; this crate is `MIT OR Apache-2.0`). Behavior is
  learned by running the tools, and the expected results of the tests are made by running them (`tools/gen-fixtures.sh`).
  PCRE2 and Oniguruma are permissively licensed (BSD): their source may be read and ported, with the notice kept
  (`NOTICE`).
- **Compatibility is claimed only where fixtures verify it** (`compat/sed.tsv`, `compat/grep.tsv`; the `status` column is
  computed from the test results, never edited by hand). A behavior that is plainly a bug of the C library's matcher
  gives the result of the definition by default, and the bug itself only with `--glibc-compat` (D10).
- **No dependency without a reason recorded in `docs/DECISIONS.md`.** The only one is `pcre2-sys` (D14).
- Unsupported syntax is a compile-time error and is never silently reinterpreted.

## Build and test

```sh
cargo build
RIPGSED_FIXTURE_TIMEOUT=300 cargo test    # all of it, in the DEBUG build: CI runs the debug build
```

- **Run the tests in the debug build before pushing**, not only `--release`: integer overflow panics only there, and a
  line-number subtraction passed every release run and failed CI.
- Run the long things in the background and wait for them with a marker file or a monitor; do not block on them.
- The code is formatted with `cargo fmt` (the standard settings, no `rustfmt.toml`) and passes
  `cargo clippy --all-targets -- -D warnings`. `git config blame.ignoreRevsFile .git-blame-ignore-revs` lets `git blame`
  skip the commits that only formatted (GitHub reads the file by itself).
- `git config core.hooksPath .githooks`, once in each clone, turns on `.githooks/pre-commit`: it runs `cargo fmt --check`
  and `cargo clippy --all-targets -- -D warnings` when a Rust file is part of the commit and stops the commit if one fails.
  It looks at the working tree, not only at what is staged. `git commit --no-verify` skips it (work in progress on a
  `research/*` branch); the tests stay for before a push.
- One fixture or a few: `RIPGSED_FIXTURES=name1,name2 RIPGSED_SHOW_FAILURES=1 cargo test --test differential fixtures_match -- --nocapture`.
  `RIPGSED_SKIP=prefix,...` leaves some out. `RIPGSED_UPDATE_PASSING=1` and `RIPGSED_UPDATE_MATRIX=1` (full runs only)
  rewrite `tests/passing.txt` and the `status` column. `RIPGSED_JOBS=1` runs the grep and the sed mode on one thread.
- A new behavior gets a fixture (`tests/README.md` says how) or a test in `tests/cli.rs` / `tests/public_api.rs`.
  Write the test first and see it fail.

## Making fixtures: GNU tools only

`tools/gen-fixtures.sh` records what the real GNU sed and GNU grep print. **If the person who installed ripgsed has
links named `sed` or `grep` that lead to it, those links shadow GNU's on the `PATH`.** Always name the tools:

```sh
SED=/usr/bin/sed GREP=/usr/bin/grep tools/gen-fixtures.sh name ...        # the tools of this machine
SED=/usr/bin/sed GREP_DIR=$PWD/target/reference/grep/bin tools/gen-fixtures.sh name ...   # GNU grep 3.11 (what CI has)
```

The grep fixtures follow the grep and PCRE2 of the CI runner (3.11 / 10.42 on Ubuntu 24.04), and `GNU_GREP_3_12` in
`src/grep/mod.rs` switches the four things that 3.12 changes. `tools/ref-grep.sh` unpacks the reference (checked against
the signed archive index) into `target/reference/`. When `ubuntu-latest` becomes 26.04 the `fixtures` job fails: see
"The day the runner has 3.12" in `docs/GREP.md`. A command whose result differs between releases stays out of the
fixtures. The files `tests/fixtures/*/files/` are inputs; `expected/` is generated, never edited.

## Documentation

- English is the source: `README.md` and `docs/*.md`. The Japanese is generated and checked in: `docs/ja/*.md` and
  `README.ja.md`, from the catalogs `docs/locale/ja/LC_MESSAGES/*.po` (and `docs/locale/ja/readme-code.json` for the code
  blocks of the README). Never edit a generated file by hand.
- After changing an English document: `tools/build-docs.sh update` (new and changed strings appear in the `.po` files,
  fuzzy ones are old translations to check), translate them, then `tools/build-docs.sh`. CI fails when the checked-in
  Japanese is not current.
- Write short sections and short paragraphs; a long paragraph becomes a list. Say what is verified and what is not.
- Tables of numbers: the Markdown builder drops a trailing zero (`0.130`); `tools/restore-numbers.py` puts it back.

## Releases

- Linear history on `main`: fast-forward or rebase, **no merge commits** (`merge.ff=only` is set). Try things on a
  `research/*` branch (CI runs on it), then fast-forward.
- Push to `main` only after the tests pass locally. Public repository: Actions are free, but do not push to see what CI
  says when a local run can tell.
- A release is a tag: bump `version` in `Cargo.toml`, add the section `## X.Y.Z - date` to `CHANGELOG.md` (it is the text of
  the GitHub release), wait for CI on that commit, then `git tag -a vX.Y.Z -m ... && git push origin vX.Y.Z`. The workflow
  `.github/workflows/release.yml` publishes to crates.io with Trusted Publishing and makes the GitHub release. There is no
  token to keep: do not make one.
- Owners of the crate: `marie-222` and the team `github:small-gear-lab:maintainers`.

## Commits and language

- Commit messages are in English and say what changed, concretely, not the name of a branch or of a merge path.
- The people who talk to you may write Japanese; the code, the documents, the commits and the issues are in English
  (the Japanese documents are the translations).

## Platforms

Linux (glibc) is where the compatibility is verified. macOS (Apple Silicon) builds, and the fixtures that apply there pass
in CI (`RIPGSED_EXTRA_PATH` gives the harness the GNU tools of Homebrew; `RIPGSED_SKIP` leaves out what is about glibc's
locales, `/dev/full` and `/proc`). Code that differs by system is written with `cfg` (`target_os`, `target_vendor`), for
example the section of the startup hook and the number of `ELOOP`. Windows does not build (`std::os::unix`).

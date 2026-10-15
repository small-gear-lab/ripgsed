# Test fixtures

Differential tests that use GNU sed 4.9 as the reference (D5 and D6 in `docs/DECISIONS.md`). `cargo test` runs every fixture against ripgsed and compares stdout, stderr, exit status, and the files left behind.

## Layout of a fixture

A fixture is a directory `tests/fixtures/<area>-<name>/`:

| Path | Content |
| --- | --- |
| `cmd` | A POSIX `sh` script. `sed` in it is the reference GNU sed when generating and ripgsed when testing. It runs in a scratch copy of `files/`. |
| `covers` | Compatibility item ids (from `compat/sed.tsv`) the fixture exercises, whitespace separated. At least one. |
| `stdin` | Optional. Standard input of the script. Otherwise `/dev/null`. |
| `files/` | Optional. Input files, copied into the working directory. |
| `env` | Optional. `KEY=VALUE` lines added to the environment, for example `LC_ALL=C.utf8` or `POSIXLY_CORRECT=1`. The default is `LC_ALL=C`. |
| `expected/` | Generated. Do not edit by hand. |

`expected/` holds `stdout`, `stderr`, `status` (only when not 0), and `files/` (only when the working directory differs from the input `files/`). Absent means empty, 0, or unchanged.

Everything under `cmd`, `stdin`, `files/`, and `env` is written for this project. GNU sed's own source and test files are not used, and the fixtures are written from the manual's description, not from the GNU test suite.

## Writing a fixture

- Keep it deterministic: no time, process ids, random values, network, or paths outside the working directory.
- Use relative paths. Error messages then name the same file for both programs.
- `cmd` may run several commands, pipe, redirect, and print results with `cat` or `echo $?`. Prefer one behavior per fixture.
- Cover the ordinary case and the edge cases from the item's `notes` column. Error cases (bad syntax, missing files) are fixtures too, because stderr and the exit status must match.
- Name the file in `covers`; a fixture without a known item fails the test.

## Generating the expected results

```sh
tools/gen-fixtures.sh                # regenerate every fixture
tools/gen-fixtures.sh sub-global     # regenerate one
```

The generator refuses to run unless `sed --version` reports `sed (GNU sed) 4.9`. The reference version is recorded in `tests/reference.json`.

## Pending and enforced fixtures

A fixture is enforced, so that its failure fails `cargo test`, if it is listed in `tests/passing.txt` (the ratchet) or covers a `verified` item. Every other fixture is pending: it runs, but a failure does not fail `cargo test`. `verified` items must have at least one fixture.

```sh
cargo test                                          # all fixtures
RIPGSED_FIXTURES=sub-global cargo test              # selected fixtures
RIPGSED_SHOW_FAILURES=1 cargo test -- --nocapture   # show why pending fixtures fail
RIPGSED_SHOW_PENDING=1 cargo test -- --nocapture    # also list pending fixtures that already pass
RIPGSED_UPDATE_PASSING=1 cargo test                 # rewrite tests/passing.txt from the results
RIPGSED_UPDATE_MATRIX=1 cargo test                  # rewrite the status column of compat/sed.tsv
```

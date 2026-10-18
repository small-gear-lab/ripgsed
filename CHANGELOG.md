# Changelog

The release notes of every version. The section of a version is the text of its GitHub release.

## 0.2.1 - 2026-10-18

### Fixed

- `--posix` accepted `-P` and read the script as PCRE2, which GNU sed does not have. The two together are now a usage error (exit status 1), in whichever order they come; `--posix` already refused the other extensions of ripgsed (`$-N`, `addr,-N`). The last of `-P` and `-E` still counts, so `--posix -P -E` is ERE. The library is not affected: it has no `--posix`.

### Development

- The tests of the reading of the end and of the two ends of a file now compare the files that `w` writes: they ran in the wrong directory and compared nothing (and left an `out.txt` in the working tree).

## 0.2.0 - 2026-10-18

More of the scripts run in parallel, a script that needs only part of the input reads only that part, and there are two new addresses. The output, the files that `w` and `-i` write, and the exit status are the ones of the sequential run, which is what the tests compare (D16).

### Added

- Addresses counted from the end of the input (an extension: GNU sed 4.9 rejects them; D15):
  - `$-N` is the line with N lines after it (`$-0` is `$`), so `$-9,$p` is the last ten lines and `$,-9!d` is `tail -n10`.
  - `addr,-N` is a range of N lines before `addr`, the mirror of `addr,+N`: `10,-3` is `7,10`. (`addr,~N`, to a multiple of N, is as it was.)
- aarch64: `memchr`, `memrchr`, counting and `memmem` use NEON. The tests run on Linux ARM in CI as well.
- macOS (Apple Silicon) builds, and the fixtures that apply there pass in CI. The README says what is verified.
- `RIPGSED_TAIL=0` and `RIPGSED_HEAD=0` turn off the shortcuts that read the end of the input and stop at the last line number, and `RIPGSED_TAIL_MIN_BYTES=N` is the size under which a file is read whole (0: every file is read from its end), to compare them with the sequential run.

### Faster

- More scripts run in parallel on large regular files: those with `q` and `Q`, `$`, `F`, ranges, and files written by `w`. `-s` and `-i` run in parallel too: each file is a stream of its own, and the files are edited on several threads.
- A script that needs only part of the input reads only that part:

  - the end (on a regular file, and on standard input when it is one): `-n '$p'`, `$!d`, `-n '$-9,$p'` read from the end of the file, as `tail` does;
  - the beginning: `-n 1,10p`, `-n 500000p` stop reading after the last line number they name;
  - both: `-n '1p;$p'`;
  - `first~step`: `-n 0~1000p`, `-n 2~7p` skip to the next line of a step, which costs what is skipped.

  The end, the beginning and both take less than 0.005 s on 1,000,000 lines, where GNU sed takes 0.02 to 0.03 s; the steps take 0.33 to 0.5 of its time. A script that cannot be proven to do nothing in between runs as before.
- `y` is a table lookup where the characters are bytes (the C locale, and the ASCII text of UTF-8): `y/abc/xyz/` takes 0.09 s against 1.02 s for GNU sed, and `y` of the 26 letters 0.09 s against 2.77 s.
- The sequential path costs less for every record: `-n '$p'` takes 376 instructions a line, from 579 (GNU sed: 410), and `-s -n '$p'` on one thread takes 0.83 of the time of GNU sed, from 1.33. The reader reuses its buffers instead of allocating one for every chunk (9,150 page faults on two files of 2,000,000 lines, against 637), and the record read ahead for `$` is held where it was read.
- On one thread no row of `tools/bench.sh` is slower than GNU sed, and none of the 38 rows differs in output. The table of the README is measured again.

### Changed

- Library: `Run::run_parallel` spreads a script with `q` or `Q` too (the output stops at the first record that reaches it, and the exit status is the one it asked for). A script that writes files with `w` is still run as `Run::run` does.

### Development

- `tools/bench.sh`: rows for the new shortcuts, for `-s` and `-i`, and for `y` of 26 letters; `ONLY=text` runs some rows only; `GNU_SED=path` names the sed to compare with, and the script stops when it is not GNU sed.
- Code formatted with rustfmt (the standard settings); CI checks rustfmt and `clippy -D warnings` on the latest stable toolchain, and runs the fixtures and the random scripts with the shortcuts forced on every file. `.githooks/pre-commit` does the same checks before a commit.
- `AGENTS.md` for coding agents; `RIPGSED_BIN` and `RIPGSED_EXTRA_PATH` for the fixture harness.

## 0.1.3 - 2026-10-16

### Fixed

- `-P`: a match that PCRE2 gives up on (its match limit, for example `^(a|aa)+$` on sixty `a` and a `!`) is now an error, not "no match" (#1).
  - `grep -P` says `grep: FILE: exceeded PCRE's backtracking limit` and ends the whole run with exit status 2 at that file, as GNU grep does. The files after it are not searched, no count is printed for it, and `-s` does not silence the message.
  - The sed mode and the library (`Run::apply`) give an error of the run: `ripgsed: exceeded PCRE's backtracking limit`, exit status 4.
- `--help` of the sed mode names ripgsed, not the name the program runs under, in the heading of the options that only ripgsed has.

## 0.1.2 - 2026-10-16

### Fixed

- `sed --help`, and the usage printed after a wrong command line, show the list of the options (with the ones that only ripgsed has) as GNU sed does, in place of one line.
- grep, for the day the switch for GNU grep 3.12 is on: `-u` gives only the usage, as that release does.

## 0.1.1 - 2026-10-16

### Fixed

- `--version` (sed and grep) said "not yet fully compatible". It now says that the program is experimental and gives the address of the project.

This is the first release made by the release workflow of the repository (Trusted Publishing of crates.io).

## 0.1.0 - 2026-10-16

The first release. It is experimental: the aim is a drop-in replacement for GNU sed 4.9 (and a grep), and what is claimed is what the tests verify. It is tested on Linux only.

### sed

- The sed language of GNU sed 4.9 with its own regular expression engine: BRE, ERE, `--posix`, `-i`, `-z`, `-s`, `--debug`, `w`/`r`/`R`/`W`, and the C locale and every locale of the C library (UTF-8, EUC-JP, Shift_JIS, GBK, GB18030, Big5, EUC-KR, EUC-TW, ISO-8859-x, KOI8-R, Windows-125x).
- 261 behaviors of the compatibility matrix are verified against GNU sed 4.9 by 1,889 differential fixtures.
- Faster than GNU sed on every measured workload: 0.30 to 0.91 of its time on one thread, and about 0.1 to 0.2 on large files, where scripts that are proven to work on one line at a time run on several threads.
- Where GNU sed shows a bug of the C library's regular expression matcher, the result is the one the definition gives; `--glibc-compat` reproduces the bug (D10).

### grep

- Run as `grep`, `egrep` or `fgrep`, or with `--grep`: the options, output, messages and exit status of GNU grep 3.x (83 behaviors, 89 fixtures made with GNU grep 3.11).
- The files of a directory tree are searched on several threads, with the output in the order of one thread.
- On one thread it is about as fast as GNU grep for a literal and faster for `-w`, `-o` and Japanese text; an alternation with a repetition and a back-reference are slower.

### Perl-style patterns

- `-P` / `--perl-regexp` reads the regular expressions of a sed script with PCRE2, and `grep -P` is PCRE2 too. PCRE2 is built with the program (a C compiler is needed to build it).

### Library

- `Script`, `Edit` (with `Address` and `Pattern`), `Run`, `Outcome`, `Error` and `Dialect` (basic, extended or Perl-style patterns). The interface may change before 1.0.

# Changelog

The release notes of every version. The section of a version is the text of its GitHub release.

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

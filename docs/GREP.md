# The grep mode

ripgsed also behaves like GNU grep. The mode is chosen by the name the program runs under, so `ln -s ripgsed grep` is enough: `grep`, `egrep`, `fgrep` and `rgrep` run the grep mode, and the first argument `--grep` does the same for the program under its own name (`ripgsed --grep -n pattern file`). The messages carry the name `grep`, as those of GNU grep do.

The aim is the one of the sed mode (D4 in [DECISIONS.md](DECISIONS.md)): the same output, the same messages on standard error, the same exit status, in the C locale and in the locales of the C library. D12 records the rules of the grep mode.

## What is compared with GNU grep

- `compat/grep.tsv` lists the behavior by area: invocation, patterns, output, context, files, binary files, colors, and `-P`. Every one of its 83 items is `verified`, which means that the fixtures that cover it give the same standard output, standard error, exit status and files as GNU grep (`tests/README.md`). There are 89 fixtures of the grep mode (`tests/fixtures/grep-*` and `pcre-*`); they are written by `tools/gen-grep-fixtures.py`, and `tools/gen-fixtures.sh` records what the grep of the machine prints.
- The reference is GNU grep 3.11 with PCRE2 10.42, the releases of Ubuntu 24.04, which is what the CI runner has (D12). Their versions are in `tests/reference-grep.json`, which the `fixtures` job of CI checks too: when the runner brings other releases, the job says so. The expected results of the fixtures were made with those releases (`tools/ref-grep.sh` unpacks them, checking their hash against the signed archive index, and `GREP_DIR=target/reference/grep/bin tools/gen-fixtures.sh` records what they print), so a machine with other releases still passes the fixtures, as long as ripgsed follows the release the fixtures were made with.
- GNU grep 3.12 (Ubuntu 26.04) differs from 3.11 in four things, which the fixtures cover (they are in `GNU_GREP_3_12` of `src/grep/mod.rs`): `-u` and `--unix-byte-offsets` warn in 3.11 and are invalid options in 3.12; `-P` in a UTF-8 locale reads `\w`, `[[:alpha:]]` and `\b` as ASCII in 3.11 and as Unicode in 3.12 (`PCRE2_UCP`); `-q -m N` exits at the first selected line in 3.11 (status 0 even after an error) and goes on in 3.12 (so a later error makes the status 2); and `-l` or `-L` with `-m N` stop at the first selected line and print no `binary file matches` message in 3.11, and read the whole input and print it in 3.12. ripgsed follows 3.11 now. The messages of PCRE2 differ between its releases too, and the fixtures leave those out.
- Random tests that are not part of `cargo test` found most of the details: random patterns (basic, extended, fixed, with `-i`, `-w`, `-x`, `-v`, in the C locale and in UTF-8 locales) and random combinations of options on sample files, run with both programs and compared on the output, the messages and the status. The last runs of several thousand cases each differ in about one case in five hundred, and those are the cases of `docs/KNOWN-DIFFERENCES.md`.

## When the runner brings GNU grep 3.12

The `fixtures` job of CI makes the expected results again with the grep of the runner and fails when they differ from the files that are checked in. When the image moves to Ubuntu 26.04 (grep 3.12, PCRE2 10.46) the job will fail on `tests/reference-grep.json` and on the commands of the four differences above, and nothing else should change. To follow 3.12:

1. Set `GNU_GREP_3_12` to `true` in `src/grep/mod.rs` (it changes the four behaviors above and nothing else).
2. On a machine with grep 3.12 and PCRE2 10.46, or from the log of the failed job, run `tools/gen-fixtures.sh $(ls tests/fixtures | rg '^(grep|pcre)-')` (without `GREP_DIR`) and commit the changed expected results and `tests/reference-grep.json`.
3. Run `cargo test` in a debug build and in a release build. A fixture that still fails shows a difference that this list does not have: find it with `tools/ref-grep.sh` and `GREP_DIR`, which run the fixtures with the other release, and add it to the constant and to this list.
4. Update the sentences of this page and of `docs/KNOWN-DIFFERENCES.md` that name 3.11, and the Japanese translation (`tools/build-docs.sh update`).

## Patterns

GNU grep reads a pattern twice: its own DFA (from gnulib) decides which lines match, and the regular expression matcher of the C library (the one GNU sed has) finds the matches for `-o` and `--color`, and decides the lines when the DFA cannot. They agree on the patterns people write and differ on odd ones: a repetition operator at the start of an expression, after an anchor or a word assertion, an interval that is not valid. ripgsed has both readings of such a pattern. The lines are selected by the DFA reading, the matches come from the reading of the C library, and the C library's reading selects the lines too where GNU grep's DFA gives up: a pattern with a back-reference, and a pattern with a class, a negated set, `\w` or a word assertion in a multibyte locale. A mistake in a pattern is the C library's message, because GNU grep compiles with it first; the DFA's message comes second (`invalid content of \{\}`).

The syntaxes are the ones of GNU grep (`RE_SYNTAX_GREP`, `RE_SYNTAX_EGREP`), not the ones of GNU sed:

- A repetition operator at the start of an ERE (`*a`, `+a`, `{1}a`, after `(` or `|`) is skipped with a warning, `grep: warning: * at start of expression`. An invalid interval (`a{1`, `a{x}`) and an unmatched `)` are characters. Only `{2,1}` and a number above 32767 are mistakes.
- A leading `*` of a BRE is a character, `\{` at the start is the character `{`, and two operators in a row (`a**`, `a\{1\}\{2\}`) are allowed.
- The escapes of sed (`\n`, `\t`, `\a`, `\x41`) do not exist: a backslash before an ordinary character is that character. Under `-i` the C library compares an escaped letter that means nothing special with the upper-cased text, so it never matches there; the lines are still selected by the DFA, which does not have the mistake.
- `-w` and `-x` are the patterns wrapped (`(?<!\w)(?:` and `)(?!\w)`, or the whole line) for the DFA. The matches of `-o` and `--color` are the plain pattern's, tested the way GNU grep tests them: the leftmost match is a word, or a shorter match at the same place is tried, or a match further on. The shorter match is looked for in text that GNU grep cuts at a place measured from where the search began, not from the start of the line, so `echo 'x y z' | grep -ow '[^a] *'` prints `x` and `z` but not `y`, and ripgsed does the same. With both `-w` and `-x`, a match that reaches the end of the line is reported as the rest of the line with its newline.
- Several patterns (`-e`, `-f`, a newline) are separate regular expressions; the match of `-o` is the leftmost, then the longest of them.
- `-F` is the pattern written with every special character escaped. `-i` and the classes follow the locale as the sed mode does.

## Reading the inputs

- An input is read as it comes: a pipe is searched while it is written (`tail -f log | grep --line-buffered x` works), and a large file does not have to fit in memory. A match that is found early ends the read (`-m`, `-l`, `-q`).
- GNU grep reads in pieces and decides per piece whether the data is binary (it has a NUL byte). A regular file is read in pieces of 96 KiB at fixed offsets, so the lines of the pieces before the first NUL are printed as text, and the lines that end in that piece or later are not (`binary file matches` follows on standard error if something was selected there). For a pipe the pieces are the reads. ripgsed does the same, with the same offsets for files; for a pipe the size of the reads decides, as in GNU grep.
- In a UTF-8 locale a line with an encoding error is not printed (the message is the same as for a binary file, except with `-I`); `-o` looks at the match only.
- A regular file on standard input is left where GNU grep leaves it: just after the last selected line when `-m` ended the search, else at the end of the last piece it read.
- Directories are read in the order of the file system, as GNU grep's `fts` does; use `sort` where a fixture or a script depends on it.

## `-P`

The PCRE2 library of the system is opened with `dlopen` when `-P` is first used, so a build does not need its files and `-P` is the only option that fails if the library is missing (`grep: Perl matching not supported in a --disable-perl-regexp build`). The results and the error messages are the library's, and follow its version. The flags are the ones of GNU grep: `DOLLAR_ENDONLY`, `CASELESS` for `-i`, `UTF`, `UCP` and `MATCH_INVALID_UTF` in a UTF-8 locale, and the pattern wrapped for `-w` and `-x`. Only one pattern is accepted, as in GNU grep.

## Speed

Measured with `gbench`-style runs on 1,000,000 log-like lines (57 MB), in `C.UTF-8`, one thread, median of three, GNU grep 3.12 (the grep of the machine) against ripgsed on the same machine. A ratio below 1 is faster than GNU grep.

| workload | GNU grep (s) | ripgsed (s) | ratio |
|---|---:|---:|---:|
| `ERROR` | 0.109 | 0.130 | 1.19 |
| `-c ERROR` | 0.066 | 0.068 | 1.04 |
| `-i error` | 0.159 | 0.142 | 0.90 |
| `-n ERROR` | 0.132 | 0.164 | 1.24 |
| `-E 'user=(alpha\|beta).*id=[0-9]{5}'` | 0.230 | 0.446 | 1.94 |
| `-o '[0-9]\+'` | 1.506 | 1.038 | 0.69 |
| `-wc alpha` | 0.298 | 0.119 | 0.40 |
| `-Fc -e alpha -e beta` | 0.156 | 0.156 | 1.00 |
| a literal that is rare | 0.072 | 0.052 | 0.72 |
| `-c '\(a\)\1'` (back-reference) | 0.638 | 1.529 | 2.40 |
| Japanese text, `-c '\w\+'` | 0.367 | 0.070 | 0.19 |

A literal that every match contains is looked for in the whole input (also without regard to case), and only the lines that have it are tested. The slow cases are a pattern with several alternatives and a repetition (the engine tests each line, GNU grep's DFA runs over the whole buffer) and a back-reference. 
### Threads

The files of a directory tree (`-r`, `-R`) and several file operands are searched on several threads, each file on one of them; what the threads print, and the messages they write on standard error, are put back in the order of the files, so the output is the bytes of a single thread. It is not done with context (`-A`, `-B`, `-C`: the separator between groups depends on the files before), with `--line-buffered`, or when standard input is an operand; `RIPGSED_JOBS=1` asks for one thread and `RIPGSED_JOBS=N` for N. With `-q` the first result that has a selected line ends the process, in the order of the files. One large file is searched on one thread.

A tree of 1,000 files (71 MB), the same machine and data as above, GNU grep 3.12 against ripgsed with one thread and with all threads (16):

| workload | GNU grep (s) | ripgsed, 1 thread (s) | ripgsed, threads (s) | ratio, threads |
|---|---:|---:|---:|---:|
| `-rc ERROR` | 0.103 | 0.113 | 0.023 | 0.23 |
| `-r 'id=99999 '` | 0.121 | 0.098 | 0.016 | 0.14 |
| `-rn 'user=[a-z]*a id=1[0-9]*$'` | 0.184 | 0.111 | 0.020 | 0.11 |
| `-rl alpha` | 0.021 | 0.029 | 0.013 | 0.63 |
| `-ric error` | 0.222 | 0.222 | 0.036 | 0.16 |

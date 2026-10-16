#!/usr/bin/env python3
"""Write the fixtures of the grep mode (tests/fixtures/grep-*, pcre-*) and compat/grep.tsv.

    tools/gen-grep-fixtures.py            write the directories and the matrix
    tools/gen-fixtures.sh $(ls tests/fixtures | rg '^(grep|pcre)-')   then record the results of GNU grep

The commands are written here, as plain text: what each prints (and its exit status) is what
GNU grep printed, recorded by tools/gen-fixtures.sh. Nothing here is taken from GNU grep's
sources or its tests: the commands are written from its manual and from asking it questions.
"""
import os
import shutil
import sys

root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
fixtures = os.path.join(root, "tests", "fixtures")

# ---- the files every fixture starts with ------------------------------------------------------
STD = {
    "a.txt": b"alpha\nbeta\ngamma\nalpha beta\n",
    "b.txt": b"one\ntwo alpha\nthree\n",
    "d/c.txt": b"alpha in d\nzeta\n",
    "d/sub/e.log": b"alpha log\n",
    "bin.dat": b"bin\0ary alpha\nsecond alpha\n",
    "empty": b"",
    "nonl.txt": b"alpha\nlast alpha",
    "ctx.txt": b"".join(b"line %d%s\n" % (i, b" hit" if i in (3, 4, 9, 15) else b"") for i in range(1, 21)),
    "words.txt": b"foo foobar barfoo foo_bar foo-bar Foo\nfoo\nfoobar\n(foo)\n",
    "crlf.txt": b"alpha\r\nbeta\r\n",
    "utf8.txt": "café\nnaïve alpha\nあいう\n".encode(),
    "bad.txt": b"caf\xe9 alpha\nplain alpha\n",
    "z.dat": b"one\0two alpha\0three\0",
    "pats.txt": b"alpha\nzeta\n",
    "sp ace.txt": b"alpha with space\n",
}
# A file of 60,000 lines, made by the fixtures that need it (see BIG).
BIG = r"""awk 'BEGIN { for (i = 1; i <= 60000; i++) printf "L%05d %s\n", i, (i % 7 ? "alpha" : "beta") }' > big.txt"""
BIGBIN = BIG + r"""
awk 'BEGIN { RS = "\n"; ORS = "\n" } NR == 30000 { sub(/^L30/, "L3\001") } { print }' big.txt | tr '\001' '\000' > bigbin.dat"""

# ---- the items of the matrix --------------------------------------------------------------------
# id, area, item, source, notes
ITEMS = [
    ("GINV-001", "invocation", "grep PATTERN FILE...: the first operand is the pattern, the rest are files", "Invoking grep", ""),
    ("GINV-002", "invocation", "-e PATTERNS: repeated, and newline separated patterns in one argument", "Invoking grep", ""),
    ("GINV-003", "invocation", "-f FILE: patterns from a file (an empty file has no pattern: nothing matches)", "Invoking grep", ""),
    ("GINV-004", "invocation", "options may follow the operands; `--` ends them", "Invoking grep", ""),
    ("GINV-005", "invocation", "long options, their unambiguous abbreviations, --option=value and --option value", "Invoking grep", ""),
    ("GINV-006", "invocation", "-NUM is -C NUM; the digits of one argument make one number", "Context Line Control", ""),
    ("GINV-007", "invocation", "an invalid option, a missing argument or a missing pattern: usage message, status 2", "Exit Status", ""),
    ("GINV-008", "invocation", "invalid values: context length, max count, -d, -D, -X, --binary-files, conflicting matchers", "Invoking grep", ""),
    ("GINV-009", "invocation", "standard input: no file operand, `-`, and --label", "Invoking grep", ""),
    ("GINV-010", "invocation", "egrep and fgrep are grep -E and grep -F", "Invoking grep", "messages say grep"),
    ("GINV-011", "invocation", "exit status: 0 for a selected line, 1 for none, 2 for an error", "Exit Status", ""),
    ("GINV-012", "invocation", "-s hides the messages about files, the status is still 2", "Invoking grep", ""),
    ("GINV-013", "invocation", "obsolete options -y (as -i) and -u (a warning)", "Invoking grep", "-u is an invalid option from grep 3.12 on"),
    ("GPAT-001", "patterns", "basic regular expressions: literals, `.`, `*`, `^` and `$`", "Regular Expressions", ""),
    ("GPAT-002", "patterns", "basic regular expressions, GNU operators: \\+ \\? \\| \\( \\) and back-references", "Regular Expressions", ""),
    ("GPAT-003", "patterns", "intervals: \\{m\\} \\{m,\\} \\{,n\\} \\{m,n\\} (ERE: {m,n})", "Regular Expressions", ""),
    ("GPAT-004", "patterns", "bracket expressions: sets, ranges, negation, a first ], classes, equivalence classes", "Regular Expressions", ""),
    ("GPAT-005", "patterns", "\\w \\W \\s \\S \\b \\B \\< \\> \\` \\'", "Regular Expressions", ""),
    ("GPAT-006", "patterns", "extended regular expressions: + ? | ( ) { } and back-references", "Regular Expressions", ""),
    ("GPAT-007", "patterns", "ERE: an operator at the start of an expression is skipped with a warning", "Regular Expressions", "grep 3.8 and later"),
    ("GPAT-008", "patterns", "ERE: an invalid interval and an unmatched ) are characters; BRE: a leading \\{ is a character", "Regular Expressions", ""),
    ("GPAT-009", "patterns", "BRE: a leading * is a character; an operator after an anchor", "Regular Expressions", ""),
    ("GPAT-010", "patterns", "a backslash before an ordinary character: no sed escapes (\\n \\t), the character itself", "Regular Expressions", ""),
    ("GPAT-011", "patterns", "-F: fixed strings, several, with characters that are special in regular expressions", "Invoking grep", ""),
    ("GPAT-012", "patterns", "-i and --no-ignore-case", "Invoking grep", ""),
    ("GPAT-013", "patterns", "-w: the match is a whole word; a shorter match at the same place is tried", "Invoking grep", ""),
    ("GPAT-014", "patterns", "-x: the match is the whole line; -x with -w", "Invoking grep", ""),
    ("GPAT-015", "patterns", "-v selects the lines that do not match", "Invoking grep", ""),
    ("GPAT-016", "patterns", "several patterns: the leftmost, then the longest match for -o and --color", "Invoking grep", ""),
    ("GPAT-017", "patterns", "the empty pattern matches every line; -v with it selects none", "Invoking grep", ""),
    ("GPAT-018", "patterns", "patterns that do not compile: the message and status 2", "Invoking grep", ""),
    ("GPAT-019", "patterns", "-z: ^ and $ and . with records that end in NUL", "Invoking grep", ""),
    ("GPAT-020", "patterns", "UTF-8: characters, classes, -i, -w, -o", "Environment Variables", ""),
    ("GPAT-021", "patterns", "patterns with a back-reference: the lines and the matches", "Regular Expressions", ""),
    ("GOUT-001", "output", "-n: line numbers", "General Output Control", ""),
    ("GOUT-002", "output", "-b: byte offsets (of the line, of the match with -o)", "General Output Control", ""),
    ("GOUT-003", "output", "-H and -h: file names in front of lines (several files, -r)", "General Output Control", ""),
    ("GOUT-004", "output", "-c: counts, with names for several files", "General Output Control", ""),
    ("GOUT-005", "output", "-l: names of the files with a selected line", "General Output Control", ""),
    ("GOUT-006", "output", "-L: names of the files with none", "General Output Control", ""),
    ("GOUT-007", "output", "-q: no output, exit at the first selected line", "General Output Control", ""),
    ("GOUT-008", "output", "-m NUM: stop after NUM selected lines", "General Output Control", ""),
    ("GOUT-009", "output", "-o: only the matches, one per line; empty matches are not printed", "General Output Control", ""),
    ("GOUT-010", "output", "-o with -b, -n, -v, -i, -c", "General Output Control", ""),
    ("GOUT-011", "output", "-T: the numbers have a width and a tab follows the prefix", "General Output Control", ""),
    ("GOUT-012", "output", "-Z: a NUL after the file name", "General Output Control", ""),
    ("GOUT-013", "output", "a last line without a newline gets one; CRLF is kept; --line-buffered", "General Output Control", ""),
    ("GOUT-014", "output", "-q over -l and -L over -c; -c with -v; -m 0", "General Output Control", ""),
    ("GOUT-015", "output", "option combinations of the output", "General Output Control", ""),
    ("GCTX-001", "context", "-A, -B, -C: lines of context, `-` after the file name and numbers", "Context Line Control", ""),
    ("GCTX-002", "context", "the group separator `--`, --group-separator and --no-group-separator", "Context Line Control", ""),
    ("GCTX-003", "context", "context with -n, -b, -H and with several files", "Context Line Control", ""),
    ("GCTX-004", "context", "-m with context: the trailing context after the last selected line", "Context Line Control", ""),
    ("GCTX-005", "context", "-v with context", "Context Line Control", ""),
    ("GCTX-006", "context", "-o, -c and -l with context", "Context Line Control", ""),
    ("GCTX-007", "context", "context near the start and the end, overlapping groups", "Context Line Control", ""),
    ("GFIL-001", "files", "a directory operand: `Is a directory`, -d read, skip and recurse", "File and Directory Selection", ""),
    ("GFIL-002", "files", "-r and -R: names, no operand means `.` without a leading ./, symlinks", "File and Directory Selection", ""),
    ("GFIL-003", "files", "--include, --exclude and --exclude-from: the last matching pattern decides", "File and Directory Selection", ""),
    ("GFIL-004", "files", "--exclude-dir", "File and Directory Selection", ""),
    ("GFIL-005", "files", "-D read and skip", "File and Directory Selection", ""),
    ("GFIL-006", "files", "a missing file: the message, the other files are searched, status 2", "Exit Status", ""),
    ("GFIL-007", "files", "a regular file on standard input is left after the last selected line with -m", "Invoking grep", ""),
    ("GFIL-008", "files", "large files and long lines: line numbers, offsets and context across the pieces read", "Invoking grep", ""),
    ("GBIN-001", "binary", "a file with a NUL byte: `binary file matches` on standard error", "File and Directory Selection", ""),
    ("GBIN-002", "binary", "-a and --binary-files=text", "File and Directory Selection", ""),
    ("GBIN-003", "binary", "-I and --binary-files=without-match", "File and Directory Selection", ""),
    ("GBIN-004", "binary", "binary files with -c, -l, -L, -q, -o, -v, -m, context", "File and Directory Selection", ""),
    ("GBIN-005", "binary", "UTF-8 text with an encoding error: the line is not printed, the message follows", "File and Directory Selection", ""),
    ("GBIN-006", "binary", "-U is accepted and changes nothing", "File and Directory Selection", ""),
    ("GBIN-007", "binary", "-z: NUL ends the lines, and the files are not binary because of it", "Invoking grep", ""),
    ("GBIN-008", "binary", "a NUL after the first 96 KiB: the lines before are printed", "File and Directory Selection", ""),
    ("GCOL-001", "color", "--color=always: the matches", "General Output Control", ""),
    ("GCOL-002", "color", "colors of file names, line numbers, byte offsets and separators", "General Output Control", ""),
    ("GCOL-003", "color", "colors with context, -v, -o and -c", "General Output Control", ""),
    ("GCOL-004", "color", "GREP_COLORS: mt ms mc sl cx fn ln bn se rv ne, unknown names, bad values", "Environment Variables", ""),
    ("GCOL-005", "color", "GREP_COLOR (obsolete)", "Environment Variables", ""),
    ("GCOL-006", "color", "--color=never, auto, tty, yes, force, no, none and a value that is not known", "General Output Control", ""),
    ("GPCRE-001", "perl", "-P: Perl-compatible regular expressions", "Invoking grep", ""),
    ("GPCRE-002", "perl", "-P: lookaround, \\K, -o and -b", "Invoking grep", ""),
    ("GPCRE-003", "perl", "-P: errors of the library, one pattern only, the empty pattern", "Invoking grep", ""),
    ("GPCRE-004", "perl", "-P with -i, -w, -x, -v, -c, context and -z", "Invoking grep", ""),
    ("GPCRE-005", "perl", "-P in UTF-8 and in the C locale", "Invoking grep", ""),
    ("GPCRE-006", "perl", "-P: a match that PCRE2 gives up on (its match limit) is an error that ends the run with status 2", "Invoking grep", ""),
]

# ---- the fixtures ------------------------------------------------------------------------------
# name, items, commands. A command that starts with `!` is shell that makes files (not printed).
# `env:KEY=VALUE` lines set the environment.
F = []
def fx(name, items, *cmds, env=None, files=None):
    F.append((name, items.split(), list(cmds), env or [], files or {}))

# -- invocation
fx("grep-inv-pattern-and-files", "GINV-001", "grep alpha a.txt", "grep alpha a.txt b.txt", "grep alpha nosuch.txt a.txt", "grep alpha 'sp ace.txt'", "grep -c alpha a.txt b.txt d/c.txt")
fx("grep-inv-e-patterns", "GINV-002", "grep -e alpha -e beta a.txt", "grep -e alpha -e beta -c a.txt", "grep -e alpha a.txt -e gamma", "grep -e 'alpha\nbeta' a.txt", "grep -e alpha -e '' -c a.txt", "grep -E 'a|b' -e c a.txt")
fx("grep-inv-f-file", "GINV-003", "grep -f pats.txt a.txt b.txt", "grep -f pats.txt -e gamma a.txt", "grep -e gamma -f pats.txt a.txt", "grep -f pats.txt -v a.txt", "grep -f /dev/null a.txt", "grep -c -f /dev/null a.txt", "grep -v -f /dev/null a.txt", "grep -f nosuch a.txt", "grep -F -f pats.txt a.txt", "grep -w -f pats.txt a.txt", "grep -x -f pats.txt a.txt", "printf 'alpha\\n' | grep -f - a.txt")
fx("grep-inv-options-anywhere", "GINV-004", "grep alpha a.txt -n", "grep -n alpha -H a.txt", "grep -nH -c alpha a.txt", "grep -- -v a.txt", "grep -e -v a.txt", "grep alpha -- a.txt", "grep -c -- alpha a.txt")
fx("grep-inv-long-options", "GINV-005", "grep --line-number alpha a.txt", "grep --line-n alpha a.txt", "grep --count alpha a.txt", "grep --max-count=1 alpha a.txt", "grep --max-count 1 alpha a.txt", "grep --regexp=alpha a.txt", "grep --ignore-case ALPHA a.txt", "grep --no-filename alpha a.txt b.txt", "grep --with-filename alpha a.txt", "grep --no alpha a.txt", "grep --co alpha a.txt", "grep --foo alpha a.txt", "grep --count=3 alpha a.txt", "grep --colour=never alpha a.txt", "grep --files-with alpha a.txt", "grep --null -l alpha a.txt | od -c")
fx("grep-inv-context-digits", "GINV-006", "grep -1 beta a.txt", "grep -12 beta a.txt", "grep -1 -2 beta a.txt", "grep -2 -1 beta a.txt", "grep -C1 -2 beta a.txt", "grep -2 -C1 beta a.txt", "grep -A1 -3 beta a.txt")
fx("grep-inv-usage-errors", "GINV-007", "grep", "grep -e", "grep -N alpha a.txt", "grep -J alpha a.txt", "grep --foo alpha a.txt", "grep --regexp", "grep -A", "grep -m", "grep --max-count", "grep -f")
fx("grep-inv-invalid-values", "GINV-008", "grep -A x alpha a.txt", "grep -A -1 alpha a.txt", "grep -B 1x alpha a.txt", "grep -C '' alpha a.txt", "grep -m x alpha a.txt", "grep -m -1 alpha a.txt", "grep -m 99999999999999999999 alpha a.txt", "grep -d foo alpha a.txt", "grep -D foo alpha a.txt", "grep -X foo alpha a.txt", "grep -X egrep 'a|b' a.txt", "grep --binary-files=foo alpha a.txt", "grep -E -F alpha a.txt", "grep -G -P alpha a.txt", "grep -E -E 'a|b' a.txt")
fx("grep-inv-stdin", "GINV-009", "grep alpha < a.txt", "grep -c alpha < a.txt", "grep -n alpha - - < a.txt", "grep -H alpha - a.txt < b.txt", "grep --label=X -H alpha - < a.txt", "grep --label=X -c alpha - a.txt < b.txt", "grep --label=X -l alpha - < a.txt", "grep -L alpha - < empty", "grep -r alpha - < a.txt", "cat a.txt | grep -n alpha", "cat a.txt | grep -c alpha - a.txt")
fx("grep-inv-egrep-fgrep", "GINV-010", "egrep 'al(ph)a' a.txt", "egrep -c 'a|b' a.txt", "egrep 'a{1' a.txt", "fgrep 'a.' words.txt", "fgrep -x alpha a.txt", "fgrep -c a a.txt", "egrep", "fgrep", "egrep -N a a.txt", "egrep '(' a.txt", "fgrep -E alpha a.txt", "egrep -F alpha a.txt")
fx("grep-inv-exit-status", "GINV-011", "grep alpha a.txt >/dev/null; echo $?", "grep zzz a.txt; echo $?", "grep alpha nosuch; echo $?", "grep alpha nosuch a.txt >/dev/null; echo $?", "grep -q alpha a.txt; echo $?", "grep -q alpha nosuch a.txt; echo $?", "grep -q zzz nosuch a.txt; echo $?", "grep -s alpha nosuch a.txt >/dev/null; echo $?", "grep -v alpha a.txt >/dev/null; echo $?", "grep -c zzz a.txt; echo $?", "grep -L alpha a.txt; echo $?", "grep -L zzz a.txt; echo $?", "grep -l zzz a.txt; echo $?")
fx("grep-inv-s-silent", "GINV-012", "grep -s alpha nosuch", "grep -s alpha nosuch a.txt", "grep -s alpha d", "grep -sq alpha nosuch a.txt", "grep -sq zzz nosuch a.txt", "grep -s -c alpha nosuch a.txt", "grep -s -L alpha d", "grep -s '[' a.txt", "grep -s", "grep -s -N a a.txt")
fx("grep-inv-obsolete", "GINV-013", "grep -y ALPHA a.txt", "grep -u alpha a.txt", "grep -uy ALPHA a.txt", "grep --unix-byte-offsets alpha a.txt")

# -- patterns: basic
fx("grep-pat-bre-basics", "GPAT-001", "grep 'al.ha' a.txt", "grep 'a*' -c a.txt", "grep -o 'a*' a.txt", "grep '^alpha' a.txt", "grep 'beta$' a.txt", "grep '^$' words.txt empty", "grep -c '^$' empty a.txt", "grep 'a.*a' a.txt", "grep -o 'l*' a.txt", "grep 'a\\.b' words.txt", "grep -c . a.txt", "grep '^a.*a$' a.txt", "grep 'x*' -c a.txt", "grep -o 'ph.' a.txt")
fx("grep-pat-bre-gnu", "GPAT-002", "grep 'al\\(ph\\)a' a.txt", "grep 'a\\+' -o a.txt", "grep 'alpha\\?' -o a.txt", "grep 'alpha\\|beta' a.txt", "grep -o 'be\\(t\\)a' a.txt", "grep '\\(a\\)\\1' words.txt a.txt", "grep '\\(.\\)\\1' -o words.txt", "grep '\\(o\\)\\1' -c words.txt", "grep -o '\\(foo\\)\\(bar\\)' words.txt", "grep '\\(a\\)\\2' a.txt", "grep '\\(a' a.txt", "grep 'a\\)' a.txt", "grep 'a\\|' -c a.txt", "grep -c '\\|a' a.txt", "grep -o 'a\\{1,\\}' a.txt")
fx("grep-pat-intervals", "GPAT-003", "grep -o 'o\\{2\\}' words.txt", "grep -o 'o\\{2,\\}' words.txt", "grep -o 'o\\{,1\\}b' words.txt", "grep -o 'o\\{1,2\\}' words.txt", "grep -E -o 'o{2}' words.txt", "grep -E -o 'o{1,2}b' words.txt", "grep -E -o 'o{,2}b' words.txt", "grep -E -c 'x{0}' a.txt", "grep 'a\\{1' a.txt", "grep 'a\\{1,2' a.txt", "grep 'a\\{x\\}' a.txt", "grep 'a\\{2,1\\}' a.txt", "grep 'a\\{\\}' a.txt", "grep -E 'a{2,1}' a.txt", "grep -E 'a{}' a.txt", "grep 'a\\{99999\\}' a.txt", "grep -E 'a{32768}' a.txt", "grep -c -E 'a{32767}' a.txt")
fx("grep-pat-brackets", "GPAT-004", "grep -o '[abc]' a.txt", "grep -o '[a-c]*' a.txt", "grep -o '[^a-m ]*' a.txt", "grep -o '[]a]' words.txt", "grep -c '[^]a]' a.txt", "grep -o '[[:alpha:]]*' words.txt", "grep -o '[[:digit:]]' ctx.txt", "grep -c '[[:upper:]]' words.txt", "grep -o '[[:space:]]' a.txt", "grep -o '[[:punct:]]' words.txt", "grep -o '[[.a.]]' a.txt", "grep -o '[[=a=]]' a.txt", "grep '[[:foo:]]' a.txt", "grep '[:alpha:]' a.txt", "grep '[[:alpha:]' a.txt", "grep '[a' a.txt", "grep '[b-a]' a.txt", "grep '[' a.txt", "grep -o '[.]' words.txt", "grep -o '[*]' words.txt", "grep -c '[\\]' a.txt", "grep -o '[a\\]' a.txt", "grep -o '[a-]' words.txt")
fx("grep-pat-gnu-escapes", "GPAT-005", "grep -o '\\w\\+' words.txt", "grep -o '\\W' words.txt", "grep -c '\\s' a.txt", "grep -o '\\S\\+' a.txt", "grep -o '\\bfoo' words.txt", "grep -o 'foo\\b' words.txt", "grep -o '\\<foo' words.txt", "grep -o 'foo\\>' words.txt", "grep -c '\\Boo' words.txt", "grep -o 'o\\B' words.txt", "grep -c '\\`alpha' a.txt", "grep -c \"beta\\\\'\" a.txt", "grep -o '\\<\\w' words.txt", "grep -E -o '\\w+' words.txt", "grep -E -o 'foo\\b' words.txt", "grep -o '\\<' a.txt", "grep -c '\\<' a.txt")
fx("grep-pat-ere", "GPAT-006", "grep -E 'al(ph)a' a.txt", "grep -E 'a+' -o a.txt", "grep -E 'alpha?' -o a.txt", "grep -E 'alpha|beta' a.txt", "grep -E -o '(a|b)+' a.txt", "grep -E '(a)\\1' words.txt a.txt", "grep -E '(.)\\1' -o words.txt", "grep -E '^(alpha|beta)$' a.txt", "grep -E '(' a.txt", "grep -E '(a' a.txt", "grep -E 'a|' -c a.txt", "grep -E '|a' -c a.txt", "grep -E '()' -c a.txt", "grep -E -o 'a{1,2}' a.txt", "grep -E 'a\\|b' a.txt words.txt", "grep -E '\\(foo\\)' words.txt", "grep -E 'o{2}|b' -o words.txt", "grep -E -x 'alpha|beta' a.txt")
fx("grep-pat-ere-leading-operators", "GPAT-007", "grep -E '*a' words.txt", "grep -E '+a' words.txt", "grep -E '?a' words.txt", "grep -E '{1}a' words.txt", "grep -E '(*a)' words.txt", "grep -E '(+a)' words.txt", "grep -E 'a|*b' b.txt", "grep -E '^*a' a.txt", "grep -E '^+' -c a.txt", "grep -E 'x^*' -c a.txt", "grep -E 'a$*' -c a.txt", "grep -E '\\<*a' -c a.txt", "grep -E '*' -c a.txt", "grep -E '**a' -c a.txt", "grep -E 'a**' -c a.txt", "grep -E 'a+*' -c a.txt", "grep -E '{' -c a.txt", "grep -E '{,1}a' -c a.txt")
fx("grep-pat-literal-braces-and-parens", "GPAT-008", "grep -E 'a{1' a.txt words.txt", "grep -E 'a{1,2' a.txt", "grep -E 'a{x}' a.txt", "grep -E 'a{,}' -c a.txt", "grep -E 'a)' a.txt words.txt", "grep -E ')' a.txt words.txt", "grep -E '(a))' a.txt", "grep -E 'o{1' words.txt", "grep -E -o '(foo)' words.txt", "grep '\\{1\\}a' a.txt", "grep -o '\\{1\\}' a.txt words.txt", "grep 'a\\{1\\}\\{2\\}' a.txt", "grep -c 'a**' a.txt", "grep -E '(a{1)' a.txt", "grep -E 'a{1}{2}' -c a.txt")
fx("grep-pat-bre-leading-star", "GPAT-009", "grep '*a' a.txt words.txt", "grep -o '*' words.txt", "grep '^*' words.txt a.txt", "grep '\\(*a\\)' a.txt words.txt", "grep 'a\\|*b' a.txt", "grep -c 'a**' a.txt", "grep -c '^*a' a.txt", "grep -c 'x\\?*' a.txt", "grep -c 'a\\{1\\}*' a.txt", "grep -c '\\<*' a.txt")
fx("grep-pat-backslash-ordinary", "GPAT-010", "grep 'a\\tb' a.txt", "grep -c '\\n' a.txt", "grep -o '\\a' a.txt", "grep '\\x41' a.txt", "grep -c '\\/' a.txt", "grep -c '\\-' words.txt", "grep -o '\\ ' words.txt", "grep -o '\\.' words.txt", "grep -c '\\z' a.txt", "grep -c '\\d' a.txt", "grep '\\' a.txt", "grep -E '\\' a.txt", "grep -i -o '\\a' a.txt", "grep -E -i -o 'a\\l' a.txt")
fx("grep-pat-fixed-strings", "GPAT-011", "grep -F 'a.' words.txt a.txt", "grep -F -e foo -e beta words.txt a.txt", "grep -F '[' words.txt", "grep -F '*' words.txt", "grep -F '\\' a.txt", "grep -F 'foo|' words.txt", "grep -F -c '' a.txt", "grep -F -x alpha a.txt", "grep -F -w foo words.txt", "grep -F -i ALPHA a.txt", "grep -F -o a a.txt", "grep -F -e alpha -e alp -o a.txt", "grep -F -v alpha a.txt", "grep -F -e 'a\nb' -c a.txt", "grep -F '(foo)' words.txt")
fx("grep-pat-ignore-case", "GPAT-012", "grep -i ALPHA a.txt", "grep -i 'B[A-Z]*A' a.txt", "grep -i -o 'FOO' words.txt", "grep -i -c 'foo' words.txt", "grep -i '[A-C]lpha' a.txt", "grep -i -E 'ALPHA|BETA' a.txt", "grep -i -w FOO words.txt", "grep -i -x FOO words.txt", "grep -i --no-ignore-case ALPHA a.txt", "grep --no-ignore-case -i ALPHA a.txt", "grep -i '\\(A\\)\\1' words.txt", "grep -i -o '\\bFOO' words.txt", "grep -i '[[:upper:]]' -c a.txt", "grep -i -F ALPHA a.txt")
fx("grep-pat-word-regexp", "GPAT-013", "grep -w foo words.txt", "grep -ow foo words.txt", "grep -w alpha a.txt", "grep -w 'foo.*' words.txt", "grep -ow 'foo.*' words.txt", "grep -ow 'fo*' words.txt", "grep -w 'foo\\|foobar' -o words.txt", "grep -w 'x*' -c words.txt", "grep -w '' -c words.txt", "grep -wc 'o*' words.txt", "grep -w '(foo)' words.txt", "grep -w -E 'foo|bar' -o words.txt", "grep -wb foo words.txt", "grep -w 'foo_bar' words.txt", "grep -ow '[a-z]*' words.txt", "grep -w -v foo words.txt", "grep -w '\\(f\\)oo' words.txt", "grep -w '\\(o\\)\\1' -o words.txt", "grep -n -w bar words.txt")
fx("grep-pat-line-regexp", "GPAT-014", "grep -x alpha a.txt", "grep -x 'alpha.*' a.txt", "grep -x -E 'alpha|beta' a.txt", "grep -xc '' a.txt empty", "grep -xv alpha a.txt", "grep -xo alpha a.txt", "grep -xo 'a.*' a.txt", "grep -wx alpha a.txt", "grep -wxo alpha a.txt", "grep -wx 'foo bar' words.txt", "grep -wxc '' a.txt empty", "grep -x '(foo)' words.txt", "grep -x -F foo words.txt", "grep -x -i FOO words.txt", "grep -xn foo words.txt")
fx("grep-pat-invert", "GPAT-015", "grep -v alpha a.txt", "grep -vc alpha a.txt", "grep -nv alpha a.txt", "grep -v -e alpha -e beta a.txt", "grep -vx alpha a.txt", "grep -vw foo words.txt", "grep -v '' a.txt", "grep -vc '' a.txt", "grep -vc zzz a.txt", "grep -vo alpha a.txt", "grep -v alpha a.txt b.txt", "grep -v -e '' -e zzz -c a.txt", "grep -vF alpha a.txt", "grep -vl zzz a.txt", "grep -vL alpha a.txt b.txt")
fx("grep-pat-several-patterns", "GPAT-016", "grep -o -e a -e al a.txt", "grep -o -e al -e alpha -e a a.txt", "grep -o -e 'b.*' -e 'be' a.txt", "grep -o -E 'a|al|alp' a.txt", "grep -ob -e eta -e et a.txt", "grep -o -e '' -e a a.txt", "grep -o -e a* -e b a.txt", "grep -c -e alpha -e beta -e gamma a.txt", "grep -n -e gamma -e alpha a.txt", "grep -F -o -e ph -e pha -e alpha a.txt", "grep -w -o -e foo -e foobar words.txt", "grep -o -e 'a' -e 'b' -e 'c' b.txt a.txt", "grep -o -i -e ALPHA -e al a.txt", "grep -ox -e alpha -e a.* a.txt")
fx("grep-pat-empty-pattern", "GPAT-017", "grep '' a.txt", "grep -c '' a.txt", "grep -c '' empty", "grep -vc '' a.txt", "grep -L -v '' a.txt", "grep -n '' b.txt", "grep -o '' a.txt", "grep -e '' -e zzz -c a.txt", "grep -x '' words.txt", "grep -xc '' a.txt", "grep -w '' words.txt", "grep -wc '' a.txt", "grep -c '' nonl.txt", "grep -E '' -c a.txt", "grep -F '' -c a.txt", "grep -F -v '' -c a.txt", "grep -f /dev/null -c a.txt", "grep -vc -x '' a.txt", "grep -vc -w '' a.txt")
fx("grep-pat-errors", "GPAT-018", "grep '[' a.txt", "grep '\\(' a.txt", "grep '\\)' a.txt", "grep '\\' a.txt", "grep 'a\\{1' a.txt", "grep 'a\\{1,2' a.txt", "grep 'a\\{x\\}' a.txt", "grep '\\(a\\)\\2' a.txt", "grep -E '(' a.txt", "grep -E 'a{1,2}{' a.txt", "grep '[[:foo:]]' a.txt", "grep '[[.x.]]' a.txt", "grep '[[=x=]]' a.txt", "grep '[b-a]' a.txt", "grep 'a\\{65536\\}' a.txt", "grep -E 'a{65536}' a.txt", "grep -e '[' -e a a.txt", "grep -e a -e '\\(' a.txt", "grep '[:space:]' a.txt", "grep '[[:space:]' a.txt")
fx("grep-pat-null-data", "GPAT-019", "grep -z alpha z.dat | od -c", "grep -zc alpha z.dat", "grep -zc '' z.dat", "grep -z 'one.two' z.dat | od -c", "grep -z '^two' z.dat | od -c", "grep -z 'alpha$' z.dat | od -c", "grep -zx one z.dat | od -c", "grep -zv one z.dat | od -c", "grep -zn t z.dat | od -c", "grep -zb t z.dat | od -c", "grep -z . a.txt | od -c", "grep -zc . a.txt", "grep -z 'alpha.beta' a.txt | od -c", "grep -z '^beta' a.txt | od -c", "grep -zo 'a$' a.txt | od -c", "grep -zl alpha z.dat | od -c", "grep -zZ -l alpha z.dat | od -c", "grep -zo 'o' z.dat | od -c", "grep -z -A1 one z.dat | od -c")
fx("grep-pat-utf8", "GPAT-020", "grep -o . utf8.txt", "grep -c . utf8.txt", "grep -i 'CAFÉ' utf8.txt", "grep -ob 'ï' utf8.txt", "grep -o 'na.ve' utf8.txt", "grep -w 'na.ve' utf8.txt", "grep -o '[[:alpha:]]*' utf8.txt", "grep -o '\\w\\+' utf8.txt", "grep -o '[^a-z]' utf8.txt", "grep -i 'あ' utf8.txt", "grep -o 'あ*' utf8.txt", "grep -c '^.\\{3\\}$' utf8.txt", "grep -x '.\\{4\\}' utf8.txt", "grep -F 'café' utf8.txt", "grep -oi 'NÏVE' utf8.txt", "grep -o -E '[あ-う]+' utf8.txt", "grep -c '[[:upper:]]' utf8.txt", "grep -io 'CAF.' utf8.txt", env=["LC_ALL=C.UTF-8"])
fx("grep-pat-utf8-c-locale", "GPAT-020", "grep -o . utf8.txt | od -c | sed -n '1,3p'", "grep -c . utf8.txt", "grep -i 'CAFÉ' utf8.txt", "grep -o 'caf.' utf8.txt | od -c")
fx("grep-pat-backref-details", "GPAT-021", "grep -o '\\(a\\)\\1' words.txt a.txt", "grep -x '\\(.\\)\\1*' words.txt", "grep -w '\\(f\\)\\(o\\)\\2' words.txt", "grep -c -E '(.)\\1' a.txt words.txt", "grep -o -E '(o)\\1' words.txt", "grep -n -E '([a-z])\\1' words.txt", "grep -i -E '(f)\\1' words.txt", "grep -E '(a)|\\1' a.txt", "grep -E '(a)(b)?\\2' -c a.txt", "grep -v -E '(.)\\1' words.txt", "grep -wo '\\(.\\)\\1' words.txt", "grep -xo '\\(foo\\).*\\1' words.txt", "grep '\\(a*\\)b\\1' -o words.txt a.txt", "grep -E '^(.*)\\1$' b.txt words.txt")

# -- output
fx("grep-out-line-numbers", "GOUT-001", "grep -n alpha a.txt", "grep -n alpha a.txt b.txt", "grep -nv alpha a.txt", "grep -n '' b.txt", "grep -nc alpha a.txt", "grep -n alpha nonl.txt", "grep -n -m1 alpha a.txt", "grep -no a a.txt", "grep -n hit ctx.txt", "grep -n 'line 1[0-9]' ctx.txt", "grep -n . empty", "cat a.txt b.txt | grep -n alpha")
fx("grep-out-byte-offsets", "GOUT-002", "grep -b alpha a.txt", "grep -b '' b.txt", "grep -bo a a.txt", "grep -bn alpha a.txt", "grep -bv alpha a.txt", "grep -b alpha nonl.txt", "grep -bc alpha a.txt", "grep -b hit ctx.txt", "grep -bo 'l.' a.txt", "grep -bH alpha a.txt b.txt", "grep -b alpha crlf.txt", "grep -b -C1 beta a.txt", "grep -bo -e a -e b a.txt")
fx("grep-out-filenames", "GOUT-003", "grep -H alpha a.txt", "grep -h alpha a.txt b.txt", "grep alpha a.txt b.txt", "grep -H -h alpha a.txt b.txt", "grep -h -H alpha a.txt b.txt", "grep -c alpha a.txt b.txt", "grep -hc alpha a.txt b.txt", "grep -Hc alpha a.txt", "grep -Hn alpha a.txt b.txt", "grep -Ho a a.txt", "grep -H alpha - < a.txt", "grep -H alpha 'sp ace.txt'", "grep -H alpha nonl.txt", "grep -Hl alpha a.txt", "grep -Hv alpha a.txt")
fx("grep-out-count", "GOUT-004", "grep -c alpha a.txt", "grep -c alpha a.txt b.txt", "grep -c zzz a.txt", "grep -c alpha a.txt nosuch b.txt", "grep -c '' a.txt empty", "grep -cv alpha a.txt", "grep -co alpha a.txt", "grep -cH alpha a.txt", "grep -ch alpha a.txt b.txt", "grep -c -m1 alpha a.txt", "grep -c -A1 alpha a.txt", "grep -c alpha nonl.txt", "grep -c alpha d c.txt", "grep -c alpha - < a.txt", "grep -c -i ALPHA a.txt")
fx("grep-out-files-with-matches", "GOUT-005", "grep -l alpha a.txt b.txt empty", "grep -l zzz a.txt", "grep -lv alpha a.txt b.txt", "grep -l alpha a.txt nosuch b.txt", "grep -l alpha - < a.txt", "grep -lc alpha a.txt", "grep -ln alpha a.txt", "grep -l -m1 alpha a.txt", "grep -lZ alpha a.txt b.txt | od -c", "grep -ls alpha nosuch a.txt", "grep -lo alpha a.txt", "grep -lH alpha a.txt", "grep -l -A1 alpha a.txt")
fx("grep-out-files-without-match", "GOUT-006", "grep -L alpha a.txt b.txt empty", "grep -L zzz a.txt", "grep -Lv alpha a.txt b.txt", "grep -L alpha a.txt nosuch b.txt", "grep -L alpha - < empty", "grep -Lc zzz a.txt", "grep -L -m1 alpha a.txt b.txt", "grep -LZ zzz a.txt b.txt | od -c", "grep -L zzz a.txt; echo $?", "grep -L alpha a.txt; echo $?", "grep -L alpha a.txt empty; echo $?", "grep -L -m0 alpha a.txt; echo $?", "grep -L -v '' a.txt")
fx("grep-out-quiet", "GOUT-007", "grep -q alpha a.txt; echo $?", "grep -q zzz a.txt; echo $?", "grep -q alpha nosuch a.txt; echo $?", "grep -q zzz nosuch a.txt; echo $?", "grep -qc alpha a.txt; echo $?", "grep -ql alpha a.txt; echo $?", "grep -qL zzz a.txt; echo $?", "grep -qn alpha a.txt; echo $?", "grep -qo alpha a.txt; echo $?", "grep --quiet alpha a.txt; echo $?", "grep --silent alpha a.txt; echo $?", "grep -q alpha d a.txt; echo $?", "grep -q -m5 alpha d a.txt; echo $?", "grep -q alpha bin.dat; echo $?", "grep -q '' empty; echo $?")
fx("grep-out-max-count", "GOUT-008", "grep -m1 alpha a.txt", "grep -m2 alpha a.txt b.txt", "grep -m0 alpha a.txt; echo $?", "grep -m1 -c alpha a.txt b.txt", "grep -m1 -n alpha a.txt b.txt", "grep -m1 -v alpha a.txt", "grep -m1 -o a a.txt", "grep -m1 -l alpha a.txt", "grep -m5 alpha a.txt", "grep --max-count=2 a a.txt", "grep -m 1 alpha -m 2 a.txt", "grep -m -1 alpha a.txt", "grep -m1 alpha nonl.txt", "grep -m0 -c alpha a.txt; echo $?", "grep -m0 -q alpha a.txt; echo $?", "grep -m1 -x 'alpha' a.txt", "grep -m2 '' a.txt")
fx("grep-out-only-matching", "GOUT-009", "grep -o alpha a.txt", "grep -o 'a.' a.txt", "grep -o 'x*' a.txt", "grep -o -e '' -e a a.txt", "grep -o 'a*' a.txt", "grep -o '^a' a.txt", "grep -o 'a$' a.txt", "grep -o 'l*' a.txt", "grep -o '[a-z]*' b.txt", "grep -o '.' nonl.txt", "grep -oE 'a|b' a.txt", "grep -o '\\<.' a.txt", "grep -o 'a\\b' a.txt", "grep -o 'alpha' bin.dat; echo $?", "grep -ao 'alpha' bin.dat | od -c", "grep -o -x alpha a.txt", "grep -o '' a.txt; echo $?", "grep -o 'a' crlf.txt | od -c")
fx("grep-out-only-matching-combinations", "GOUT-010", "grep -ob 'a.' a.txt", "grep -on a a.txt", "grep -onb a a.txt", "grep -ov alpha a.txt; echo $?", "grep -oc alpha a.txt", "grep -oi ALPHA a.txt", "grep -ow foo words.txt", "grep -oH alpha a.txt b.txt", "grep -oh alpha a.txt b.txt", "grep -ol alpha a.txt", "grep -oq alpha a.txt; echo $?", "grep -o -A1 alpha a.txt", "grep -o -C1 alpha a.txt", "grep -oF a a.txt", "grep -ovn alpha a.txt; echo $?", "grep -o -m1 a a.txt", "grep -on '' a.txt; echo $?")
fx("grep-out-initial-tab", "GOUT-011", "grep -T alpha a.txt", "grep -T -n alpha a.txt", "grep -T -nb alpha a.txt", "grep -T -H -n alpha a.txt", "grep -T -H alpha a.txt", "grep -T -b alpha a.txt", "grep -T -n alpha a.txt b.txt", "grep -T -c alpha a.txt", "grep -T -n -C1 hit ctx.txt", "grep -T -o a a.txt", "grep -T -n '' empty a.txt", "grep -T -nb '' b.txt", "grep -T -n alpha - < a.txt", "cat a.txt | grep -T -n alpha", "grep -T -n 'line 1' ctx.txt", "grep -T -nH hit ctx.txt")
fx("grep-out-null-after-name", "GOUT-012", "grep -Z alpha a.txt b.txt | od -c", "grep -ZH alpha a.txt | od -c", "grep -Zc alpha a.txt b.txt | od -c", "grep -Zn alpha a.txt b.txt | od -c", "grep -Zl alpha a.txt b.txt | od -c", "grep -ZL zzz a.txt b.txt | od -c", "grep -Zo a a.txt b.txt | od -c", "grep -Z -C1 beta a.txt b.txt | od -c", "grep -Z alpha a.txt | od -c")
fx("grep-out-line-endings", "GOUT-013", "grep alpha nonl.txt | od -c", "grep -c '' nonl.txt", "grep -v zzz nonl.txt | od -c", "grep last nonl.txt a.txt | od -c", "grep -o last nonl.txt | od -c", "grep -n last nonl.txt", "grep alpha crlf.txt | od -c", "grep -c 'alpha$' crlf.txt", "grep -o 'a.$' crlf.txt | od -c", "grep -U alpha crlf.txt | od -c", "grep --line-buffered alpha a.txt", "grep -h last nonl.txt nonl.txt | od -c", "printf 'x' | grep x | od -c", "printf '' | grep -c x", "printf '\\n' | grep -c ''")
fx("grep-out-precedence", "GOUT-014", "grep -c -l alpha a.txt b.txt", "grep -c -L alpha a.txt b.txt empty", "grep -c -q alpha a.txt; echo $?", "grep -l -L alpha a.txt b.txt", "grep -L -l alpha a.txt b.txt", "grep -q -l alpha a.txt; echo $?", "grep -vc alpha a.txt", "grep -cv '' a.txt; echo $?", "grep -c -m0 alpha a.txt; echo $?", "grep -l -m0 alpha a.txt; echo $?", "grep -L -m0 alpha a.txt b.txt; echo $?", "grep -c -o alpha a.txt", "grep -n -c alpha a.txt", "grep -H -l alpha a.txt")
fx("grep-out-combinations", "GOUT-015", "grep -nH alpha a.txt", "grep -nb alpha a.txt b.txt", "grep -Hnb alpha a.txt", "grep -vn -m1 alpha a.txt", "grep -i -n ALPHA a.txt", "grep -w -n foo words.txt", "grep -x -n alpha a.txt", "grep -E -n 'a|g' a.txt", "grep -F -n 'a b' a.txt", "grep -ic ALPHA a.txt b.txt", "grep -lc alpha a.txt", "grep -hn alpha a.txt b.txt", "grep -on 'a.' a.txt b.txt", "grep -nvx alpha a.txt")

# -- context
fx("grep-ctx-basic", "GCTX-001", "grep -A1 hit ctx.txt", "grep -B1 hit ctx.txt", "grep -C1 hit ctx.txt", "grep -A0 hit ctx.txt", "grep -B0 hit ctx.txt", "grep -C0 hit ctx.txt", "grep -A2 'line 20' ctx.txt", "grep -B2 'line 1 ' ctx.txt", "grep -B5 'line 3' ctx.txt", "grep -A1 -B2 hit ctx.txt", "grep -B2 -A1 hit ctx.txt", "grep -C1 -A0 hit ctx.txt", "grep -A0 -C1 hit ctx.txt", "grep --context=1 hit ctx.txt", "grep --after-context=1 hit ctx.txt", "grep --before-context=1 hit ctx.txt", "grep -A100 'line 18' ctx.txt", "grep -C 1 'line 10' ctx.txt")
fx("grep-ctx-group-separator", "GCTX-002", "grep -A1 hit ctx.txt", "grep -A1 --group-separator='##' hit ctx.txt", "grep -A1 --group-separator= hit ctx.txt", "grep -A1 --group-separator hit ctx.txt", "grep -A1 --no-group-separator hit ctx.txt", "grep -A1 --group-separator=X --no-group-separator hit ctx.txt", "grep -A1 --no-group-separator --group-separator=X hit ctx.txt", "grep -C1 --group-separator='a b' hit ctx.txt", "grep -A0 'hit' ctx.txt", "grep -B0 'line 3 ' ctx.txt", "grep -A1 'line 3' ctx.txt a.txt", "grep -C1 alpha a.txt b.txt", "grep -A1 --group-separator=--- alpha a.txt b.txt", "grep -C1 -c hit ctx.txt")
fx("grep-ctx-prefixes", "GCTX-003", "grep -n -C1 hit ctx.txt", "grep -nb -C1 hit ctx.txt", "grep -H -C1 'line 9' ctx.txt", "grep -Hn -C1 'line 9' ctx.txt", "grep -C1 hit ctx.txt ctx.txt", "grep -C1 -h hit ctx.txt ctx.txt", "grep -C1 -n hit ctx.txt a.txt", "grep -bn -A1 hit ctx.txt", "grep -n -B1 'line 15' ctx.txt", "grep -Hb -A1 'line 4' ctx.txt", "grep -C1 alpha a.txt b.txt d/c.txt", "grep -n -C1 alpha a.txt b.txt", "grep -c -A1 hit ctx.txt a.txt", "grep -Z -n -C1 hit ctx.txt a.txt | od -c | sed -n '1,6p'")
fx("grep-ctx-max-count", "GCTX-004", "grep -m1 -A1 alpha a.txt", "grep -m1 -A2 hit ctx.txt", "grep -m1 -C2 hit ctx.txt a.txt", "grep -m2 -A1 hit ctx.txt", "grep -m2 -B1 -n hit ctx.txt", "grep -m1 -A3 -n 'line 3' ctx.txt", "grep -m1 -A1 '' a.txt", "grep -m2 -A5 hit ctx.txt", "grep -m1 -B1 hit ctx.txt", "grep -m3 -A1 -v line ctx.txt; echo $?", "grep -m1 -A1 -c hit ctx.txt", "grep -m1 -A1 hit ctx.txt ctx.txt", "grep -m1 -A2 alpha a.txt b.txt")
fx("grep-ctx-invert", "GCTX-005", "grep -vC1 'line [0-9]*$' ctx.txt", "grep -A1 -v hit ctx.txt", "grep -B1 -v hit ctx.txt | sed -n '1,8p'", "grep -C1 -v line ctx.txt; echo $?", "grep -vn -A1 alpha a.txt", "grep -v -B1 beta a.txt", "grep -vC1 gamma a.txt", "grep -vA1 -c hit ctx.txt", "grep -v -A2 -m2 hit ctx.txt", "grep -vx -C1 'line 1' ctx.txt | sed -n '1,6p'", "grep -v -C1 -e hit -e 'line 1[0-9]' ctx.txt")
fx("grep-ctx-other-options", "GCTX-006", "grep -o -C1 hit ctx.txt", "grep -o -A1 hit ctx.txt", "grep -o -B1 hit ctx.txt", "grep -on -A1 hit ctx.txt", "grep -c -A1 hit ctx.txt", "grep -l -A1 hit ctx.txt", "grep -L -A1 hit ctx.txt", "grep -q -A1 hit ctx.txt; echo $?", "grep -ob -A1 hit ctx.txt", "grep -ov -A1 hit ctx.txt; echo $?", "grep -ow -C1 'line' ctx.txt | sed -n '1,8p'", "grep -oH -A1 'line 3' ctx.txt", "grep -oc -C1 hit ctx.txt")
fx("grep-ctx-edges", "GCTX-007", "grep -C3 'line 1 ' ctx.txt", "grep -C3 'line 20' ctx.txt", "grep -C2 'line 5' ctx.txt", "grep -C2 'line 3 ' ctx.txt", "grep -C1 -e 'line 3 ' -e 'line 5' ctx.txt", "grep -C1 -e 'line 3 ' -e 'line 6' ctx.txt", "grep -C1 -e 'line 3 ' -e 'line 7' ctx.txt", "grep -A1 -B1 -e 'line 3 ' -e 'line 6' ctx.txt", "grep -A1 -e 'line 3 ' -e 'line 5' ctx.txt", "grep -B1 -e 'line 3 ' -e 'line 5' ctx.txt", "grep -C1 'line' ctx.txt | sed -n '1,5p'", "grep -C30 hit ctx.txt | sed -n '1,3p'", "grep -C1 alpha nonl.txt", "grep -A1 alpha nonl.txt | od -c")

# -- files
fx("grep-fil-directories", "GFIL-001", "grep alpha d; echo $?", "grep -s alpha d; echo $?", "grep -c alpha d", "grep -c alpha d a.txt", "grep -L alpha d", "grep -l alpha d a.txt", "grep -d read alpha d", "grep -d skip alpha d a.txt; echo $?", "grep -d recurse alpha d 2>&1 | sort", "grep -d skip -c alpha d; echo $?", "grep --directories=skip alpha d a.txt", "grep -n alpha d a.txt", "grep alpha d d/c.txt", "grep -d skip -L alpha d; echo $?")
fx("grep-fil-recursive", "GFIL-002", "grep -r alpha d 2>&1 | sort", "grep -r alpha . 2>&1 | sort", "grep -r alpha 2>&1 | sort", "grep -rh alpha d 2>&1 | sort", "grep -rl alpha . 2>&1 | sort", "grep -rL alpha . 2>&1 | sort", "grep -rc alpha d 2>&1 | sort", "grep -rn alpha d 2>&1 | sort", "grep -rH alpha a.txt", "grep -r alpha a.txt", "grep -r alpha d/ 2>&1 | sort", "grep -r alpha d// 2>&1 | sort", "grep -r alpha ./d 2>&1 | sort", "grep -r alpha a.txt d 2>&1 | sort", "grep -r -A1 alpha d 2>&1 | sort", "grep -r alpha d/sub/e.log", "grep -R alpha d 2>&1 | sort", "grep -r --no-filename alpha . 2>&1 | sort", "grep -r -o alpha d 2>&1 | sort", "grep -rs alpha nosuch d 2>&1 | sort", "grep -r alpha - < a.txt", "grep -ri ALPHA d 2>&1 | sort")
fx("grep-fil-symlinks", "GFIL-002", "!ln -s a.txt link.txt; ln -s d dlink; ln -s nosuch broken", "grep -r alpha . 2>&1 | sort", "grep -R alpha . 2>&1 | sort", "grep -r alpha link.txt dlink 2>&1 | sort", "grep -R alpha link.txt dlink 2>&1 | sort", "grep alpha link.txt", "grep -r alpha broken; echo $?", "grep -R alpha broken; echo $?", "grep -rs alpha broken; echo $?", "grep -rl alpha . 2>&1 | sort", "grep -Rl alpha . 2>&1 | sort")
fx("grep-fil-include-exclude", "GFIL-003", "grep -r --include='*.txt' alpha . 2>&1 | sort", "grep -r --include='*.log' alpha . 2>&1 | sort", "grep -r --exclude='*.log' alpha . 2>&1 | sort", "grep -r --exclude='a*' alpha . 2>&1 | sort", "grep -r --include='*.txt' --exclude='b*' alpha . 2>&1 | sort", "grep -r --exclude='b*' --include='*.txt' alpha . 2>&1 | sort", "grep -r --include='sub' alpha . 2>&1 | sort", "grep -r --include='d/*' alpha . 2>&1 | sort", "grep -r --exclude='d/*' alpha . 2>&1 | sort", "grep -r --exclude='*/c.txt' alpha . 2>&1 | sort", "grep alpha a.txt --exclude='a.txt'; echo $?", "grep alpha d/c.txt --exclude='c.txt'; echo $?", "grep alpha d/c.txt --exclude='d/c.txt'; echo $?", "grep alpha d/c.txt --exclude='*c.txt'; echo $?", "grep alpha a.txt --include='b*'; echo $?", "grep alpha a.txt --include='a.txt'", "grep -r alpha a.txt d --include='c.txt' 2>&1 | sort", "printf '*.log\\n' > ex.lst; grep -r --exclude-from=ex.lst alpha . 2>&1 | sort", "grep -r --exclude-from=/dev/null alpha d 2>&1 | sort", "grep -r --exclude-from=nosuch alpha d; echo $?", "grep -r --include='*.[tl]*' alpha . 2>&1 | sort", "grep -r --exclude='[ab].txt' alpha . 2>&1 | sort", "grep -r --include='?.txt' alpha . 2>&1 | sort", "grep alpha - --exclude='*' < a.txt")
fx("grep-fil-exclude-dir", "GFIL-004", "grep -r --exclude-dir=sub alpha . 2>&1 | sort", "grep -r --exclude-dir=d alpha . 2>&1 | sort", "grep -r --exclude-dir='s*' alpha d 2>&1 | sort", "grep -r --exclude-dir=sub alpha d/sub; echo $?", "grep -r --exclude-dir=d/sub alpha . 2>&1 | sort", "grep -r --exclude-dir=. alpha .; echo $?", "grep -r --exclude-dir='*' alpha .; echo $?", "grep -r alpha d --exclude-dir=d; echo $?", "grep -r alpha d/ --exclude-dir=d 2>&1 | sort", "grep -r --exclude-dir=sub --exclude-dir=d alpha . 2>&1 | sort", "grep --exclude-dir=d alpha d; echo $?", "grep -d skip --exclude-dir=d alpha d a.txt", "grep -r --exclude-dir=sub/ alpha d 2>&1 | sort", "grep -r --exclude-dir=SUB alpha d 2>&1 | sort", "grep -r --exclude-dir='su?' alpha d 2>&1 | sort")
fx("grep-fil-devices", "GFIL-005", "grep -D skip alpha a.txt", "grep -D read alpha a.txt", "grep -D skip alpha - < a.txt", "grep -D skip -c alpha /dev/null; echo $?", "grep -D read -c alpha /dev/null; echo $?", "grep -c alpha /dev/null; echo $?", "grep -r -D skip alpha d 2>&1 | sort", "grep -D skip alpha a.txt b.txt 2>&1 | sort", "printf 'alpha\\n' | grep -D skip alpha", "printf 'alpha\\n' | grep -D skip alpha -")
fx("grep-fil-missing", "GFIL-006", "grep alpha nosuch; echo $?", "grep alpha nosuch a.txt; echo $?", "grep alpha a.txt nosuch b.txt; echo $?", "grep -c alpha nosuch a.txt", "grep -l alpha nosuch a.txt", "grep -L alpha nosuch a.txt", "grep -q alpha nosuch a.txt; echo $?", "grep -s alpha nosuch a.txt; echo $?", "grep alpha ''; echo $?", "grep alpha 'sp ace.txt' nosuch", "grep alpha ./nosuch/x; echo $?", "grep -r alpha nosuch; echo $?", "grep -rs alpha nosuch d | sort; echo $?", "chmod 000 a.txt; grep alpha a.txt b.txt; echo $?; chmod 644 a.txt")
fx("grep-fil-stdin-position", "GFIL-007", "( grep -m1 alpha; cat ) < a.txt", "( grep -m2 -A1 alpha; cat ) < a.txt", "( grep -m1 beta; echo --; grep -m1 gamma; echo --; cat ) < a.txt", "( grep -q alpha; cat ) < a.txt", "( grep -l alpha; cat ) < a.txt", "( grep -c alpha; cat ) < a.txt", "( head -n 1; grep alpha ) < a.txt", "( grep -m1 -c alpha; cat ) < a.txt", "( grep -m1 -v alpha; cat ) < a.txt", "( grep -m1 zzz; cat ) < a.txt", "( grep -m1 -n alpha - a.txt; cat ) < b.txt", "cat a.txt | ( grep -m1 alpha; cat )")
fx("grep-fil-large-files", "GFIL-008", "!" + BIG, "grep -c alpha big.txt", "grep -n 'L59999' big.txt", "grep -nb 'L45000' big.txt", "grep -c beta big.txt", "grep -m3 beta big.txt", "grep -m3 -A2 beta big.txt", "grep -C2 'L04999' big.txt", "grep -C3 'L07561' big.txt", "grep -C3 'L07562' big.txt", "grep -B3 'L15123' big.txt", "grep -A3 'L15122' big.txt", "grep -n -B2 -A2 'L22685' big.txt", "grep -c . big.txt", "grep -vc alpha big.txt", "grep -c L big.txt big.txt", "grep -b '^L3000' big.txt", "grep -o 'L0000.' big.txt", "grep -l beta big.txt", "grep -L beta big.txt", "cat big.txt | grep -c beta", "cat big.txt | grep -n 'L59999'", "head -c 100000 big.txt | grep -c L", "head -c 98304 big.txt | grep -c L", "head -c 98305 big.txt | grep -c L", "grep -c '' big.txt", "grep -x 'L00001 alpha' big.txt", "grep -w 'beta' -c big.txt", "grep -F 'L12345' big.txt", "grep -i 'l12345' big.txt", "grep -E 'L1234[0-9]' -c big.txt", "grep -A1 -m2 'L0000[1-9]' big.txt", "sed '$d' big.txt | head -c 700000 | grep -c L")
fx("grep-fil-long-lines", "GFIL-008", "!awk 'BEGIN { printf \"start\\n\"; for (i = 0; i < 300000; i++) printf \"x\"; printf \" needle \"; for (i = 0; i < 200000; i++) printf \"y\"; printf \"\\nend needle\\n\" }' > longline.txt", "grep -n needle longline.txt | cut -c1-20", "grep -c needle longline.txt", "grep -o needle longline.txt", "grep -ob needle longline.txt", "grep -C1 start longline.txt | cut -c1-12", "grep -C1 end longline.txt | cut -c1-12", "grep -c 'x*' longline.txt", "grep -c 'needle$' longline.txt", "grep -c '^end' longline.txt", "grep -o 'x*needle' longline.txt | wc -c", "grep -c -w needle longline.txt", "grep -ow 'needle' longline.txt", "grep -v start longline.txt | cut -c1-12", "grep -m1 needle longline.txt | wc -c")

# -- binary
fx("grep-bin-nul", "GBIN-001", "grep alpha bin.dat", "grep alpha bin.dat a.txt", "grep -n alpha bin.dat a.txt", "grep -H alpha bin.dat", "grep ary bin.dat", "grep bin bin.dat", "grep second bin.dat", "grep zzz bin.dat; echo $?", "grep -v alpha bin.dat; echo $?", "grep -v zzz bin.dat", "grep . bin.dat", "grep -c alpha bin.dat", "grep -r alpha . 2>&1 | sort", "grep alpha bin.dat bin.dat", "cat bin.dat | grep alpha", "grep -s alpha bin.dat", "grep alpha bin.dat 2>/dev/null; echo $?", "grep -h alpha bin.dat a.txt")
fx("grep-bin-text", "GBIN-002", "grep -a alpha bin.dat | od -c", "grep -a -c alpha bin.dat", "grep -a -n . bin.dat | od -c", "grep -a -b . bin.dat | od -c", "grep --binary-files=text alpha bin.dat | od -c", "grep -a -o ary bin.dat", "grep -a 'bin.ary' bin.dat | od -c", "grep -a -z . bin.dat | od -c", "grep -a -v alpha bin.dat; echo $?", "grep -a alpha a.txt bin.dat | od -c", "grep -ao 'alpha' bin.dat", "grep -a -x 'second alpha' bin.dat", "grep -a -w ary bin.dat | od -c")
fx("grep-bin-without-match", "GBIN-003", "grep -I alpha bin.dat; echo $?", "grep -I -c alpha bin.dat", "grep -I -l alpha bin.dat; echo $?", "grep -I -L alpha bin.dat", "grep -I alpha bin.dat a.txt", "grep -I -v alpha a.txt bin.dat", "grep --binary-files=without-match alpha bin.dat; echo $?", "grep -I -r alpha . 2>&1 | sort", "grep -I -c alpha bin.dat a.txt", "grep -I -q alpha bin.dat; echo $?", "grep -I -s alpha bin.dat; echo $?", "grep -Ia alpha bin.dat | od -c", "grep -aI alpha bin.dat; echo $?")
fx("grep-bin-options", "GBIN-004", "grep -c alpha bin.dat", "grep -c '' bin.dat", "grep -l alpha bin.dat", "grep -L alpha bin.dat", "grep -L zzz bin.dat", "grep -q alpha bin.dat; echo $?", "grep -o alpha bin.dat", "grep -o -a alpha bin.dat", "grep -v alpha bin.dat; echo $?", "grep -m1 alpha bin.dat", "grep -m1 -c alpha bin.dat", "grep -A1 alpha bin.dat", "grep -n alpha bin.dat", "grep -b alpha bin.dat", "grep -l -m1 alpha bin.dat", "grep -L -m1 alpha bin.dat", "grep -c -m1 alpha bin.dat", "grep -l -m1 zzz bin.dat; echo $?", "grep -m1 -A1 alpha bin.dat", "grep -x 'second alpha' bin.dat", "grep -w ary bin.dat", "grep -F second bin.dat", "grep -E 'a|b' bin.dat", "grep -i ALPHA bin.dat", "grep -lc alpha bin.dat", "grep -Z -l alpha bin.dat | od -c", "grep -C1 alpha bin.dat", "grep --color=always alpha bin.dat")
fx("grep-bin-encoding-errors", "GBIN-005", "grep alpha bad.txt", "grep -c alpha bad.txt", "grep -n plain bad.txt", "grep -a alpha bad.txt | od -c", "grep -o alpha bad.txt", "grep -v zzz bad.txt", "grep -L alpha bad.txt", "grep -l alpha bad.txt", "grep -r alpha . 2>&1 | sort", "grep . utf8.txt", "grep -B1 plain bad.txt", "grep -A1 caf bad.txt", "grep -C1 plain bad.txt", "grep -A2 alpha bad.txt bad.txt", "grep -I alpha bad.txt", "grep -I -c alpha bad.txt", "grep -h caf bad.txt; echo $?", "grep -x 'plain alpha' bad.txt", "grep -m1 alpha bad.txt", "grep -b alpha bad.txt", "grep -w alpha bad.txt", "grep --color=always alpha bad.txt | od -c | sed -n '1,4p'", "grep -c . bad.txt utf8.txt", env=["LC_ALL=C.UTF-8"])
fx("grep-bin-encoding-c-locale", "GBIN-005", "grep alpha bad.txt | od -c", "grep -c alpha bad.txt", "grep -o 'caf.' bad.txt | od -c", "grep . bad.txt | od -c | sed -n '1,3p'")
fx("grep-bin-u-flag", "GBIN-006", "grep -U alpha a.txt", "grep -U alpha crlf.txt | od -c", "grep -U alpha bin.dat", "grep --binary alpha a.txt", "grep -U -c alpha crlf.txt", "grep -U -o 'a.' crlf.txt | od -c", "grep -U 'a$' crlf.txt; echo $?", "grep -U -c 'a.$' crlf.txt")
fx("grep-bin-null-data", "GBIN-007", "grep -z alpha z.dat | od -c", "grep -z -c alpha z.dat", "grep -z one z.dat | od -c", "grep -z three z.dat | od -c", "grep -zc '' z.dat", "grep -z alpha bin.dat | od -c", "grep -z -c . bin.dat", "grep -z -l alpha bin.dat", "grep -z -n alpha bin.dat | od -c", "grep -z -o alpha bin.dat | od -c", "grep -z -a -c alpha bin.dat", "grep -zv one z.dat | od -c", "grep -z -x one z.dat | od -c", "grep -z -m1 '' z.dat | od -c", "grep -zb two z.dat | od -c", "grep -z . a.txt | od -c", "grep -z 'beta.gamma' a.txt | od -c", "grep -z -c 'alpha' a.txt")
fx("grep-bin-large", "GBIN-008", "!" + BIGBIN, "grep -c alpha bigbin.dat", "grep -n alpha bigbin.dat | tail -n 3", "grep alpha bigbin.dat | wc -l", "grep -a -c alpha bigbin.dat", "grep -c L bigbin.dat", "grep 'L00001' bigbin.dat", "grep 'L99999' bigbin.dat; echo $?", "grep -m2 beta bigbin.dat", "grep -l alpha bigbin.dat", "grep -I alpha bigbin.dat | wc -l", "grep -I -c alpha bigbin.dat", "grep -L zzz bigbin.dat", "cat bigbin.dat | grep -c alpha", "grep 'L2[0-9]*' bigbin.dat | wc -l", "grep -A1 'L07561' bigbin.dat", "grep -v zzz bigbin.dat | wc -l", "grep -c beta bigbin.dat")
fx("grep-bin-chunk-edges", "GBIN-008", "!awk 'BEGIN { for (i = 1; i <= 20000; i++) printf \"L%05d alpha\\n\", i }' > t.txt; for n in 100 7000 7560 7561 7562 8000 15000; do awk -v n=$n 'NR == n { sub(/^L0/, \"L\\001\") } { print }' t.txt | tr '\\001' '\\000' > n$n.dat; done", "grep -c alpha n100.dat n7000.dat n7560.dat n7561.dat n7562.dat n8000.dat n15000.dat", "grep alpha n100.dat | wc -l", "grep alpha n7000.dat | wc -l", "grep alpha n7561.dat | wc -l", "grep alpha n8000.dat | wc -l", "grep alpha n15000.dat | wc -l")

# -- color
fx("grep-col-matches", "GCOL-001", "grep --color=always alpha a.txt", "grep --color=always -i ALPHA a.txt", "grep --color=always -E 'a|b' a.txt", "grep --color=always -E 'x*' a.txt", "grep --color=always '' a.txt", "grep --color=always -w foo words.txt", "grep --color=always -x alpha a.txt", "grep --color=always -F al a.txt", "grep --color=always 'a' a.txt", "grep --color=always -o alpha a.txt", "grep --color=always alpha nonl.txt", "grep --color=always alpha crlf.txt", "grep --color=always -e alpha -e beta a.txt", "grep --color=always '^a' a.txt", "grep --color=always 'a$' a.txt", "grep --color=always -v zzz a.txt", "grep --color=always -c alpha a.txt")
fx("grep-col-prefixes", "GCOL-002", "grep --color=always -n alpha a.txt", "grep --color=always -H alpha a.txt", "grep --color=always -b alpha a.txt", "grep --color=always -Hnb alpha a.txt", "grep --color=always -T -n alpha a.txt", "grep --color=always -c alpha a.txt b.txt", "grep --color=always -l alpha a.txt b.txt", "grep --color=always -L alpha a.txt b.txt", "grep --color=always -Z -l alpha a.txt", "grep --color=always -Z -H alpha a.txt", "grep --color=always -C1 --group-separator=XX hit ctx.txt", "grep --color=always -A1 hit ctx.txt", "grep --color=always -n -A1 hit ctx.txt a.txt", "grep --color=always -ob alpha a.txt", "grep --color=always -on alpha a.txt", "grep --color=always --no-group-separator -A1 hit ctx.txt")
fx("grep-col-context-and-invert", "GCOL-003", "grep --color=always -A1 beta a.txt", "grep --color=always -B1 beta a.txt", "grep --color=always -vn alpha a.txt", "grep --color=always -vC1 beta a.txt", "grep --color=always -v -n -A1 alpha a.txt", "grep --color=always -C1 gamma a.txt", "grep --color=always -v -o alpha a.txt; echo $?", "grep --color=always -o -A1 alpha a.txt", "grep --color=always -c -A1 alpha a.txt", "grep --color=always -vx alpha a.txt", "grep --color=always -m1 -A1 alpha a.txt")
fx("grep-col-grep-colors", "GCOL-004", "GREP_COLORS='mt=01;32' grep --color=always alpha a.txt", "GREP_COLORS='mt=01;32' grep --color=always -v alpha a.txt", "GREP_COLORS='ms=4:mc=5' grep --color=always -C1 gamma a.txt", "GREP_COLORS='ms=4:mc=5' grep --color=always -vC1 gamma a.txt", "GREP_COLORS='sl=1' grep --color=always alpha a.txt", "GREP_COLORS='cx=2' grep --color=always -A1 beta a.txt", "GREP_COLORS='sl=1:cx=2' grep --color=always -nA1 beta a.txt", "GREP_COLORS='sl=1:cx=2:rv' grep --color=always -nvA1 beta a.txt", "GREP_COLORS='fn=1:ln=2:bn=3:se=4' grep --color=always -Hnb alpha a.txt", "GREP_COLORS='ne' grep --color=always -Hn alpha a.txt", "GREP_COLORS='mt=' grep --color=always alpha a.txt", "GREP_COLORS='ms=' grep --color=always alpha a.txt", "GREP_COLORS='ms=1;;2' grep --color=always alpha a.txt", "GREP_COLORS='ms=x' grep --color=always alpha a.txt", "GREP_COLORS='foo=1:ms=4' grep --color=always alpha a.txt", "GREP_COLORS='ms=4:foo=1' grep --color=always alpha a.txt", "GREP_COLORS='ms=4:ln=' grep --color=always -n alpha a.txt", "GREP_COLORS=':ms=4' grep --color=always alpha a.txt", "GREP_COLORS='ms=4:' grep --color=always alpha a.txt", "GREP_COLORS='sl=1' grep --color=always alpha crlf.txt | od -c", "GREP_COLORS='sl=7:ms=1' grep --color=always -o alpha a.txt", "GREP_COLORS='ne:ms=4' grep --color=always -c alpha a.txt b.txt", "GREP_COLORS='sl=1:cx=2' grep --color=always -v -A1 alpha a.txt")
fx("grep-col-grep-color-obsolete", "GCOL-005", "GREP_COLOR='4' grep --color=always alpha a.txt", "GREP_COLOR='4' GREP_COLORS='ms=5' grep --color=always alpha a.txt", "GREP_COLOR='4' GREP_COLORS='mt=5' grep --color=always alpha a.txt", "GREP_COLOR= grep --color=always alpha a.txt", "GREP_COLOR='1;32' grep --color=always -v alpha a.txt")
fx("grep-col-when", "GCOL-006", "grep --color=auto alpha a.txt", "grep --colour=never alpha a.txt", "grep --color=tty alpha a.txt", "grep --color=if-tty alpha a.txt", "grep --color=yes alpha a.txt", "grep --color=force alpha a.txt", "grep --color=no alpha a.txt", "grep --color=none alpha a.txt", "grep --color alpha a.txt", "grep --colour alpha a.txt", "grep --color=always --color=never alpha a.txt", "grep --color=never --color=always alpha a.txt", "grep --color=foo alpha a.txt | sed -n '1,2p'; echo $?", "grep --colo=always alpha a.txt")

# -- perl
fx("pcre-basic", "GPCRE-001", "grep -P 'al.ha' a.txt", "grep -P '\\d' words.txt ctx.txt | sed -n '1,3p'", "grep -oP '\\w+' words.txt", "grep -cP '^$' a.txt", "grep -P '(a)\\1' words.txt", "grep -P '(\\w)\\1' words.txt", "grep -P 'foo(?!bar)' -o words.txt", "grep -P '\\bfoo\\b' words.txt", "grep -P 'a|b' -c a.txt", "grep -P '[[:alpha:]]+' -o a.txt", "grep -P 'a{2}' -c words.txt", "grep -P '^\\s*$' -c words.txt empty", "grep -P '\\t' -c a.txt", "grep -P 'beta' a.txt b.txt", "grep -Pn alpha a.txt", "grep -Pv alpha a.txt", "grep -Pc alpha a.txt b.txt", "grep -Pl alpha a.txt b.txt", "grep -Pq alpha a.txt; echo $?")
fx("pcre-lookaround-and-keep", "GPCRE-002", "grep -oP 'a\\Kl' a.txt", "grep -oP '(?<=alpha )beta' a.txt", "grep -oP '(?=f)' words.txt; echo $?", "grep -oP 'o*' words.txt", "grep -obP 'o' words.txt", "grep -oP '(?<=foo)bar' words.txt", "grep -oP 'foo\\Kbar' words.txt", "grep -oP '\\w+(?= beta)' a.txt", "grep -oP '(?i)ALPHA' a.txt", "grep -oP 'a(?=l)' a.txt", "grep -oP '\\bb\\w*' a.txt", "grep -oP '[a-c]+' a.txt", "grep -oP '.\\K.' b.txt", "grep -oP '^\\w' a.txt")
fx("pcre-errors", "GPCRE-003", "grep -P '(' a.txt", "grep -P '[a' a.txt", "grep -P '*a' a.txt", "grep -P 'a{2,1}' a.txt", "grep -P '\\' a.txt", "grep -P -e a -e b a.txt", "grep -P 'a\nb' a.txt", "grep -P -f pats.txt a.txt", "grep -P '' a.txt", "grep -P -c '' a.txt", "grep -P -f /dev/null a.txt; echo $?", "grep -P '\\p{Foo}' a.txt", "grep -P '(?P<n>a)(?P=n)' a.txt; echo $?", "grep -P '\\1' a.txt", "grep -P 'a)' a.txt")
fx("pcre-with-other-options", "GPCRE-004", "grep -iP ALPHA a.txt", "grep -wP foo words.txt", "grep -xP foo words.txt", "grep -xP 'foo|foo bar' words.txt", "grep -wP 'fo*' words.txt", "grep -P -w 'foo|foobar' -o words.txt", "grep -vP foo words.txt", "grep -cP alpha a.txt", "grep -nP -A1 alpha a.txt", "grep -P -C1 beta a.txt", "grep -zP '^beta' a.txt; echo $?", "grep -zoP 'alpha.*' a.txt | od -c", "grep -zP 'a$' a.txt; echo $?", "grep -P -m1 alpha a.txt", "grep -P -H alpha a.txt", "grep -P --color=always alpha a.txt", "grep -P --color=always -o 'a.' a.txt", "grep -P -x -o 'alpha' a.txt", "grep -P -b -o alpha a.txt", "grep -P -r alpha d 2>&1 | sort", "grep -P -L alpha a.txt b.txt")
fx("pcre-utf8-locale", "GPCRE-005", "grep -oP '\\w+' utf8.txt", "grep -oP '[[:alpha:]]+' utf8.txt", "grep -oP '.' utf8.txt", "grep -iP 'CAFÉ' utf8.txt", "grep -oP '\\p{L}+' utf8.txt", "grep -cP '^.{3}$' utf8.txt", "grep -P 'あ' utf8.txt", "grep -oP '[あ-う]+' utf8.txt", "grep -oP '(?<=na)ï' utf8.txt", "grep -wP 'naïve' utf8.txt", "grep -P alpha bad.txt", "grep -c alpha bad.txt -P", "grep -P -a alpha bad.txt | od -c", "grep -P -o 'caf.' bad.txt | od -c", env=["LC_ALL=C.UTF-8"])
DEEP = r"""awk 'BEGIN { s = sprintf("%60s", ""); gsub(/ /, "a", s); print s "!" }' > deep.txt"""
fx("pcre-limit", "GPCRE-006", "!" + DEEP, "grep -P '^(a|aa)+$' deep.txt; echo $?", "grep -cP '^(a|aa)+$' a.txt deep.txt b.txt; echo $?", "grep -P 'al|^(a|aa)+$' a.txt deep.txt b.txt; echo $?", "grep -oP '(a|aa)+$' deep.txt; echo $?", "grep -qP '^(a|aa)+$' deep.txt; echo $?", "grep -qP alpha a.txt deep.txt; echo $?", "grep -sP '^(a|aa)+$' deep.txt; echo $?", "grep -lP '^(a|aa)+$' a.txt deep.txt b.txt; echo $?", "grep -vP '^(a|aa)+$' deep.txt a.txt; echo $?")
fx("pcre-c-locale", "GPCRE-005", "grep -oP '\\w+' utf8.txt | od -c | sed -n '1,3p'", "grep -oP '.' utf8.txt | od -c | sed -n '1,3p'", "grep -iP 'CAFÉ' utf8.txt; echo $?", "grep -cP '^.{3}$' utf8.txt", "grep -P 'caf' utf8.txt", "grep -P alpha bad.txt | od -c", "grep -oP '[^a-z]' utf8.txt | od -c | sed -n '1,2p'")


# ---- writing -------------------------------------------------------------------------------------
def shquote(s):
    return "'" + s.replace("'", "'\\''") + "'"

def write_fixture(name, items, cmds, env, files):
    d = os.path.join(fixtures, name)
    if os.path.isdir(d):
        # keep expected/ (it is regenerated by tools/gen-fixtures.sh) but start the rest again
        for entry in os.listdir(d):
            path = os.path.join(d, entry)
            if entry != "expected":
                shutil.rmtree(path) if os.path.isdir(path) else os.remove(path)
    os.makedirs(d, exist_ok=True)
    for rel, data in STD.items():
        full = os.path.join(d, "files", rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as f:
            f.write(data)
    lines = ["# Written by tools/gen-grep-fixtures.py: each command is shown, run, and followed by its status.", ""]
    for c in cmds:
        if c.startswith("!"):
            lines.append(c[1:])
            continue
        lines.append("printf '%s\\n' " + shquote("=== " + c))
        lines.append(c)
        lines.append('echo "[$?]"')
    with open(os.path.join(d, "cmd"), "w") as f:
        f.write("\n".join(lines) + "\n")
    with open(os.path.join(d, "covers"), "w") as f:
        f.write(" ".join(items) + "\n")
    if env:
        with open(os.path.join(d, "env"), "w") as f:
            f.write("\n".join(env) + "\n")
    elif os.path.exists(os.path.join(d, "env")):
        os.remove(os.path.join(d, "env"))

def main():
    known = {i[0] for i in ITEMS}
    names = set()
    for name, items, cmds, env, files in F:
        assert name not in names, name
        names.add(name)
        for i in items:
            assert i in known, (name, i)
        write_fixture(name, items, cmds, env, files)
    # fixtures that are no longer in the list
    for entry in sorted(os.listdir(fixtures)):
        if (entry.startswith("grep-") or entry.startswith("pcre-")) and entry not in names:
            shutil.rmtree(os.path.join(fixtures, entry))
            print("removed", entry)
    # the matrix: keep the statuses that are there
    status = {}
    path = os.path.join(root, "compat", "grep.tsv")
    if os.path.exists(path):
        for line in open(path).read().splitlines()[1:]:
            cols = line.split("\t")
            if len(cols) == 6:
                status[cols[0]] = cols[3]
    with open(path, "w") as f:
        f.write("id\tarea\titem\tstatus\tsource\tnotes\n")
        for id_, area, item, source, notes in ITEMS:
            f.write("\t".join([id_, area, item, status.get(id_, "unsupported"), source, notes]) + "\n")
    print("%d fixtures, %d items" % (len(F), len(ITEMS)))

if __name__ == "__main__":
    main()

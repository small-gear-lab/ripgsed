#!/usr/bin/env python3
"""Compare ripgsed with GNU sed 4.9 in locales on every character of the locale's character set.

    tools/locale-diff.py                       # the default locales and cases
    tools/locale-diff.py --locales en_US.UTF-8,tr_TR.UTF-8
    tools/locale-diff.py --locales ja_JP.eucJP,ja_JP.SJIS,zh_CN.GBK,zh_TW.BIG5,ko_KR.eucKR
    tools/locale-diff.py --locales de_DE.ISO-8859-1,ru_RU.KOI8-R
    tools/locale-diff.py --ours target/debug/ripgsed --quick

For a UTF-8 locale the input is one line per code point (0 to 0x10FFFF, without the surrogates
and the newline), so each case below is decided for 1,112,063 characters. For a single-byte
character set it is one line per byte value (255 lines). For a multibyte character set it is
every byte, every two-byte sequence that starts with a byte of 0x80 or more, and (EUC-JP) every
three-byte sequence of code set 3: valid characters and invalid or incomplete sequences alike.
A line the reference reads as one character (`/^.$/`) counts as a valid character; any other
line of two bytes or more counts as an invalid sequence. A case is a sed script run by both
programs on that input; the answers are compared line by line (for `=` scripts: as the sets of
line numbers). Error messages and exit statuses are compared as well.

Locales that the machine does not have are made with `localedef` in a scratch directory and
selected with LOCPATH (`--make`, or whenever a locale is missing): this needs the definitions
under /usr/share/i18n (the Debian/Ubuntu package `locales`). The character sets are those of
LOCALE_DEFS below; the text of a pattern is written in the character set of the locale.

Both programs run as `sed` (GNU sed prints argv[0] in its messages) with a clean environment.
Exit status: 0 when every case matches, 1 otherwise.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

DEFAULT_LOCALES = ["C.UTF-8", "en_US.UTF-8", "de_DE.UTF-8", "sv_SE.UTF-8", "cs_CZ.UTF-8", "lt_LT.UTF-8", "tr_TR.UTF-8"]

# Locales of other character sets: name -> (charmap, definition, Python codec, bytes of a character).
# The names are made by `localedef -f <charmap> -i <definition>` under LOCPATH.
LOCALE_DEFS = {
    "ja_JP.eucJP": ("EUC-JP", "ja_JP", "euc_jp", 3),
    "ja_JP.SJIS": ("SHIFT_JIS", "ja_JP", "shift_jis", 2),
    "zh_CN.GBK": ("GBK", "zh_CN", "gbk", 2),
    "zh_TW.BIG5": ("BIG5", "zh_TW", "big5", 2),
    "ko_KR.eucKR": ("EUC-KR", "ko_KR", "euc_kr", 2),
    "zh_CN.GB18030": ("GB18030", "zh_CN", "gb18030", 4),
    # Python has no EUC-TW codec: only what ASCII can write is encoded for the patterns.
    "zh_TW.eucTW": ("EUC-TW", "zh_TW", "ascii", 4),
    "de_DE.ISO-8859-1": ("ISO-8859-1", "de_DE", "latin-1", 1),
    "cs_CZ.ISO-8859-2": ("ISO-8859-2", "cs_CZ", "iso8859-2", 1),
    "fr_FR.ISO-8859-15": ("ISO-8859-15", "fr_FR", "iso8859-15", 1),
    "tr_TR.ISO-8859-9": ("ISO-8859-9", "tr_TR", "iso8859-9", 1),
    "ru_RU.KOI8-R": ("KOI8-R", "ru_RU", "koi8-r", 1),
    "ru_RU.CP1251": ("CP1251", "ru_RU", "cp1251", 1),
    "de_DE.CP1252": ("CP1252", "de_DE", "cp1252", 1),
}
NON_UTF8_LOCALES = ",".join(LOCALE_DEFS)

# Bracket expressions checked as `/^PAT$/=`, without and with the `I` flag.
BRACKETS = [
    "[a-z]", "[A-Z]", "[a-c]", "[n-o]", "[0-9]", "[!-/]", "[ -~]", "[α-ω]", "[а-я]", "[ぁ-ん]",
    "[^a-z]", "[[:alpha:]]", "[[:upper:]]", "[[:lower:]]", "[[:punct:]]", "[[:space:]]",
    "[[:alnum:]]", "[[:digit:]]", "[[:print:]]", "[[=a=]]", "[[=e=]]", "[[.a.]]", "[[.-.]]",
    "[a-cx-z]", "[[:digit:]a-f]",
    # One letter, read under I (the capitals and small forms the two matchers of GNU sed agree on).
    "[µ]", "[в]", "[В]", "[ᲀ]", "[ſ]", "[ς]", "[ǆ]", "[ß]", "[ı]", "[i]", "[İ]", "[k]", "[K]",
    # Ranges between ASCII digits are read by code point (see the notes in docs/ARCHITECTURE.md).
    "[0-9_]", "[[:digit:]0-9]", "[0-9a]", "[0-9a-a]", "[-0-9]", "[0-95-7]", "[0-9[:alpha:]]", "[0-2a-c]", "[^0-9]", "[1-3]", "[7-9]",
]
# More bracket expressions for the other character sets; those the character set cannot write
# (a Greek range in ISO-8859-1) are left out.
OTHER_BRACKETS = [
    "[ぁ-ん]", "[ァ-ヶ]", "[一-龥]", "[亜-熙]", "[가-힣]", "[ㄱ-ㅎ]", "[а-я]", "[А-Я]", "[ё]", "[Ё]", "[à-ÿ]", "[À-Ö]",
    "[é]", "[É]", "[è-ë]", "[ä]", "[Ä]", "[ö]", "[ß]", "[ÿ]", "[ž]", "[Ž]", "[š]", "[ł]", "[Ł]", "[ő]", "[ı]", "[İ]", "[ğ]",
    "[€]", "[Œ]", "[œ]", "[ａ-ｚ]", "[０-９]", "[ａ]", "[あ]", "[ア]", "[漢]", "[한]", "[〇]", "[‘-”]", "[α-ω]", "[Α-Ω]",
    "[[=e=]é]", "[[.é.]]", "[^é]", "[^あ]", "[^а]",
]
SHORTHANDS = ["\\w", "\\W", "\\s", "\\S"]
LITERALS = ["i", "I", "s", "S", "k", "K", "a", "z", "ß", "ǆ", "ǅ", "ς", "σ", "İ", "ı", "é", "в", "ᲀ", "ω", "Ω", "ᾳ", "ａ", "ⱥ", "ɐ"]
OTHER_LITERALS = ["é", "É", "è", "ä", "Ä", "ö", "ß", "ÿ", "ž", "ł", "Ł", "ő", "ğ", "ı", "İ", "с", "С", "я", "Я", "あ", "ア", "ａ", "Ａ", "漢", "한", "Œ", "œ", "µ", "€"]
# Differences that are known and listed in docs/KNOWN-DIFFERENCES.md: reported, but not a failure.
# A back-reference under `I` after a character whose upper-case form is longer in UTF-8 (`ɐɐ`).
KNOWN = {"/^\\(.\\)\\1$/I=": "back-reference under I after a letter whose capital is longer"}
# The same for `C.UTF-8` only, where ripgsed folds a letter under `I` with its own rule
# (docs/KNOWN-DIFFERENCES.md, "With the `I` flag, glibc matches ... in one direction only").
KNOWN_C_UTF8 = {"/^[µ]$/I=", "/^[в]$/I=", "/^[В]$/I=", "/^[ᲀ]$/I=", "/^[ς]$/I=", "/^в$/I="}
# Known differences of one locale (docs/KNOWN-DIFFERENCES.md): the label of a case in a locale.
KNOWN_LOCALE = {
    # Eight Big5 codes (0xf9e9..0xf9eb, 0xf9f9..0xf9fd) are one character for `mbrtowc`, but the
    # matcher of the library reads their first byte as a character of its own, which a range
    # by collation takes.
    ("zh_TW.BIG5", "s/[[.a.]-z]/X/"): "eight Big5 codes read as two bytes by the library's matcher",
    ("zh_CN.GB18030", "/^[^0-9]$/I="): "GNU sed's answer for the GB18030 letters 0xa8a1..0xa8bf under I varies from run to run",
    ("zh_TW.eucTW", "s/[[.a.]-z]/X/"): "EUC-TW characters of 0x8e read as a lone byte by the library's matcher",
}
# Scripts whose whole output is compared, line by line.
CONVERSIONS = ["s/.*/\\U&/", "s/.*/\\L&/", "s/.*/\\u&/", "s/.*/\\l&/", "s/\\b/|/g", "s/\\B/|/g"]
# Scripts run on the characters inside a word (`a<char>b`) or after a digit: where the matches
# are, and the word boundaries around the character.
CONTEXT = ["s/\\b/|/g", "s/\\B/|/g", "s/\\w*/[&]/g", "s/./X/g", "s/x*/-/g", "s/[[:alpha:]]*/[&]/g", "s/[^a]*/<&>/g",
           "s/\\(.\\)\\(.\\)/\\2\\1/g", "s/.*/\\U&/", "s/\\<./[&]/g", "s/.\\>/[&]/g", "s/b\\|./X/g", "y/ab/AB/", "s/.\\{2\\}/X/g"]
# Patterns that GNU sed refuses (or accepts) depending on the locale: the messages and the
# status are compared.
ERRORS = ["s/[b-a]/X/", "s/[é-z]/X/", "s/[a-é]/X/", "s/[[.space.]]/X/", "s/[[:foo:]]/X/", "s/[[=ab=]]/X/", "s/[[.a.]-z]/X/", "s/[[:alpha:]-z]/X/", "s/[z-a]/X/I"]


def write_inputs(path, twice_path, pair_path, ctx_path, limit):
    """UTF-8: one line per code point; every code point twice in a line; every code point after a
    `3`; every code point between `a` and `b`."""
    cps = [c for c in range(0, limit) if c != 10 and not 0xD800 <= c < 0xE000]
    return write_lines(path, twice_path, pair_path, ctx_path, [chr(c).encode("utf-8") for c in cps])


def write_lines(path, twice_path, pair_path, ctx_path, chars):
    with open(path, "wb") as f:
        f.write(b"".join(c + b"\n" for c in chars))
    with open(twice_path, "wb") as f:
        f.write(b"".join(c * 2 + b"\n" for c in chars))
    with open(pair_path, "wb") as f:
        f.write(b"".join(b"3" + c + b"\n" for c in chars))
    with open(ctx_path, "wb") as f:
        f.write(b"".join(b"a" + c + b"b\n" for c in chars))
    return chars


SAMPLE_LEADS = [0x80, 0x81, 0x83, 0x8E, 0x8F, 0x95, 0xA1, 0xA3, 0xA4, 0xA5, 0xB0, 0xC7, 0xD6, 0xE0, 0xFE, 0xFF, 0x84, 0x98, 0xA8]


def charset_inputs(maxlen, sample=False):
    """Every byte, every two-byte sequence with a first byte of 0x80 or more, and for three-byte
    sets every sequence of code set 3 (0x8f, then two bytes of 0xa1 or more), without newlines."""
    seqs = [bytes([b]) for b in range(256) if b != 10]
    leads = SAMPLE_LEADS if sample else range(0x80, 256)
    if maxlen >= 2:
        seqs += [bytes([a, b]) for a in leads for b in range(256) if b != 10]
    if maxlen >= 3:
        seqs += [bytes([0x8F, a, b]) for a in (range(0xA1, 0xA4) if sample else range(0xA1, 0xFF)) for b in range(0xA1, 0xFF)]
    if maxlen >= 4:
        # GB18030: four bytes 0x81..0xfe, 0x30..0x39, 0x81..0xfe, 0x30..0x39; EUC-TW: 0x8e, a plane
        # 0xa1..0xb0 (0xa2.. for the unused ones), then two bytes of 0xa1 or more.
        for a in ([0x81, 0x84, 0x90, 0xE3, 0xFE] if sample else range(0x81, 0xFF)):
            for b in (range(0x30, 0x3A) if not sample else (0x30, 0x35, 0x39)):
                for c in ([0x81, 0xA1, 0xFE] if sample else range(0x81, 0xFF)):
                    for d in ((0x30, 0x39) if sample else range(0x30, 0x3A)):
                        seqs.append(bytes([a, b, c, d]))
        for plane in range(0xA1, 0xB0) if not sample else (0xA1, 0xA2, 0xA3, 0xAF, 0xB0):
            for b in ((0xA1, 0xC4, 0xFE) if sample else range(0xA1, 0xFF)):
                for c in range(0xA1, 0xFF) if not sample else (0xA1, 0xFE):
                    seqs.append(bytes([0x8E, plane, b, c]))
    return seqs


def make_locales(names, locdir):
    """Makes the locales that LC_ALL=<name> cannot select; returns True when LOCPATH is needed."""
    needed = False
    for name in names:
        if name in ("C", "POSIX", "C.UTF-8", "C.utf8"):
            continue
        base = name.split(".")[0]
        charmap = "UTF-8"
        if name in LOCALE_DEFS:
            charmap, base = LOCALE_DEFS[name][0], LOCALE_DEFS[name][1]
        else:
            have = subprocess.run(["locale", "charmap"], env={"PATH": "/usr/bin:/bin", "LC_ALL": name}, capture_output=True, text=True).stdout.strip()
            if have == "UTF-8":
                continue
        os.makedirs(locdir, exist_ok=True)
        target = os.path.join(locdir, name)
        if not os.path.isdir(target):
            r = subprocess.run(["localedef", "-f", charmap, "-i", base, target], capture_output=True, text=True)
            if not os.path.isdir(target):
                print(f"cannot make {name}: {r.stderr.strip()}", file=sys.stderr)
                continue
        needed = True
    return needed


def run(prog_dir, locale, locpath, args, input_path):
    # LANGUAGE=C: the messages of the C library are not translated, whatever catalogs are installed.
    env = {"PATH": prog_dir + ":/usr/bin:/bin", "LC_ALL": locale, "LANGUAGE": "C"}
    if locpath:
        env["LOCPATH"] = locpath
    r = subprocess.run(["sed"] + args, env=env, stdin=open(input_path, "rb"), capture_output=True)
    return r.returncode, r.stdout, r.stderr


def aborting_lines(prog_dir, locale, locpath, args, lines):
    """The lines on which the reference dies from a signal (GNU sed aborts when a case conversion
    gives a character the character set cannot write): by bisection."""
    def dies(chunk):
        r = subprocess.run(["sed"] + args, env={"PATH": prog_dir + ":/usr/bin:/bin", "LC_ALL": locale, "LANGUAGE": "C", **({"LOCPATH": locpath} if locpath else {})},
                           input=b"".join(l + b"\n" for l in chunk), capture_output=True)
        return r.returncode < 0
    def find(lo, hi):
        if lo >= hi or not dies(lines[lo:hi]):
            return []
        if hi - lo == 1:
            return [lo]
        mid = (lo + hi) // 2
        return find(lo, mid) + find(mid, hi)
    return find(0, len(lines))


def encode_all(texts, codec):
    """The texts written in the character set, without those it cannot write."""
    out = []
    for t in texts:
        try:
            t.encode(codec)
        except UnicodeEncodeError:
            continue
        out.append(t)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ours", default=os.path.join(os.path.dirname(__file__), "..", "target", "release", "ripgsed"))
    ap.add_argument("--locales", default=",".join(DEFAULT_LOCALES))
    ap.add_argument("--loc-dir", default=None, help="where `localedef` makes missing locales (default: a scratch directory)")
    ap.add_argument("--max-cp", type=lambda v: int(v, 0), default=0x110000, help="only the code points below this one")
    ap.add_argument("--quick", action="store_true", help="fewer cases")
    ap.add_argument("--sample", action="store_true", help="multibyte character sets: only some first bytes (and code set 3 rows)")
    ap.add_argument("--filter", default=None, help="only the cases whose label contains this text")
    ap.add_argument("--verbose", action="store_true", help="also list the matching cases")
    ap.add_argument("--glibc-compat", action="store_true", help="run ours with --glibc-compat (the glibc quirks are reproduced)")
    ap.add_argument("--summary", action="store_true", help="per locale: the number of cases, characters and differences")
    args = ap.parse_args()

    ours = os.path.abspath(args.ours)
    if not os.path.exists(ours):
        sys.exit(f"{ours} does not exist: run cargo build --release")
    work = tempfile.mkdtemp(prefix="locale-diff-")
    try:
        # `sed` that is ripgsed, in its own directory (the reference is /usr/bin/sed by path).
        ours_dir = os.path.join(work, "ours")
        os.makedirs(ours_dir)
        os.symlink(ours, os.path.join(ours_dir, "sed"))
        gnu_dir = os.path.join(work, "gnu")
        os.makedirs(gnu_dir)
        os.symlink(shutil.which("sed", path="/usr/bin:/bin"), os.path.join(gnu_dir, "sed"))
        version = subprocess.run([os.path.join(gnu_dir, "sed"), "--version"], capture_output=True, text=True).stdout.splitlines()[0]
        version = version.rsplit("/", 1)[-1] if "/" in version.split(" ")[0] else version
        version = version[version.index("sed (GNU") :] if "sed (GNU" in version else version
        if version != "sed (GNU sed) 4.9":
            print(f"warning: the reference is `{version}`, not GNU sed 4.9", file=sys.stderr)
        paths = {k: os.path.join(work, k + ".txt") for k in ("all", "twice", "pair", "ctx")}
        utf8_chars = None

        names = args.locales.split(",")
        locdir = args.loc_dir or os.path.join(work, "loc")
        locpath = locdir if make_locales(names, locdir) else os.environ.get("LOCPATH")

        failed = 0
        print(f"{'locale':18} {'case':28} {'characters':>10} {'differ':>7} {'match %':>8}", flush=True)
        for loc in names:
            codec = None
            if loc in LOCALE_DEFS:
                codec, maxlen = LOCALE_DEFS[loc][2], LOCALE_DEFS[loc][3]
                chars = write_lines(paths["all"], paths["twice"], paths["pair"], paths["ctx"], charset_inputs(maxlen, args.sample))
                enc = lambda t: t.encode(codec)
            else:
                if utf8_chars is None:
                    utf8_chars = write_inputs(paths["all"], paths["twice"], paths["pair"], paths["ctx"], args.max_cp)
                chars = utf8_chars
                enc = lambda t: t.encode("utf-8")
            single_byte = codec is not None and LOCALE_DEFS[loc][3] == 1
            multibyte = codec is not None and not single_byte
            cases = []  # (label, argv, kind)
            brackets = BRACKETS[:8] if args.quick else BRACKETS
            literals = LITERALS[:4] if args.quick else LITERALS
            if codec:
                brackets = encode_all(brackets + (OTHER_BRACKETS[:12] if args.quick else OTHER_BRACKETS), codec)
                literals = encode_all(literals + OTHER_LITERALS, codec)
            for b in brackets:
                for flag in ("", "I"):
                    cases.append((f"/^{b}$/{flag}=", ["-n", f"/^{b}$/{flag}="], "set"))
            for s in SHORTHANDS:
                cases.append((f"/^{s}$/=", ["-n", f"/^{s}$/="], "set"))
            for lit in literals:
                cases.append((f"/^{lit}$/I=", ["-n", f"/^{lit}$/I="], "set"))
            if codec:
                cases.append(("/^.$/=", ["-n", "/^.$/="], "set"))
                cases.append(("/^..$/=", ["-n", "/^..$/="], "set"))
                cases.append(("/^.*$/=", ["-n", "/^.*$/="], "set"))
                cases.append(("/./=", ["-n", "/./="], "set"))
                cases.append(("/^[^a]$/=", ["-n", "/^[^a]$/="], "set"))
                cases.append(("/^[^a]*$/=", ["-n", "/^[^a]*$/="], "set"))
                cases.append(("/^\\W$/=", ["-n", "/^\\W$/="], "set"))
                cases.append(("/^\\w*$/=", ["-n", "/^\\w*$/="], "set"))
                cases.append(("/\\xa1/=", ["-n", "/\\xa1/="], "set"))
                cases.append(("/\\x41/=", ["-n", "/\\x41/="], "set"))
                cases.append(("/a/=", ["-n", "/a/="], "set"))
            for c in CONVERSIONS + (["s/./X/g", "s/x*/-/g", "s/./[&]/g", "s/\\(.\\)/\\1\\1/", "s/.*/&&/", "s/$/!/", "s/^/!/", "s/[[:alpha:]]/X/g", "s/[[:upper:]]/X/g", "s/[[:lower:]]/X/g", "s/[^[:alpha:]]/X/g"] if codec else []):
                cases.append((c, [c], "lines"))
            cases.append(("/^\\(.\\)\\1$/I=", ["-n", "/^\\(.\\)\\1$/I="], "set2"))
            if codec:
                cases.append(("/^\\(.\\)\\1$/=", ["-n", "/^\\(.\\)\\1$/="], "set2"))
                cases.append(("/^.\\{2\\}$/=", ["-n", "/^.\\{2\\}$/="], "set2"))
                cases.append(("/^[[:alpha:]]*$/=", ["-n", "/^[[:alpha:]]*$/="], "set2"))
                cases.append(("s/./X/g (twice)", ["s/./X/g"], "lines2"))
                cases.append(("y/ab/AB/ (twice)", ["y/ab/AB/"], "lines2"))
            for c in CONTEXT:
                cases.append((c + " (a_b)", [c], "linesctx"))
            # Where the matches are (the second matcher of GNU sed, collation) once a line has passed
            # the first (code points for digit ranges): `3٣` has two digits for `s/[0-9]/N/g`.
            for c in ["s/[0-9]/N/g", "s/[0-9a]/N/g", "s/[1-5]/N/g", "s/[^0-9]/N/g", "s/[0-9a-c]/N/g", "s/[0-9]\\+/N/g"]:
                cases.append((c + " (3x)", [c], "lines3"))
            for e in encode_all(ERRORS, codec) if codec else ERRORS:
                cases.append((e, [e], "error"))
            if args.filter:
                cases = [c for c in cases if args.filter in c[0]]

            totals = {"cases": 0, "bad_valid": 0, "bad_invalid": 0, "bad_cases": 0}
            # Which of the input lines the reference reads as one character: all of them in a
            # single-byte or UTF-8 locale.
            flags = [True] * len(chars)
            if multibyte:
                rc, out, _ = run(gnu_dir, loc, locpath, ["-n", "/^.$/="], paths["all"])
                flags = [False] * len(chars)
                for n in out.split():
                    flags[int(n) - 1] = True
            n_valid = sum(flags)
            for label, argv, kind in cases:
                src = paths["twice" if kind in ("set2", "lines2") else "pair" if kind == "lines3" else "ctx" if kind == "linesctx" else "all"]
                argv_b = [a.encode(codec) for a in argv] if codec else argv
                rg, og, eg = run(gnu_dir, loc, locpath, argv_b, src)
                kept = None
                if rg < 0 and kind != "error":
                    # The reference aborts on some lines: compare on the others.
                    lines = open(src, "rb").read().split(b"\n")[:-1]
                    dead = set(aborting_lines(gnu_dir, loc, locpath, argv_b, lines))
                    kept = [i for i in range(len(lines)) if i not in dead]
                    # With --glibc-compat ours aborts on those lines the same way.
                    for i in sorted(dead):
                        one = os.path.join(work, "one.txt")
                        with open(one, "wb") as f:
                            f.write(lines[i] + b"\n")
                        g1 = run(gnu_dir, loc, locpath, argv_b, one)
                        o1 = run(ours_dir, loc, locpath, ["--glibc-compat"] + argv_b, one)
                        if (g1[0], g1[2]) != (o1[0], o1[2]):
                            failed += 1
                            print(f"  --glibc-compat does not abort like GNU sed on {lines[i]!r}: {g1[0]} {g1[2]!r} / {o1[0]} {o1[2]!r}", flush=True)
                    src = os.path.join(work, "kept.txt")
                    with open(src, "wb") as f:
                        f.write(b"".join(lines[i] + b"\n" for i in kept))
                    rg, og, eg = run(gnu_dir, loc, locpath, argv_b, src)
                ro, oo, eo = run(ours_dir, loc, locpath, (["--glibc-compat"] if args.glibc_compat else []) + argv_b, src)
                total = len(chars) if kept is None else len(kept)
                diff = []  # the indexes of the input lines that differ
                if kind == "error":
                    total = 1
                    diff = [0] if (rg, og, eg) != (ro, oo, eo) else []
                elif (rg, eg) != (ro, eo):
                    diff = list(range(total))
                elif kind in ("set", "set2"):
                    a, b = set(og.split()), set(oo.split())
                    diff = sorted(int(n) - 1 for n in a ^ b)
                else:
                    la, lb = og.split(b"\n"), oo.split(b"\n")
                    diff = [i for i, (x, y) in enumerate(zip(la, lb)) if x != y]
                    diff += [min(len(la), len(lb)) + k for k in range(abs(len(la) - len(lb)))]
                if kept is not None:
                    diff = [kept[i] if i < len(kept) else i for i in diff]
                bad = len(diff)
                bad_valid = sum(1 for i in diff if i < len(flags) and flags[i]) if kind != "error" else bad
                bad_invalid = bad - bad_valid
                totals["cases"] += 1
                totals["bad_valid"] += bad_valid
                totals["bad_invalid"] += bad_invalid
                known = bad != 0 and (label in KNOWN or (loc == "C.UTF-8" and label in KNOWN_C_UTF8))
                why = KNOWN.get(label, "one-direction letter folding of C.UTF-8")
                if bad and loc == "zh_CN.GB18030" and label.startswith("/^[^") and label.endswith("$/I="):
                    known, why = True, KNOWN_LOCALE[("zh_CN.GB18030", "/^[^0-9]$/I=")]
                elif bad and (loc, label) in KNOWN_LOCALE:
                    known, why = True, KNOWN_LOCALE[(loc, label)]
                if bad and codec and bad_valid == 0 and label.startswith("/^[^") and label.endswith("$/I="):
                    # Unassigned two-byte codes (GBK 0xfe61..0xfe6f, Shift_JIS 0x8461..0x846f): the
                    # library's case folding takes them for a negated bracket under I.
                    known = True
                    why = "negated bracket under I on unassigned codes of the set"
                failed += bad != 0 and not known
                totals["bad_cases"] += bad != 0 and not known
                if bad or args.verbose:
                    note = "  (known: " + why + ")" if known else ""
                    if kept is not None:
                        note += f"  ({len(chars) - len(kept)} lines on which GNU sed aborts are left out)"
                    detail = f"   ({bad_valid} valid / {bad_invalid} invalid)" if multibyte else ""
                    print(f"{loc:18} {label:28} {total:10d} {bad:7d} {100.0 * (total - bad) / total:8.4f}{detail}{note}", flush=True)
            if args.summary or codec:
                print(f"== {loc}: {totals['cases']} cases, {n_valid} valid characters + {len(chars) - n_valid} invalid sequences; "
                      f"differing lines: {totals['bad_valid']} on valid characters, {totals['bad_invalid']} on invalid sequences; "
                      f"{totals['bad_cases']} cases differ", flush=True)
        print("all cases match" if not failed else f"{failed} cases differ", flush=True)
        sys.exit(1 if failed else 0)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()

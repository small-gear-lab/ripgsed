#!/usr/bin/env python3
"""Random patterns and texts in locales of other character sets, ripgsed against GNU sed 4.9.

    tools/locale-fuzz.py                         # 300 cases per locale
    tools/locale-fuzz.py --cases 2000 --seed 7 --locales ja_JP.eucJP,ja_JP.SJIS
    tools/locale-fuzz.py --invalid               # also bytes that are not characters of the set

Every case is `s/PATTERN/<&>[\\1]/g` (BRE, with `I` and `M` sometimes) over a few lines written
in the character set of the locale (see `tools/locale-diff.py`, which checks every character one
by one; this checks them in company: matches next to each other, word boundaries, empty
matches, groups and back-references). The outputs and the exit statuses are compared.
Locales are made with `localedef` as in `tools/locale-diff.py`. Exit status 1 when a case differs.
"""
import argparse
import importlib.util
import os
import random
import shutil
import subprocess
import sys
import tempfile

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("locale_diff", os.path.join(here, "locale-diff.py"))
ld = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ld)

ASCII = ["a", "b", "x", "B", "1", "_", " ", "-", ".", "é"]
EXTRA = {
    "euc_jp": ["あ", "ア", "漢", "ａ", "１", "ー", "ｱ", "。"],
    "shift_jis": ["あ", "ア", "漢", "ａ", "１", "ー", "ｱ", "。", "ソ", "表"],
    "gbk": ["中", "文", "ａ", "１", "。", "α", "я"],
    "gb18030": ["中", "文", "ａ", "１", "。", "α", "я", "𠀀", "€"],
    "big5": ["中", "文", "ａ", "１", "。", "α", "功"],
    "euc_kr": ["한", "글", "ａ", "１", "。", "α"],
    "latin-1": ["é", "É", "ß", "ñ", "ÿ", "µ", "ª", "²", "½"],
    "iso8859-2": ["é", "É", "ß", "ł", "Ł", "ő", "ž"],
    "iso8859-15": ["é", "É", "€", "Œ", "œ", "Ÿ", "ß"],
    "iso8859-9": ["ı", "İ", "ğ", "Ğ", "ş", "é"],
    "koi8-r": ["ж", "Ж", "я", "Я", "ё", "Ё"],
    "cp1251": ["ж", "Ж", "я", "Я", "ё", "Ё", "ї"],
    "cp1252": ["é", "É", "€", "œ", "Ÿ", "ß", "’"],
}
BRACKET_ITEMS = ["a", "b", "x", "a-c", "0-9", "[:alpha:]", "[:upper:]", "[:digit:]", "[:space:]", "[:punct:]", "é", "a-z", "A-Z"]


def encodable(items, codec):
    return ld.encode_all(items, codec)


def gen_pattern(rng, chars):
    nest = [0]

    def atom():
        r = rng.random()
        if r < 0.40:
            return rng.choice(chars)
        if r < 0.50:
            return "."
        if r < 0.62:
            items = encodable(BRACKET_ITEMS, CODEC[0]) + chars
            body = "".join(rng.choice(items) for _ in range(rng.randint(1, 3)))
            return "[" + ("^" if rng.random() < 0.3 else "") + body + "]"
        if r < 0.70:
            return rng.choice(["\\w", "\\W", "\\s", "\\S", "\\b", "\\B", "\\<", "\\>"])
        if r < 0.76 and nest[0] == 0:
            # (Not inside a group: `\(^a*\)\{1,2\}` with `M` is a quirk of the matcher that is the
            # same in every locale and is not what this checks.)
            return rng.choice(["^", "$"])
        if r < 0.88 and nest[0] < 2:
            nest[0] += 1
            inner = "".join(piece() for _ in range(rng.randint(1, 3)))
            nest[0] -= 1
            return "\\(" + inner + "\\)"
        return rng.choice(chars)

    def piece():
        a = atom()
        if a in ("^", "$", "\\b", "\\B", "\\<", "\\>"):
            return a
        r = rng.random()
        if r < 0.15:
            return a + "*"
        if r < 0.22:
            return a + "\\+"
        if r < 0.27:
            return a + "\\?"
        if r < 0.30:
            return a + "\\{1,2\\}"
        return a

    branches = ["".join(piece() for _ in range(rng.randint(1, 4))) for _ in range(rng.choice([1, 1, 1, 2]))]
    pat = "\\|".join(branches)
    return pat


CODEC = [None]


def gen_text(rng, chars, invalid, raw):
    out = b""
    for _ in range(rng.randint(0, 8)):
        if invalid and rng.random() < 0.1:
            out += bytes([rng.choice(raw)])
        else:
            out += rng.choice(chars).encode(CODEC[0])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ours", default=os.path.join(here, "..", "target", "release", "ripgsed"))
    ap.add_argument("--locales", default=ld.NON_UTF8_LOCALES)
    ap.add_argument("--cases", type=int, default=300)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--invalid", action="store_true")
    ap.add_argument("--loc-dir", default=None)
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()
    work = tempfile.mkdtemp(prefix="locale-fuzz-")
    try:
        for name, target in (("ours", os.path.abspath(args.ours)), ("gnu", shutil.which("sed", path="/usr/bin:/bin"))):
            os.makedirs(os.path.join(work, name))
            os.symlink(target, os.path.join(work, name, "sed"))
        locdir = args.loc_dir or os.path.join(work, "loc")
        names = args.locales.split(",")
        ld.make_locales(names, locdir)
        failed = 0
        for loc in names:
            codec = ld.LOCALE_DEFS[loc][2]
            CODEC[0] = codec
            chars = encodable(ASCII + EXTRA[codec], codec)
            raw = [0x80, 0x81, 0xA1, 0xA4, 0xB1, 0xE0, 0xFE, 0xFF, 0x8E, 0x8F, 0x40, 0x5C]
            rng = random.Random(args.seed)
            bad = 0
            for i in range(args.cases):
                pat = gen_pattern(rng, [c for c in chars if c not in (".",)] or ["a"])
                flags = rng.choice(["", "", "", "I", "M"])
                if codec == "gb18030" and flags == "I":
                    flags = ""  # GNU sed's answers under I for some GB18030 letters vary from run to run
                texts = b"\n".join(gen_text(rng, chars, args.invalid, raw) for _ in range(4)) + b"\n"
                extended = rng.random() < 0.25
                if extended:
                    for a, b in (("\\(", "("), ("\\)", ")"), ("\\|", "|"), ("\\+", "+"), ("\\?", "?"), ("\\{", "{"), ("\\}", "}")):
                        pat = pat.replace(a, b)
                script = ("s/" + pat + "/<&>[\\1]/g" + flags).encode(codec)
                res = []
                for d in ("gnu", "ours"):
                    env = {"PATH": os.path.join(work, d) + ":/usr/bin:/bin", "LC_ALL": loc, "LANGUAGE": "C", "LOCPATH": locdir}
                    # GNU sed's own results, quirks of the C library's matcher included.
                    opts = ["--glibc-compat"] if d == "ours" else []
                    r = subprocess.run(["sed"] + opts + (["-E"] if extended else []) + [script], input=texts, env=env, capture_output=True)
                    res.append((r.returncode, r.stdout, r.stderr))
                if res[0] != res[1]:
                    bad += 1
                    if bad <= 8 or args.verbose:
                        print(f"{loc}: DIFF case {i}: {script!r}\n  input {texts!r}\n  gnu  {res[0]!r}\n  ours {res[1]!r}", flush=True)
            print(f"{loc}: {args.cases} cases, {bad} differ", flush=True)
            failed += bad
        sys.exit(1 if failed else 0)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()

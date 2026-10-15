#!/usr/bin/env python3
"""Compare ripgsed with GNU sed 4.9 in UTF-8 locales on every Unicode scalar value.

    tools/locale-diff.py                       # the default locales and cases
    tools/locale-diff.py --locales en_US.UTF-8,tr_TR.UTF-8
    tools/locale-diff.py --ours target/debug/ripgsed --quick

The input is one line per code point (0 to 0x10FFFF, without the surrogates and the newline),
so each case below is decided for 1,112,063 characters. A case is a sed script run by both
programs on that input; the answers are compared line by line (for `=` scripts: as the sets of
line numbers). Error messages and exit statuses are compared as well.

Locales that the machine does not have are made with `localedef` in a scratch directory and
selected with LOCPATH (`--make`, or whenever a locale is missing): this needs the definitions
under /usr/share/i18n (the Debian/Ubuntu package `locales`).

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
SHORTHANDS = ["\\w", "\\W", "\\s", "\\S"]
LITERALS = ["i", "I", "s", "S", "k", "K", "a", "z", "ß", "ǆ", "ǅ", "ς", "σ", "İ", "ı", "é", "в", "ᲀ", "ω", "Ω", "ᾳ", "ａ", "ⱥ", "ɐ"]
# Differences that are known and listed in docs/KNOWN-DIFFERENCES.md: reported, but not a failure.
# A back-reference under `I` after a character whose upper-case form is longer in UTF-8 (`ɐɐ`).
KNOWN = {"/^\\(.\\)\\1$/I=": "back-reference under I after a letter whose capital is longer"}
# Scripts whose whole output is compared, line by line.
CONVERSIONS = ["s/.*/\\U&/", "s/.*/\\L&/", "s/.*/\\u&/", "s/.*/\\l&/", "s/\\b/|/g", "s/\\B/|/g"]
# Patterns that GNU sed refuses (or accepts) depending on the locale: the messages and the
# status are compared.
ERRORS = ["s/[b-a]/X/", "s/[é-z]/X/", "s/[a-é]/X/", "s/[[.space.]]/X/", "s/[[:foo:]]/X/", "s/[[=ab=]]/X/", "s/[[.a.]-z]/X/", "s/[[:alpha:]-z]/X/", "s/[z-a]/X/I"]


def write_inputs(path, twice_path, pair_path, limit):
    """One line per code point; every code point twice in a line; every code point after a `3`."""
    cps = [c for c in range(0, limit) if c != 10 and not 0xD800 <= c < 0xE000]
    with open(path, "wb") as f:
        f.write(b"".join(chr(c).encode("utf-8") + b"\n" for c in cps))
    with open(twice_path, "wb") as f:
        f.write(b"".join(chr(c).encode("utf-8") * 2 + b"\n" for c in cps))
    with open(pair_path, "wb") as f:
        f.write(b"".join(b"3" + chr(c).encode("utf-8") + b"\n" for c in cps))
    return cps


def make_locales(names, locdir):
    """Makes the locales that LC_ALL=<name> cannot select; returns True when LOCPATH is needed."""
    needed = False
    for name in names:
        if name in ("C", "POSIX", "C.UTF-8", "C.utf8"):
            continue
        base = name.split(".")[0]
        have = subprocess.run(["locale", "charmap"], env={"PATH": "/usr/bin:/bin", "LC_ALL": name}, capture_output=True, text=True).stdout.strip()
        if have == "UTF-8":
            continue
        os.makedirs(locdir, exist_ok=True)
        target = os.path.join(locdir, name)
        if not os.path.isdir(target):
            r = subprocess.run(["localedef", "-f", "UTF-8", "-i", base, target], capture_output=True, text=True)
            if not os.path.isdir(target):
                print(f"cannot make {name}: {r.stderr.strip()}", file=sys.stderr)
                continue
        needed = True
    return needed


def run(prog_dir, locale, locpath, args, input_path):
    env = {"PATH": prog_dir + ":/usr/bin:/bin", "LC_ALL": locale}
    if locpath:
        env["LOCPATH"] = locpath
    r = subprocess.run(["sed"] + args, env=env, stdin=open(input_path, "rb"), capture_output=True)
    return r.returncode, r.stdout, r.stderr


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ours", default=os.path.join(os.path.dirname(__file__), "..", "target", "release", "ripgsed"))
    ap.add_argument("--locales", default=",".join(DEFAULT_LOCALES))
    ap.add_argument("--loc-dir", default=None, help="where `localedef` makes missing locales (default: a scratch directory)")
    ap.add_argument("--max-cp", type=lambda v: int(v, 0), default=0x110000, help="only the code points below this one")
    ap.add_argument("--quick", action="store_true", help="fewer cases")
    ap.add_argument("--filter", default=None, help="only the cases whose label contains this text")
    ap.add_argument("--verbose", action="store_true", help="also list the matching cases")
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
        input_path = os.path.join(work, "all.txt")
        twice_path = os.path.join(work, "twice.txt")
        pair_path = os.path.join(work, "pair.txt")
        cps = write_inputs(input_path, twice_path, pair_path, args.max_cp)

        names = args.locales.split(",")
        locdir = args.loc_dir or os.path.join(work, "loc")
        locpath = locdir if make_locales(names, locdir) else os.environ.get("LOCPATH")

        cases = []  # (label, argv, kind)
        brackets = BRACKETS[:8] if args.quick else BRACKETS
        for b in brackets:
            for flag in ("", "I"):
                cases.append((f"/^{b}$/{flag}=", ["-n", f"/^{b}$/{flag}="], "set"))
        for s in SHORTHANDS:
            cases.append((f"/^{s}$/=", ["-n", f"/^{s}$/="], "set"))
        for lit in LITERALS[:4] if args.quick else LITERALS:
            cases.append((f"/^{lit}$/I=", ["-n", f"/^{lit}$/I="], "set"))
        for c in CONVERSIONS:
            cases.append((c, [c], "lines"))
        cases.append(("/^\\(.\\)\\1$/I=", ["-n", "/^\\(.\\)\\1$/I="], "set2"))
        # Where the matches are (the second matcher of GNU sed, collation) once a line has passed
        # the first (code points for digit ranges): `3٣` has two digits for `s/[0-9]/N/g`.
        for c in ["s/[0-9]/N/g", "s/[0-9a]/N/g", "s/[1-5]/N/g", "s/[^0-9]/N/g", "s/[0-9a-c]/N/g", "s/[0-9]\\+/N/g"]:
            cases.append((c + " (3x)", [c], "lines3"))
        for e in ERRORS:
            cases.append((e, [e], "error"))

        if args.filter:
            cases = [c for c in cases if args.filter in c[0]]
        failed = 0
        print(f"{'locale':14} {"case":26} {'code points':>11} {'differ':>7} {'match %':>8}", flush=True)
        for loc in names:
            for label, argv, kind in cases:
                src = twice_path if kind == "set2" else pair_path if kind == "lines3" else input_path
                rg, og, eg = run(gnu_dir, loc, locpath, argv, src)
                ro, oo, eo = run(ours_dir, loc, locpath, argv, src)
                total = len(cps)
                if kind == "error":
                    bad = int((rg, og, eg) != (ro, oo, eo))
                    total = 1
                elif (rg, eg) != (ro, eo):
                    bad = total
                elif kind in ("set", "set2"):
                    a, b = set(og.split()), set(oo.split())
                    bad = len(a ^ b)
                else:
                    la, lb = og.split(b"\n"), oo.split(b"\n")
                    bad = sum(1 for x, y in zip(la, lb) if x != y) + abs(len(la) - len(lb))
                known = bad != 0 and label in KNOWN
                failed += bad != 0 and not known
                if bad or args.verbose:
                    note = "  (known: " + KNOWN[label] + ")" if known else ""
                    print(f"{loc:14} {label:26} {total:11d} {bad:7d} {100.0 * (total - bad) / total:8.4f}{note}", flush=True)
        print("all cases match" if not failed else f"{failed} cases differ", flush=True)
        sys.exit(1 if failed else 0)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()

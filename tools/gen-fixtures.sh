#!/usr/bin/env bash
# Generate the expected results of fixtures by running GNU sed.
#
#   tools/gen-fixtures.sh           regenerate every fixture
#   tools/gen-fixtures.sh NAME...   regenerate the named fixtures
#
# A fixture is a directory under tests/fixtures (see tests/README.md). Its `cmd`
# script runs under sh with `sed` resolved to the reference GNU sed, in a
# scratch copy of the fixture's `files/` directory. The result is stored in
# `expected/`.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
fx="$root/tests/fixtures"
real_sed=${SED:-$(command -v sed)}

# GNU sed prints argv[0] in its messages, so always run it as plain `sed`.
probe=$(mktemp -d)
ln -s "$real_sed" "$probe/sed"
version=$("$probe/sed" --version | "$probe/sed" -n '1p')
rm -rf "$probe"
version=${version##*/}   # drop the directory printed as argv[0]
case $version in
  "sed (GNU sed) 4.9") ;;
  *)
    if [ "${ALLOW_OTHER_SED:-}" != 1 ]; then
      echo "reference must be 'sed (GNU sed) 4.9' (D6), got: $version" >&2
      echo "set ALLOW_OTHER_SED=1 to override" >&2
      exit 1
    fi ;;
esac

printf '{"reference": "%s"}\n' "$version" > "$root/tests/reference.json"

# The fixtures of the grep mode (grep-*, pcre-*) run the GNU grep of this machine, as the sed
# fixtures run its sed. The version of grep and of PCRE2 is recorded in tests/reference-grep.json,
# which the `fixtures` job of CI compares too: when the runner brings other versions, the job
# fails and says so, and the expected results are made again (tools/ref-grep.sh unpacks another
# release of grep to see what its version changes).
# GREP_DIR=target/reference/grep/bin (made by tools/ref-grep.sh) records the results of that
# release instead: GNU grep 3.11 and PCRE2 10.42, which are what the CI runner has.
real_grep=${GREP:-$(command -v grep)}
# The reference must be GNU grep: a link named grep that leads to ripgsed (on the PATH of the person
# who installed it) must not become the reference of its own fixtures.
if [ -z "${GREP_DIR:-}" ] && ! LC_ALL=C "$real_grep" --version 2> /dev/null | LC_ALL=C sed -n '1p' | LC_ALL=C sed -n '/^grep (GNU grep)/p' | LC_ALL=C sed -n '1p' | read -r _; then
  echo "$real_grep is not GNU grep: set GREP=/usr/bin/grep (or GREP_DIR=...)" >&2
  exit 1
fi
reference_grep() {
  if [ -n "${GREP_DIR:-}" ]; then
    cp "$GREP_DIR/../versions.json" "$root/tests/reference-grep.json"
    return
  fi
  # (The numbers are major.minor: the package revisions of the machines differ.)
  printf '{"grep": "%s", "pcre2": "%s"}\n' \
    "$(LC_ALL=C "$real_grep" --version | LC_ALL=C sed -n '1p')" \
    "$( (LC_ALL=C pcre2-config --version 2> /dev/null || LC_ALL=C dpkg-query -W -f='${Version}' libpcre2-8-0 2> /dev/null || echo unknown) | sed -E 's/^([0-9]+\.[0-9]+).*/\1/')" \
    > "$root/tests/reference-grep.json"
}
reference_grep

if [ $# -gt 0 ]; then names=("$@"); else names=($(ls "$fx")); fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

for name in "${names[@]}"; do
  d="$fx/$name"
  for f in cmd covers; do
    [ -f "$d/$f" ] || { echo "$name: missing $f" >&2; exit 1; }
  done

  w="$scratch/$name"
  rm -rf "$w"
  mkdir -p "$w/cwd" "$w/bin"
  if [ -d "$d/files" ]; then cp -a "$d/files/." "$w/cwd/"; fi
  ln -s "$real_sed" "$w/bin/sed"
  case $name in
    grep-* | pcre-*)
      # GNU grep prints argv[0] in its messages, so run it as plain `grep` (and `egrep`, `fgrep`).
      for t in grep egrep fgrep; do
        if [ -n "${GREP_DIR:-}" ]; then
          ln -s "$(cd "$GREP_DIR" && pwd)/$t" "$w/bin/$t"
        else
          printf '#!/bin/bash\nexec -a %s %s "$@"\n' "$t" "$(dirname "$real_grep")/$t" > "$w/bin/$t"
          chmod +x "$w/bin/$t"
        fi
      done ;;
  esac

  extra=()
  if [ -f "$d/env" ]; then mapfile -t extra < "$d/env"; fi
  stdin=/dev/null
  if [ -f "$d/stdin" ]; then stdin="$d/stdin"; fi

  set +e
  (
    cd "$w/cwd"
    env -i "PATH=$w/bin:/usr/bin:/bin" HOME=/nonexistent LC_ALL=C "${extra[@]}" \
      timeout 20 sh "$d/cmd" < "$stdin" > "$w/stdout" 2> "$w/stderr"
  )
  status=$?
  set -e
  if [ "$status" -eq 124 ]; then
    echo "$name: the reference run timed out (an endless loop?)" >&2
    exit 1
  fi

  rm -rf "$d/expected"
  mkdir -p "$d/expected"
  if [ -s "$w/stdout" ]; then cp "$w/stdout" "$d/expected/stdout"; fi
  if [ -s "$w/stderr" ]; then cp "$w/stderr" "$d/expected/stderr"; fi
  if [ "$status" -ne 0 ]; then printf '%d\n' "$status" > "$d/expected/status"; fi

  before="$scratch/empty"
  mkdir -p "$before"
  if [ -d "$d/files" ]; then before="$d/files"; fi
  if ! diff -r "$before" "$w/cwd" > /dev/null 2>&1; then
    mkdir -p "$d/expected/files"
    cp -a "$w/cwd/." "$d/expected/files/"
  fi
  rmdir "$d/expected" 2> /dev/null || true
  echo "generated: $name"
done

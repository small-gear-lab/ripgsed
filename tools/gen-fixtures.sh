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

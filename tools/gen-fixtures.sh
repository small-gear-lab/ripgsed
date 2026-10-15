#!/usr/bin/env bash
# Generate expected fixture outputs by running GNU sed.
set -euo pipefail
export LC_ALL=C

root=$(cd "$(dirname "$0")/.." && pwd)
fx="$root/tests/fixtures"
sed_bin=${SED:-sed}

version=$("$sed_bin" --version | sed -n '1p')
case $version in
  *"GNU sed"*) ;;
  *) echo "GNU sed is required: $version" >&2; exit 1 ;;
esac

if [ $# -gt 0 ]; then names=("$@"); else names=($(ls "$fx")); fi

for name in "${names[@]}"; do
  d="$fx/$name"
  args=()
  if [ -f "$d/args" ]; then mapfile -t args < "$d/args"; fi
  set +e
  "$sed_bin" "${args[@]}" -f "$d/script.sed" "$d/input.bin" > "$d/expected.bin" 2> /dev/null
  status=$?
  set -e
  printf '{"exit_status": %d}\n' "$status" > "$d/expected.json"
  printf '{"reference": "%s", "locale": "C", "os": "%s", "generated": "%s"}\n' \
    "$version" "$(uname -sr)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$d/profile.json"
  echo "generated: $name"
done

#!/usr/bin/env bash
# Print the number of compatibility items per area and status.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
tsv=$(mktemp)
trap 'rm -f "$tsv"' EXIT
# The matrices of sed and grep, one table.
awk 'FNR == 1 && NR != 1 { next } { print }' "$root/compat/sed.tsv" "$root/compat/grep.tsv" > "$tsv"

printf '%-16s %12s %8s %9s\n' area unsupported partial verified
awk -F'\t' '
  NR == 1 { next }
  { count[$2 "\t" $4]++; areas[$2] = 1 }
  END {
    for (a in areas)
      printf "%-16s %12d %8d %9d\n", a, count[a "\tunsupported"], count[a "\tpartial"], count[a "\tverified"]
  }
' "$tsv" | sort
awk -F'\t' '
  NR == 1 { next }
  { total[$4]++ }
  END { printf "%-16s %12d %8d %9d\n", "total", total["unsupported"], total["partial"], total["verified"] }
' "$tsv"

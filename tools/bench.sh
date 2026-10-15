#!/usr/bin/env bash
# Compare ripgsed with GNU sed on fixed workloads.
#
#   tools/bench.sh            run every workload (5 runs each, median)
#   RUNS=9 SIZE=2000000 tools/bench.sh
#
# Inputs are generated deterministically (fixed seed) in a scratch directory. For every
# workload the two programs must print identical output (compared by hash) or the row is
# marked MISMATCH. Times are wall-clock medians in seconds; RSS is the peak resident set in
# MiB of the median run. Nothing here is a claim: record the machine and the build with the
# numbers (see docs/DECISIONS.md, D9).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
runs=${RUNS:-5}
size=${SIZE:-1000000}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

cargo build --release --manifest-path "$root/Cargo.toml" > /dev/null 2>&1
ours="$root/target/release/ripgsed"
gnu=$(command -v sed)

# Deterministic input: mixed log-like lines, about $size lines.
awk -v n="$size" 'BEGIN {
  srand(42)
  split("INFO WARN ERROR DEBUG", lv, " ")
  split("alpha beta gamma delta epsilon zeta eta theta", w, " ")
  for (i = 1; i <= n; i++)
    printf "%d %s user=%s id=%d msg=%s %s %s\n", 1700000000 + i, lv[1 + int(rand() * 4)], w[1 + int(rand() * 8)], int(rand() * 100000), w[1 + int(rand() * 8)], w[1 + int(rand() * 8)], w[1 + int(rand() * 8)]
}' > "$scratch/log.txt"
# One long line of the same data, and many short lines.
tr '\n' ' ' < "$scratch/log.txt" | head -c 20000000 > "$scratch/long.txt"; echo >> "$scratch/long.txt"
awk 'BEGIN { for (i = 1; i <= 2000000; i++) print i }' > "$scratch/nums.txt"

printf '| workload | GNU sed (s) | ripgsed (s) | ratio | GNU RSS | ripgsed RSS |\n|---|---:|---:|---:|---:|---:|\n'

measure() { # program file args... -> "median_seconds rss_mib hash"
  local prog=$1 file=$2; shift 2
  local out=() i t m
  for ((i = 0; i < runs; i++)); do
    /usr/bin/time -f '%e %M' -o "$scratch/t" "$prog" "$@" "$file" > "$scratch/out" 2> /dev/null || true
    out+=("$(cat "$scratch/t")")
  done
  printf '%s\n' "${out[@]}" | sort -n | sed -n "$((runs / 2 + 1))p" | awk -v h="$(md5sum < "$scratch/out" | cut -c1-12)" '{ printf "%s %.0f %s\n", $1, $2 / 1024, h }'
}

row() { # name file args...
  local name=$1 file=$2; shift 2
  read -r gt gr gh <<< "$(measure "$gnu" "$scratch/$file" "$@")"
  read -r ot or oh <<< "$(measure "$ours" "$scratch/$file" "$@")"
  local ratio status=""
  ratio=$(awk -v a="$ot" -v b="$gt" 'BEGIN { if (b > 0) printf "%.2f", a / b; else print "-" }')
  [ "$gh" = "$oh" ] || status=" MISMATCH"
  printf '| %s | %s | %s | %s%s | %s MiB | %s MiB |\n' "$name" "$gt" "$ot" "$ratio" "$status" "$gr" "$or"
}

row 's/literal/X/g'            log.txt 's/alpha/X/g'
row 's/[0-9]+/N/g (ERE)'      log.txt -E 's/[0-9]+/N/g'
row 's/regex$/ anchored'       log.txt 's/[a-z]*a$/END/'
row '/ERROR/d'                 log.txt '/ERROR/d'
row '-n /user=beta.*id=1/p'    log.txt -n '/user=beta.*id=1/p'
row 'y/abc/xyz/'               log.txt 'y/abc/xyz/'
row 's/\(a\)\(b\)/\2\1/g'      log.txt 's/\(a\)\(l\)/\2\1/g'
row '$!N;P;D'                  nums.txt '$!N;P;D'
row 'sed -n 2000000p'          nums.txt -n '1000000p'
row 'one 20 MB line, s///g'    long.txt 's/alpha/X/g'

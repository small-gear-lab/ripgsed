#!/usr/bin/env bash
# Compare ripgsed with GNU sed on fixed workloads.
#
#   tools/bench.sh            run every workload (5 runs each, median)
#   RUNS=9 SIZE=2000000 tools/bench.sh
#   ONLY='y/' tools/bench.sh   only the rows whose name has that text (every row if it is not set)
#
# Inputs are generated deterministically (fixed seed) in a scratch directory. For every
# workload the two programs must print identical output (compared by hash) or the row is
# marked MISMATCH. Times are wall-clock medians in seconds; RSS is the peak resident set in
# MiB of the median run. Nothing here is a claim: record the machine and the build with the
# numbers (see docs/DECISIONS.md, D9).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
# Both programs run in one fixed locale. The default is glibc's C.UTF-8, the path of ripgsed
# that uses its own tables; BENCH_LOCALE=en_US.UTF-8 measures the path through the C library.
export LC_ALL=${BENCH_LOCALE:-C.UTF-8}
runs=${RUNS:-5}
size=${SIZE:-1000000}
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# OURS=path measures another binary (for example a build from an earlier commit).
if [ -z "${OURS:-}" ]; then
  cargo build --release --manifest-path "$root/Cargo.toml" > /dev/null 2>&1
fi
ours=${OURS:-$root/target/release/ripgsed}
# GNU_SED=path names the sed to compare with (it must be GNU sed: a sed on the PATH that leads to ripgsed makes the
# table a comparison of ripgsed with itself, with every ratio 1.00).
gnu=${GNU_SED:-$(command -v sed)}
if "$gnu" --version 2> /dev/null | head -n 1 | grep -qi ripgsed; then
  echo "bench.sh: $gnu is ripgsed, not GNU sed: put GNU sed first on the PATH (PATH=/usr/bin:\$PATH) or set GNU_SED=/usr/bin/sed" >&2
  exit 2
fi
if ! "$gnu" --version 2> /dev/null | head -n 1 | grep -q "GNU sed"; then
  echo "bench.sh: $gnu does not say that it is GNU sed (set GNU_SED=)" >&2
  exit 2
fi

# Deterministic input: mixed log-like lines, about $size lines.
awk -v n="$size" 'BEGIN {
  srand(42)
  split("INFO WARN ERROR DEBUG", lv, " ")
  split("alpha beta gamma delta epsilon zeta eta theta", w, " ")
  for (i = 1; i <= n; i++)
    printf "%d %s user=%s id=%d msg=%s %s %s\n", 1700000000 + i, lv[1 + int(rand() * 4)], w[1 + int(rand() * 8)], int(rand() * 100000), w[1 + int(rand() * 8)], w[1 + int(rand() * 8)], w[1 + int(rand() * 8)]
}' > "$scratch/log.txt"
# One long line of the same data, and many short lines.
head -c 20000000 "$scratch/log.txt" | tr '\n' ' ' > "$scratch/long.txt"; echo >> "$scratch/long.txt"
awk 'BEGIN { for (i = 1; i <= 2000000; i++) print i }' > "$scratch/nums.txt"

# Japanese text with numbers (for UTF-8 locales).
awk -v n=$((size / 2)) 'BEGIN {
  srand(7)
  split("エラー 警告 情報 デバッグ", lv, " ")
  split("こんにちは 世界 ユーザー 処理 完了 失敗 café naïve", w, " ")
  for (i = 1; i <= n; i++)
    printf "%d %s user=%s id=%d メッセージ=%s %s\n", 1700000000 + i, lv[1 + int(rand() * 4)], w[1 + int(rand() * 8)], int(rand() * 100000), w[1 + int(rand() * 8)], w[1 + int(rand() * 8)]
}' > "$scratch/jp.txt"

printf '| workload | GNU sed (s) | ripgsed 1 thread (s) | ratio | ripgsed auto (s) | ratio | GNU RSS | ripgsed RSS |\n|---|---:|---:|---:|---:|---:|---:|---:|\n'

measure() { # program file args... -> "median_seconds rss_mib hash"; JOBS=n forces RIPGSED_JOBS
  local prog=$1 file=$2; shift 2
  export RIPGSED_JOBS=${JOBS:-}
  [ -n "$RIPGSED_JOBS" ] || unset RIPGSED_JOBS
  local out=() i t m
  for ((i = 0; i < runs; i++)); do
    /usr/bin/time -f '%e %M' -o "$scratch/t" "$prog" "$@" "$file" > "$scratch/out" 2> /dev/null || true
    out+=("$(cat "$scratch/t")")
  done
  printf '%s\n' "${out[@]}" | sort -n | sed -n "$((runs / 2 + 1))p" | awk -v h="$(md5sum < "$scratch/out" | cut -c1-12)" '{ printf "%s %.0f %s\n", $1, $2 / 1024, h }'
}

measure_inplace() { # program args... -> "median_seconds rss_mib hash"; edits four fresh copies of log.txt in place
  local prog=$1; shift
  export RIPGSED_JOBS=${JOBS:-}
  [ -n "$RIPGSED_JOBS" ] || unset RIPGSED_JOBS
  local out=() files=() i k
  mkdir -p "$scratch/inplace"
  for k in 1 2 3 4; do files+=("$scratch/inplace/f$k.txt"); done
  for ((i = 0; i < runs; i++)); do
    # (The copies are made before the clock starts: only the edit is timed.)
    for k in "${files[@]}"; do cp "$scratch/log.txt" "$k"; done
    /usr/bin/time -f '%e %M' -o "$scratch/t" "$prog" "$@" "${files[@]}" > /dev/null 2> /dev/null || true
    out+=("$(cat "$scratch/t")")
  done
  # The hash is of the files that the last run left.
  printf '%s\n' "${out[@]}" | sort -n | sed -n "$((runs / 2 + 1))p" | awk -v h="$(cat "${files[@]}" | md5sum | cut -c1-12)" '{ printf "%s %.0f %s\n", $1, $2 / 1024, h }'
}

table_row() { # name measure-function arguments-of-the-function...: the function gets the program first
  local name=$1 fn=$2; shift 2
  if [ -n "${ONLY:-}" ] && [[ "$name" != *"$ONLY"* ]]; then
    return
  fi
  read -r gt gr gh <<< "$(JOBS= "$fn" "$gnu" "$@")"
  read -r st sr sh <<< "$(JOBS=1 "$fn" "$ours" "$@")"
  read -r ot or oh <<< "$(JOBS= "$fn" "$ours" "$@")"
  local r1 r2 status=""
  r1=$(awk -v a="$st" -v b="$gt" 'BEGIN { if (b > 0) printf "%.2f", a / b; else print "-" }')
  r2=$(awk -v a="$ot" -v b="$gt" 'BEGIN { if (b > 0) printf "%.2f", a / b; else print "-" }')
  [ "$gh" = "$sh" ] && [ "$gh" = "$oh" ] || status=" MISMATCH"
  printf '| %s | %s | %s | %s%s | %s | %s | %s MiB | %s MiB |\n' "$name" "$gt" "$st" "$r1" "$status" "$ot" "$r2" "$gr" "$or"
}

row() { # name file args...
  local name=$1 file=$2; shift 2
  table_row "$name" measure "$scratch/$file" "$@"
}

row_inplace() { # name args...: -i on four copies of log.txt, the result is the files
  local name=$1; shift
  table_row "$name" measure_inplace "$@"
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
row 'UTF-8: s/ユーザー/U/g'     jp.txt 's/ユーザー/U/g'
row 'UTF-8: -E s/[0-9]+/N/g'  jp.txt -E 's/[0-9]+/N/g'
row 'UTF-8: s/\w+/<&>/g'       jp.txt 's/\w\+/<&>/g'

# Scripts that ran only sequentially before the chunk executor took q, Q, $, F, ranges and w files.
# The w files are written in the scratch directory; only standard output is compared by hash.
row 'q: 500000q (quits half way)'      log.txt '500000q'
row 'q: /NEVERMATCH/q (never quits)'   log.txt '/NEVERMATCH/q'
row '$!s/alpha/X/'                      log.txt '$!s/alpha/X/'
row 'short ranges /ERROR/,/WARN/'      log.txt '/ERROR/,/WARN/s/alpha/X/'
row 'long ranges id=1234x..id=5678x'   log.txt '/id=1234[0-9] /,/id=5678[0-9] /s/alpha/X/'
row 'F every 1000th line'              log.txt '0~1000F'
row 'w: /ERROR/w file'                 log.txt "/ERROR/w $scratch/w1.out"
row 's/alpha/X/w file'                 log.txt "s/alpha/X/w $scratch/w2.out"

# -s and -i: each file is a stream of its own, and the files are read on several threads. The second file of
# -s is log.txt again (the argument comes before the file that every row ends with).
row '-s $!s/alpha/X/ (2 files)'        log.txt -s '$!s/alpha/X/' "$scratch/log.txt"
row '-s -n $p (2 files)'               log.txt -s -n '$p' "$scratch/log.txt"
row '-s ranges /ERROR/,/WARN/ (2 files)' log.txt -s '/ERROR/,/WARN/s/alpha/X/' "$scratch/log.txt"
row_inplace '-i s/alpha/X/g (4 files)'          -i 's/alpha/X/g'
row_inplace '-i $!s/alpha/X/ (4 files)'         -i '$!s/alpha/X/'
row_inplace '-i ranges /ERROR/,/WARN/ (4 files)' -i '/ERROR/,/WARN/s/alpha/X/'
row_inplace '-i.bak 1d;$d (4 files)'            -i.bak '1d;$d'

# Scripts that read only a part of the input: the end of a file (read from its end), the lines up to the last number
# of the script (and a quit that comes early), the two ends, and every Nth line. Each is compared with GNU sed, which
# reads it all. ($-N is not here: GNU sed does not take it, so there is nothing to compare the output with.)
row "-n '\$p' (last line)"            log.txt -n '$p'
row "\$!d (last line)"                log.txt '$!d'
row "-n 1,10p (first lines)"         log.txt -n '1,10p'
row "-n 500000p (half way)"          log.txt -n '500000p'
row "-n '1p;\$p' (both ends)"         log.txt -n '1p;$p'
row "10q (quits at once)"            log.txt '10q'
row "-n 0~1000p (every 1000th)"      log.txt -n '0~1000p'
row "-n 2~7p (every 7th)"            log.txt -n '2~7p'
row 'y/a-z/A-Z/ (26 letters)'         log.txt 'y/abcdefghijklmnopqrstuvwxyz/ABCDEFGHIJKLMNOPQRSTUVWXYZ/'

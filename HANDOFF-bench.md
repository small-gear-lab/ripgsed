# Handoff: benchmark of the parallel chunk executor and the shortcuts (2026-10-17)

A temporary note for whoever picks this branch up. It is not documentation: delete it once the point below is dealt with.

## What was measured

- Commit measured: `3f32e51` (`research/parallel-chunks`). Machine: Intel Core i7-1260P (16 threads), 61 GiB, Linux 7.2.6, rustc 1.99.0.
- `GNU_SED=/usr/bin/sed tools/bench.sh` as it is (`RUNS=5`, `SIZE=1000000`), GNU sed 4.9. Nothing else was running (load average 0.15, no busy `sed`).
- The `y` rows were also run with `ONLY='y/' RUNS=21` for `f6d4200`, `4a9b694` and the commit above, to see the table-driven `y` (`50a9efe`).
- The README table is these numbers; the Japanese one is generated from them.

## Results

- No `MISMATCH` in 38 rows (and none in the `y` runs).
- `y` is fast on this machine too (ratios to GNU sed, one thread / automatic, 21 runs):

  | build | `y/abc/xyz/` | `y/a-z/A-Z/` |
  |---|---:|---:|
  | `f6d4200` | 0.84 / 0.21 | 1.06 / 0.25 |
  | `4a9b694` | 1.01 / 0.26 | 1.11 / 0.27 |
  | now | 0.09 / 0.03 | 0.03 / 0.01 |

  The regression of `4a9b694` is real (0.84 to 1.01, and 1.06 to 1.11) and the table lookup removes it: `y/abc/xyz/` takes 0.09 s (GNU sed 1.01 s), the 26-letter one 0.09 s (GNU sed 2.76 s).
- Existing rows against the report at `4a9b694` (ratios): none is 10% worse. The two nearest are `-i` with ranges, automatic 0.62 to 0.68 (+9.7%), and
  `$!N;P;D`, automatic 0.86 to 0.93 (that script is sequential, so one thread and automatic are the same code; 0.12 s and 0.14 s). Both look like single-run noise; neither was repeated.
- Memory: the shortcut rows take 4 to 16 MiB; the ranges and `-i` rows 70 to 169 MiB as before; `y` 39 to 43 MiB (it was 54 to 71).

## Still open

- **`-s -n '$p'` (two files) is still slower than GNU sed on one thread**: 1.14 here (0.07 s against 0.08 s); 1.29 and 1.33 in the earlier runs. With the threads it is 0.43.
  The difference is about 10 ns a line. It was not looked for further; the ratio is of two very short times.

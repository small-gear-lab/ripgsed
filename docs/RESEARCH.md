# Research notes: fast matching for ripgsed

What the literature and existing engines suggest for making ripgsed faster than GNU sed 4.9 while keeping its exact behavior (D4, D9). This is a survey, not a decision: each idea is adopted only after a benchmark (`tools/bench.sh`) shows a gain and the fixtures and fuzz tests still pass. Claims are taken from the sources linked below; where only an abstract or an excerpt was read, the note says so.

## Where we are

- The engine finds the leftmost-longest match with a breadth-first (Pike-style) simulation that is linear in the input when there are no back-references, then finds group positions in a second pass. Back-references use memoized depth-first search, which is polynomial.
- Input is cut into numbered chunks of whole records, and scripts proven record-local run on a worker pool. A single reader thread and a single writer thread are the current limits of scaling.

## Engines and how they are combined

- **A meta engine, not one algorithm.** The Rust regex engine combines a prefilter, a lazy DFA for match bounds, and a capture-capable engine (one-pass DFA, bounded backtracker, Pike VM) run only on the found span. It reports the lazy DFA as usually close to a full DFA without the exponential construction cost, and the bounded backtracker as about twice as fast as the Pike VM in rough experiments ([regex internals](https://burntsushi.net/regex-internals/)).
- **Match span first, captures second.** A DFA cannot report captures, so the cheap pass finds where the match is and the expensive pass runs only there. This fits `s///` well: most lines do not match, and the ones that do need captures only for the replacement.
- **Reverse searches.** A reverse DFA finds the match start; "reverse suffix" and "reverse inner" strategies scan for a rare literal and run the regex backwards or forwards from it, with a guard that falls back when the scan would become quadratic ([regex internals](https://burntsushi.net/regex-internals/)).
- **Cache discipline.** A lazy DFA has a bounded cache; repeated clearing is detected and the engine falls back to the Pike VM ([regex internals](https://burntsushi.net/regex-internals/)).

## Literals and SIMD

- **Skip the engine when a literal decides.** A pattern that is a literal, or a small set of literals, is answered by substring search alone; otherwise extracted literals feed a prefilter. The same source reports literal search as often at least an order of magnitude faster than the regex engine ([regex internals](https://burntsushi.net/regex-internals/)).
- **Single literals.** Two-Way for the general case, with a SIMD variant that tests two rare bytes of the needle chosen from a byte-frequency table ([regex internals](https://burntsushi.net/regex-internals/), [SIMD-friendly substring search](https://timiskhakov.github.io/posts/haystacks-needles-and-hardware-intrinsics/)). The original idea compares the first and last bytes against 16 or 32 haystack bytes at once and verifies the candidates.
- **Several literals.** Teddy, from Hyperscan, finds candidates in 16- or 32-byte chunks with bit operations on fingerprints and then verifies them; it works well for fewer than about a hundred patterns, and Aho-Corasick is the fallback for more ([aho-corasick design](https://github.com/BurntSushi/aho-corasick/blob/master/DESIGN.md), [Hyperscan](https://www.usenix.org/system/files/nsdi19-wang-xiang.pdf)).
- **Do not split into lines first.** GNU grep searches the whole buffer for a required literal and finds line boundaries only around candidates, because looking for newlines means touching every byte ([discussion of GNU grep's speed](https://news.ycombinator.com/item?id=19521872)). For `/re/d` and `-n /re/p` this means unmatched lines can be copied or skipped in bulk.
- **Fast line boundaries.** Comparing 64 bytes at a time and turning the result into a bitmask (movemask on x86, an equivalent on ARM) is the basis of SIMD parsers; a block boundary needs a look at the next block ([CSV splitting at 3 GB/s](https://www.tinybird.co/blog/simd)). We cannot use the `memchr` crate (D8), so this would be our own code on `std::arch` with a portable fallback.

## Submatches with POSIX leftmost-longest

- **Tagged DFA.** Laurikari's tagged automata give submatches in a single DFA pass. Kuklewicz's version and the Okui-Suzuki disambiguation implement POSIX rules; re2c uses a lookahead variant and reports the basic Okui-Suzuki algorithm as the most robust POSIX one, with a lazy variant often faster but with unbounded memory ([re2c benchmarks](https://re2c.org/benchmarks/benchmarks.html), [lookahead TDFA](https://arxiv.org/pdf/1907.08837)).
- **Register minimization.** Tags at a fixed distance from another tag need no registers; liveness analysis and register allocation cut register counts substantially (an example goes from 20 to 5 registers). Multi-pass TDFA records states going forward and collects submatches in one backward pass, suiting just-in-time use ([A closer look at TDFA](https://arxiv.org/html/2206.01398)).
- **Caution.** GNU sed's observable rules are glibc's, not the POSIX text (see the known differences in DESIGN.md), so any TDFA must be validated against GNU sed; fuzzing re2c against Regex-TDFA found rare bugs in the latter ([re2c lookahead TDFA](https://www.researchgate.net/publication/343608985_RE2C_A_lexer_generator_based_on_lookahead-TDFA)).

## Derivative-based matching (RE#)

- RE# computes leftmost-longest (POSIX-like) matches without backtracking from symbolic derivatives cached in a lazy DFA. The search runs twice over the input: a reverse pass finds the earliest start, a forward pass finds the longest end, linear in the input; finding all matches can be quadratic in the worst case. It reports being over 71% faster than the next fastest Rust engine on its baseline benchmarks. Captures and back-references are not covered in the part read, and there is no NFA fallback yet ([RE#](https://arxiv.org/abs/2407.20479), [HTML](https://arxiv.org/html/2407.20479v1)).
- Relevant idea for us: leftmost-longest by construction is the semantics sed needs, so the match-span pass can follow RE#'s reverse-then-forward scheme on a lazy DFA. Submatch extraction would still use a separate engine on the span.

## Back-references

- No linear-time algorithm is known for general back-references. Recent results give polynomial algorithms for restricted classes (quadratic for one pattern shape, near-linear for 1-use expressions) and register-set automata ([MFCS 2025](https://arxiv.org/html/2504.18247), [register set automata](https://dl.acm.org/doi/10.1145/3808281)). Memoization makes backtracking linear only without back-references ([selective memoization](https://fservant.github.io/papers/DavisServantLee-SelectiveMemo-IEEE-SP21.pdf)). Back-references stay on the slow path; the fast paths must not depend on them.

## Parallelism

- **By records is the safe kind.** The chunked, ordered pipeline already does this. GNU parallel documents the limits of a single reader: about 1 GB/s of input and 100 MB/s of output through `--pipe`, and recommends reading ranges of a file directly (`--pipe-part`) to scale ([GNU parallel manual](https://www.gnu.org/software/parallel/man.html)). For regular files the reader can therefore be parallel too: workers read their own byte ranges and align to record boundaries; line numbers need counts of earlier ranges only for scripts that use them.
- **Inside one record is speculative.** Splitting the input of a single DFA run needs the start state of every chunk but the first; speculative methods bound or guess it and re-match on failure ([speculative parallel DFA](https://arxiv.org/abs/1210.5093), [PaREM](https://arxiv.org/pdf/1412.1741)). It only matters for very long single lines and comes last.

## Candidate order of work

1. (Done) Own SIMD (with portable fallback) `memchr`, `memmem` with rare-byte selection, and newline counting; no dependency.
2. (Done, except the chunk-level skip of unmatched lines) Literal extraction from the AST and a prefilter; skip the engine for pure literals.
3. (Done, without state acceleration) A lazy DFA for match bounds (forward, and reverse for the start) with a fallback to the existing engine, which remains the capture pass and the reference.
4. (Partly done: record buffers are reused; line-number seeks skip records by counting separators) Fewer allocations per record and a cheaper output path.
5. Parallel reading of regular files by byte ranges, then a lighter ordered writer.
6. Only then, research-grade options: a POSIX tagged DFA for the capture pass, or RE#-style derivatives for the span pass.

Every step is measured with `tools/bench.sh` before and after, and none may change an output byte (fixtures, regex differential tests, and the execution fuzz test run in sequential and forced-parallel modes).

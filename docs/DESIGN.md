# ripgsed design notes

**Status: design document; the sed-compatible core, the grep mode, `-P` (PCRE2) and the Rust library are implemented and published as an experimental 0.1.** The sequential VM, the regex engine, and automatic parallel execution exist (see [ARCHITECTURE.md](ARCHITECTURE.md) and the README for measured results); editor integration, `--explain` and the choice of the regular expression engine described below do not. Module names, CLI options, types, and extension syntax for the parts not yet built are proposals and may change. Statements marked "recommended" are current leanings, not settled decisions.

## How to read this

- The sed-compatible core is built and verified item by item against GNU sed 4.9 (`compat/sed.tsv`, [COMPATIBILITY.md](COMPATIBILITY.md)); the decisions it needed are in [DECISIONS.md](DECISIONS.md). The "to decide" items below belong to the parts that are not built (editor integration, `--explain`, the choice of the engine): each should be resolved, and its scope and acceptance criteria recorded in the repository, before that part is built.
- Priorities: **P0** sequential core, **P1** correctness and public boundaries, **P2** optimization, **P3** extensions and integrations. A priority label does not mean "implemented".

## Goals and non-goals

### Goals

1. Keep the sed language but separate syntax, semantics, and execution strategy.
2. Preserve line semantics while leaving room for buffered I/O, chunked processing, and safe parallelism.
3. Expose one core through a CLI, a Rust API, and Vim/Neovim.
4. Make differences from GNU sed, the reasons behind execution plans, and resource limits observable.
5. Be a drop-in replacement for GNU sed: the same stdout, exit status, and file effects for any valid GNU sed invocation. This is the minimum requirement of the project (D4 in [DECISIONS.md](DECISIONS.md)), not an optional goal. The grep mode aims at the same for GNU grep. Work proceeds in stages (see P0 and the compatibility tracking below), and no stage is claimed until it is verified against the GNU tool.

### Initial non-goals

- Compatibility with ripgrep, or parallelizing every script. (Full GNU sed compatibility is a requirement, not a non-goal; P0 is only the first milestone on the way, not the scope of the product.)
- Treating PCRE2 as an equivalent replacement for GNU sed regular expressions.
- Re-implementing an editor UI, or claiming to be faster than existing tools without measurements.
- Introducing mmap, SIMD, JIT, or async I/O from the start.

## P0: first vertical slice

**Recommended scope:** this is the order of work, not a reduction of the product's scope. Start with a literal single-line substitution `s/old/new/g` over LF-delimited input, stdin to stdout (the byte policy is already decided, D2). This does not mean the full sed grammar is accepted yet. Unsupported syntax must never be silently reinterpreted; it is a compile-time error until it is implemented.

### Order of work

1. CLI input contract and error classification.
2. Parse into an AST with source spans.
3. Sequential VM with substitution and auto-print.
4. Fixtures comparing stdout and exit status against GNU sed.
5. Add `-n`, `p`, `d`, regex addresses, `-e`, `-f` incrementally.
6. Stabilize pattern/hold space and the cycle before adding branching and multi-line commands.

### P0 done when

- The agreed subset matches the reference output on every fixture.
- Fixtures cover empty input, empty lines, missing final newline, multiple matches, no match, and malformed syntax.
- Unsupported syntax is a compile-time error; it never panics and never exits successfully.
- Memory use does not grow with total file size on streamed input (growth with a single record's length is documented).
- Broken pipe and other I/O errors are defined, and both the run result and the CLI exit status are tested.

## Architecture

```text
CLI / Rust API / Vim adapter
             │
      script + options
             ▼
         Parser ─────→ diagnostics + source spans
             │ AST
             ▼
       Effect analysis
             │ proven properties / unknown reasons
             ▼
          Planner
          ┌──┴──────────────────┐
          ▼                     ▼
    Sequential VM          Parallel map
    reference semantics    approved subset only
          │                     │ numbered chunks
          │                Ordered merge
          └─────────┬───────────┘
                    ▼
               Output writer

Shared: regex backend / buffered input / limits / cancellation
```

| Module | Responsible for | Not responsible for |
| --- | --- | --- |
| syntax | tokenizing, AST, source spans, syntax diagnostics | execution, file writes |
| regex | per-dialect compilation and matching | implicit semantic changes between engines |
| analysis | state, I/O, and control-flow dependency facts | declaring unanalyzable code safe |
| plan | choosing an execution strategy, recording why | redefining VM semantics |
| runtime | cycle, pattern/hold space, command evaluation | CLI argument parsing |
| io | record reading, output, backpressure | script syntax |
| cli | mode selection, argument validation, exit status | a second implementation of the transformation logic |
| adapters | API and editor bindings | duplicating core semantics |

**Recommended:** start as one crate with modules and split into a workspace once dependency boundaries settle.

## Semantics and data model

### Input and output

- Do not assume input is a `String`; keep a representation that can hold arbitrary bytes. If the first version is UTF-8 only, state explicitly that it is outside GNU sed compatibility.
- Keep the record terminator separate from the record body; never normalize a missing final newline.
- Stage support for LF, NUL-separated, CRLF, and multiple files rather than implementing all at once.
- Memory limits cover more than the input buffer: pattern space, hold space, output queue, and regex working memory.

### Sequential VM state (sketch)

```rust
// Illustrates roles only; not a settled API.
struct VmState {
    pattern: Vec<u8>,
    hold: Vec<u8>,
    named_holds: HashMap<HoldName, Vec<u8>>,
    input_position: InputPosition,
    program_counter: usize,
    substitution_flag: bool,
    range_states: Vec<RangeState>,
    pending_output: Vec<OutputEvent>,
}
```

**To decide:** when auto-print, `d`, `D`, `n`, `N`, `p`, `P`, `q`, `Q` produce output and how deferred output is ordered; when the `t` flag resets; how range start/end is evaluated; how line numbers and state carry across multiple files. Specify each against GNU sed's observable behavior and test it as a cycle transition.

## AST and static analysis

### What the AST needs

- Addresses: single line, regex, range, negation, whether a GNU extension is used.
- Commands: substitution and flags, print, delete, branch, label, hold operations, file I/O.
- Source spans: which `-e`/`-f` a piece came from, byte offsets, display line/column.
- Regex dialect and a reference to the compiled expression.

### Example effect facts

```text
reads_previous_records
consumes_additional_records
reads_or_writes_hold
uses_range_state
uses_global_position_or_eof
may_terminate_early
performs_external_io
has_unknown_control_flow
```

This is a conservative initial classification. Presence flags alone are not enough; command order and control flow must be analyzed too.

### Plan selection principles

- Only a proven record-local subset is a parallel candidate.
- The initial allow-list is side-effect-free substitution and line selection, and combinations must be validated too.
- Unknown, unsupported, looping, or cross-record scripts run sequentially. An analysis failure is distinct from a syntax error.
- Numbering input lines does not make every command parallelizable.
- Small inputs may prefer sequential execution; set any threshold only after measuring.

### `--explain` (proposal)

```text
$ ripgsed --explain 's/old/new/g' input.txt
plan: parallel-map
proof: record-local, no external effects
output: ordered

$ ripgsed --explain '1h; 2,$G' input.txt
plan: sequential
reason: hold space carries state across records
```

**Open:** whether `--explain` only prints the plan and exits, or also runs. These examples treat it as plan-only. The output goes somewhere that cannot mix with the transformed stdout.

## Automatic parallelism, ordering, and fallback

### Input splitting

Number chunks aligned to record boundaries. Chunk size is a byte target; never split a huge single record at a fixed length. Support regular files and stdin as the base path; compare mmap later.

### Output reordering

Each worker returns its chunk's results in input order. Deletions are empty output and multiple outputs are an ordered event list. The merger waits for the next needed chunk number. Bounded queues and a dispatch cap prevent unbounded buildup behind a slow leading chunk.

### When to stay sequential

| Example | Initial decision | Reason |
| --- | --- | --- |
| `s/old/new/g` | parallel candidate | record-local subset |
| `1h; 2,$G` | sequential | hold space crosses records |
| `N; P; D` | sequential | reads and cycles cross records |
| range addresses | parallel, on a guess | each chunk starts from the state the last chunk the writer took ended in, and is run again if the chunk before it ends in another |
| `$`, `$-N` | parallel | each chunk is told how many records follow it, up to what the script looks ahead |
| `-n '$p'`, `$!d`, `-n '$-9,$p'` | the end of the file | what is before the last few records leaves nothing: they are read from the end of the file (D16) |
| `-n '1,10p'`, `3!d` | stop at the last line number | what is after it leaves nothing: the run stops there (D16) |
| `-n '1p;$p'` | both ends | what is between them leaves nothing: the two ends are read (D16) |
| `-n '0~1000p'` | skip to the next line | the lines between are counted, not run |
| `F`, `s//x/` after a known regexp | parallel | the chunk knows its input; the regexp is the one just tried |
| `q` / `Q` | parallel | record-local; the first quit in input order wins and the chunks after it are dropped |
| `-s`, `-i` | parallel | each file is a stream of its own: the first chunk of a file starts from the state of the start of an input, its `$` and `$-N` stop at its end, and with `-i` the writer begins and finishes the file that replaces each input; a file named twice, `-`, and a file that the script writes and that is an input stay sequential |
| `w`, `W`, `s///w` | parallel | each chunk keeps what it writes to each file and the writer puts it there in order; `/dev/stdout` and `/dev/stderr` stay sequential |
| `r`, `R`, `e` | sequential | `r` streams a file that may never end, `R` keeps its place in its file, `e` runs commands |
| unknown extensions | rejected or known sequential VM | never run unknown semantics |

"Fallback" does not mean restarting after partial output has been written. Choose the plan before execution. If adaptive switching is introduced later, its state, side-effect, and re-execution contract needs separate design.

**Acceptance:** stdout and exit status match the sequential VM regardless of worker count and chunk boundaries. Plans with external side effects are out of scope. Cancellation and I/O failure leave no stray workers.

## CLI, mode selection, and sed/grep flag conflicts

As a drop-in replacement, one binary serves both tools and picks the mode from the program name.

**Built (D12, D14):**

- The mode comes from the basename of `argv[0]`: `sed` selects the sed mode, and `grep`, `egrep`, `fgrep` and `rgrep` select the grep mode. Installing means putting a symlink or hard link named `sed` or `grep` on `PATH`; no shell alias is involved, so scripts, cron, and other programs that run `sed` or `grep` use it too.
- Under the name `sed` or `grep`, the command line is interpreted as the GNU tool would, including its error behavior, with one addition: an option that the GNU tool rejects as invalid may mean something here (`-P` and `--glibc-compat` in the sed mode), so no command line that is valid for the GNU tool changes its meaning.
- Under any other name (such as `ripgsed`), the default is the sed mode, and a leading `--grep` selects the grep mode explicitly (the first argument only). The options that only ripgsed has are in its `--help`. The proposals `--explain`, `--engine` and `--extensions` are not built.
- Once the mode is fixed, the remaining arguments are parsed with that tool's rules. Everything after `--` is an operand.

The same option letter means different things in the two tools, which is why the mode must be fixed before parsing:

| Option | sed | grep |
| --- | --- | --- |
| `-n` | suppress auto-print | line numbers |
| `-i` | edit in place | ignore case |
| `-r` | GNU sed alias for ERE | recursive search |
| `-e` | script | pattern |
| `-f` | script file | pattern file |
| `-E` | ERE | ERE |

```sh
# Installed as drop-in replacements (symlinks named sed and grep)
sed 's/old/new/g' input.txt
sed -n '/ERROR/p' app.log
grep -n -i 'error' app.log
grep -- '-deprecated' app.log

# Called by its own name: sed mode by default, --grep selects the grep mode
ripgsed -E -f rules.sed input.txt
ripgsed --grep -n -i 'error' app.log
```

Both modes aim at long-term compatibility with the GNU tools, reached in stages. Until a feature is verified against the GNU tool, it is unsupported rather than approximated. ripgrep behavior such as automatic ignore rules is not a goal. Options such as `-v`, `-c`, recursion, and symlink handling in the grep mode are specified and verified one by one.

### Deliberate incompatibility: version and help text

The only intentional difference from the GNU tools is how the program identifies itself. Everything else is meant to be compatible.

- `--version` prints `ripgsed X.Y.Z` under the name `ripgsed`. Under the names `sed` and `grep` it keeps the GNU layout for the first line but names this project, for example `sed (ripgsed) X.Y.Z`. The next line states which GNU version the behavior targets and that the program is experimental.
- It never prints `GNU sed` or `GNU grep` as its own name. Scripts that detect GNU by reading the first line of `--version` (such as configure scripts) then take their non-GNU path instead of relying on extensions that may be unimplemented. The reference sed used by `tools/gen-fixtures.sh` is checked in the same way, so ripgsed is never mistaken for it.
- The version number is ripgsed's own and is never disguised as a GNU version.
- `--help` lists the options (with the ones that only ripgsed has) and the home page of ripgsed. Like the GNU tools, `--version` and `--help` write to stdout and exit with status 0.
- Error messages, option behavior, and exit statuses stay compatible; error text matches GNU sed word for word (D5), apart from the program-name prefix.

**Decided:** unambiguous prefixes of long options are accepted as `getopt_long` does, and `egrep`, `fgrep` and `rgrep` are program names (the messages of the grep mode say `grep`).

## Regular expressions and GNU sed compatibility

The regular expression engine is written for this project from the start (D7 in [DECISIONS.md](DECISIONS.md)). Rust's `regex` and similar libraries do not implement GNU sed's BRE/ERE: POSIX leftmost-longest matching, back-references, the GNU extensions, the `--posix` restrictions, locale behavior, and invalid multibyte input all differ, and a stopgap would only have to be thrown away. The observable behavior of GNU sed 4.9 is the specification, including the places where it differs from the POSIX text, and differential tests decide what that behavior is.

The syntax a pattern is read in is a dialect: BRE or ERE, each with the GNU extensions enabled or restricted as under `--posix` (and the `POSIXLY_CORRECT` differences, such as escapes inside bracket expressions), plus the locale. A dialect is part of the compile input, not a separate engine.

**Recommended:** separate the syntax dialect (sed script, BRE, ERE) from engine selection. Never silently switch to PCRE2 when the default engine lacks a construct. Document what semantics the GNU-compatible profile and the extension profile allow.

### Engine abstraction

The project's own engine is the default, and other engines (PCRE2, later experiments) must be able to sit beside it, so the rest of the code depends on an interface, not on a library. See D3 in [DECISIONS.md](DECISIONS.md). In the code, PCRE2 sits beside the own engine inside `Regex` (`Regex::compile_pcre`, D14), not behind the traits below, which are the shape that was considered.

```rust
// Illustrates the shape only; not a settled API.
trait RegexEngine {
    type Regex;
    // `Syntax` carries the dialect: BRE or ERE, GNU or --posix rules, locale.
    fn compile(&self, pattern: &[u8], syntax: Syntax, flags: Flags)
        -> Result<Self::Regex, CompileError>;
    fn capabilities(&self) -> Capabilities;
}

trait Matcher {
    // Search `haystack` from `start`, keeping the text before `start`
    // visible so anchors and word boundaries behave as in the full record.
    fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures)
        -> Result<bool, MatchError>;
}
```

- Patterns and haystacks are bytes (D2). Matching can start at an offset with the earlier text still visible, which `s///g`, empty matches, and `^` / `\<` need.
- Results are capture offsets, not copied text, so substitution builds its output from the original record.
- `Capabilities` states what an engine guarantees (back-references, POSIX leftmost-longest, supported dialects, resource limits). The compiler checks a script against the chosen engine and reports unsupported constructs as compile-time errors.
- Resource exhaustion is a `MatchError`, never "no match" (see the PCRE2 section).
- Compile once per script and reuse. Whether dispatch is by generics, an enum, or a trait object is decided by measurement; the interface does not depend on it.
- Conformance tests are written against the interface and run on every engine, so a replacement engine must pass the same differential fixtures.

### Tracking compatibility

- Treat "parses" and "behaves correctly" as separate states.
- Record per command whether it is unsupported, partially supported, or verified.
- Test separately: empty regex, repeated empty matches, `&` and back-references in replacements, escaped delimiters.
- Decide whether to start with `LC_ALL=C` only or split Unicode rules into a separate mode.
- Do not use existing option names such as `--posix` before implementing their semantics.

## PCRE2 and ReDoS protection

PCRE2 is an opt-in extension: `-P` in the sed mode and in the grep mode, with the library built into the program through `pcre2-sys` (D14). Backtracking can be expensive depending on input; PCRE2 support does not imply ReDoS immunity.

**Built:** the error for an exceeded limit (issue #1). A match that the library gives up on is an error, never "no match": `grep -P` says `grep: FILE: exceeded PCRE's backtracking limit` and ends the whole run with status 2 at that file, as GNU grep does (the files after it are not searched, `-s` does not silence it); the sed mode and the library give an error of the run (`ripgsed: exceeded PCRE's backtracking limit`, status 4). **Not built yet:** the options for the limits below; the default limits of the library apply.

```sh
# Hypothetical CLI: the options for the limits do not exist. Values are examples, not recommended safe defaults.
ripgsed -P \
  --match-limit=100000 \
  --depth-limit=1000 \
  --heap-limit-kib=8192 \
  's/(?<=user=)[a-z]+/REDACTED/g' app.log
```

- A match limit bounds search work; it is not a millisecond timeout.
- Verify which execution paths honor depth/heap limits; do not assume JIT behaves the same. Manage the JIT stack separately.
- Bound resource use in both the compile phase and the match phase.
- Exceeding a limit is a dedicated error, never converted into "no match", and reports the input position.
- Enforcing total wall time needs callouts with cancellation points or process isolation; a timeout on the waiting side does not stop FFI execution.
- Also limit script size, input, pattern/hold space, output, and worker queues.

**Acceptance:** test expensive inputs, limit exhaustion, cancellation, and JIT on/off separately. Choose safe defaults from measurement and operational requirements.

## Named hold spaces

**Tentative syntax:** `h{header}` / `g{header}` / `x{scratch}`. Plain `h/g/x` use the unnamed space. GNU-compatible mode rejects the extension; only an explicit extension mode allows it.

```sh
# Print the saved first line whenever a line containing BEGIN appears
ripgsed -n --extensions=named-hold '
  1h{header}
  /BEGIN/{
    g{header}
    p
  }
' input.txt
```

**To decide:** lexical rules for names, uninitialized spaces, extending `H/G`, `x` on a nonexistent name, limits on name count and total bytes, newline handling for empty spaces. Initial leaning: every space is empty at start; dynamic creation and error handling remain open.

Naming does not remove state dependence. Keep spaces isolated per run and never share or copy them across workers unconditionally.

## Rust crate API (sketch)

```rust
// Not a published API. Types and signatures are for design discussion.
let program = Compiler::new()
    .dialect(Dialect::GnuSed)
    .compile("s/old/new/g")?;
let plan = program.plan(Execution::Auto)?;
program.run(reader, writer, &plan)?;
```

- `Compiler`: takes dialect, regex backend, and allowed extensions; produces an immutable `Program`.
- `Program`: compiled syntax; holds no execution state.
- `Plan`: analysis result and strategy; design ownership so it cannot be misused with a `Program` built under different settings.
- `RunContext` (candidate): memory limit, cancellation, I/O permissions, run statistics.
- `RunOutcome` (candidate): termination reason, amount processed, normal early exit. I/O failures and limit overruns are `Error`s, not outcomes.

**To decide:** the `Read`/`Write` vs `BufRead` boundary, byte orientation, `Send`/`Sync` requirements, error type, statistics overhead, running one `Program` several times. Validate real signatures through the same code path as the minimal CLI.

## Differential testing against GNU sed and fuzzing

### Contract

Pin the reference GNU sed version, OS, and locale. For the same script and input bytes, compare stdout, exit status, and the contents of files created or modified. For stderr, compare the error category first; exact message text is a separate requirement.

```text
fixture
  script.sed
  input.bin
  args            # optional extra arguments
  profile.json    # reference version, locale, OS
  expected.bin    # generated by running the reference sed
  expected.json   # exit status, file effects
```

### Test groups

- Empty input, empty lines, missing final newline, NUL, invalid UTF-8, huge lines, multiple files.
- Combinations of addresses, ranges, negation, substitution, empty matches, back-references, branching, hold, N/D/P.
- Parser fuzzing: arbitrary bytes never panic, with time and memory bounded.
- Execution fuzzing: compare with GNU sed only on the accepted subset; do not confuse a limit hit on either side with a mismatch.
- Optimization fuzzing: compare the sequential VM and the parallel path while varying worker count and chunk boundaries.
- Minimize findings into fixtures and keep the seed and reproduction command.
- Run scripts using `e/r/w` in an isolated temporary area under an allow policy so they cannot reach arbitrary host commands or files.

Fixture inputs and scripts are written for this project; expected outputs are generated by executing GNU sed as a black box. See `tests/README.md` in the repository. If GNU sed's own test code is ever copied, check its license separately.

## Benchmark methodology

**There are no measurements yet.** Build workloads with equivalent results first; then measure speed and memory.

| Record | Required fields |
| --- | --- |
| workload | input size, line length, match density, script, seed, input hash |
| machine | CPU, RAM, OS, storage, power/core conditions |
| build | Rust and dependency versions, optimization, features, reference tool versions |
| run | command, worker count, cache conditions, repetitions |
| result | wall/CPU time, peak RSS, distribution, raw data, output equality |

- Separate startup/parse cost on small inputs from throughput on large ones.
- Separate I/O-bound from CPU-bound cases.
- Do not conveniently exclude reordering cost, PCRE2 compile/JIT, or input reading; label micro-benchmarks that do.
- Compare GNU sed / grep / ripgrep only on equivalent work.
- Report distributions, not just best values, and keep the cases where it is slower.

## Vim / Neovim buffer ranges

Interfaces that show contents and line numbers while editing already exist (Ex mode). The adapter's value is previewing a transformation before applying it.

```vim
" Transform a range, preview the diff, then apply (proposal)
:'<,'>Ripgsed s/ERROR/WARN/g

" Show matching lines with line numbers in a separate window (proposal)
:%SedG/ERROR/

" Existing Ex global command, for comparison
:%g/ERROR/
```

`SedG` is a proposed command defined here. The existing `:global` runs an Ex command on matching lines and is not the same as a plugin's extract-and-preview.

### Adapter contract (proposal)

1. Pass the selected range as one independent stream. Line numbers and `$` are relative to the range; there is no hold state outside it.
2. Invoke the CLI through argv and stdin, never by concatenating a shell string. Recommended: CLI subprocess first, FFI later.
3. Record the buffer id, range, and `changedtick` when starting an asynchronous run.
4. Preview the result and diff; never overwrite automatically if the buffer changed.
5. Apply only on explicit confirmation, as a single undo unit. Errors and cancellation leave the original buffer untouched.
6. Reject `-i` and external I/O in the buffer-processing profile.

**To decide:** encoding, final newline, results that span multiple lines, jumping from a `SedG` result window to the source line, which async API to use in Vim vs Neovim.

**Acceptance:** verify Japanese text, missing final newline, edits during a run, deletion and multi-line expansion, cancellation, and undo on real buffers.

## Licensing and code provenance

### Independent implementation

Implementing compatible behavior in original code does not by itself make GNU sed's GPL apply. The project is licensed under `MIT OR Apache-2.0`, which fits an independent implementation. Check dependencies and any borrowed tests or documents individually.

### If existing GPLv3 code were used

GNU sed is GPLv3 or later. Copying or modifying that code in a distributed derivative requires following the applicable GPL terms: retain copyright, license, and no-warranty notices; mark changes and dates; include the license text; and provide corresponding source as the distribution form requires. A combined derivative work cannot be assumed redistributable under MIT / Apache-2.0 alone.

### Policy for this repository

- Do not copy GNU sed or GNU grep source code into this repository, and do not read it either: the behavior was learned by running the tools (D12), and where the source had to be consulted, another reader gave only the observable rule. Local clones under `reference/` are untracked. PCRE2 is BSD-licensed and may be read and ported (see D14).
- Expected test output is generated by running GNU sed as a command, not copied from its test suite.
- Record provenance per file; do not try to fix a permissive label onto copied code afterwards.
- Subprocess invocation, linking, and code copying are different acts; evaluate the actual composition and distribution form.

This document contains no GNU sed implementation code.

## Open decisions

| Decision | Recommended first step | Can defer |
| --- | --- | --- |
| PCRE2 | built: `-P`, through `pcre2-sys` (D14), with the error for an exceeded limit | the options for the limits; a pure-Rust engine |
| explain | plan-only, fixed output stream | machine-readable schema |
| workers / chunks | built: record-local scripts run in parallel over ordered chunks (D9) | thresholds, auto-tuning |
| Named hold | specify grammar and initial state separately | the full extension commands |
| API stability | published as an experimental 0.1: the interface may change before 1.0 | compatibility guarantees |
| Editor | subprocess plus preview | FFI or a custom UI |

## References

- [GNU sed manual](https://www.gnu.org/software/sed/manual/sed.html)
- [GNU sed](https://www.gnu.org/software/sed/)
- [GNU GPL v3](https://www.gnu.org/licenses/gpl-3.0.html)
- [GNU GPL FAQ](https://www.gnu.org/licenses/gpl-faq.html)
- [PCRE2 API](https://www.pcre.org/current/doc/html/pcre2api.html)
- [PCRE2 pattern specification](https://www.pcre.org/current/doc/html/pcre2pattern.html)

Pin the versions of the references you rely on when implementing.

# Performance

How to find out why a Petal program is slow, what the runtime already does
about it, and where the remaining headroom is.

## The measurement loop

Three tools, in the order you should reach for them.

**1. Count what ran.** `petal run --profile <file>` prints an opcode histogram,
a builtin histogram, user-call and collection totals, and an instructions/second
rate, then two attribution tables: **top functions** (instructions retired in
each function's own body, labelled `name file:line`, plus the wall time of the
natives it called directly) and **natives by time** (calls, total and per-call
wall time; host callbacks included). Counting is a runtime switch, so a release
binary profiles without a rebuild. Embedders get the same report through
`Env::profile_report` (C: `pb_vm_set_profiling` / `pb_vm_profile_report`, which
adds the memo counters for the measured runs); timing every native call has
overhead of its own (~40 ns a call), so read cheap natives' times as upper
bounds. Start here: it is exact, it is cheap, and a surprising count (half of
all instructions being `Move`; 83% of builtin calls being `slice`) points at the
problem far more directly than a time profile does.

**2. Attribute the time.** A count is not a cost. Build with symbols —
`cd rust && cargo build --profile profiling` — and sample the binary:

```bash
./rust/target/profiling/petal run test/benchmarks/spreadsheet.ptl & \
  sleep 0.3 && sample $(pgrep -n petal) 2 -mayDie -f /tmp/prof.txt
```

The "Sort by top of stack" section of the output is self time per function.

**3. Time the change.** `./tools/bench-opts.ts` for whole programs;
`cd petal-ui && cargo run --release --example bench_panel -- <file.ptl>` for
per-frame cost of a panel script. Take the **minimum** of several runs, not the
mean: on a loaded machine the minimum is much the more stable estimator. The
bench runs under the frame gate: a quiet script is skipped after its first
frame, so pass `--wiggle` (move the pointer every frame) to measure an
interactive frame, `--scenario s.json|monkey:<seed>` to time a realistic
session (the [petal-ui-run scenario format](headless-ui-run.md#scenario-files)),
or `--policy replay` (or `--no-gate`) to measure the script itself.
`--no-memo` turns off memoized scopes; with both off, a frame costs what it did
before either layer existed. `--policy baseline` also turns off the optimizer.
Under a scenario the bench also reports the frames that ran on their own and
the session's total script time, which is the number to compare.

For one function rather than a whole program, `petal bench <file> --fn <name>`
re-runs the file and reports what a call of that function costs, over the
calls the script makes: instructions and milliseconds per call (min, median,
p95; inclusive of its callees, and self), heap allocations, copy-on-write
copies and collections per call, with the optimizer on and off in adjacent
columns. It is the tool for "did this change make `step` cheaper": the
instruction and copy counts are exact and repeat to the digit, so a change of
a few percent that wall time cannot resolve still shows. Three things to know
when reading it:

- It is not a per-instruction hook. The VM consults the bench only where a
  user-function frame is pushed and popped (`CallBench` in
  `rust/src/profile.rs`), so a benched run stays on the fast dispatch loop and
  retires exactly the instructions a plain run does — unlike `--profile`,
  which takes the general path for every instruction and so is no use for
  timing.
- Timing a frame costs two clock reads, about 50 ns together (the report
  prints the figure measured on the spot). Each call of a benched function
  pays it, and so does each user function it calls directly, which is timed so
  the caller's self time can leave it out. On a variant of
  `test/benchmarks/calls.ptl` (300,000 calls a run of a four-instruction
  function) benching that function took a run from 55 ms to 67 ms, ~42 ns a
  call, and its reported 46 ns/call is mostly that. A function that
  takes microseconds is unaffected; one that takes tens of nanoseconds reads as
  mostly overhead, and should be judged by its instruction count.
- `ms to lower` in the header is the optimizer's own cost, paid once per
  program. It is usually well under a millisecond, but it is not always small:
  the physics playground's solver takes ~48 ms to lower optimized against
  0.45 ms unoptimized, which is more than the optimizer saves over a
  120-frame run of it. A console script that runs once pays that; a host that
  keeps the program loaded does not.

`bench` runs core-host scripts, like `petal run`. For a library used by a
panel, write a console driver that imports it and calls the function in a
loop; for the panel itself, `bench_panel` above.

The allocation and copy counters behind `bench` and `run --dup-stats`
(`rust/src/stats.rs`) are a runtime switch on the heap, off until one of those
turns it on, so a release binary has them without the `dup-stats` cargo
feature (which now only sets the default, as a debug build does). Off, each
recording site is one branch on a flag in the heap. Measured as whole-process
`petal run` wall time, minimum of 15 alternating runs, before and after the
switch replaced the compile-time gate:

| Program | Before | After |
|---|---|---|
| `test/benchmarks/append.ptl` | 8.0 ms | 7.9 ms |
| `test/benchmarks/arith.ptl` | 16.2 ms | 16.2 ms |
| `test/benchmarks/calls.ptl` | 38.8 ms | 38.4 ms |
| `test/benchmarks/life.ptl` | 27.6 ms | 27.5 ms |
| `test/benchmarks/particles.ptl` | 32.5 ms | 32.6 ms |
| `test/benchmarks/audio_synth.ptl` | 326.9 ms | 321.9 ms |
| `test/benchmarks/spreadsheet.ptl` | 702.4 ms | 701.6 ms |

Every difference is inside the run-to-run noise (about 1%).

For the specific question "did the optimizer help", `PETAL_OPT_STATS=1` reports
what it did to the program, and `PETAL_POLICY=baseline` (or `--policy
baseline`, or `--no-opt`) gives the unoptimized baseline for a same-binary A/B.

## What a program costs

A Petal program is interpreted: there is no JIT, and every instruction pays a
dispatch. So the first-order model is **runtime ≈ instructions retired ÷ ~50 M/s**
(release build, this machine), and the way to make a program faster is to make
it execute fewer instructions — either by lowering it to fewer, or by writing
less work into the script.

Two consequences:

- **An unoptimized build is ~10× slower.** `cargo build` (dev profile) runs a
  script-heavy panel at ~19 ms a frame where a release build runs it at ~2.5 ms.
  A host that embeds Petal and cares about frame rate should build the `petal`
  dependency optimized even in its own debug builds; Garden's workspace does
  this with a `[profile.dev.package.petal] opt-level = 3` override.
- **A frame that runs replays the calls whose inputs did not change.** Every
  user-function call is a memoized scope: a call whose arguments, captures and
  recorded reads (`hovered(r)`, a `state` slot, a `var`) are what they were
  last frame is replayed from its record — output spliced back, writes
  re-applied — instead of run. A pointer move re-runs the rows it crossed and
  the top-level code that calls them, not the whole list. See
  [memo-scopes.md](memo-scopes.md), and `petal-ui-run --memo-stats` for why a
  widget keeps running (something in it prints, draws randomness, or hands out
  a closure over its own `var`). The top-level script body is not a scope, so
  anything expensive there that does not change per frame still belongs
  behind a `state` variable with a revision check — which is what
  `examples/productivity/spreadsheet` does for its formula recompute — or in a
  function.
- **A frame whose inputs have not changed does not run at all.** Every host
  consults the runtime's frame gate (`Env::run_needed`) before a run: if
  nothing the last run read has moved and the run settled, the last frame's
  output is kept. A quiet panel costs a few microseconds a frame; one that
  reads `time()` or animates costs a full run. See
  [frame-gate.md](frame-gate.md), and `petal-ui-run --gate-stats` for why a
  panel keeps running.

## What the optimizer does

Lowering (`backend::bytecode::lower`) gives every IR term its own register, so
the raw instruction stream is roughly half register-to-register copies. Four
passes then shape it, each individually switchable through
[`OptFlags`](../../rust/src/backend/mod.rs) so any one can be turned off to
isolate a bug. `OptFlags` is the lowering half of a run's
[`RunPolicy`](../../rust/src/policy.rs), which also carries memoization and
frame gating:

| Pass | What it does |
|---|---|
| `escape` (route B) | Proves loop-carried accumulators unique, so mutations lower to in-place heap writes instead of clone-and-alloc. |
| `lastuse` (route A) | The same for straight-line mutation of a freshly allocated, dead-after container. |
| `cells` | Lets a write through a `var` cell (`set xs[i] = v`, including from a function that did not declare the cell) mutate the cell's container whenever the cell is its only holder, which the heap tracks per cell at run time. See [var.md](../var.md#writing-a-container-in-place). |
| `copyprop` | Copy propagation, dead-move elimination, and jump threading. Removes ~25% of the instruction stream. |

`copyprop` has two deliberate limitations, both of them "do not delete what
something is reading".

The observation buffer records a value per *named* term, and a host reads a
run's bindings out of it (`--observe`, Garden's `panel.values`, the debug
server's `/state`). Deleting the instruction that writes a named register would
silently drop that binding — so when observation or the `explain` trace is on,
`OptFlags::preserve_observations` holds those moves back.

The execution trace is read per *term*, named or not — provenance, `explain`,
and direct manipulation all key on term ids, and solving `x0 + i * spacing` for
`spacing` needs the value the anonymous read of the loop counter `i` took. A
dead move is exactly what that read lowers to, so the observation guard is too
narrow for it: `OptFlags::preserve_trace` keeps *every* instruction carrying an
origin term (including self-moves, whose trace event is the only record of that
term) and is set whenever the trace buffer is enabled.

Both are part of the bytecode cache key, so switching a facility on re-lowers
rather than reusing code compiled without the guard. Observation costs about 15%
on top of its own recording overhead, and the trace guard more, since it saves
strictly more instructions. Toggle them between runs, never inside one.

## Where the remaining headroom is

Measured on `test/benchmarks/spreadsheet.ptl`, which is the formula engine from
`examples/productivity/spreadsheet` recomputing its whole grid:

- **Interpreter dispatch is the dominant cost** (~55% of samples across
  `step_in` / `exec_inst` / `run_batch` before the fast path below).
  `run_batch` now hands each frame to `run_straight` (`vm/fast.rs`) first: a
  loop that keeps `ip` and the register file in locals and retires the happy
  path of the hot instructions (constants, moves, jumps, loop steps, number
  arithmetic and comparisons, field and index reads, in-place index writes,
  and `var` cell reads and writes), checking the GC budget only at
  back-edges. Anything else — a call, an allocation, a `Pending` operand, an
  error — stops the loop at that instruction and `step_in` runs it as before.
  A cell access stays in the loop unless an open memo scope has to record it
  (a cell the scope did not create) or the frame gate wants a fingerprint of a
  `state var` before its first mutation of the run; before that, every `x[i]`
  through a cell left the loop and re-entered it, which cost a solver written
  over local `var` lists a quarter of its time. That took ~18% off Cheesecake's `neon` script time. Cutting
  further means retiring fewer instructions (or a JIT), not cheaper dispatch.
- **`Move` is ~30% of instructions.** What is left are live phi copies
  around loops — the copy in and the carry out. Register coalescing (giving a
  phi and its sources one register when their live ranges do not interfere)
  is the pass that would remove them.
- **`JumpIfPending` is ~7%**, one per branch, and almost never taken. The
  dispatch loop now takes a `JumpIfPending` and the `JumpIfFalse` on the same
  register that follows it in one step (both still counted), so the pair costs
  one dispatch; folding the test into the opcode itself would also shrink the
  code, at the cost of a two-target instruction — which the CFG helpers
  (`branch_target`) currently assume does not exist.
- **Native calls cost more than their bodies** for cheap builtins: a `PetalCxt`
  is built per call, and arguments are gathered into a `SmallVec` first.
- **Records hash their field names.** Map payloads are `heap::RecordMap`
  (`IndexMap<String, Value>` under the Fx hash in `fxhash.rs`; SipHash was
  most of a field read). A field read still hashes a string, and every record
  owns a `String` per key, so building one is a malloc per field. Interning
  field names to symbol ids — better, shapes (hidden classes: one shared key
  layout per record literal, a `Vec<Value>` per record) — would make
  `GetField` an indexed load and a record one allocation.
- **Memo scopes cost every call.** Opening a scope clones the frame path and
  snapshots the output buffers; most calls in a game loop are small and get
  folded into their parent anyway. A call site whose scopes are folded
  `TINY_AFTER_FOLDS` times in a row stops opening them (one probe per run, like
  a cold site); `memo_stats().tiny` counts the skipped ones. On a game whose
  calls rarely replay (Cheesecake's `neon`), memo still costs a few percent
  over `--no-memo`.

And one that is not the runtime's to fix: the spreadsheet's own `digit_val`
classifies a character by slicing a 10-character string ten times, which is why
`slice` accounts for 83% of its builtin calls. A script's algorithm is still the
biggest lever it has.

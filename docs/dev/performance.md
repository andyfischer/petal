# Performance

How to find out why a Petal program is slow, what the runtime already does
about it, and where the remaining headroom is.

## The measurement loop

Three tools, in the order you should reach for them.

**1. Count what ran.** `petal run --profile <file>` prints an opcode histogram,
a builtin histogram, user-call and collection totals, and an instructions/second
rate. Counting is a runtime switch, so a release binary profiles without a
rebuild. Start here: it is exact, it is cheap, and a surprising count (half of
all instructions being `Move`; 83% of builtin calls being `slice`) points at the
problem far more directly than a time profile does.

**2. Attribute the time.** A count is not a cost. Build with symbols —
`cd rust && cargo build --profile profiling` — and sample the binary:

```bash
./rust/target/profiling/petal run test/benchmarks/spreadsheet.ptl & \
  sleep 0.3 && sample $(pgrep -n petal) 2 -mayDie -f /tmp/prof.txt
```

The "Sort by top of stack" section of the output is self time per function.

**3. Time the change.** `./ts/bin/bench-opts.ts` for whole programs;
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
the raw instruction stream is roughly half register-to-register copies. Three
passes then run over it, each individually switchable through
[`OptFlags`](../../rust/src/backend/mod.rs) so any one can be turned off to
isolate a bug. `OptFlags` is the lowering half of a run's
[`RunPolicy`](../../rust/src/policy.rs), which also carries memoization and
frame gating:

| Pass | What it does |
|---|---|
| `escape` (route B) | Proves loop-carried accumulators unique, so mutations lower to in-place heap writes instead of clone-and-alloc. |
| `lastuse` (route A) | The same for straight-line mutation of a freshly allocated, dead-after container. |
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
  `step_in` / `exec_inst` / `run_batch`). Cutting it further means retiring
  fewer instructions, not making dispatch cheaper.
- **`Move` is ~30% of instructions.** What is left are live phi copies
  around loops — the copy in and the carry out. Register coalescing (giving a
  phi and its sources one register when their live ranges do not interfere)
  is the pass that would remove them.
- **`JumpIfPending` is ~7%**, one per branch, and almost never taken. Folding
  the pending test into the conditional-branch opcodes would remove all of them,
  at the cost of a two-target instruction — which the CFG helpers
  (`branch_target`) currently assume does not exist.
- **Native calls cost more than their bodies** for cheap builtins: a `PetalCxt`
  is built per call, and arguments are gathered into a `SmallVec` first.
- **Records hash their field names.** Map payloads are `IndexMap<String, Value>`
  under the default SipHash, so a field read hashes a string. Interning field
  names to symbol ids (or a linear scan for the small records that dominate)
  would make `GetField` a comparison of integers.

And one that is not the runtime's to fix: the spreadsheet's own `digit_val`
classifies a character by slicing a 10-character string ten times, which is why
`slice` accounts for 83% of its builtin calls. A script's algorithm is still the
biggest lever it has.

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
mean: on a loaded machine the minimum is much the more stable estimator.

For the specific question "did the optimizer help", `PETAL_OPT_STATS=1` reports
what it did to the program, and `PETAL_OPT=off` (or `--no-opt`) gives the
unoptimized baseline for a same-binary A/B.

## What a program costs

A Petal program is interpreted: there is no JIT, and every instruction pays a
dispatch. So the first-order model is **runtime ≈ instructions retired ÷ ~50 M/s**
(release build, this machine), and the way to make a program faster is to make
it execute fewer instructions — either by lowering it to fewer, or by writing
less work into the script.

Two consequences worth internalizing:

- **An unoptimized build is ~10× slower.** `cargo build` (dev profile) runs a
  script-heavy panel at ~19 ms a frame where a release build runs it at ~2.5 ms.
  A host that embeds Petal and cares about frame rate should build the `petal`
  dependency optimized even in its own debug builds; Garden's workspace does
  this with a `[profile.dev.package.petal] opt-level = 3` override.
- **A panel re-runs its whole script every frame.** There is no incremental
  evaluation. Anything expensive that does not change per frame belongs behind
  a `state` variable with a revision check — which is what
  `examples/testbed/25-spreadsheet` does for its formula recompute.

## What the optimizer does

Lowering (`backend::bytecode::lower`) gives every IR term its own register, so
the raw instruction stream is roughly half register-to-register copies. Three
passes then run over it, each individually switchable through
[`OptFlags`](../../rust/src/backend/mod.rs) so any one can be turned off to
isolate a bug:

| Pass | What it does |
|---|---|
| `escape` (route B) | Proves loop-carried accumulators unique, so mutations lower to in-place heap writes instead of clone-and-alloc. |
| `lastuse` (route A) | The same for straight-line mutation of a freshly allocated, dead-after container. |
| `copyprop` | Copy propagation, dead-move elimination, and jump threading. Removes ~25% of the instruction stream. |

`copyprop` has one deliberate limitation. The observation buffer records a value
per *named* term, and a host reads a run's bindings out of it (`--observe`,
Garden's `panel.values`, the debug server's `/state`). Deleting the instruction
that writes a named register would silently drop that binding — so when
observation or the `explain` trace is on, `OptFlags::preserve_observations`
holds those moves back. It is part of the bytecode cache key, so switching
observation on re-lowers rather than reusing code compiled without the guard.
Observation therefore costs about 15% on top of its own recording overhead.
Toggle it between runs, never inside one.

## Where the remaining headroom is

Measured on `test/benchmarks/spreadsheet.ptl`, which is the formula engine from
`examples/testbed/25-spreadsheet` recomputing its whole grid:

- **Interpreter dispatch is now the dominant cost** (~55% of samples across
  `step_in` / `exec_inst` / `run_batch`). Cutting it further means retiring
  fewer instructions, not making dispatch cheaper.
- **`Move` is still ~30% of instructions.** What is left are live phi copies
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

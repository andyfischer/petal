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
`cd core && cargo build --profile profiling` — and sample the binary:

```bash
./core/target/profiling/petal run test/benchmarks/spreadsheet.ptl & \
  sleep 0.3 && sample $(pgrep -n petal) 2 -mayDie -f /tmp/prof.txt
```

The "Sort by top of stack" section of the output is self time per function.

**3. Time the change.** `./tools/bench-opts.ts` for whole programs;
`cd core-libs/petal-ui && cargo run --release --example bench_panel -- <file.ptl>` for
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
  `core/src/profile.rs`), so a benched run stays on the fast dispatch loop and
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
  program and again on each hot reload that recompiles. It is usually well
  under a millisecond, but grows with the program: the physics playground's
  solver (`physics.ptl`) takes 2.8 ms to lower optimized against 0.18 ms
  unoptimized, and a 10,000-line game 69 ms. Those were 19 ms and 225 ms
  while copy propagation solved its dataflow per instruction; see
  [Lowering cost](#lowering-cost).

`bench` runs core-host scripts, like `petal run`. For a library used by a
panel, write a console driver that imports it and calls the function in a
loop; for the panel itself, `bench_panel` above.

The allocation and copy counters behind `bench` and `run --dup-stats`
(`core/src/stats.rs`) are a runtime switch on the heap, off until one of those
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

## Lowering cost

Lowering a program to bytecode runs the optimizer passes, and a hot reload
that recompiles pays for it before the next frame. `examples/reload_timing.rs`
in `core/` times it on a real program (and each reload path with it):

```bash
cd core && cargo run --release --example reload_timing -- path/to/game.ptl
```

On a 10,346-line game (62,733 terms, 14 files), release build:

| Lowered with | Time |
|---|---|
| no passes | 3.5 ms |
| escape analysis only (`in_place_mutation`) | 24 ms |
| last-use rewriting only (`in_place_straight_line`) | 19 ms |
| the cell plan only (`in_place_cells`) | 8 ms |
| copy propagation only | 29 ms (was 175 ms) |
| all passes | 69 ms (was 225 ms) |

A program's top level is a single
function, tens of thousands of instructions long in a script this size, so a
pass must be close to linear in a function's length. Copy propagation was not:
it kept a hash map of copy facts per instruction, and straight-line code
accumulates a fact per `Move`. It now solves available copies and liveness per
basic block over bit sets and replays each block once
(`core/src/backend/bytecode/copyprop.rs`). The per-instruction solver is kept
in that file's tests as the oracle; `cargo test --release --lib copyprop --
--ignored` compares the two on every `.ptl` file in the repository.

The change does not alter the code produced. Measured as whole-process
`petal run --seed 1` wall time, minimum of 15 alternating runs, before and
after the hot-reload work (which also added the constant slots a value edit
writes through):

| Program | Before | After |
|---|---|---|
| `test/benchmarks/append.ptl` | 5.2 ms | 5.2 ms |
| `test/benchmarks/arith.ptl` | 10.8 ms | 10.7 ms |
| `test/benchmarks/audio_synth.ptl` | 104.2 ms | 102.7 ms |
| `test/benchmarks/calls.ptl` | 27.1 ms | 27.2 ms |
| `test/benchmarks/life.ptl` | 13.4 ms | 13.0 ms |
| `test/benchmarks/particles.ptl` | 9.4 ms | 9.4 ms |
| `test/benchmarks/spreadsheet.ptl` | 203.3 ms | 201.4 ms |

`petal bench test/benchmarks/spreadsheet.ptl --fn evaluate` reports
20,449,661 instructions a run on both, and 2.8 ms to lower against 5.4 ms.

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
the raw instruction stream is roughly half register-to-register copies. Five
passes then shape it, each individually switchable through
[`OptFlags`](../../core/src/backend/mod.rs) so any one can be turned off to
isolate a bug. `OptFlags` is the lowering half of a run's
[`RunPolicy`](../../core/src/policy.rs), which also carries memoization and
frame gating:

| Pass | What it does |
|---|---|
| `escape` (route B) | Proves loop-carried accumulators unique, so mutations lower to in-place heap writes instead of clone-and-alloc. |
| `lastuse` (route A) | The same for straight-line mutation of a freshly allocated, dead-after container. |
| `cells` | Lets a write through a `var` cell (`set xs[i] = v`, including from a function that did not declare the cell) mutate the cell's container whenever the cell is its only holder, which the heap tracks per cell at run time. See [var.md](../var.md#writing-a-container-in-place). |
| `copyprop` | Copy propagation, dead-move elimination, and jump threading. Removes ~25% of the instruction stream. |
| `match_dispatch` | Enters a run of `match` arms keyed on string literals, int literals or enum variants through a jump table (`MatchSwitch`) instead of testing each arm in turn. See below. |

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


### Match dispatch

A `match` lowers to one `MatchArm` test per arm, tried in order. For a run of
consecutive arms whose every alternative is a string literal, an int literal
or an enum variant, lowering puts a `MatchSwitch` in front of the run: a hash
table from each key to the first arm of the run that names it. A string, int
or variant subject jumps straight there, or past the run when no arm names it.
An arm that still has something to decide (a guard, a payload pattern) is
entered at its own test, which falls through to the following arms as before.
Any other subject (a float that an int literal may equal, say) skips the table
and takes the arms in order. An arm with any other pattern ends the run. The
chain of tests stays in the code, so the table changes where a match starts
and never what it finds: first match wins either way. With the optimizer off
there is no table, which is what the differential tests compare against.

`test/benchmarks/match_dispatch.ptl` runs a 40-arm match a million times on
each kind of key (release build, this machine, median of 5):

| | chain of tests | jump table |
|---|---|---|
| 40 string arms, 1M matches | 0.48 s | 0.10 s |
| 40 int arms, 1M matches | 0.47 s | 0.09 s |
| 40 payload-free variant arms, 1M matches | 0.49 s | 0.10 s |
| the same loop and call with no match | 0.07 s | 0.07 s |
| `match_dispatch.ptl` (all three) | 1.45 s | 0.30 s |

Per match that is about 410 ns down to about 30 ns, and it no longer grows
with the number of arms.

## Frozen data and collection pauses

The collector is a full mark-and-sweep paced by allocation: a collection runs
when the work allocated since the last one reaches twice what a collection
costs (`Heap::should_collect`, `collection_cost`). Two things follow. A
collection's pause is the mark over everything rooted, plus the sweep's walk
over every slot, plus freeing the garbage; and the more live data there is,
the more garbage is allowed to pile up before the next one. A big rooted table
therefore makes every pause longer twice over.

`freeze(v)` ([Builtins.md](../Builtins.md#frozen-data)) moves a value into a
frozen generation, and a collection then does no work for it at all:

- the marker stops at a frozen object;
- the sweep does not read a frozen slot. Each slab keeps one bit per slot
  (`Slab::permanent_bits`) and the sweep reads a 64-slot word of it before
  touching the slots, skipping the frozen ones;
- `collection_cost` skips them the same way, so the budget goes back to what
  the rest of the program needs;
- `freeze` itself takes the frozen objects' share out of the budget in force,
  so a table that was built, collected and then frozen does not leave the
  collector waiting for a table's worth of garbage before it runs again.

**One collection, nothing to free** (`cd core && cargo run --release --example
gc_pause`; the table is the only root, three heap objects a record):

| Records | Rooted | Frozen |
|---|---|---|
| 10,000 | 0.49 ms | 0.002 ms |
| 100,000 | 2.13 ms | 0.009 ms |
| 400,000 | 9.21 ms | 0.034 ms |

What is left of the frozen figure is the scan of the bit index, one word for
64 slots: about 1/240 of the rooted cost at every size.

**Under allocation** (`./tools/bench-gc-pause.ts`, which runs
`test/benchmarks/micro/gc_rooted_table.ptl`: the same table in a top-level
`let`, then 2,000,000 short-lived records; the collections the build triggers
are subtracted):

| Records | Table | Collections | GC total | Pause per collection |
|---|---|---|---|---|
| 10,000 | rooted | 42 | 46 ms | 1.10 ms |
| 10,000 | frozen | 385 | 42 ms | 0.11 ms |
| 100,000 | rooted | 5 | 68 ms | 13.5 ms |
| 100,000 | frozen | 385 | 93 ms | 0.24 ms |

The pause with 100,000 records drops from 13.5 ms to 0.24 ms, and the frozen
runs collect on the same schedule at both sizes (385 collections: the budget
is the floor, as it would be with no table at all).

The frozen pause is not quite the same at the two sizes, and the difference
is not the frozen data. Building each row's name (`"row " ++ str(i)`) leaves
one dead temporary string per row, so the string slab ends up with 100,000
free slots between the frozen ones, and the sweep walks free slots on every
collection (it rebuilds the free list from them). With the name a literal
the 100,000-record frozen run measures 0.12 ms a collection, the same as
10,000. Nothing in this benchmark allocates strings afterwards; a program
that does reuses those slots. Total collection time is about what the
garbage costs to free either way: freezing shortens pauses and does not buy
throughput.

Before the frozen generation landed, the same rooted runs measured 9.5 ms a
collection (7 collections) with 10,000 records and 30 ms (3 collections) with
100,000 (measured earlier, under different load). Part of the difference on the
rooted rows above is the budget, not freezing: `collection_cost` used to
charge the full trace cost for every slot, free ones included, so the garbage
a large budget let pile up raised the next budget. It now charges live,
unfrozen objects plus a small per-slot walk cost (`SLOT_WALK_COST`).
`heap::tests::steady_garbage_keeps_a_steady_heap` holds that a steady
allocator reaches a steady slot count. Of the eight `test/benchmarks/*.ptl`
programs, three collect at all: `spreadsheet.ptl` 9 times for 1.1 ms,
`audio_synth.ptl` 621 times for 3.1 ms, `particles.ptl` 4 times for 0.3 ms.

All figures are from one machine under load from other builds; read them as
ratios.

## Enum data reads

A variant value is its tag (`"Kind.park"`, an interned string) and its
payload. What the enum declares about the variant, including its constant
data, is looked up when a field is read, which is what lets a variant kept in
`state` outlive an edit of its enum. That lookup is integer work:

- Lowering numbers every declared variant (`enum_table::EnumLayout`): a
  dense slot, enum by enum in declaration order.
- The stack keeps a table by slot (`enum_table::EnumTable`, `Stack::enums`):
  the tag's interned id, the data record this run's declaration published,
  and per enum the `Kind.all` list. The table roots the tag strings, so a
  tag id found in its index is that variant. The index is an exact
  open-addressing map from id to slot, built when the table is.
- `k.code` probes the index with the tag's id, loads the slot's record and
  reads the field through the `GetField` site's inline cache. The variants
  of one enum declare the same keys and so share a record shape; a site
  that sees every variant still hits.
- A tag the index does not hold is resolved by its text, as every read used
  to be: a variant a host built with a free-form tag, a variant of an enum
  the program no longer declares, a tag longer than the interning limit.
- `Kind.variant` with no payload is built from its slot, with no string
  interned. `Kind.all` is one list per stack, built on the first read. It
  comes from a builtin (`__enum_all`) that is not on the list of builtins
  returning a fresh container, so no in-place rewrite will write through
  it, and a `var` cell does not own a value it was handed: `append(K.all, x)`
  copies.
- One read does get a list of its own: a `K.all` that starts a variable a
  loop or a branch rebinds (`let bag = Piece.all`, then `bag[i] = …` or
  `bag = append(bag, …)` in a loop). The escape analysis marks the read
  (`copies_enum_all`: the builtin call is the init of a phi), the lowering
  sets the call's `in_place` flag, and the VM answers with a copy of the
  shared list. That copy is a fresh root, so the loop rewrites it in place:
  one allocation for the loop, as when `K.all` allocated on every read.
  Without this the shared list, not being fresh, made every iteration copy
  (100 appends starting from `K.all`: 4.8 µs before the list was shared,
  13.6 µs with it shared and no copy at the read, 4.8 µs now; the
  Fisher-Yates bag in `examples/games/tetris` is the one instance in the
  corpus). The mark is a property of the term alone and the lowering copies
  for exactly the marked terms, so the analysis cannot treat a read as
  fresh that runs shared.
- Inside a recording memo scope a read is a dependency on the stack's
  enum-data revision. The first read of a run of reads notes it on the
  general path and the rest stay in the straight-line loop.

`petal bench --iters 30 test/benchmarks/micro/enum_reads.ptl --fn …` (the
command is in the file), minimum of three runs, nanoseconds per read over
the empty loop, optimizer on:

| Read | Before | After |
|---|---|---|
| record field `r.code` | 1.7 | 1.8 |
| `k.code`, `k` in a local | 51.3 | 3.5 |
| `K.v.code` | 71.3 | 6.6 |
| payload field `op.dy` | 33.9 | 11.5 |
| `K.all` (eight variants) | 227.8 | 18.8 |
| `k.code` for each `k` in `K.all` | 91.4 | 17.3 |

`K.all` allocated a list per read (200,001 allocations for 200,000 reads)
and now allocates nothing after the first (2). The loop itself is 21 ns an
iteration on this machine (six instructions), so `K.v.code` in a loop went
from 93 ns to 28 ns. With the optimizer off the figures are within 3 ns of
these. `K.all` still costs more than a field read because the builtin's
argument, the enum's name, is a string constant that is interned on every
load, as every string literal is.

**Declaring the data.** An enum's constant data is evaluated by every run:
one record built and one `__enum_data` call per variant. `cd core && cargo
run --release --example rerun_timing -- ../test/benchmarks/micro/enum_decl_data.ptl
../test/benchmarks/micro/enum_decl_plain.ptl` runs a 50-variant enum with
three data keys, and the same enum without data, 20,000 times on one stack:

| | Before | After |
|---|---|---|
| 50 variants with data | 9.54 µs | 6.67 µs |
| 50 variants, no data | 2.04 µs | 2.04 µs |
| per variant with data | 150 ns | 93 ns |

A run that publishes what the run before published, value for value, swaps
the new record into the slot in the straight-line loop: no tag copied, no
deep comparison, no general-path builtin call. Data that differs, or that
builds an equal list or record afresh, is compared in depth as before and
moves the revision only if it changed. What is left is building the record
(about 64 ns for three literal fields, measured on its own) and loading the
tag constant (about 20 ns). Neither goes away until the data is not
re-evaluated at all, which is what static values are for
([proposal](../proposals/static-values.md)).

**Kept out of line.** The enum cases in the straight-line loop
(`backend/bytecode/vm/fast.rs`) call `#[inline(never)]` helpers. Inlined,
they made every other instruction slower: `life.ptl` went from 15.0 to
18.5 ms and `audio_synth.ptl` from 104 to 123 ms. With the helpers out of
line the eight `test/benchmarks/*.ptl` programs time as they did before
(minimum of 12 runs: arith 12.3 and 12.4 ms, calls 29.8 and 30.0, life 15.0
and 15.0, audio_synth 110.6 and 109.2, particles 11.2 and 11.3,
match_dispatch 317.8 and 314.6, append 6.1 and 6.4, spreadsheet 205.9 and
213.3). The machine was running other builds during all of these.

## A state hit

A `state` declaration whose slot already exists is a hit: the `StateInit`
instruction finds the slot and jumps over the init block. A hit hashes its
key (declaration id plus call path) four times in a run: the probe and the
insert of the per-run touched set, the lookup in the state table, and the
end-of-run sweep that drops untouched slots. Under the standard library's
SipHash those four were most of the cost. The state table and the touched
set now use their own hasher (`StateHasher` in `core/src/stack.rs`: one
multiply per word and a fold at the end), which is the whole change. Keys,
the touched set, the sweep, memo dependencies and the static audit's state
watch are as they were.

`cd core && cargo run --release --example rerun_timing --
../test/benchmarks/micro/state_hit_top.ptl
../test/benchmarks/micro/state_hit_fn.ptl
../test/benchmarks/micro/state_hit_none.ptl` runs each file 20,000 times on
one kept stack. Median microseconds per run, and nanoseconds per hit (200
hits a run; the control for the top-level file is a two-line program at
2.0 µs, for the function file it is `state_hit_none.ptl`):

| | Before | After | Per hit before | Per hit after |
|---|---|---|---|---|
| 200 top-level `state` | 16.9 µs | 10.9 µs | 74 ns | 44 ns |
| 200 calls of a function with one `state`, in a loop | 57.0 µs | 44.6 µs | 214 ns | 149 ns |
| the same calls, no `state` | 14.3 µs | 14.8 µs | | |

"Before" is one run of the benchmark and "after" the middle of three; the
machine was running other builds. A hit inside a function costs three times
a top-level one because its key has a path: the path is assembled from the
frames on every hit, the key is three words longer to hash and compare, and
it is copied into the touched set.

What was not done: resolving a top-level slot once per program and reading
it by index. The table is a public map that hosts, `--state-storage`, hot
reload's state transfer, `restore_state`, a fork and the sweep all read and
write by key, so an index beside it would have to be kept true through each
of them, for at most the 44 ns that are left. Moving the key into the touched
set instead of cloning it (and probing once instead of twice) was tried and
measured no different.

## Static values

A host runs the whole script every frame, so every top-level table used to be
rebuilt every frame. The static-value cache (`core/src/static_cache.rs`;
`OptFlags::static_cache`, part of `fast` and `replay`) keeps the top-level
values that depend on no dynamic data on the stack and jumps over the code
that builds them. `petal show-static` says which declarations qualify, and
why the others do not; the cache keeps those of them that code running every
frame reads, wherever the stretch of static code they stand in makes at
least one allocation. Each value is built and stored on its own, so a
reload keeps the values the edit cannot have changed and the next run
rebuilds the others and nothing else ([hot-reload.md](../hot-reload.md#static-values-across-a-reload)).
It is off under `baseline` and `explain`, whenever the per-term trace is
on, and for a plain `petal run`. It stays on while observations are on
(every Garden panel): see "Under observation" below.

Over the panel corpus (`cd core-libs/petal-ui && cargo test --release --test
static_cache -- --ignored --nocapture measure`: 55 apps, 60 frames of the
monkey scenario, every frame run, release, 13 October 2026) a frame runs
0.0% to 29.7% fewer instructions, median 4.6%, and makes 1.0% to 46.1% fewer
allocations, median 19.1% (0.74% and 10.51% over the sum of the corpus,
2,940 slots):

| App | Slots | Instructions per frame, off | On | Allocations per frame, off | On |
|---|---|---|---|---|---|
| games/snake | 66 | 2,992 | 2,754 (−8.0%) | 144 | 90 (−37.7%) |
| games/memory | 96 | 30,490 | 21,422 (−29.7%) | 1,430 | 787 (−45.0%) |
| productivity/settings | 55 | 36,727 | 35,536 (−3.2%) | 1,914 | 1,676 (−12.4%) |
| productivity/kanban | 42 | 3,954 | 3,752 (−5.1%) | 184 | 136 (−26.2%) |
| dashboards/analytics-dashboard | 82 | 29,817 | 29,490 (−1.1%) | 2,386 | 2,301 (−3.6%) |
| dashboards/network-graph | 102 | 783,883 | 782,403 (−0.2%) | 1,257 | 1,088 (−13.5%) |
| garden/examples/panels/plant | 33 | 1,066 | 942 (−11.6%) | 82 | 57 (−30.1%) |

The saving is a fixed amount per frame (the tables), so it is a large share
of a small panel and noise in one that simulates or draws a lot. Every UI app
keeps at least 11 slots, which are the UI prelude's own tables. Where a
panel's tables are derived from the screen size, the theme or text
measurement, nothing is kept: those are not static.

A second effect: the memo compares a call's arguments structurally within
a budget of 512 nodes, so a larger table rebuilt every frame made the call
miss every frame. Kept, it is the same object and compares equal at once: a
400-record `let` table as a memo argument goes from 29 misses in 29 frames
to none.

Costs. A recompile classifies the program (48 ms on a 34,000-line game of
192,000 terms, 14 ms on one of 78,000, 3 to 5 ms on an example panel;
`core/examples/reload_timing.rs` prints it) and runs the pass; nothing is
classified on a reload that only moves positions or changes literals. The
answer is kept beside the program (`Program::static_analysis`) by whichever
asks first, the `const` check of a compile or the lowering, and serves the
other, the audit and `Env::static_report`, so that is once per recompile.
Per
frame, one test per stretch of static code plus one register write per slot
that something outside the stretch reads. The frame that builds everything
(the first, and the one after a reload that cleared the table) pays two
instructions more per stored value, a test in front and the store behind:
933 on the 8,284 of Moses's static code, where one store per stretch was 68. With the cache off the cost is two comparisons per
dispatch into the VM and one more bit tested in the in-place heap
operations: the `test/benchmarks/*` scripts under `petal run` time within
1.2% either way of the same source with both compiled out, and their
instruction counts are unchanged.

**Under observation.** A host that reads `Env::get_observations_json`
after each frame must read the same thing from a frame that skipped a
segment as from one that ran it. Each segment keeps a log of what was
observed while it last executed (the last value of each named term, the
locals of the functions it called included) and a taken skip records the
log again, as a memoized scope does with `Dep::Observed`. So a skip costs
one hash insert per logged name where the segment cost one per named
instruction plus the instructions, and the values in a log are held for as
long as the segment is kept. Garden's own panels under Garden's host
(`cd garden && cargo test -p garden-script --lib -- --ignored --nocapture
measure_static`: every example panel and GPP drawer, 60 frames, every frame
run, no input and no host data, 13 October 2026) run 2.4% to 17.8% fewer
instructions a frame and make 9.8% to 28.9% fewer allocations (6.6% and
18.4% over their sum):

| Panel | Slots | Instructions per frame, off | On | Allocations per frame, off | On |
|---|---|---|---|---|---|
| examples/panels/plant | 33 | 1,042 | 918 (−11.9%) | 107 | 83 (−22.8%) |
| examples/panels/sketch | 17 | 1,206 | 1,110 (−8.0%) | 88 | 66 (−24.9%) |
| examples/panels/gallery | 14 | 4,911 | 4,767 (−2.9%) | 294 | 257 (−12.6%) |
| examples/panels/layers | 24 | 1,840 | 1,704 (−7.4%) | 125 | 94 (−25.1%) |
| gpp-apps/git-viewers/git_panel | 22 | 2,572 | 2,437 (−5.2%) | 211 | 179 (−15.2%) |
| gpp-apps/garden-diff/garden_diff | 21 | 1,290 | 1,179 (−8.6%) | 99 | 74 (−24.8%) |
| gpp-apps/sqlite-browser/db_view | 20 | 2,037 | 1,916 (−5.9%) | 186 | 156 (−16.2%) |
| gpp-apps/main-menu/main_menu | 14 | 5,546 | 5,411 (−2.4%) | 268 | 242 (−9.8%) |

"Off" is what every panel ran before. The drawers have no data here, so
their frames are small and the share is larger than a populated pane would
show; the saving itself is the fixed 100 to 150 instructions and 22 to 41
allocations of the tables. Over the 55-app corpus with observations on
(`measure_observed` in the same test file as above) the sum is 0.65% fewer instructions and 11.04% fewer allocations, 2,945
slots.
What observation costs the cache: a statement that edits in place a
container another statement built is not skipped, and after a reload that
recompiles each kept segment executes once more to write its log
([hot-reload.md](../hot-reload.md#static-values-across-a-reload)).

**Enum constant data** is a static value too, where its expressions are
static. `enum Kind  park(code: 2) end` publishes one record per variant by a
call the declaration makes on every run; with the cache on that call is
static code like any other, the record it publishes has a slot, and a frame
that skips it leaves the record the first run published where it is.
On `test/benchmarks/micro/enum_decl_data.ptl` (50 variants, three keys each,
`rerun_timing`, release) a run costs 7.0 µs with the cache off and 2.5 µs
with it on, against 2.1 µs for the same enum without data: about 9 ns a
variant, down from about 100, which is loading 50 slots into registers.
Data that is not static (a function that reads `state`) is published on
every run as before.

**A script whose host is not here.** A game written for an engine cannot run
under this repository's binary: its dynamic code calls natives nothing here
registers. Its static declarations can, since static code calls only
constant natives. `cd core && cargo run --release --example static_frame --
game.ptl [frames]` runs the static-only build (`petal run --static-only`:
every statement that is not static is jumped over) on one stack with the
cache off and then on, and prints the instructions and allocations of the
first frame and of a later one. The difference between the two later frames
is exactly what the cache takes out of every frame of the real program,
since the dynamic statements cost the same on both sides. It says nothing
about what the dynamic code then gains from a kept table (a memo hit, less
to trace in a collection). Moses, 27,000 lines, release, 13 October 2026:

| | Instructions, static part of a frame | Allocations | of them records |
|---|---|---|---|
| As written, cache off | 8,065 | 1,230 | 482 |
| As written, cache on | 2,641 (−5,424) | 45 (−1,185) | 1 (−481) |
| The 74 constant `state` cells as `const`, cache off | 25,804 | 4,687 | 1,383 |
| …cache on | 2,557 (−23,247) | 40 | 1 |

(451 slots behind 42 checks as written, 548 behind 54 with the 74 cells.)
The first frame after a load builds everything and costs what it does with
the cache off (8,284 instructions as written, 35,195 with the 74 cells)
plus the tests and stores: 933 and 1,135. The first frame after a reload
builds what the edit reached: 2,642 instructions after an edit to dynamic
code and 2,681 after an edit to one table, against 9,218 when every value
is rebuilt (2,558 and 2,600 against 36,331 with the 74 cells;
`core/examples/reload_follow.rs`, numbers in
[hot-reload.md](../hot-reload.md#what-it-costs)). What is left on a later
frame is the function declarations (a closure is made on every run) and the
stretches that allocate nothing.

To see what a script would keep without running it: `PETAL_OPT_STATS=1
PETAL_POLICY=fast petal show-bytecode app.ptl >/dev/null` prints the
stretches and the segments they are cut into, the slots by declaration, and
the root instructions a frame leaves out. Tables that are written to are
never kept (`let by_sec = [0, 0, 0]` followed by `by_sec[i] = v`). A
function declaration ends a stretch, since its closure is made on every
run where it stands; a table sitting alone between two of them is kept all
the same, at one check a frame. Numbers standing alone are not: loading
one costs what the check does.

**What a program can tell.** The cache is held to printing, returning,
raising and storing exactly what the same program does with it off, and one
builtin is outside that: `frozen` answers about the heap object, not the
value. A kept table passed to `freeze` is frozen once and is still frozen at
the top of the next run; rebuilt on every run it was a new unfrozen table
each time (and a leak of one table per run). A constant string and a
payload-free variant have always behaved the kept way. The test is
`freezing_a_kept_value_freezes_it_once` in `core/tests/static_cache.rs`.

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
- **Memo scopes cost a recursive program most of all.** A record is as large
  as its call path is long, and none of a recursion's records is replayed
  within the run that made them. `petal run` therefore leaves memoization off
  unless a policy is named, and a host that keeps it on is bounded: no scope
  past 96 frames deep, and a table of at most 96 MB of records
  ([memo-scopes.md](memo-scopes.md), "Bounds"). Release build, peak RSS:

  | Program | before, `fast` | `petal run` now | `--policy fast` now |
  |---|---|---|---|
  | `fib(27)` | 541 MB, 0.45 s | 4.6 MB, 0.04 s | 131 MB, 0.25 s |
  | `fib(30)` | not measured | 4.7 MB, 0.18 s | 133 MB, 0.96 s |
  | linear recursion, depth 4,900 | 479 MB, 0.26 s | 7.4 MB, <0.01 s | 7.9 MB, <0.01 s |
  | linear recursion, depth 10,000 | over the call limit | 10 MB, <0.01 s | 10.6 MB, <0.01 s |

  `test/vitest/recursion-memory.test.ts` holds `fib(30)` and the depth-10,000
  case under a memory ceiling.
- **`state` inside a deep recursion is quadratic.** A slot is keyed by its
  call path, so a function that declares `state` and recurses `d` deep keeps
  `d` keys of up to `d` parts: 480 MB at depth 4,990 (about 1 GB with
  memoization on). `MAX_STATE_CALL_DEPTH` stops path-keyed state at 5,000
  frames for that reason, while calls without it may nest to
  `MAX_CALL_DEPTH` (20,000). Interning paths would fix both this and the size
  of a memo record.

And one that is not the runtime's to fix: the spreadsheet's own `digit_val`
classifies a character by slicing a 10-character string ten times, which is why
`slice` accounts for 83% of its builtin calls. A script's algorithm is still the
biggest lever it has.

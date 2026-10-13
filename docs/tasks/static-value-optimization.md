# Static value optimization: plan

Status: S0 (the `CONST` flag, the classifier, `show-static`, `--static-audit`)
is implemented and the go/no-go gate **passed** on 12 October 2026: S1 may
start. See the status note under each section for the rest.

Design and evidence: [`docs/proposals/static-values.md`](../proposals/static-values.md).
This file is the work order. Where the two disagree, the proposal's design
sections win and this file's order and gates win.

## Goal

A value that depends on no dynamic data is built once, kept between runs, and
rebuilt after a hot reload only when the source diff touches something it
depends on. The reason is live coding (constant tables parked in `state` do
not hot-reload), not speed: the measured gain is about 5% of instructions on
Moses and 0 to 3% on the example panels.

## Decisions made

| Question | Decision |
|---|---|
| Explicit `const` on top of automatic detection | Yes. `const NAME = expr` is a `let` that must be static; otherwise a compile error with the reason chain. |
| Stage 0 as a go/no-go | Yes. If the classifier marks much less of the corpus static than the survey's estimate, fall back to explicit `const` with a per-declaration hash and no interprocedural analysis (option C in the proposal). |
| Order | The enum read path and the frozen heap generation go first. They are independent of static values and each delivers more measured speed. |

Still open, to settle before the stage that needs them:

- Eager or lazy top-level builds (stage 1). The proposal says eager.
- Re-seed a never-assigned `state` cell whose initializer read a changed
  constant, or only report it (stage 2). The proposal only reports.
- A new `CONST` native flag, or tighten `PURE` (stage 0). The proposal says a
  new flag.
- Whether doing nothing in Garden panels is acceptable for the first version
  (stage 1).

## Order of work

| # | Work | Depends on | Size |
|---|---|---|---|
| A1 | Enum data read path | — | small (150–300 lines) |
| A2 | Frozen heap generation, upstreamed from Cheesecake | — | medium |
| A3 | Cheaper `state` hit (optional) | — | small |
| S0 | Classifier, `show-static`, `CONST` flag, audit. No behaviour change | — | large |
| — | **Go/no-go gate** | S0 | — |
| S1 | Slots, skip lowering, guard bit, `const`, lint, clear-all on reload | S0, gate | large |
| S2 | Diff-driven dirtying and the reload report | S1 | medium to large |
| Later | Garden observation replay; constants inside functions; per-input-class slots; keeping slow intermediates | S2, and a profile that asks for it | — |

A1, A2 and A3 touch different files and can run in parallel. S0 can start
alongside them.

## A1. Enum data read path

**Status: done 2026-10-12** (items 1 to 4). Commits `20836ee` (benchmarks),
`aa36342` (the change), `f9315d7` (the straight-line loop's enum work moved
out of line) and the documentation commit after them.

What shipped:

- **Item 1, the dense index.** `enum_table::EnumLayout` numbers every
  declared variant at lowering (a slot; one enum's variants are a contiguous
  run) and the bytecode carries the slot of every constant that is a
  variant's tag. `Kind.variant` is built from its slot. One departure from
  the wording above: the index is not stored in the variant *value*. A value
  is still only its tag and payload (`Value::EnumVariant` is unchanged, for
  hosts and for the three JSON spellings), and a read finds the slot from
  the tag's interned id through an exact integer index the stack's table
  keeps. The table roots the tag strings, so an id it holds can only be that
  variant. This costs one probe more than a stored index would and needs no
  rewrite of values after a reload; a tag the index does not hold (a host's
  free-form tag, a variant an edit removed, a tag too long to intern) is
  resolved by its text, as before.
- **Item 2.** `Stack::enums` (`enum_table::EnumTable`) replaces the
  string-keyed `Stack::enum_data`: data records in one vector by slot.
- **Item 3.** `Kind.all` compiles to the `__enum_all` intrinsic, which
  builds the list once per stack (it does not depend on the data, so "per
  data revision" was not needed) and hands the same list out after. It is
  not on `returns_fresh_container`'s list, so neither in-place analysis
  roots a rewrite on it, and a `var` cell does not own a value it is handed.
  `all_is_one_list_that_nothing_can_write_through` holds this with the
  optimizer on and off.
- **Item 4.** A run that publishes what the run before published, value for
  value, swaps the record in without the tag `to_string`, the deep compare
  or the general-path builtin call. The by-id case the item names never
  occurs today (each run builds a new record); it is covered and will start
  to hit when S1 keeps the record.
- Also: a recording memo scope notes the enum-data dependency once per run
  of reads and no longer sends every read to the general path, which is what
  a read inside any memoized function was paying.

Measured (`docs/dev/performance.md`, "Enum data reads"; this machine, under
load from the other tracks' builds, minimum of three `--iters 30` runs,
nanoseconds over the empty loop):

| | Before | After |
|---|---|---|
| record field | 1.7 | 1.8 |
| `k.code` | 51.3 | 3.5 |
| `K.v.code` | 71.3 | 6.6 |
| payload field `op.dy` | 33.9 | 11.5 |
| `K.all` (8 variants) | 227.8, a list allocated per read | 18.8, no allocation |
| declaring 50 variants with data, per run | 9.54 µs | 6.67 µs (2.04 µs without data) |

The baselines below (66 / 41 / 173 ns, 13.6 µs) were taken with a different
script on a quieter machine; the "Before" column is the recreated benchmark
on a binary built when this work started (`7e7a7c5` plus whatever the other
tracks had in the tree at that minute, none of it on the enum path).

Not done, and why:

- **The declaration is cheaper, not cheap**: 93 ns per variant, down from
  150. What is left is building the record and loading the tag constant, on
  every run. Only not re-evaluating the data removes it, which is S1.
- **A payload field read is 11.5 ns**, not within a few nanoseconds of a
  record field. It is found by comparing the field name against the
  declared payload names; the Done-when names only the data reads.
- **`K.all` is 18.8 ns**, most of it interning the enum-name constant that
  is the builtin's argument (every string literal load interns).

Review (2026-10-12), after the commits above:

- **Fixed: a loop that rewrites a list starting from `K.all` copied on
  every iteration**, because its root was no longer a fresh allocation.
  The corpus does have one: the Fisher-Yates bag in
  `examples/games/tetris/tetris.ptl` (`let bag = Piece.all`, then
  `bag[i] = …` in a `while`), 12 copies a shuffle. A `K.all` read that is
  the init of a phi now hands out a copy (`escape::Analysis::copies_enum_all`
  marks it, the `BuiltinCall`'s `in_place` flag carries it, the VM copies),
  which is a fresh root, so the loop is in place again. 100 appends from
  `K.all`: 4.83 µs before A1, 13.6 µs after it, 4.75 µs now; the tetris
  shuffle 1.29 / 1.33 / 1.17 µs. Every other read still shares the list.
- Verified by re-running: the read benchmark (record 1.7, `k.code` 3.8,
  `K.v.code` 6.6, payload 11.4, `K.all` 17 ns over a 22 ns loop; no
  allocation per `K.all`), the declaration timing (6.42 µs / 2.04 µs),
  `petal check` output on `K.all` uses against the pre-A1 binary
  (identical), `cargo test` in `core` (57 suites) and `core-libs/petal-ui`,
  vitest (1566) and `./tools/test-examples.ts` (33), all passing.
- Two tests added to `core/tests/enums.rs`: every way of rewriting a list
  that began as `K.all` (wrapper function, index write, record field,
  parameter, nested list, `state`; four frames, optimizer on and off), and
  the accumulator above with its allocation count.
- Still not done: Garden, `petal-c-bridge` and the wasm build were not
  built (no user of `Stack::enum_data` or the changed `ops` signatures
  outside `core` by grep); Moses was not re-measured (Cheesecake vendors
  Petal); the payload read and `K.all` figures are as above.

For S0/S1: `Kind.all` is now a `BuiltinCall("__enum_all")` in the IR, not
an `AllocList` of `MakeEnumVariant`s. Like `__enum_from` it is a `PURE` row
and not `CONST`, because it answers from the program's enum table; with the
constant enum name the compiler writes, it depends only on the enum's
declaration, which is how the classifier in `static_classify` treats both.
A static value that keeps `K.all` keeps the stack's shared list, which is
fine for the same reason a `let` may: nothing writes through it.

Tests: twelve cases added to `core/tests/enums.rs` (host-built variants with
free-form and with declared tags, a tag past the interning limit, a variant
in `state` through every row of "Editing an enum" including a reorder,
the revision under constant and dynamic data, memo records over reads in a
loop, the shared `all` list, two stacks and a fork, switching the optimizer
between runs). `cargo test` in `core` and `core-libs/petal-ui`, vitest, and
`./tools/test-examples.ts` (optimizer on and off) pass.

**Problem.** A field read on a variant (`k.code`) costs about 41 ns with `k`
in a local, 66 ns as `K.v.code`, against 0.8 ns for a record field. `K.all`
costs 173 ns and allocates a list on every read. Declaring enum data costs
about 220 ns per variant on every run. This is why Moses keeps derived
`T`/`F`/`USE` int tables beside its enums
(`~/cheesecake/games/moses/config.ptl:128-153`).

**Cause.**

- `enum_variant_field` (`core/src/backend/ops.rs`) splits the tag string,
  does two string-hash lookups in `program.enum_variant(tag)`, then
  `enum_data.get(tag)` on `Stack::enum_data: HashMap<String, MapId>`
  (`core/src/stack.rs:118-131`), then an uncached map get.
- `K.all` lowers to one `enum` instruction per variant plus a `list`.
- Each run calls `__enum_data` per variant: a map allocation,
  `tag.to_string()`, and a deep compare against the last run's record
  (`declare_enum_data`, `core/src/backend/bytecode/vm/native.rs:235-249`).

**Work.**

1. Give each variant a dense index within its enum, resolved at compile time
   and carried where the tag is today, so a read does no string work.
2. Store enum data in a per-enum vector indexed by variant, not a
   string-keyed map.
3. Build `K.all` once per enum per data revision and reuse the list. It must
   be treated as non-fresh by the in-place analyses, or guarded, so a caller
   cannot mutate the shared list.
4. Make the per-run declaration cheap: skip the deep compare when the
   variant's data record is identical to the last run's by id, and avoid the
   per-run `to_string`.

**Not in A1.** Folding a literal `Enum.variant.key` to a constant at
lowering. A literal edit in enum data is a patched reload that recompiles
nothing, so folded copies at use sites would go stale unless the patcher
learns about them. Once S1 exists, enum data becomes a static value and this
is revisited there. (Revisited 2026-10-12 and still not done: see "S1, finishing
and the gate result".)

**Constraints.**

- Enum data may legally be dynamic today (`Dep::EnumData` exists because "an
  expression in it may answer differently each time"). Keep
  `enum_data_revision` and the memo dependency working.
- Variants built by hosts (`pb_builder_begin_enum`, `PetalCxt::emit`) carry a
  free-form tag with no declared enum. They must keep working.
- A variant held in `state` across a reload that edits its enum must behave
  as documented in `docs/hot-reload.md` ("Editing an enum").
- The three JSON spellings of a variant (headless trace, `--state-storage`,
  static value) stay as they are; the index is not serialized.

**Done when.** With the benchmark below, `k.code` and `K.v.code` are within a
few nanoseconds of a record field read, `K.all` does not allocate, and
`cargo test`, vitest and `./tools/test-examples.ts` pass with the optimizer on
and off.

**Baselines.** `petal bench --iters 30` on the script at
`<session scratchpad>/const-survey/a.ptl` (recreate it under
`test/benchmarks/` as part of this work): 66 ns / 41 ns / 173 ns over a
7.4 ns empty loop. Declaration: 13.6 µs per run for 50 variants with data
against 2.7 µs without.

**Follow-up in Cheesecake.** After re-vendoring, Moses can drop `T`, `F`,
`USE` and `F_PEOPLE` in favour of reading the variant's data directly, and
re-measure.

## A2. Frozen heap generation

**Status: done 2026-10-12** (items 1 to 4; item 5 is Cheesecake's).

What shipped (`fd1b18d` and the documentation commit after it):

- The patch, ported by hand: `Heap::freeze` / `is_frozen`, the frozen bit on
  every slab slot, the marker and sweep skipping it, the seven in-place
  operations copying, `collection_cost` and `mark_map` as described below.
  Added here: `Heap::frozen_counts` (a leak diagnostic). Cheesecake's five
  unit tests are in `heap.rs` with three more (fork, budget, all-or-nothing),
  and `core/tests/frozen_heap.rs` runs twelve script-level cases with the
  optimizer on and off.
- `freeze` / `frozen` are registered in the builtin table (after the enum
  intrinsics, where the patch had them), declared in `core/packages/core/src/collections.ptl`, documented in
  `docs/Builtins.md` ("Frozen Data", with the limits below), and in
  `docs/stdlib.json`.
- Effects: both are `NativeEffects::EFFECT` rows and neither is `CONST`
  (`frozen` answers from heap state, not from the value's contents). In
  `builtins/effects.rs`, `freeze` has a row with no property (not `FRESH`,
  not `NO_REF`, not `PURE`) and `frozen` is `NO_REF` only, both pinned by
  tests. Passing a value to `freeze` therefore ends its unique ownership, and
  no script shape tried reaches an in-place write on a frozen value; the
  run-time copy in the heap is the backstop.
- Fork and restore: `Heap::fork` clones the slots, so the frozen bit travels;
  `inherit_generations` does not touch it. Tests:
  `heap::tests::a_fork_carries_the_frozen_bit` and
  `frozen_heap::fork_and_restore_carry_the_frozen_set`.

A review pass the same day added three things (heap.rs, with tests):

- **The sweep no longer reads frozen slots.** Each slab keeps one bit per
  slot beside the slot's own flag (`Slab::permanent_bits`); the sweep and
  `collection_cost` read a 64-slot word of it and skip the frozen slots
  unread. This is what makes the pause independent of the frozen table; it
  did not need the "frozen generation that can release objects" under Later.
- **`freeze` lowers the budget in force** by the frozen objects' share. A
  table built, collected and then frozen used to leave the collector waiting
  for a table's worth of garbage before its next run (one long pause, and
  slabs grown to hold that garbage, walked by every later sweep).
- **`frozen(variant)` checks the tag as well as the payload.** Every
  payload-free variant shares one payload list, so after any frozen value
  held one, `frozen` answered true for all of them.

Measured (one machine, under load from other builds; `docs/dev/performance.md`
has the tables):

| | Before | After, rooted | After, frozen |
|---|---|---|---|
| Pause per collection, 100,000 records, 2,000,000 short-lived records allocated (`tools/bench-gc-pause.ts`) | 30.0 ms (3 collections, 90 ms) | 13.5 ms (5 collections, 68 ms) | 0.24 ms (385 collections, 93 ms) |
| The same with 10,000 records | 9.5 ms (7 collections, 66 ms) | 1.10 ms (42 collections, 46 ms) | 0.11 ms (385 collections, 42 ms) |
| One collection, 100,000 records, nothing to free (`cargo run --release --example gc_pause`) | — | 2.13 ms | 0.009 ms |
| The same with 400,000 records | — | 9.21 ms | 0.034 ms |

The "Before" column is the implementer's run at `42142fd`, taken under
heavier load than the other two; the plan's own baseline below (10.0 ms and
1.06 ms) came from a scratch script that is gone, and the recreated benchmark
has three heap objects a record and a heavier allocation phase. The rooted
column moved without freezing because of the `collection_cost` change.

Against the "done when":

- **The frozen pause no longer scales with the table.** One collection with
  nothing to free costs 9 µs with 100,000 frozen records (a scan of the bit
  index, 1/240 of the rooted cost). In the script benchmark the frozen runs
  collect on the same schedule at both sizes. The remaining 0.11 ms against
  0.24 ms there is not frozen data: building each row's name leaves a dead
  temporary string, so 100,000 *free* string slots sit between the frozen
  ones and the sweep walks free slots. With the name a literal both sizes
  measure about 0.12 ms.
- **Total collection time is about the same frozen or not**: it is what
  freeing the garbage costs. Freezing shortens pauses; it does not buy
  throughput. `SLOT_WALK_COST` is left at 8, as Cheesecake tuned it.
- **Left for later, on evidence:** a sweep that does not walk free slots (it
  rebuilds the free list from them on every collection). It is independent
  of freezing and changes slot reuse order, which `inherit_generations`
  relies on.

Not done: Cheesecake's 344-test suite on a re-vendored tree (item 5, in
their repository). For that re-vendor: the local modification drops out
whole, `VENDORED.md` loses its entry, and
`engine/script/packages/cheesecake/memory.ptl` no longer needs to declare
`freeze` / `frozen` (the stock `core` package does).

**Problem.** Every collection re-marks all rooted long-lived data. A rooted
100,000-record table costs about a 10 ms pause per collection; Moses pauses
about 7 ms every 43 frames. Static values will keep more data alive.

**Source.** Cheesecake carries this as a local patch, described in
`~/cheesecake/third_party/petal/VENDORED.md` under "Local modifications":

- `Heap::freeze` / `Heap::is_frozen` in `core/src/heap.rs`, and builtins
  `freeze(v)` / `frozen(v)` in `core/src/builtins/{mod,collections}.rs`, with
  declarations in `core/packages/core/src/collections.ptl`.
- The marker never traces a frozen object and the sweep never frees one.
- The in-place list, map and f64-array operations copy when the target is
  frozen.
- A value holding a closure or a `var` cell cannot be frozen.
- `collection_cost` counts only live, non-frozen objects plus a small
  per-slot walk cost; `mark_map` no longer allocates.
- Freezing is transitive, including held vec3s and the shared empty enum
  payload.

Their result: GC pauses went from about 25 ms every 100 frames to about 2 ms
every 5 frames.

**Work.**

1. Port the patch upstream with its unit tests (they sit at the end of
   Cheesecake's `heap.rs`).
2. Document `freeze` and `frozen` in `docs/Builtins.md` and regenerate
   `docs/stdlib.json`.
3. Give both builtins effect rows and add them to
   `core/src/builtins/effects.rs` so escape analysis never treats a frozen
   value as a fresh root.
4. Check the fork and restore paths (`core/src/env/fork.rs`): a deep-copied
   heap must carry the frozen bit.
5. Cheesecake drops its local modification at the next re-vendor and updates
   `VENDORED.md`.

**Limits to state in the docs.** Freezing is one-way: there is no unfreeze
and a frozen object is never freed. It suits data that lives as long as the
program. It is **not** the mechanism S1 uses to protect cached values,
because a rebuilt static value must be released; S1 uses a separate
immutable bit that leaves tracing and sweeping alone. A frozen generation
that can release objects is later work.

**Done when.** The ported tests pass, a benchmark holding a 100,000-record
frozen table shows the per-collection pause no longer scales with it, and
Cheesecake's 344-test suite passes on the re-vendored tree with the local
patch removed.

**Baselines.** `<session scratchpad>/critic-perf/h_gc100000.ptl` (recreate
under `test/benchmarks/`): 10.0 ms per collection with 100,000 rooted
records, 1.06 ms with 10,000.

## A3. Cheaper `state` hit (optional)

A `StateInit` hit costs about 72 ns: it builds a path key, calls
`touch_state`, does a hash lookup and notes a memo dependency
(`core/src/backend/bytecode/vm/dispatch.rs:593-611`). Moses runs 406 per
frame, about 1% of its frame. A top-level `state` has an empty call path, so
its slot could be resolved once per program and read by index. Do this only
if it falls out simply; static values remove the 74 constant cells from that
count anyway.

**Status (2026-10-12): the simple half shipped; the slot index was not built.**

Measured first, with `test/benchmarks/micro/state_hit_top.ptl` (200 top-level
`state`), `state_hit_fn.ptl` (one `state` in a function called 200 times from
a loop) and `state_hit_none.ptl` (the control), under
`core/examples/rerun_timing.rs` (20,000 runs on one kept stack, medians):
a top-level hit cost 74 ns, which agrees with the 72 ns above, and a hit
inside a function 214 ns.

Where it went: a hit hashes its `RuntimeStateKey` four times a run (the
touched set's probe and insert, the table lookup, the end-of-run sweep), all
with SipHash. The state table and the touched set now use a small
multiplicative hasher (`StateHasher`, `StateMap`, `StateKeySet` in
`core/src/stack.rs`). After: 44 ns at the top level, 149 ns in a function.
No state instruction changed, so `touch_state`, the memo dependency notes,
per-call-path keys, the sweep, state transfer, `--state-storage`,
fork/restore and the static audit's state watch are untouched; the full
`core` test suite passes in debug. The public surface moved slightly:
`snapshot_state` and `get_all_state` return `StateMap`, and `restore_state`
takes any iterator of slots so a caller's plain `HashMap` still compiles.
Figures and method are in `docs/dev/performance.md` (A state hit).

Not built, and why: resolving a top-level slot once per program and reading
it by index. `Stack::state` is a public map written by key from the sweep,
`transfer_state`, `state_storage`, `restore_state`, `set_state`, forks and
hosts; an index beside it has to be invalidated or rebuilt by every one of
them, and S2 is editing the reload path now. That is not "falls out simply",
for at most the 44 ns per hit that remain (406 hits on Moses: under 18 µs of
a 2.71 ms frame, and static values take 74 of those cells away).

Also tried and reverted: moving the key into the touched set at the end of
`StateInit`/`StateRead` instead of cloning it. No measurable difference.

Open:

- Moses was not re-timed. The expected gain is about 12 µs a frame (0.4%),
  below what a headless run resolves, and Cheesecake vendors Petal, so it
  sees nothing until re-vendored.
- The in-function hit is still 149 ns. What is left is assembling the path
  from the frames on every hit and copying the key into the touched set; a
  per-frame cached path hash would be the next thing to try, on evidence.
- The first figure of each pair is a single run of the benchmark before the
  change, on a machine running other builds.
- Other crates in this repository were not rebuilt (disk). None names the
  map's type or calls `snapshot_state`/`get_all_state` (grep), so none should
  be affected.

## S0. The classifier, with no behaviour change

**Status: done 2026-10-12, gate passed (go).** Step 1 was the `CONST` flag and
the audit of the `PURE` rows, step 2 the classifier and `petal show-static`,
step 3 `--static-audit` and the gate. Nothing reads the classifier's answer
yet, so there is still no behaviour change. The gate's numbers are under
[Gate result](#gate-result); step 3's notes come first, then step 2's, then
step 1's.

Step 3, `--static-audit` (`core/src/static_audit.rs`, `Env::set_static_audit`
/ `Env::static_audit_report`, `petal run --static-audit` / `--static-only`,
the same two flags on `petal-ui-run`, `PETAL_STATIC_AUDIT=watch`):

- **What it watches.** The VM knows which root instruction is in progress
  (the root frame's, however deep the call it made). While that
  instruction's term is one the classifier called static, a finding is
  recorded for: a change in any activity counter around a native call
  (binding read, by name; host-data read; resource read; emit; effect), the
  random stream moving around a native call, any `state` instruction in any
  frame, and any call of a native whose `constness` is `Dynamic` (so `noise`,
  `font`, `fn_info`, which move no counter, are caught too; `__enum_from` and
  `__enum_all` are exempt because the classifier answers them structurally).
  A finding names the declaration whose initializer was running, the input,
  the native and the call site.
- **Two modes.** `Watch` runs the program as it is and audits every static
  term, including one inside a dynamic top-level loop or branch.
  `StaticOnly` rewrites the lowered root code so that every top-level
  statement that is not static is jumped over, and runs what is left: a build
  of the static declarations alone. Static code calls only constant natives,
  so it needs no host, which is how Moses is audited from the plain CLI. A
  build that raises is a finding.
- **Found while building it, for S1.** The root code is *not* one contiguous
  range per top-level statement: the lowering materializes a constant where a
  later statement first uses it, so a statement can be several segments with
  another statement's instruction between them (Moses has these). The slice
  works per segment. S1's run splitting has to expect the same.
- **Cost when off.** One branch per native call and per `state` instruction.
  Measured in review against the same source with the hook compiled out
  (release, minimum of 12 alternating runs): 6 million builtin calls in a
  loop 342.0 ms against 338.2 ms, about 0.6 ns a call; a million in-function
  `state` reads and writes 673.8 ms against 671.8 ms; `particles`, `calls`,
  `life`, `spreadsheet` and `audio_synth` within ±2% either way, which is
  the noise of the machine.
- **What it cannot see.** A read of a `var` cell or of a value another
  statement updates in place moves no counter; a native that answers from
  host state without reporting it is as invisible as it is to the effect
  audit; a call the memo replays is not made (audit with memoization off).
  The value comparisons below cover part of that.
- **Tests.** `core/tests/static_audit.rs` (8: natives registered `CONST`
  that read a binding, read host data, draw from the random stream or print
  are each caught and named, in both modes; a static term inside a dynamic
  loop; the static-only build of a script whose host natives nothing
  registers; a build that raises; turning it off), `static_audit_env.rs`, a
  CLI test, and `core-libs/petal-ui/tests/static_audit.rs` over the 55-app
  panel corpus in both modes.

### Review of S0 (2026-10-12)

An independent pass over the three steps after their commits. The gate
stands.

- **Reproduced.** Moses 2,356 declarations, 517 static, 493 of 1,368
  functions static-pure; Moonbear 1,222 and 378; every named case; the
  static-only builds of both games with 0 findings; the 55-app corpus tests
  in both modes and the value-for-value comparison; core's classifier, audit,
  CLI and native-row tests.
- **The static-only build with the optimizer off.** `petal run --policy
  baseline --static-only` over the 297 `.ptl` files of `examples/`, `garden/`
  and `test/` gives the same answer as the default policy: 280 clean, the 2
  deliberate error fixtures, 15 the plain CLI refuses to compile. The slice
  works on unoptimized lowering too.
- **Tried to get a moving value past the classifier** through the mechanisms
  the first leak test leaves out: a default parameter, `state` declared
  inside a function, a function called where it is written, one picked by a
  match or out of a list, an early `return`, nested declarations, a dynamic
  native held in a name, a higher-order builtin called as a method, an
  optional read, a user `to_string` / `eq` / `lt` method (nothing dispatches
  to these implicitly), a module's function and table that read its own
  `state`. None got through. Eleven of them are now a second leak test
  (`nothing_classified_static_moves_between_runs_part_two`), each checked to
  really move over ten runs.
- **No term is left unclassified.** The fixpoint turns whatever is still at
  its optimistic start into data at the end, which would be a hole if a term
  were never visited. Counted on Moses, Moonbear, `plant.ptl` and the
  settings app: zero such terms.
- **Fixed.** A function that reads a name declared below it compiles to an
  error term under the check-only compile `show-static` uses; the reason was
  "has a compile error" with no message. It now carries the compiler's
  message. The audit's snapshot of the random stream is read only while the
  audit is watching. A test holds that a host call made after the run is not
  attributed to the script's last static statement.
- **For S1, the limit of `mutated_by`.** It follows a declaration through
  copies and phis at the top level only. It does not follow the value into a
  user function that writes its parameter (`fn bump(xs) xs[0] = … end`,
  `bump(TABLE)`), nor from an element to its container (`let INNER =
  NEST[0]`, `push(INNER, v)` flags `INNER`, not `NEST`). Both are correct
  today because the caller's value is copied before the write; under S1 that
  rests on a slot load being a non-fresh root and on the guard bit, exactly
  as the proposal says. `mutated_by` is the cheap first filter, not the
  guarantee. Widening it to parameters would also take the slot away from
  every table handed to a "return a changed copy" helper.
- **Still open** (none of it blocks S1): the release-profile failure of
  `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` was not
  bisected (no second target directory was allowed); Moses has not run under
  its own host; a read of a `var` cell under a static term is invisible to
  the audit and is covered only by the value comparisons; `freeze` is an
  effect, so a `let` that freezes its table is dynamic (37 `freeze` calls in
  the Cheesecake games, most on `state` or `var` caches); a function
  passed to a user-written higher-order function is dynamic.

### Gate result

Run 2026-10-12 with this repo's release `petal` (commit `7a7a9cb`).
**Go: every row of the gate holds.**

| Measure | Survey estimate | Measured | Result |
|---|---|---|---|
| Top-level tables and derived values classified static | 1,580 | **1,693** static non-scalar `let`s (1,159 whose initializer starts with a `[` or `{` literal, 534 other derived values), out of 4,720 static `let`s; 3,027 more are scalar literals | pass |
| …specifically the constructor-built tables | — | every constructor in the named cases is static-pure and every table built with them is static; no class of constructor-built table is dynamic for a reason the classifier could fix (breakdown below) | pass |
| Moses's 74 constant `state` cells, if rewritten as `let` | all plausibly static | **74 of 74** static in a scratch copy with the 74 rewritten (73 of 74 by `initializer_static` on the game as written; the 74th, `TIMELINE`, sorts `RAW`, which is static once `RAW` is a `let`) | pass |
| Named cases | — | all static: `T = codes(TILES)` (`codes` static-pure); `hd`, `st`, `wk` static-pure and `HOODS_SRC`, `SITES_SRC`, `WORKS_SRC` static as `let`s; `TOOLS` static; `ch` static-pure with `C_MAJ`…`E_MIN` and `THEME_CHORDS` static; `build_glyphs` static-pure and `GLYPHS` static as a `let` | pass |
| Audit | — | **0** static terms touched a dynamic input, over everything listed below | pass |

**The sweep.** `petal show-static --json` on each of 744 `.ptl` files as an
entry file (the survey scanned 684): `examples/` 150, `garden/` 27, `test/`
120, the rest of this repo 103, `~/cheesecake/games` 231, `~/worlds-fair/ui/ptl`
19, `~/.garden` 72, petal-lang.org, the experiment apps and `~/tools` 22. 723
compile under the plain CLI; 21 do not. 17,698 declarations: 4,747 static
(3,437 `let`, 1,283 `config let`, 27 `enum`), of which 4,118 are read by
dynamic code; 6,152 functions; 6,799 dynamic. Functions declared with `fn`:
6,063, of which 2,706 static-pure.

The survey counted "plausibly constant" by a text heuristic, so the
comparison needs one too: a `let` is *scalar* when the rest of its first line
is one number, string, boolean, `nil`, colour or symbol literal, a *table*
when it starts with `[` or `{`, and *derived* otherwise.

| Tree | `let`s | scalar static / dynamic | table static / dynamic | derived static / dynamic |
|---|---|---|---|---|
| `examples/` | 4,666 | 1,328 / 204 | 431 / 96 | 239 / 2,348 |
| `garden/` | 408 | 68 / 32 | 11 / 9 | 15 / 273 |
| `test/` | 78 | 24 / 14 | 9 / 4 | 19 / 5 |
| rest of this repo | 647 | 151 / 32 | 93 / 24 | 20 / 281 |
| `~/cheesecake/games` | 2,642 | 1,386 / 41 | 585 / 53 | 235 / 337 |
| `~/worlds-fair/ui/ptl` | 31 | 16 / 0 | 1 / 0 | 0 / 14 |
| `~/.garden` | 33 | 4 / 0 | 0 / 0 | 1 / 28 |
| petal-lang.org, experiments, `~/tools` | 221 | 50 / 13 | 29 / 8 | 5 / 116 |
| **All** | **8,726** (survey 7,408) | **3,027** / 336 (survey 2,350) | **1,159** / 194 (survey 827 literal tables) | **534** / 3,402 |

86% of the lets that start with a table literal are static. "Derived" is
mostly dynamic because the heuristic puts `screen_width() - 40` and
`items[sel]` there; the survey did not count those as plausibly constant.

**Why the 3,596 non-scalar dynamic `let`s are dynamic** (last link of the
reason chain):

| Root cause | Lets | Verdict |
|---|---|---|
| Calls a host native that the plain CLI does not register (`screen_width` 744, `screen_height` 221, `dt` 85, `mouse_x` 81, `time` 59, `palette` 49, …) | 1,460 | dynamic; under their own host these are the screen, the clock, input and the theme, none of them constant |
| Is, reads or writes `state` | 929 | dynamic |
| Reads a top-level `var` | 610 | dynamic |
| The name is assigned again | 418 | dynamic by the proposal's rule ("a value that is mutated is never cached"); see below |
| The file does not compile under the plain CLI (it calls host functions it never declares with `native fn`; all 18 worlds-fair screens are this) | 157 | not classifiable here |
| A function passed to a user-written higher-order function | 8 | the stricter-than-proposal rule from step 2; none in the games |
| An alias of a hoisted function taken above its declaration (`let _draw_line = draw_line`) | 6 | dynamic, and a function, not a table |
| `random` 3, `noise` 1, `freeze` 1 | 5 | dynamic (`freeze` marks the heap object; A2 owns it) |
| The table holds a closure | 3 | dynamic by rule |

Of the lets that call a function of their own file directly, 50 are static
and 343 are dynamic: 136 through `state`, 85 reassigned, 57 through a `var`,
50 through a host native, 15 for the smaller causes in the table. None is a
constructor-built table of constants.

The one class that could be argued is the 418 reassigned names. A sample of
22 of the 100 that start as a table literal, read by eye: most are per-frame
accumulators over dynamic data (`let shot_items = []` then a loop over
`shots`), and about a quarter loop over what looks like constants (`let
mod_by_sec = [0, 0, 0, 0, 0]` then a loop over `SETTINGS`, which is the
proposal's own example of a value that must stay inline). The loop bodies
were not classified, so that share is an impression, not a count.
Classifying those static means keeping the
loop's final value and nothing before it; that is a run-level change for S1 or
later, not a classifier fix, and Moses has 8 reassigned lets in 379.

**Moses** (`show-static --all games/moses/game.ptl`, all modules).

| | Declarations | Static | `let`s static | `state` cells |
|---|---|---|---|---|
| As written | 2,356 | 517 | 365 of 379 | 451 |
| Scratch copy, the 74 cells as `let` | 2,356 | 591 | 439 of 453 | 377 |

The scratch copy lives outside both repositories; `~/cheesecake` was not
edited. Moonbear: 1,222 declarations, 378 static.

**The audit.**

| Where | Mode | Covered | Findings |
|---|---|---|---|
| Panel corpus, 55 apps under the UI host's natives, 45 frames of the monkey scenario each | watch | 2,266 static declarations, 568,377 native calls under static terms | 0 |
| Same corpus | static only | 60,358 top-level statements run, 24,708 skipped | 0 |
| Same corpus | static only against the whole program | 1,525 static values, built alone and built by the whole program | 0 differ |
| The whole petal-ui test suite under `PETAL_STATIC_AUDIT=watch` | watch | 18 test binaries, 295 tests | 0 |
| The whole core test suite under `PETAL_STATIC_AUDIT=watch` | watch | 2,409 tests | 0 outside the audit's own tests, which register lying natives on purpose |
| Every file of the sweep that has a declaration, plain CLI | static only | 516 of 556 files run; 472,542 statements run, 34,567 skipped | 0 inputs touched; 2 builds raised |
| The same files | watch | the 133 that link and run under the plain CLI; 885,950 native calls under static terms | 0 |
| Moses as written, plain CLI | static only | 517 static declarations, 7,009 statements run, 658 skipped | 0 |
| Moses with the 74 cells as `let` | static only | 591 static declarations, 21,767 statements run, 584 skipped; the build is 35,195 instructions | 0 |
| All 14 Cheesecake games, every file as an entry file (226 files, Moses and Moonbear among them) | static only | 20,430 static declarations, a module's counted once per file that imports it | 0 |

Checks of the check: registering `screen_width` as `CONST` in petal-ui (and
reverting it) makes the corpus test report 56 failures, each naming the
declaration and `screen_width`. Two `--seed`s of the Moses static-only build
observe the same 2,004 named values.

What the audit rows do not show:

- **Moses was never run under its host.** Cheesecake vendors an older Petal
  and is not runnable headlessly from here, so Moses, Moonbear and the other
  games were audited only as static-only builds with this repo's binary, in
  which every Cheesecake native is unregistered, therefore dynamic, therefore
  skipped. That checks that the static part builds alone, calls nothing but
  core `CONST` builtins and touches no input. It does not exercise a single
  Cheesecake `CONST` row, because there are none yet.
- **The two builds that raised** are `garden/gpp-apps/gpp-test-app/src/runtime_error.ptl`
  and `runtime_error_long.ptl`, the deliberate error fixtures: a static
  `let n = len(obj.error)` and a failing `assert`. Both raise the same error
  in a whole-program run. A static declaration that raises is correct
  classification; S1 must not store it.
- **40 files cannot be run by the plain CLI at all** (they call host
  functions with no `native fn` declaration): 18 worlds-fair screens, 12
  petal-lang.org and tool scripts, 8 in `docs/aspirational-ideas` and
  `core-runtime/glsl/tests`, 2 in `~/.garden`. `petal run` refuses them
  before the first line. The survey expected almost nothing static there.
- Under `PETAL_STATIC_AUDIT=watch` one core test fails by design: the one
  that checks the variable is off by default. The core suite was run in the
  release profile, where
  `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` also
  fails with or without the audit: it asserts a value edit is 5× cheaper
  than a recompile and measures 3.9× (2.9 ms against 11.3 ms). It passes in
  the debug profile it was written for (17 ms against 183 ms) and does not
  touch anything this step changed. 2,408 of 2,409 otherwise.

**For S1.** Building all of Moses's static declarations, the 74 tables
included, is 35,195 instructions, so "clear every static value on a reload"
costs Moses that and not the 2.3 s of its boot. The static-only build is the
prototype of the static run: it is the same code with the dynamic statements
jumped over, and the corpus test that compares its values with the whole
program's is the differential S1 can keep.

**Cheesecake follow-ups (not done; that repository is read-only here).**
Pass `PB_FX_CONST` for the natives that qualify and re-vendor the bridge;
then run `--static-audit` in the engine, since that is the first time a
Cheesecake `CONST` row is exercised. After S1, rewrite the 74 constant
`state` cells as `let` or `const`. worlds-fair's `wf-ui-ffi` likewise.

Step 2, the classifier (`core/src/static_classify/`, `Env::static_report` /
`Env::static_analysis`, `petal show-static [--json] [--all]`; no behaviour
change, nothing reads its answer yet):

- **Lattice.** Per term: data (a function of the source text, holding no
  closure), function (a set of known targets, user or native), dynamic. A
  function's summary is "no dynamic term in its body, given data parameters",
  plus the class of what it returns; every call the pass calls static checks
  that its arguments are data. Solved as one optimistic fixpoint over all
  terms and summaries (5 rounds on Moses).
- **Callee resolution.** Direct captures, the write-once cell of a hoisted
  forward reference (an unnamed root `CellNew` with exactly one root-level
  write; a top-level read above the write is dynamic), the function's own
  self reference, a builtin held in a name, a function returned by a static
  call, and the capture of its own name that `make_overload_set` patches when
  one arity of an overloaded function calls another (`fn f(a) f(a, 0) end` —
  found on the corpus: without it 51 Moses and petal-ui functions were not
  static-pure).
- **Regions.** A loop, branch, match or short-circuit operator is static only
  if its inputs and every term of every child and guard block are. A phi is
  static only if the control term that picks its value is. Only root-block
  names are declarations, so a binding inside a top-level loop is never
  offered as one value.
- **Mutation.** A top-level `let` bound more than once (`xs[i] = v`,
  `xs = append(xs, v)`, a rebind in a top-level `if` or loop) is dynamic at
  every definition. A static declaration whose value (under any alias)
  reaches the container operand of a write or of a mutating builtin stays
  static, so what is built from it stays static, but carries `mutated_by` and
  must not get a slot.
- **Methods and enum data.** A method call is static only if every user
  method of that name, every built-in class method of that name and the
  global native of that name are. A field read (or method call) whose name is
  a constant-data key of some enum is static only if every record carrying
  that key is. `__enum_data`, `__enum_from` and `__enum_all` are classified
  structurally (they answer from the program's own declarations), not from
  their effect rows.
- **Extensions to the proposal's section 1, both stricter or equal.** A
  function passed to a *user* function is dynamic (a parameter is taken to be
  data), so a user-written higher-order helper is not static-pure; that
  affected 8 lets in the corpus and none in Moses. One impure arity makes a
  whole overloaded name impure (arity dispatch is not modelled).
- **Reported per declaration.** Class; reason chain (one link per
  declaration or function crossed); for static data `read_by_dynamic` and
  `readers` (dynamic operand, closure capture, free-standing statement, or
  another static declaration) and `mutated_by`; for a plain `state`
  `initializer_static`; `direct_depends_on` and the transitively closed
  `depends_on`, by module-qualified name (`lib::BASE`; every method of a
  called name; the enums whose data a field read may reach). S2 can pair
  these with `source_diff`'s `(kind, name)`: the kinds are the same words.
  For S1's lowering, `StaticAnalysis::term_class` gives the class of every
  term.
- **Not done here, for S1 to know.** A read *by name* from a host or tool is
  invisible to the compiler and is not counted in `read_by_dynamic`. Retained
  size is not reported (nothing is built). A static declaration read only by
  an *unkept* static declaration is reported as read by static code; S1's run
  splitting has to treat an always-executing run as a reader.
- **Without host natives** (`petal show-static`, `petal check`, the LSP) an
  unregistered `native fn` is dynamic, tested as "never more static than
  with the host".
- **Tests.** 52 unit tests in `core/src/static_classify/tests.rs`, one or
  more per rule, compiled from source; a CLI test in
  `core/tests/cli_args.rs`. One of them
  (`nothing_classified_static_moves_between_runs`) is a single program that
  routes a run counter through 28 mechanisms (captures, callbacks, returned
  and stored functions, overloads, recursion, method dispatch, enum data,
  branches, guards, loops, `var`s, rebinding) next to 20 static uses of the
  same mechanisms, runs it ten times on one stack, and checks that each
  leak is dynamic and really moves, and that nothing static does.
- **A first oracle, written before `--static-audit` existed.**
  `core-libs/petal-ui/tests/static_classify.rs` classifies each of the 55
  corpus apps against the UI host's own natives, drives it for 45 frames
  under the monkey scenario, and requires every static `let` to be bound to
  the same value on every frame: 1,525 static values compared, none moved.
  (Classifying `state` as data, as a check of the check, fails it in five
  apps.) It only sees a wrong answer when the corpus happens to move the
  value, which is why `--static-audit` is still wanted. Under `petal run`
  the same comparison between two random seeds over `examples/console` and
  `test/`: 118 static values, none moved.
- **Measured (release build, plain CLI, no host natives).**

  | Corpus | Declarations | Static data | of which read by dynamic code | Functions (static-pure) | Dynamic |
  |---|---|---|---|---|---|
  | `examples/` (150 files, entry files only) | 7,571 | 2,012 (1,870 `let`, 126 `config let`, 16 `enum`) | 1,850 | 1,838 (1,039) | 3,721 |
  | Whole ecosystem sweep (391 of 412 files compiled) | 9,139 | 2,284 | 2,077 | 2,404 (1,337) | 4,451 |
  | Moses, all modules (`show-static --all games/moses/game.ptl`) | 2,356 | 517 (365 `let`, 148 `config let`, 4 `enum`) | 494 | 1,368 (493) | 471 |

  The survey's estimate was 1,580 plausibly constant tables and derived
  values; the classifier finds 2,123 static `let`s over the same trees
  (scalars included). In Moses 365 of 378 top-level `let`s are static; of the
  13 that are not, 8 are reassigned, 3 read `dt`/`time` and 2 read `state`.
  Of Moses's 74 upper-case `state NAME =` cells, 73 have a static
  initializer; the 74th (`history.ptl` `TIMELINE`) sorts another `state`
  cell and becomes static once that one is a `let`. The named cases:
  `T = codes(TILES)` static, `hd`/`st`/`wk` static-pure, `TOOLS` static,
  `build_glyphs` static-pure with `GLYPHS`'s initializer static. In the
  examples the dynamic `let`s are dominated by `state` (637), the screen
  size natives (777), and reassignment (461); under a UI host that registers
  nothing `CONST` for those, the same.
  The pass costs about 85 ms on Moses (192,534 terms, 1,573 functions) and 5
  to 8 ms on an example panel; `show-static` on Moses is 0.38 s end to end
  against 0.55 s for `petal check`. S1 should run it on a recompile only, and
  can cut it with a worklist if that matters.
  In Moonbear (1,220 declarations, 378 static) `ch` is static-pure and the
  six chords built with it (`moonbear/sound.ptl:361`) and `THEME_CHORDS` are
  static.

Step 1, the `CONST` flag:

- Shipped: `NativeEffects::CONST` (a `constant` bit on the row, `PURE` plus
  that bit), `NativeEffects::is_const()`, and
  `NativeFnTable::constness(id)` / `constness_of_name(name)` with the same
  two on `Env` as `native_constness` / `native_constness_of_name`. They
  answer `NativeConstness::{Dynamic, Const, ConstIfCallbacksAre}`. The
  classifier should ask the `Env`/table form, not the row: `map`, `filter`,
  `reduce`, `sort` and `sort_by` answer `ConstIfCallbacksAre` (constant
  exactly when every function argument is static-pure; vacuous for
  `sort(xs)`), an unregistered name answers `Dynamic`, and a row that says
  `CONST` next to a read, probe, emit or effect answers `Dynamic`.
- C bridge: `PB_FX_CONST` (`1u << 5`), `petal::fx::Const`. `PB_FX_PURE` (0)
  stays dynamic.
- Audit of every bare `PURE` row, 133 in all. Core, 116 rows: 105 `CONST`
  (98 builtins and the `Rect` constructor with its six methods), pinned by
  name in `const_builtins_are_exactly`; 11 left `PURE` — `noise`,
  `is_valid`, `fn_info`, `fn_ast`, `__enum_from`, the higher-order `map`,
  `filter`, `reduce`, `forEach`, `sort_by`, and `sort`, `PURE` as a row
  because of its comparator form. `forEach` is `Dynamic`, not
  `ConstIfCallbacksAre`. (`__enum_all`, added by A1 the same day, is `PURE`
  and dynamic for the same reason as `__enum_from`.)
  petal-ui: `ui_version` is `CONST`; `font` and the ten pure panel stubs
  stay dynamic. Garden's config natives `editor`, `process`, `panel` are
  `CONST` (record builders); the fantasy console's `nes_version` and
  `master_rgb` are `CONST`. petal-query, petal-desktop-sdl and the web
  integrations have no bare `PURE` row.
- Two judgement calls for the classifier's author to know about: `assert` /
  `assert_eq` are `CONST` (same arguments pass or raise the same way; a
  build that raises is not stored), so a validating constructor stays
  static-pure; `symbol` is `CONST` because the symbol table belongs to the
  `Env` and ids are stable for its life.
- `native fn` declarations carry no effect information; constness comes only
  from the registration, which is what makes `petal check` and the LSP
  report less, never more.
- Documented for embedders in `docs/ffi.md` ("Constant natives") and
  `docs/embedding-c.md`. No behaviour change: nothing reads the flag yet.
- Out of scope, for cheesecake: none of its natives is constant until it
  passes `PB_FX_CONST` for the ones that are (and re-vendors the bridge);
  worlds-fair's `wf-ui-ffi` likewise.

**Ships.**

- An interprocedural pass that marks every top-level declaration static or
  dynamic and records the reason chain for a dynamic one. Rules are in the
  proposal, section 1. The parts that must not be skipped:
  - a three-point lattice: data, function, dynamic;
  - callee resolution through direct closure captures and through the
    write-once cells that hoisted forward references use;
  - an optimistic fixpoint for recursion; method calls static only if every
    method of that name is;
  - loops and branches static only as whole regions;
  - a declaration whose value reaches a mutation is not cacheable;
  - the classification never reads a literal's value.
- A `CONST` flag on native registrations, separate from `PURE`, opt-in, and
  exposed through the C bridge. Audit every existing `PURE` row; known
  non-constant ones are `noise`, `fn_info`, `fn_ast`, `is_valid`,
  `__enum_from`, petal-ui's `font`, and the Garden panel stubs.
- `petal show-static <file>`: each declaration's class, reason chain, and
  whether dynamic code reads it.
- `--static-audit`: evaluate each static declaration while watching the
  dynamic-input counters, and fail if one is touched.
- The per-declaration resolved dependency sets that S2 needs, since the pass
  computes them anyway.

**Go/no-go gate.** Run `show-static` over the corpus and Moses and compare
with the survey:

| Measure | Survey estimate | Go if |
|---|---|---|
| Top-level tables and derived values classified static | 1,580 | most of them, and specifically the constructor-built tables |
| Moses's 74 constant `state` cells, if rewritten as `let` | all plausibly static | nearly all |
| Named cases | — | `codes(TILES)` (`moses/config.ptl:129`), `hd`/`st`/`wk` rows (`moses/map.ptl`), `TOOLS` (`moses/agent.ptl:410`), `ch(...)` (`moonbear/sound.ptl:361`), `build_glyphs` (`moses/widgets.ptl:455`) all static |
| Audit | — | zero failures over the corpus, Moses and the petal-ui suite |

If the classifier cannot reach the constructor-built tables without becoming
fragile, stop and build option C: `const` only, a per-declaration hash, and
the syntactic `hoistable_const_lets` predicate extended with `CONST`
builtins for automatic detection.

**Also in S0.** Fix the documentation conflict now: `docs/writing-petal-guide.md:104-106`
should say that a hoisted constant `let` is about declaration order and is
still evaluated every run.

## S1. Caching, cleared on every reload

**Status of the cache itself: done 2026-10-12** (the first seven items of
"Ships", down to "Off under `baseline` and `explain`…"). `const`, the lint
and the hover are a separate track and have their own note. Enum constant
data, the joins between the two tracks, the items both notes list as open and
the gate are in "S1, finishing and the gate result" below; where that note
and an "Open" list here disagree, that note is the later one.

What shipped (`core/src/static_cache.rs` is the module and its header is the
description):

- **The flag** is `OptFlags::static_cache`, on in `fast` and `replay`, off
  in `baseline` and `explain`; `+static` / `-static` are the policy
  modifiers (`fast-static`, `baseline+static`), `RunPolicy::with_static_cache`
  and `Env::set_static_cache` the calls. `Env::effective_opt_flags` drops it
  whenever observations or the trace are preserved and under the static
  audit, so Garden panels get nothing, as decided. `petal run`,
  `petal check` and `petal show-bytecode` leave it off unless a policy is
  named (`PETAL_POLICY=fast petal show-bytecode f.ptl` shows the
  instructions); `petal run --runs <n>` is new, runs the script n times on
  one stack, and keeps it on.
- **The instructions** are `StaticSkip { after, run, slots }` and
  `StaticStore { slots }`, root code only, added by a pass that runs after
  every other one (`static_cache::apply`, from
  `bytecode::lower_with_statics` / `Env::lower_with`). Nothing is moved or
  removed. A `StaticSkip` is one comparison: the stack keeps, per run, how
  many of the slots the run feeds are still empty (`StaticRun::feeds`).
- **The table** is `Stack::statics` (`StaticSlots`), a GC root, stamped with
  the lowering's `StaticLayout::id` (new for every lowering, so for every
  program and flag set) and the constant table's revision (moved by a
  literal patched in place, which changes no lowering). Cleared in
  `transfer_stack_state`, so also by `restore_execution`. Read with
  `Env::static_values(stack)`, emptied with `Env::clear_static_values`.
- **Runs and slots are decided on the lowered code, by register**, which is
  what S0's finding about non-contiguous statements called for: a candidate
  instruction belongs to a root term that is static data, is not written to
  and lowers to nothing that must run; a run is a maximal stretch of them
  that control enters and leaves only at its ends; a register gets a slot
  when something executing on every run reads it (dynamic code, a capture, an
  unskippable static statement, the program's result). A register only a
  later skippable run reads gets none: the earlier run then feeds that run's
  slots too and is skipped only with it. A run lighter than two allocations
  (a call or a loop counts as two) is left alone. A run that reads a register
  a dynamic instruction writes is out, whatever the classifier said.
- **Three guards on a kept value**, each of which holds the mutation
  regression on its own (checked by switching the other two off): the plan
  leaves out every root term whose value reaches a write
  (`StaticPlan`, plus the classifier's `mutated_by`) and the pass refuses a
  slot to a register that reaches an in-place instruction; escape and
  last-use analysis take planned terms as non-fresh roots
  (`analyze_with_static_roots`, `apply_with_static_roots`); and
  `Heap::mark_immutable` sets a bit, separate from the frozen one, that the
  seven in-place heap operations treat as "copy". A value holding a
  function, a `var` cell, a handle or a pending result is rejected at the
  store, for good, and `StaticValues::warnings()` names the declaration.

Measured (release, this machine, 12 October 2026):

| What | Cache off | Cache on |
|---|---|---|
| 55 panel apps, 60 monkey frames, `replay`: instructions per frame | — | −0.0% to −29.7%, median −3.9%; 22 apps at −5% or better |
| …allocations per frame | — | −1.0% to −44.5%, median −18.1% (−10.0% over the sum) |
| snake | 2,992 instructions, 144 allocations | 2,792 (−6.7%), 99 (−30.9%) |
| analytics-dashboard | 29,817, 2,386 | 29,513 (−1.0%), 2,315 (−3.0%) |
| network-graph | 783,883, 1,257 | 782,445 (−0.2%), 1,098 (−12.7%) |
| settings | 36,727, 1,914 | 35,571 (−3.1%), 1,691 (−11.6%) |
| memory (the largest gain) | 30,490, 1,430 | 21,447 (−29.7%), 793 (−44.5%) |
| A 400-record `let` table as a memo argument, 29 frames after the first | 29 misses | 0 misses, 29 hits |
| Moses as written: root instructions a frame leaves out | 0 of 25,190 | 4,887 (1,079 direct allocations) for 33 checks; 378 slots |
| Moses with the 74 constant cells as `let` (scratch copy) | 0 of 25,042 | 19,693 (3,716 direct allocations) for 47 checks; 481 slots |
| `test/benchmarks/*` under `petal run` (cache off; against the same source with the table sync and the bit check compiled out, minimum of 15) | — | −1.2% to +1.2%, noise; instruction counts exact and unchanged (life 3,913,944, spreadsheet 20,449,651) |
| Lowering Moses (`show-bytecode`, compile included) | about 640 ms | about 715 ms: the classifier and the pass, once per recompile |

Every panel app keeps at least 11 slots, which are the UI prelude's own
tables. The Moses rows are counts from the layout
(`PETAL_OPT_STATS=1 PETAL_POLICY=fast petal show-bytecode game.ptl`), not a
run: root instructions only, so a floor (a skipped constructor call saves
its body too). Of the candidate runs, 136 are too light (389 instructions,
scalars) and one feeds nothing; none is refused for a tainted or mutated
register.

Tests: `core/tests/static_cache.rs` (15: the four defects of the first
draft, reloads against the cache off and against a cold start, a patched
literal on a second stack, forks and restore, collections, where it is off,
rejection, the bit, the memo benchmark, and 52 console and `test/` scripts
under four policy pairs); heap unit tests for the bit;
`core-libs/petal-ui/tests/static_cache.rs` (the 55-app corpus frame for
frame, cache on against off, under `replay`, `replay-memo`, `baseline` and
`fast`, plus the measurement above as an ignored test);
`static_cache_fuzz` in `bytecode/fuzz.rs` (the existing generator, three
runs a program, three policy pairs; 6,000 seeds clean in a soak, 150 by
default). `cell_fuzz` now runs its frames with the cache on against a
`baseline` oracle too, since `fast` and `replay` have it. `hot_reload.rs`
is unchanged and passes under all three of its policies.
`tools/test-examples.ts` runs each example twice more (`--runs 2` under
`replay` and `replay-static`): 33 pass, 11 s. `tools/verify.ts` needs no
change: `petal-ui-run` has the cache on by default, so its golden step
already compares cache-on traces with hashes taken before the cache
existed, and a wrapper that adds `--policy fast-static` as
`--before-ui-bin` makes `run-diff` a cache differential (136 identical
traces, none changed).

Three existing tests asserted memo hits on a top-level call with constant
arguments, which is now a static value and is not called twice; they pin
the policy to `-static` (`memo::env_tests`, `cell_in_place.rs`, `enums.rs`).

Open:

- **Moses has still not run under its host**, so the gate's "83.1k to about
  79k instructions per frame" is unverified; the layout count above (4,887
  root instructions) is in line with the hand simulation's 4.0k.
- Not run here: the Garden workspace and `petal-fantasy-nes` (no target
  directory on this disk, and none could be created); Garden panels have the
  cache off by construction.
- A slot per register means scalars read by dynamic code get slots when
  they sit in a run with tables (most of Moses's 378). A load costs what the
  `LoadConst` it replaces cost, so this is neutral, not a gain.
- Function declarations split runs (a `MakeClosure` is never skipped), and
  a table that sits alone between two of them with one allocation is left
  alone. Nothing moves code, as decided.
- A `StaticStore` that finds its slot filled keeps the old value. Under S1
  that cannot differ; S2's partial dirtying has to revisit it together with
  "a rebuilt value that is structurally equal keeps the old heap value".
- No C entry point switches the cache or clears the table
  (`pb_vm_set_static_cache`, `pb_vm_clear_static_values`); a C host has it
  on with the default policy. Cheesecake follow-ups are unchanged from S0:
  pass `PB_FX_CONST`, re-vendor, run `--static-audit` in the engine, then
  rewrite the 74 cells.

**Ships.**

- A dense per-stack slot table, GC-rooted, stamped with (program epoch, opt
  flags) and discarded on mismatch. Cleared in `transfer_stack_state`
  (`core/src/transfer_state.rs:30-57`) beside closures, enum data and memo.
- Lowering under a new `OptFlags` bit: `StaticSkip { after, slots }` before
  each contiguous run of static instructions, jumping only when every slot
  the run feeds is filled; `StaticStore` at each declaration that dynamic
  code reads. No instruction is moved or removed. A run that feeds no slot
  always executes.
- Slots only for declarations that dynamic code reads. No slot for a run
  with fewer than about two allocations.
- An immutable bit on stored values, checked in the seven `*_in_place` heap
  operations (`core/src/heap.rs:1222-1419`), which copy when it is set. Slot
  loads are non-fresh roots in `escape.rs` and `lastuse.rs`.
- A stored value may not contain a closure, `var` cell, handle or pending
  result; a value that does is not stored and the declaration warns.
- Off under `baseline` and `explain`, for run-once `petal run`, and whenever
  observations or the per-term trace are preserved.
- `const NAME = expr`, with the reason-chain error.
- Lint `state-as-constant`: a never-assigned plain `state` with a static
  initializer gets a suggestion, with a fix, to become `const`.
- LSP hover shows the class.
- Enum constant data becomes a static value where its expressions are
  static, and the fold deferred from A1 is reconsidered.

**Tests.**

- A regression for each defect the design review found: the loop-variant
  value, in-place mutation of a cached literal, a top-level `assert` that
  must keep running, an unused failing binding that must keep raising.
- Cache on versus off gives identical output over `examples/`, the golden
  corpus and the petal-ui suite, with the optimizer on and off.
- A reloaded run equals a cold start of the new source with state preserved.
- `core/tests/hot_reload.rs` passes unchanged.
- A fuzz target in the style of `cell_fuzz.rs`.

**Gate.** All oracles green, and the Moses headless profile shows the drop
the hand simulation measured (83.1k → about 79k instructions per frame,
record allocations down by about 40%). `test/benchmarks/*` under `petal run`
must not regress.

### S1, finishing and the gate result (2026-10-12)

**Status: done; gate passed, with one figure measured by a substitute**
(Moses still does not run under this repository's binary, so the "83.1k to
about 79k" figure is checked on the static part of its frame, not on a whole
frame under the engine).

What shipped after the two halves landed:

- **`const` and the cache are one thing.** `Env::check_consts` keeps its
  analysis beside the program (`Program::static_analysis`, stamped with the
  native table's revision and the term count) and the lowering that follows
  is served from it: a program with a `const` is classified once per
  recompile, not twice. A static value that cannot be stored (a function, a
  `var` cell, a handle, a pending result) is rebuilt every run and now says
  so: `Env::take_static_warnings(stack)` / `take_all_static_warnings()` hand
  out one line per value, once per load and once more after a reload;
  `petal run` and the SDL host print it, and the line says `const` when the
  declaration was one. The lint's rewritten `const` is cached like its `let`
  (the Moses numbers below are that rewrite).
- **Hover** says `// static, kept between runs` for a declaration the cache
  gives a slot. That costs a lowering, so the note is worked out by the first
  hover after a change and kept until the next change; a change classifies
  nothing (`hover_classifies_lazily_and_once_per_change`). The timings in the
  doc comment of `DocumentStore::static_note` (48 ms, 130 ms, 0.6 s) were
  taken once and not repeated at the final head.
- **Enum constant data is a static value** where its expressions are static.
  The per-variant `__enum_data` publication is a candidate instruction and
  the record it publishes has a slot; a frame that skips the run leaves the
  record in the stack's enum table. Both tables are emptied together, and a
  literal patched inside the data moves the constant revision. Measured
  (`rerun_timing`, release, 50 variants with three keys): 7.04 µs a run with
  the cache off, 2.50 µs with it on, 2.04 µs for the same enum without data.
  That is about 9 ns a variant against about 100.
- **The literal `Enum.variant.key` fold is not done, on purpose.** A patched
  reload gives the edited literal's term a constant slot of its own and
  retargets the one `LoadConst` lowered from that term
  (`env/reload.rs`, `retarget_constants`). A folded read is a second
  instruction loading the same constant from somewhere else, usually a
  function body, and nothing ties it to the term. It could be made correct in
  two ways: drop the lowering the first time a folded literal is patched,
  which turns a 0.8 ms value edit of enum data into a relowering (hundreds of
  milliseconds on Moses), in exactly the case live tuning is for; or teach
  the patcher a table of folded copies, which breaks "no pass reads a
  constant's value" for every later pass. The gain is the difference between
  a path read and a record read, about 5 ns (6.6 against 1.4), only for data
  that is a single literal, and a top-level `let x = K.v.key` is already kept
  by the cache. Revisit if a profile shows path reads in a hot loop.
- **The fuzz target** is `core/src/backend/bytecode/static_fuzz.rs`: static
  tables (`let`, `const`, `config let`, constructor-built, enum data) between
  dynamic declarations, every route of writing to a value that may be kept,
  top-level loops, and three reloads (literals patched, a statement added,
  back to the first text), cache on against off under `replay`,
  `replay-memo`, `fast` and `baseline`. With the plan filter, the non-fresh
  roots and the immutable bit all switched off it fails. A second test holds
  the generator to programs that compile, keep values and reload both ways.
- **From the two "Open" lists:** the C bridge has `pb_vm_set_static_cache`,
  `pb_vm_static_cache`, `pb_vm_clear_static_values` and
  `pb_vm_static_values_json`; the lint offers a `pub state`;
  `Env::set_static_only_keeps` and `core/examples/static_frame.rs` measure
  the static part of a frame for a script whose host is not here.

**Gate.**

| Gate item | Result |
|---|---|
| All oracles green | Yes, at `858a252`; list below |
| Moses: 83.1k to about 79k instructions per frame (−4.1k) | −5,391 on the static part of a frame, as written (8,065 to 2,674); not run under the engine |
| Moses: record allocations down about 40% | −480 records per frame (482 to 2 in the static part); the share of a whole frame's records is not known here |
| `test/benchmarks/*` under `petal run` must not regress | No regression; instruction counts exact and unchanged |

Moses, `static_frame`, release, 30 frames on one stack, 0 audit findings and
no value rejected on either copy:

| | Instructions, static part of a later frame | Allocations | Records | Slots, runs |
|---|---|---|---|---|
| As written, cache off | 8,065 | 1,230 | 482 | |
| As written, cache on | 2,674 (−5,391) | 58 (−1,172) | 2 (−480) | 426, 34 |
| 74 constant `state` cells as `const` (the lint's `--fix`, scratch copy), cache off | 25,804 | 4,687 | 1,383 | |
| …cache on | 2,566 (−23,238) | 46 | 1 | 529, 48 |

How to read it. The static-only build skips every dynamic statement, and
those cost the same with the cache on and off, so the difference between two
rows of one copy is exactly what the cache removes from every frame of the
real program. It is 1.3 times the hand simulation's 4.1k because a skipped
constructor call saves its body, which the layout count (4,887 root
instructions) does not see. The two copies are not comparable row for row:
as written, the 74 cells are `state` and their per-frame cost (the 406
`StateInit` hits of the baseline) is dynamic code, which this build skips.
What the rewrite shows is the hazard the lint's note describes: a `const`
table costs 25.8k instructions a frame wherever the cache is off, against a
`StateInit` hit for the `state` it replaced, and 2.6k with it on. The first
frame after a load or a reload costs the same either way (8,284 and 35,195
instructions) plus 68 to 96 for the stores.

Other numbers at the final head (release):

- Panel corpus, 55 apps, 60 monkey frames, `replay`: −0.73% instructions and
  −10.03% allocations over the sum; snake 2,992 to 2,773 (−7.3%), 144 to 96
  allocations (−33.6%); memory −29.7% and −44.5%; analytics-dashboard −1.0%
  and −3.0%; network-graph −0.2% and −12.7%; settings −3.1% and −11.6%.
  2,594 slots, no value rejected. Snake gained four slots and three
  allocations over the first note from enum data.
- A 400-record `let` table as a memo argument: 0 misses (the test in
  `core/tests/static_cache.rs`).
- `test/benchmarks/*` under `petal run --seed 1`, cache off there, minimum of
  15 at load average 6: append 6.8 ms (6.1 as the minimum of 60), arith 12.0,
  audio_synth 106.5, calls 29.7 (29.1 of 60), life 14.5, particles 10.7 (10.5
  of 60), spreadsheet 209.6, match_dispatch 316.2. The first S1 note has 6.4,
  12.0, 108.0, 28.8, 14.4, 10.5, 212.4, 313.4 for the same day. life
  3,913,944 instructions, spreadsheet 20,449,651 and 9 collections, as
  before the plan.

Oracles run at `858a252` (debug unless said): core `cargo test
--no-fail-fast` 62 binaries, 2,505 passed, 0 failed, 16 ignored;
`core-libs/petal-ui` 19 binaries, 296 passed (the 55-app corpus frame for
frame, cache on against off, under four policies); `petal-query` 33;
`integrations/petal-c-bridge` 19 and `make test-c-bridge` 4 of 4;
`integrations/petal-desktop-sdl` 54; vitest 1,570 in 75 files; `tsc`;
`./tools/test-examples.ts` 33 (optimizer on and off, cache on and off over
two runs, golden); `./tools/verify.ts --plan compiler` with the UI driver's
cache off on one side and on on the other, golden step included: 136
identical traces, none changed, 145 unsupported, 19 modules; `static_fuzz`
and `static_cache_fuzz` at 3,000 seeds each in release (and one earlier
`static_fuzz` soak of 23 minutes), clean. `core/tests/hot_reload.rs` is
unchanged and passes in debug.

One vitest run made while three builds were going failed two timing-bound
tests (`string-helpers` 100k appends in 5.5 s against a 3 s bound, and a
`fix-imports` case that printed nothing); both pass on a quiet machine and
neither touches this work.

**Open after S1.**

- Moses under its own engine: re-vendor, pass `PB_FX_CONST` for the natives
  that qualify, run `--static-audit` there, then `petal lint --fix
  --rules-include=state-as-constant games/moses`, and take the headless
  profile of "Benchmarks that gate the work". Until the engine has the cache
  on, do not apply the rewrite: see the 25.8k row. worlds-fair's `wf-ui-ffi`
  and the petal-lang.org WASM build likewise know neither `const` nor the
  cache until re-vendored.
- Not run: the release-profile core suite (so the known release-only
  `hot_reload` timing failure was not looked at again), clippy, the Garden
  workspace and `petal-fantasy-nes` (disk). Garden panels record
  observations, so they have the cache off by construction; Garden prints no
  static warning because it never has a slot.
- `petal lint` has no host, so a `state` table built with host natives is
  never offered. Unchanged.
- Scalars in a run with tables get slots (neutral), function declarations
  split runs, and a `StaticStore` that finds its slot filled keeps the old
  value, which S2's partial dirtying has to revisit. Unchanged from the
  first note.
- Disk: `core/target/debug/deps` held 87,665 stale codegen-unit objects of
  core's own test binaries (about 4 GB); they were deleted when free space
  fell to 1.6 GB. Garden's artifacts in the same directory were left.

### S1, the language surface: `const`, the lint, hover

**Status: done 2026-10-12** (three commits: `const`, the lint rule, the
hover). It depends only on the S0 classifier and was built beside the cache.

**`const NAME = expr`.** A `StmtKind::Let` with `is_const`, in both parse
pipelines, carried to the value term (`Term::is_const`). It formats,
exports (`pub const`), hoists, hot-reloads and runs as its `let`, and the
cache keeps it as it keeps the `let` (a test compares the filled slots).
What it adds is `Env::check_consts`, which ends every compile of a program
that declares one: a `const` that is not static data becomes an error
diagnostic with the reason chain.

- **It is an error diagnostic, not a failed load.** `petal check` exits
  non-zero and `petal run` does not start, as for an unknown name. A host
  that prints the findings of a reload and runs anyway gets the value the
  `let` would have had, and the cache does not keep it. A failed load would
  have cost the language server everything else it knows about the file.
- **Keyword.** Hard, not contextual. Swept 745 `.ptl` files (this repo 400,
  `~/worlds-fair/ui/ptl` 19, `~/.garden` 72, petal-lang.org 17, the two
  experiment apps 2, `~/tools` 3, `~/cheesecake/games` 232; `~/biz/hotlaps`
  is not on this machine): no binding, parameter, function or field is
  named `const`. Three files contain the word, all inside string literals of
  GLSL source. It stays a record key and a field name, like every keyword.
- **Without the host's natives** (`petal check`, lint, the language server).
  A failed `const` is classified a second time with every unregistered
  native, and every method name nothing defines, taken to be a constant that
  takes no callback: the most static any host could make the program. Still
  dynamic: an error everywhere, and the chain shown is that pass's, so it
  names the real cause and not the missing native. Static only under the
  assumption: `ConstCheck::Unverified`, no diagnostic; `show-static` and
  hover say "cannot be verified without its host", and the host reports it
  on load if it is wrong there. So a check without the host never calls a
  `const` verified that a host rejects, and never rejects one for a native
  it does not have. The second pass runs only when a `const` failed.
- **Rules.** Top level only (inside a function, loop or block is an error;
  constants in function bodies are the deferred item below). A name bound
  again fails with the classifier's `is assigned again`. A function value,
  or a table holding one, is rejected. `config const` does not parse; a
  `config let` of literals is static already.
- **Reload.** `ConstructKind::Const`; `let` <-> `const` is a structural
  edit. A literal edit inside a `const` is a value edit, as for its `let`.
  The chain's line numbers follow a reload that does not recompile:
  diagnostics can now cite several lines (`Diagnostic::error_citing`).
- **Editors.** tree-sitter grammar regenerated (its 25 corpus cases pass, one of them new),
  highlight queries, vim syntax, the LSP keyword list, `SymbolKind::Constant`
  in the outline; `core/tests/keyword_sync.rs` holds the five lists equal.

**Lint `state-as-constant`.** A plain top-level `state` (no key, not `state
var`, not `pub`, not in a function) that no `StateWrite` in the compiled
program targets and whose initializer is static becomes a `const`. The rule
compiles its own rewrite, keeps only what the compiler then accepts as
constant, and repeats until nothing changes.

- **It is opt-in** (`--rules-include=state-as-constant`), the first such
  rule. The proposal asks for a suggestion, not an automatic rewrite, and a
  blanket `petal lint --fix` is automatic. Three things the compiler cannot
  see make it a judgement: a host writing the cell by name or restoring
  saved state into it; a value kept across reloads on purpose; and a `state`
  initializer running once in every host, where a `const` is built once only
  when the cache is on (`state boot = settle(G, …, 140, …)` in
  `examples/dashboards/network-graph` would be rebuilt every frame wherever
  observations are recorded, which is every Garden panel today). Making it default-on is `opt_in: false` in
  `core/src/lint/mod.rs`.
- **Measured.** This repository: 30 findings (`examples/` 21,
  `integrations/` 8, `core-runtime/` 1, `garden/` and `test/` 0). The ones
  read by eye are constants or cells nothing writes (`lut`, `events`,
  `metrics`, `platforms`, `player_y`, `boot`). A scratch copy of the Moses scripts:
  76 findings, the 74 upper-case cells among them (`TIMELINE`, which sorts
  `RAW`, in the second round); `--fix` converts all of them, `show-static
  --all` goes from 517 to 591 static declarations (the figure S0 got by
  hand-rewriting the 74 as `let`), no `const` fails its check, and a second
  run finds nothing. The rule compiles a file twice per round (once to
  classify, once to check its rewrite); the report over the directory took
  4 minutes with a debug binary.

**Hover.** A top-level declaration, or a use of one, shows `// static`,
`// dynamic: <first link of the chain>`, `// function, static-pure` or
`// function, not static-pure: <reason>`, plus "its initializer is static"
on such a `state` and the `const` verdict. `state` declarations had no
definition in the language server at all; they do now.

**Tests.** `core/tests/const_decl.rs` 30, `core/tests/lint_state_const.rs`
16 (an assigned state in six forms, a keyed state, a `state var`, a state in
a function or loop, seven kinds of dynamic initializer, a host native, `pub
state`, silencing, the opt-in), `core/tests/lsp_tests.rs` +9. The whole
core suite in the debug profile, with both cache commits underneath: 2,492
passed, 0 failed. The 376 documentation snippets compile and print what
they say. Not run: the petal-ui and Garden suites (disk), and the release
profile.

**Cost.** `check_consts` runs the classifier once per compile of a program
that declares a `const`, and nothing otherwise. `petal check` on Moses with
the 74 cells as `const`, debug binary: 12.2 s against 11.5 s. S0 measured
the pass at 85 ms on Moses in release, against 0.55 s for the check.

**Open.**

- The classifier now runs twice on a recompile of a program with a `const`
  and the cache on: once for the check and once for the lowering. One
  analysis could serve both if it were kept beside the program.
- Hover classifies on every document change (5 to 85 ms by S0's numbers), on
  top of the compile the analysis already does. Fine today; lazy if not.
- The rule does not offer a `pub state`. It could: a module's exports are
  read-only to importers, so the declaring file's own writes are all there
  are.
- `petal lint` has no host, so a `state` table built with host natives is
  never offered. A host-side lint would need the host's `Env`.
- A `const` whose build raises is static and passes the check; the error is
  the run's, on every run, as for a `let`.
- Cheesecake follow-ups (not done, read-only here): re-vendor Petal, then
  `petal lint --fix --rules-include=state-as-constant games/moses`; the
  natives that should be `PB_FX_CONST` decide whether a `const` built from
  them is accepted by the engine. worlds-fair's vendored copy and the
  petal-lang.org and hotlaps WASM builds do not know `const` until they are
  re-vendored.

### Review of S1 (2026-10-12)

An independent pass over all of S1 after its commits (`2d93ed1` to
`bbeac09`), aimed at a stale or wrongly shared kept value. One defect found
and fixed, one difference found and kept; the gate stands as the finishing
note has it.

- **Fixed: a restore on a context two stacks share.** `restore_execution`
  replaces the whole context, heap included. A second stack bound to that
  context kept the static values of its last run, and its next run loaded
  ids the restored heap never had (`stale heap id`, a panic; it ran with the
  cache off). The restore now empties, on every other stack of the context,
  what a run rebuilds: static values, memo records, enum records, captured
  functions. That stack's own `state` is beyond help, as before.
  `a_restore_empties_the_table_of_every_stack_on_the_restored_context`.
- **Found and kept: `frozen` can tell.** `let T = [...]` then `freeze(T)`
  freezes the one kept table, so `frozen(T)` is true from the top of the
  next run, where the table rebuilt every run was new and unfrozen (and
  leaked one table a run). A constant string and a payload-free variant
  already behave the kept way with the cache off, so this is the constant
  pool's behaviour extended to tables, and the better one. Pinned
  (`freezing_a_kept_value_freezes_it_once`) and written into
  `docs/Builtins.md`, `docs/dev/performance.md` and the changelog.
  `let T = freeze(build())` is dynamic and unchanged.
- **Tried to write through to a kept value**, cache on against off under
  `replay`, `replay-memo`, `baseline+memo`, `baseline` (and by CLI under
  nine policies): a user function writing its parameter flat, nested and
  through a record; an element taken out, a loop variable, a match binding;
  a capture, a `var` cell, `state` seeded from it and then written; the
  callbacks of `map` and `sort_by`; tables built from each other; f64 arrays
  (`set_at`, `swap`) alone and inside a list; enum payloads, `K.all`, variant
  data, `K.from`; a fresh container holding kept rows and written two levels
  down; a module's `pub let` and `pub const` written by the importer; a host
  `call_function` between frames. None got through. The heap has exactly
  seven mutating entry points and all seven test the bit; nothing outside
  `heap.rs` takes a mutable payload.
- **Tried to skip something that was needed.** A kept table directly in
  front of each construct dynamic code opens with (the three loops, `if`,
  `match`, `&&`, `||`, `??`, a collecting loop with `continue`, `var`,
  `state`, `fn`); tables read only through a capture, a lambda, a method, a
  default parameter, a nested closure; frames that raise or `return` part of
  the way down, in every third and fifth frame, with later runs depending on
  earlier ones; 75 runs fed by one table, past the cap on feeds. Same output
  everywhere.
- **Tried to keep a value past its time.** `set_config_value` (three paths,
  two stacks); a literal patched inside a function a kept value calls, and
  back; a structural reload; a reload with `state` holding part of the old
  table, then `clear_static_values`, across collections (the rebuilt value
  is a new object); ten switches of what the lowering preserves between
  frames; fork, reload of the parent, run of the fork, restore; speculative
  runs before and after the first store.
- **Reproduced.** Core `cargo test --no-fail-fast` at the fix: 2,517 passed,
  0 failed, 16 ignored (debug). The petal-ui corpus differential, 55 apps
  under four policies: passes. `./tools/test-examples.ts` 33,
  `integrations/petal-desktop-sdl` 54 (its timeline is the one caller of
  `restore_execution`), `integrations/petal-c-bridge` 19. `static_fuzz` and `static_cache_fuzz` at
  4,000 seeds each in release: clean. Moses under `static_frame`: 8,065 to
  2,674 instructions and 482 to 2 records on a later frame, 426 slots, 34
  runs, as reported.
- **Open, none of it new work for S2.** The lowering cache is keyed by the
  optimizer flags, not by the native table: a native registered again with
  a different effect row after the program first ran keeps its old
  classification until the next reload (the documented rule is to register
  before `load_program`). Moses under its own engine, the release-profile
  core suite, clippy, the Garden workspace and `petal-fantasy-nes` are still
  not run.

## S2. Following the diff

**Status: done 2026-10-12; gate passed** (no stale value in any differential;
the time on Moses is measured and is small, see the gate table). The rule,
the module and the tests are described in
[docs/hot-reload.md](../hot-reload.md#static-values-across-a-reload) and in
the header of `core/src/env/static_follow.rs`.

What shipped:

- **Where things are written, not `(kind, name)`.** `source_diff` now gives
  the old-text span of every changed or removed construct, the statement
  each changed literal is in, and whether paired statements changed order;
  it also lists value-only statements beside a structural change, which the
  equal-length path used to leave out. A declaration is *changed* when a
  term that binds it lies inside a changed span. That one question covers a
  class with its methods, an overloaded function, an enum and a
  destructuring `let`, where pairing the diff's names with the classifier's
  would have needed a rule each (the classifier's `Point.len2` is inside the
  diff's `class Point`).
- **The kept test** is the three conditions of the plan, asked on both
  programs: nothing that writes the slot is inside a changed span, nothing
  in its dependency set is changed, and after a recompile the slot of the
  same qualified name has a dependency set with the same names, none
  changed in the new program either. `StaticAnalysis::root_dependencies`
  answers for any static top-level term, so an unnamed value has a set too.
- **Unnamed values have an identity.** 23% of the slots of `examples/` are
  values with no declaration (a table written inside a call, `len(TILES)`
  hoisted out of one), and a run executes when any slot it feeds is empty,
  so leaving them out would have rebuilt most runs on every recompile. They
  are matched by the text of their statement, the shape it compiled to and
  their place in it; a key two slots share is dropped.
- **Clear-all remains** where the diff says nothing usable (`change: full`,
  an unreadable file), where statements were reordered, where the file
  table changed (an import added or removed), and under
  `Env::set_static_follow(false)`, which is the oracle.
- **Identical rebuilt values keep the old heap value**
  (`static_cache::identical`, stricter than `==`; the old value goes into
  the register as well). A slot that kept its value while its run was
  executed for another is written back into its register too. A slot whose
  only writer is one literal is given the literal's new value with no run.
- **Enum data is followed like everything else**: a kept record is published
  into the stack's enum table again by the reload. Without that a program
  with enums rebuilt one slot per variant on every reload (Moses: 46).
- **Runs are less coupled.** A register with a slot of its own no longer
  makes its run feed the later runs that read it (S1 did, harmlessly, since
  everything was rebuilt together). A dependent is now rebuilt alone.
- **The report**: `ReloadReport::statics` (`StaticReload`: kept, rebuilt,
  rebuilt names, deferred, stale seeds, why everything was cleared),
  `static_lines()`, `statics` in `pb_vm_last_reload_json`, printed by the
  SDL host and by Garden's config script and panels. A stale seed is a
  top-level `state` that holds a value and whose declaration, or something
  its initializer depends on, is changed.
- **Debounce**: runs are timed; under `set_config_value` driven closer
  together than 150 ms a slot whose runs took 4 ms or more keeps its value
  and is rebuilt by the first frame after the pause (`run_needed` asks for
  it), by the next reload, or by `Env::flush_static_rebuilds`. Slots rebuilt
  beside a waiting one are rebuilt again with it, since they may have read
  it.
- **A followed reload lowers the new program**, which the next frame used
  to do: that is how the report can say what was kept.

**Gate.**

| Gate item | Result |
|---|---|
| No stale value in the differential | None. Every reload of `hot_reload.rs` (42 cases, three policies) and of its sweep; 23 cases of `static_reload.rs` under four policies; the 55-app panel corpus through five edits under three policies (275 followed reloads, 11,352 values kept, 1,752 rebuilt); the whole-corpus sweep in release over this repository (403 files: 264 relocated, 128 patched, 218 recompiled) and over the Cheesecake games, worlds-fair, `~/.garden`, the website and `~/tools` (343 files: 160, 9, 154); `static_fuzz` at 3,000 seeds in release |
| Structural-edit time on Moses against S1 | Unchanged within noise: 542 ms rebuilt, 534 ms followed (reload + lowering + first frame). The frame after the reload is 0.62 ms and 8,353 instructions rebuilt, 0.35 ms and 2,675 followed |

Moses, static-only build (it still does not run under this binary),
`core/examples/reload_follow.rs`, release, minimum of 7 cycles:

| Copy | Edit | Rebuilt: reload + lowering + frame | Followed | Frame's instructions | Followed | Kept / rebuilt |
|---|---|---|---|---|---|---|
| as written (426 slots) | a function added | 153.3 + 388.5 + 0.62 ms | 533.4 + 0 + 0.35 ms | 8,353 | 2,675 | 426 / 0 |
| | one table, by shape | 156.6 + 383.9 + 0.59 ms | 544.1 + 0 + 0.38 ms | 8,354 | 2,889 | 425 / 1 |
| | the same table, one number (patched) | 5.97 + 0 + 0.45 ms | 5.99 + 0 + 0.29 ms | 8,352 | 2,887 | 425 / 1 |
| 74 cells as `const` (529 slots) | a function added | 219.0 + 261.7 + 1.39 ms | 485.0 + 0 + 0.42 ms | 35,292 | 2,567 | 529 / 0 |
| | one table, by shape | 223.5 + 262.1 + 1.42 ms | 486.8 + 0 + 0.42 ms | 35,293 | 2,781 | 528 / 1 |
| | the same table, one number (patched) | 5.91 + 0 + 1.31 ms | 5.84 + 0 + 0.32 ms | 35,291 | 2,779 | 528 / 1 |

How to read it. A recompile of Moses is half a second and building its
static values was never more than 1.4 ms of that, so following the diff
cannot show in a structural edit here; the plan's "not all 2.3 s of them"
is `state`-held grids, which this does not touch. What it changes is the
frame after a reload (65% to 93% fewer instructions) and a value edit
(7.2 ms to 6.2 ms with the 74 tables). Deciding what to keep costs about
0.02 ms on 426 slots. The classifier's extra pass for unnamed values costs
4 ms of 358 on Moses (`show-static`), lowering 8 ms of 645.

Checks of the checks: with nothing called dirty, ten `hot_reload` cases, 18
of the 23 `static_reload` cases and `static_fuzz` fail; with the new-program
conditions switched off, the shadowing, overload, method and enum cases
fail; without the rebuild list the drag case fails; without rooting the
remembered values the collection case panics with `stale heap id`.

Oracles at the final code (debug unless said): core `cargo test
--no-fail-fast` 63 binaries, 2,542 passed, 0 failed, 16 ignored;
`core-libs/petal-ui` 20 binaries, 297 passed; `integrations/petal-c-bridge`
20 and `make test-c-bridge` 4 of 4; `integrations/petal-desktop-sdl` 54;
`./tools/test-examples.ts` 33; vitest 1,570 in 75 files; `garden-script`
type-checks (`cargo check`, into `core/target`). The C++ bridge test
drives a drag through `pb_vm_set_static_debounce` and
`pb_vm_flush_static_rebuilds`.

**Open after S2.**

- **The unit is the run.** One rebuilt slot re-executes its whole run and
  discards the other values; a deferred rebuild takes its run's cheap
  values with it. Splitting runs is the S1 item about function
  declarations, still open.
- **A run with no slot of its own is not timed** (it has no `StaticStore`
  to end at), so the debounce undercounts a table that only exists to build
  another one in a separate run.
- **Only a lone literal skips the wait.** The dragged value gets its new
  value directly when its slot holds one literal and nothing else. A
  dragged literal inside an expression (`config let K = 2 * 3`) that shares
  a run with a slow table waits with the table.
- **Other stacks of the program start empty**, as under S1.
- **A wrong `CONST` flag now outlives a reload** that does not reach the
  declaration; `Env::clear_static_values` is the remedy (written into
  `docs/ffi.md`).
- **`frozen` across a reload**: a kept table stays frozen where a rebuilt
  one would be new; the same documented difference as between runs.
- **Garden panels print nothing**: they record observations, so the cache
  is off and `ReloadReport::statics` is `None`. Stale seeds could be
  reported without the cache (they need only the classifier), at the price
  of running it on every recompile of every panel; not done.
- **Not run**: `tools/verify.ts`, `tsc`, clippy, the release-profile core suite,
  the Garden workspace's tests, `petal-query`, `petal-fantasy-nes`, Moses
  under its own engine.
- **Cheesecake follow-ups** (read-only here): re-vendor Petal and the
  bridge; the engine can then print `statics` from
  `pb_vm_last_reload_json` after a reload, keep calling `pb_vm_set_config`
  during a drag as it does now, and call `pb_vm_flush_static_rebuilds` when
  the drag ends (`pb_vm_set_static_debounce` and `pb_vm_set_static_follow`
  are the other two switches). worlds-fair's vendored copy and the WASM
  builds likewise.

**Ships.**

- On a structural reload, keep a slot only if its declaration is unchanged
  in `source_diff`'s result, its resolved dependency set has the same names
  in the old and new program, and nothing in that set changed. Identity is
  the module-qualified declaration name.
- On a patched (literal-only) reload, dirty the slots that depend on the
  edited literal's enclosing declaration.
- A rebuilt value that is structurally equal to the old one keeps the old
  heap value.
- The reload report gains: constants rebuilt, constants kept, and stale
  seeds (`state` cells whose initializer read something that changed). It is
  in `ReloadReport`, in `pb_vm_last_reload_json`, and printed by default by
  the SDL and Garden hosts.
- Debounce rebuilds whose last build exceeded a time threshold while
  `set_config_value` is being driven continuously.

**Tests.** Every reload in the hot-reload suite, run with diff-driven
dirtying, must leave the same slot contents as clearing everything. Add
cases for a changed import, a shadowing declaration, an added overload, an
added method, and a literal inside a function body that a constant calls.

**Gate.** Structural-edit time on Moses against S1, and no stale value in
the differential test.

### Review of S2 and A3 (2026-10-12)

An independent pass over S2 (`694c946` to `2067db1`) and A3 (`24c39af`),
aimed at a constant that survives a reload it should not. One defect found
and fixed (`775d08a`), one difference found and written down; the gate
stands.

- **Fixed: a table another stack's edit had made stale.** A literal patched
  through one stack moves the constant revision and leaves the table of
  every other stack of the program (and of a fork) as it was; that stack's
  next run empties it. `begin_static_follow` asked only whether the table
  was filled under the current lowering, so a reload asked of such a stack
  before it ran again took the stale values for current and kept the ones
  its own edit did not reach. Two stacks, `w: 3` patched to `w: 40` through
  the first, then any edit through the second: the second stack showed
  `w: 3` on every later frame. The same through a fork and
  `set_config_value`. The table now has to carry the program's constant
  revision too. `review_a_table_another_stacks_reload_made_stale_is_not_kept`.
- **Found and kept: `frozen` can tell, once more.** The store of a run is at
  its end, so when a run is evaluated again for one value, a neighbour built
  from a kept value (`let C = [B, 1]`) holds the run's new copy of `B`, not
  the kept one. They are equal and neither can be written to; `freeze(B)`
  then freezes the kept one only, and `frozen(C[0])` says `false` where a
  rebuild of everything says `true`. Fixing it needs a store per value,
  which is the open item about splitting runs. Written into
  `docs/hot-reload.md`. The swap of a rebuilt value for an identical old one
  does not have this problem: `identical` compares `frozen` at every level,
  so a frozen old value is never swapped in for an unfrozen new one
  (`review_a_shared_element_frozen_through_one_name`).
- **Tried to keep a value past its time**, each against the reload that
  rebuilds everything, under `replay`, `replay-memo`, `fast` and
  `baseline+static`, frames and tables compared after the reload and one
  frame later (16 cases added to `core/tests/static_reload.rs`, all named
  `review_…`): functions, classes and enums renumbered by declarations added
  in front of the ones a kept value holds; positions after a comment or a
  longer literal moved everything, twice in a row, and in text with
  characters wider than a byte; a reload that does not compile (with the
  table's edit already in it) and then one that does; a reload after
  `set_config_value` left the program differing from any file; a named
  import switched between two modules both already loaded; a name swapped
  between two declarations, rebound, and a reader moved across the second
  binding; a function held in a record or a list; methods of one name on
  two classes, and a method, an arity or a variant added in a module the
  reader does not import; every tier of the module system's lookup (an
  importer's export over its import's, a second arity from another module,
  a module export over `std`'s, a facade that passes on another module's
  name or declares its own, a selective import over a plain one); plain
  `let`, `const` and `config let` literals read through every kind of
  expression; values taken apart by `match`; a drag with rebuilds put off,
  interrupted at random by structural reloads, value reloads, failed
  reloads, comment-only reloads and drags of a second value, compared after
  each flush (40 rounds a policy, 2,467 put-off rebuilds); two stacks and a
  fork. Only the first item above got through.
- **The checks have teeth.** With the dependency test removed, 9 of the
  first 11 review cases fail; with the new-program conditions removed, the
  name and module-tier cases fail.
- **The report.** `follow()` now checks every followed reload of the file:
  `kept` plus `deferred` is the number of slots holding a value straight
  after the reload, `kept + deferred + rebuilt` is the table, every name
  under `rebuilt_names` is a slot waiting to be built, and `cleared` means
  nothing kept. No mismatch.
- **`identical`.** Read against `==`: kinds, numbers by bits, strings by
  content, lists in order, records by class name, key order and `frozen`,
  variants by tag and payload; anything a kept value cannot hold is never
  identical. A kept value never holds a function (a table of functions gets
  no slot), which is what makes renumbering harmless.
- **A3.** The commit changes two hashers and three signatures and no
  instruction, so every semantic the step lists is the old code's; the 2,558
  core tests (the `state_call_paths`, `state_lifecycle`, `state_decl_ids`,
  `hot_reload`, fork and audit suites among them) pass. Nothing iterates the
  state table or the touched set where order could show, and no crate
  outside `core` names the map's type (grep). Re-measured on this machine by
  switching the hasher back in place: 17.4 µs to 10.5–10.8 µs for 200
  top-level hits, 57.5 µs to 44.4–44.7 µs for 200 in-function hits, control
  14.6–15.1 µs. That agrees with the status note. The slot index stays
  unbuilt for the reason given there.
- **Reproduced at the fix** (debug unless said): core `cargo test
  --no-fail-fast` 63 binaries, 2,558 passed, 0 failed, 16 ignored; the
  whole-corpus reload oracle in release over this repository, and over the
  Cheesecake games, worlds-fair, `~/.garden`, the website and `~/tools`
  (343 files: 160 relocated, 9 patched, 154 recompiled); `static_fuzz` and
  `static_cache_fuzz` at 3,000 seeds in release; the petal-ui corpus
  differentials (`static_reload`, `static_cache`); `petal-c-bridge` 20;
  `petal-desktop-sdl` 54; `./tools/test-examples.ts` 33; vitest 1,570.
- **Still open.** Everything under "Open after S2" except what is above.
  Not run by this pass either: clippy, `tsc`, `tools/verify.ts` (it needs a
  second build to compare with), the release-profile core suite as a whole,
  the Garden workspace's tests, `petal-query`, `petal-fantasy-nes`,
  `make test-c-bridge`, Moses under its own engine. The debounce defaults
  are still untuned against a real drag.

## Later, each on evidence

Do one of these only when a profile or an author report asks for it.

- **Garden.** Replay named observations for skipped runs, the way memo
  scopes do with `Dep::Observed`, so panels can use the cache.
- **Constants inside function bodies.** 279 sites, mostly small. Needs a
  thunk per constant because registers are per call.
- **Per-input-class slots.** Values that depend only on the viewport, or
  only on the theme, cached until that class's revision changes. This is the
  rest of `docs/dev/reactive-rendering-plan.md` P2, and it is where UI
  panels would start to benefit.
- **Keeping slow intermediates.** Only if one expensive intermediate feeds
  several kept values and edits downstream of it are common.
- **A frozen generation that can release objects**, so static values can
  live in it.

## Benchmarks that gate the work

| Benchmark | Baseline |
|---|---|
| Moses headless, 600 frames (`build/main/engine/app/cheesecake games/moses/game.ptl --headless --mute --quiet --frames 600 --set 'START="play"' --set START_YEAR=1956 --vm-profile`, from `~/cheesecake`) | 49,871,337 instructions; 2.71 ms per frame; 14 collections; 406 `StateInit` per frame |
| Example panels in the same headless host | snake 2,617 instructions per frame; analytics-dashboard 11,005; network-graph 153,493; settings 31,624 |
| Enum reads (A1) | 66 / 41 / 173 ns |
| GC pause with a rooted 100,000-record table (A2) | 10.0 ms per collection |
| A 400-record `let` table as a memo argument | 58 misses in 30 frames; target 0 |
| `petal check` on Moses | 587 ms |
| Patched reload (`core/examples/reload_timing.rs`) | about 0.8 ms |
| `test/benchmarks/*` under `petal run` | as listed in `docs/dev/performance.md`; must not regress |

The micro-benchmark scripts used for these numbers were written in a session
scratch directory and are not in the repository. Recreating them under
`test/benchmarks/` is the first step of A1 and A2.

### Full test pass after A1, A2 and S0 (12 October 2026)

Status: done, at `591ad55` (the three tracks rebased onto the commit that
makes `petal run` check a program before running it) plus two test fixes.

- **Passing.** `cargo test` in `core` (2,417 tests) and its four corpus
  sweeps (`make expensive_lint_tests`); `core-libs/petal-ui` (295, which
  includes the effect-audit, static-audit, static-classify, frame-gate and
  memo corpus oracles); `core-libs/petal-query` (33);
  `integrations/petal-c-bridge` (19, and `make test-c-bridge`, 4 of 4);
  `integrations/petal-desktop-sdl` (54); `garden-script` (180);
  `examples/custom-apps/petal-fantasy-nes` (58); vitest (1,568); `tsc`;
  `docs/stdlib.json` is current; `./tools/test-examples.ts` (33, optimizer
  on, off, and golden); `./tools/verify.ts --plan compiler` (every in-repo
  UI app matches its hash in `test/ui-golden/`); `--effect-audit`,
  `--static-audit` and `--static-only` over `examples/console` and
  `test/benchmarks` (45 files, no findings); the copy-propagation oracle,
  the 4-token small-scope check and the whole-corpus reload oracle in
  release.
- **Fixed.** `test/vitest/enums.test.ts` expected the compiler's wording for
  a bare variant where `run` now prints check's. `core/tests/overload_name_display.rs`
  rewrote one temp file from parallel tests, which failed two of them once
  in a release run.
- **`test/benchmarks/*` under `petal run --seed 1`** (release, minimum of 15,
  on a machine at load average 18): append 5.7 ms, arith 11.5, audio_synth
  105.4, calls 28.3, life 13.7, particles 9.9, spreadsheet 206.0,
  match_dispatch 311. That is 2% to 10% above the table in
  `docs/dev/performance.md` (5.2, 10.7, 102.7, 27.2, 13.0, 9.4, 201.4) and
  below what A1's same-day before-and-after runs measured (arith 12.3,
  calls 29.8, life 15.0), so it reads as the machine. No pre-plan binary was
  available for an A/B. Instruction and collection counts are exact and
  unchanged: life 3,913,944; spreadsheet 20,449,651 and 9 collections;
  audio_synth 621 collections; particles 4.
- **Micro-benchmarks re-measured.** Enum reads 1.4 / 3.5 / 6.6 / 11.7 /
  18.4 / 16.6 ns (record, local, path, payload, `all`, walk); 50 variants
  with data 6.96 µs a run; one collection over 100,000 records 2.03 ms
  rooted and 0.009 ms frozen; `petal check` on Moses 0.53 s.
- **Still failing.** `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile`
  in the release profile only (3.9× against the 5× it asserts; it passes in
  debug, which is what CI runs). The edit parses the one 1,352-line file
  twice (2.1 of its 2.7 ms) against a 10.7 ms recompile. None of the files
  on either side were touched by this plan.
- **Not run.** The rest of the Garden workspace: `cargo check --workspace
  --all-targets` passes, but building `garden-app`'s tests ran the disk down
  to 1.4 GB and was stopped. `tools/oracle-external.ts` (worlds-fair's
  `wf-ui` binary is not built). Moses under its own host.
- **Left as found.** Five UI apps have no entry in `test/ui-golden/index.json`
  (fractal-explorer, particle-sandbox, file-browser, settings,
  stock-trading). Nineteen scripts outside the repository (worlds-fair's and
  `~/.garden`'s) moved from `identical-trace` to `unsupported` in the verify
  plan, because a script that fails check no longer starts.

## Risks to watch

- **A wrong `CONST` flag is a stale value forever.** The audit only covers
  natives a test run calls. Every embedder's natives default to dynamic.
- **Silent fall back to per-frame.** One dynamic read makes a whole table
  dynamic. `const` is the guard; the lint must rewrite to `const`, not
  `let`.
- **Half-reload.** A source table reloads while a derived value held in
  `state` behind an `if X == nil` guard does not (`moses/map.ptl:1472`). The
  stale-seed report names the cell; the rewrite is manual.
- **Another lowering flag doubles the differential-oracle matrix.**
- **`petal check` and the LSP compile without host natives**, so they may
  report less as static than a host does. They must never report more.

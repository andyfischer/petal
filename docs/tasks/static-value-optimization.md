# Static value optimization: plan

Status: **A1, A2, A3, S0, S1 and S2 are implemented, reviewed and on `main`**
(12 October 2026); both gates passed. **Cheesecake has adopted it** (13
October 2026, its commits `de6bff3` to `52b80e2` and the docs commit after
them; "Cheesecake: the adoption" in Results): it vendors `4269f9f`, declares
three natives `PB_FX_CONST`, prints the reload report's `statics`, and Moses
has 60 of its 76 constant `state` tables as `const`. Under its engine the
600-frame Moses run is 49,871,337 to 46,506,542 instructions (83.1k to 77.5k
a frame, where the plan said about 79k), the record literals script code
builds are down 62% (the plan said about 40%), and the live-coding check
passes: a table that sat in `state` follows an edit with no restart, and an
edit to an unrelated function rebuilds no constant. The adoption found
three things here, all fixed (`6a102e1`, `4269f9f`): a C host could not see
a `const` that is not constant, the stale-seed report named states it
should not, and `PETAL_STATIC_CHECK` called a frozen kept value a failure.
Not done there: the map's source tables stay in `state` (the half-reload
risk is real and the stale-seed report does not name the cells), and `T`,
`F` and `USE` stay (they still pay). **The adoption was reviewed the same
day** on Cheesecake `fd51d84` ("Review of the adoption" in Results): the
vendored tree is this repository's at `4269f9f`, the three constant natives
were read again, and every figure above was taken again and is the same. It
found one more half-reload, fixed in the game (Cheesecake `fa36156`: the
skin in force was a `state` cache over the constant `SKINS`, and the run is
46,469,822 instructions with it a constant), two it left, and one thing
here that is not fixed: a constant handed to `append` is rebuilt on every
run, and only `petal show-static` says so (follow-up 11). worlds-fair and
the WASM builds are not re-vendored. Of the "Later" items
the first two, Garden and constants inside function bodies, are done (13
October 2026), and so are in-repo follow-ups 5 to 8; the rest (per-input-class
slots, keeping slow intermediates, a frozen generation that can release
objects) are not started, for want of a profile that asks for them. The
whole test plan passed on `4e5b584` on 13 October with nothing failing, the
release-only timing test included; what that pass skipped for want of disk
ran later the same day and found one miscompile in the in-place optimizer,
older than the plan and fixed, and nothing in the cache ("What the disk had
kept back"). See [Results](#results-12-october-2026)
for baseline against now, the final test pass and the follow-ups, and the
status note under each section for the rest.

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
  as documented in `docs/features/hot-reload.md` ("Editing an enum").
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
re-measure. **Done 2026-10-13, in part, on the measurement:** `F_PEOPLE` is
gone (Cheesecake `52b80e2`; four readers, none in a loop). `T`, `F` and
`USE` stay: a table read is one instruction and `Fabric.x.code` two, and
all 220 reads written through the enum cost the 460-frame autoplay run 0.5
to 0.7% ("Cheesecake: the adoption" in Results).

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

**Done 2026-10-13** (Cheesecake `03319c2`): item 5. Its suite, 347 tests
now, passes on the tree re-vendored at `518c008` with the local patch
removed, `VENDORED.md` updated and `memory.ptl` deleted ("Cheesecake, under
its engine" in Results). What this note said before that:
Cheesecake's 344-test suite on a re-vendored tree (item 5, in
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

**Cheesecake follow-ups.** **Done 2026-10-13** ("Cheesecake: the adoption"
in Results): `hash01`, `hex_color` and `sweep_hits` are `PB_FX_CONST`, the
bridge is re-vendored at `4269f9f`, all 13 games ran in the engine under
`PETAL_STATIC_AUDIT=watch` and `PETAL_STATIC_CHECK=1` with no finding, and
60 of the 76 cells the lint offers in Moses are `const` (the 16 of `map.ptl`
are kept in `state` on purpose). What these asked for, as written: pass
`PB_FX_CONST` for the natives that qualify and re-vendor the bridge;
then run `--static-audit` in the engine, since that is the first time a
Cheesecake `CONST` row is exercised. After S1, rewrite the 74 constant
`state` cells as `let` or `const`. Not done: worlds-fair's `wf-ui-ffi`
likewise.

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
- Documented for embedders in `docs/usage/ffi.md` ("Constant natives") and
  `docs/usage/embedding-c.md`. No behaviour change: nothing reads the flag yet.
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
  79k instructions per frame" is unverified (it ran there on 13 October
  2026: 77.5k, "Cheesecake: the adoption" in Results); the layout count above (4,887
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
  rewrite the 74 cells (all done 2026-10-13, see S0).

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
frame under the engine). **Measured under the engine on 13 October 2026**,
once Cheesecake had re-vendored: 83.1k to 77.6k instructions a frame with
the game as written (Petal `518c008`), and 77.5k once it had adopted `const`
(Petal `4269f9f`, 60 tables rewritten); the record literals its script code
builds are down 61.9% over the run (516,335 to 196,762), counted by the
`AllocMap` and `AllocMapSpread` instructions the engine's profile now
prints, against the same binary with the cache off. Both figures were taken
again by the review of the adoption and are exact. So the gate passes on the
engine's own numbers, and the substitute is no longer what it rests on. See
the gate table below and "Cheesecake, under its engine" and "Cheesecake: the
adoption" in Results.

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
| Moses: 83.1k to about 79k instructions per frame (−4.1k) | −5,391 on the static part of a frame, as written (8,065 to 2,674). **Under the engine, 13 October 2026:** 83,119 to 77,573 (−5,546, −6.7%), the 600-frame total divided by 600 as the baseline was; the same binary with the cache off (under `PETAL_STATIC_AUDIT=watch`) runs 83,079, so the cache is −5,506 of it. **After the adoption** (Petal `4269f9f`, three `PB_FX_CONST` natives, 60 tables as `const`, `F_PEOPLE` dropped): 77,511 (46,506,542 over the run, −5,608, −6.7%); the natives moved nothing, the rewrite −62 a frame; and 77,450 (46,469,822) at Cheesecake `fa36156`, once the review had made the skin palettes a constant |
| Moses: record allocations down about 40% | −480 records per frame (482 to 2 in the static part); the share of a whole frame's records is not known here. **Under the engine, 13 October 2026** (its `--vm-profile` counts allocating instructions since Cheesecake `b618c58`): record literals built by script code 516,335 to 196,762 over the 540 profiled frames (956 to 364 a frame, −61.9%), list literals and comprehensions 454,507 to 42,242 (−90.7%). "Before" is the same binary with the cache off, the pre-plan engine being gone, and records built inside natives are not counted. Collections over the run went from 14 to 10 or 11 (18 with the cache off under the audit) |
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
  on, do not apply the rewrite: see the 25.8k row. **Done 2026-10-13**
  ("Cheesecake: the adoption" in Results; the 25.8k row is 98.2k a frame
  under the engine with the cache off). worlds-fair's `wf-ui-ffi`
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
- Cheesecake follow-ups (**done 2026-10-13**, "Cheesecake: the adoption" in
  Results; they found that the bridge could not show the engine a `const`
  that is not constant, fixed in `6a102e1`): re-vendor Petal, then
  `petal lint --fix --rules-include=state-as-constant games/moses`; the
  natives that should be `PB_FX_CONST` decide whether a `const` built from
  them is accepted by the engine. Not done: worlds-fair's vendored copy and the
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
the time on Moses is measured and is small, see the gate table; that time
is of the static-only build in this repository. **Measured under the
Cheesecake engine on 13 October 2026**, in the adoption and again in its
review: a structural edit of the running game is 522 to 550 ms with 529 or
530 of its 530 values kept, a value edit 13 to 18 ms with the edited table
and what is built from it rebuilt and nothing else, and the rebuilt table is
what the game then answers with. There is no "before" for those times (the
pre-plan engine binary is gone), and the frame after a reload does not
isolate the constants there, because the game declares its procedural
assets again after any reload). The rule,
the module and the tests are described in
[docs/features/hot-reload.md](../features/hot-reload.md#static-values-across-a-reload) and in
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
| Structural-edit time on Moses against S1 | Unchanged within noise: 542 ms rebuilt, 534 ms followed (reload + lowering + first frame). The frame after the reload is 0.62 ms and 8,353 instructions rebuilt, 0.35 ms and 2,675 followed. **Under the Cheesecake engine, 13 October 2026** (the agent's `reload` reply, Moses with 60 tables as `const`, 530 values): 522 to 550 ms for a structural edit with 529 or 530 kept (531.9 and 539.4 ms in the review's two), 13 to 18 ms for a value edit (2 rebuilt: the table and the one sorted from it). No engine figure with follow off, and none from before the plan |

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
  declarations, still open. *Done 2026-10-13: "A store per value" below.*
- **A run with no slot of its own is not timed** (it has no `StaticStore`
  to end at), so the debounce undercounts a table that only exists to build
  another one in a separate run. *Done 2026-10-13, same note.*
- **Only a lone literal skips the wait.** The dragged value gets its new
  value directly when its slot holds one literal and nothing else. A
  dragged literal inside an expression (`config let K = 2 * 3`) that shares
  a run with a slow table waits with the table. *Done 2026-10-13, same
  note.*
- **Other stacks of the program start empty**, as under S1.
- **A wrong `CONST` flag now outlives a reload** that does not reach the
  declaration; `Env::clear_static_values` is the remedy (written into
  `docs/usage/ffi.md`).
- **`frozen` across a reload**: a kept table stays frozen where a rebuilt
  one would be new; the same documented difference as between runs. (The
  other `frozen` difference, the copy of a kept neighbour that the review
  below found, is gone since 2026-10-13.)
- **Garden panels print nothing**: they record observations, so the cache
  is off and `ReloadReport::statics` is `None`. Stale seeds could be
  reported without the cache (they need only the classifier), at the price
  of running it on every recompile of every panel; not done.
- **Not run**: `tools/verify.ts`, `tsc`, clippy, the release-profile core suite,
  the Garden workspace's tests, `petal-query`, `petal-fantasy-nes`, Moses
  under its own engine.
- **Cheesecake follow-ups** (**done 2026-10-13**: the reload line and the
  agent's `reload` reply carry `statics`, and the Tuning panel calls the
  flush when a drag ends; the two switches are not called, the defaults
  stand): re-vendor Petal and the
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
  `docs/features/hot-reload.md`. The swap of a rebuilt value for an identical old one
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

### A store per value (2026-10-13)

**Status: done.** The four items the S1 and S2 notes left open about the
unit of rebuilding (follow-ups 6 and 7 of "Results"). The header of
`core/src/static_cache.rs` is the description; commit `800ff90`.

What shipped:

- **Stretches, segments, groups.** A maximal sequence of skippable static
  instructions is a *stretch* (what S1 called a run). It is cut into
  *segments*, one behind each value that something outside its own segment
  reads, and each segment has its own `StaticSkip` in front and
  `StaticStore` behind. A stretch of several segments has one more
  `StaticSkip` in front, the *group*, which loads what code outside the
  stretch reads and jumps over all of it: a frame with everything kept
  makes one check per stretch, as before. No instruction is moved or
  removed. A cut is never placed inside a top-level statement, across a
  jump, or between two writes of a register that has a slot, so a loop and
  its result, and the values one statement binds, stay one segment; where
  no cut is sound the stretch is one segment, as under S1.
- **(1) A rebuilt value is rebuilt alone, and holds its kept neighbours
  themselves.** A register gets a slot when anything outside its segment
  reads it, another segment included, so a segment that executes finds the
  kept values it is built from in their registers (the skips in front of
  it loaded them). `let C = [B, 1]` rebuilt beside a kept `B` holds that
  `B`. Two rules in the reload keep the other direction from showing:
  a value is kept only together with what its segment read
  (`StaticRun::reads`) and with what the same store holds, so a rebuilt
  `B` takes `C` with it whatever the edit was (a rebuilt number, boolean,
  symbol or `nil` does not count: it has no identity); and a rebuilt value
  that comes out identical gives way to the old one only when it holds no
  kept object the old one does not (`static_cache::replaces`), with a
  store of several values swapped whole or not at all. **The `frozen`
  difference the S2 review wrote down is gone**: which objects are shared
  is the same after a followed reload as after one that rebuilds
  everything. `docs/features/hot-reload.md` says so now, and what remains there is
  the between-runs difference (a kept table stays frozen, a rebuilt one is
  new).
- **(2) Function declarations.** A `MakeClosure` still ends a stretch, and
  that is the plan's rule, not an oversight: a check that jumped over it
  would leave the function's register holding last run's closure, which a
  slot may not hold (a kept value holds no function, because a reload
  renumbers them and a run captures them again), so the only way to span a
  declaration is to move it, and nothing moves. What changed is the
  weight: a stretch is worth a check at one allocation, not two
  (`MIN_WEIGHT`), since a kept table costs one test where building it costs
  the allocation and the instructions around it. A table alone between two
  function declarations is kept
  (`a_single_table_between_two_function_declarations_is_kept`). A stretch
  that allocates nothing is still left alone. The cost of a declaration
  between two tables is one more check a frame: Moses has 42 where it had
  34, the 8 new ones being stretches that were too light before.
- **(3) Every build is timed.** Every segment ends in a store, so there is
  no run without one. A slow table that only one value reads has no slot
  and is built, timed and put off with that value; one that a value in
  another stretch reads has a slot and a time of its own.
- **(4) A dragged literal inside an expression does not wait.** Its value
  is a segment of its own with its own build time, so `let SCALED = [K *
  3]` beside a slow table is rebuilt on every tick and only the table
  waits (`a_dragged_expression_does_not_wait_with_its_slow_neighbour`,
  both layouts). Nothing was added for this; it follows from (1).
- **Two side effects, both improvements.** A value built before a
  statement that raises is now kept (it was stored before the raise), and
  a value beside one that cannot be kept (a pending result) is kept, where
  `StaticSlotState::NotKept` used to cover the whole run; `NotKept` is now
  only two values of one statement. `StaticValues::runs` is the number of
  checks a frame makes and `StaticValues::segments` is new.

Measured (release, this machine, load average about 7; the "before"
binaries were built from `f03dab6`):

| | Before | After |
|---|---|---|
| Panel corpus, 55 apps, 60 monkey frames, `replay`: slots | 2,594 | 2,940 |
| …instructions per frame over the sum, cache off to on | −0.73% | −0.74% (27,274 fewer; was 26,663) |
| …allocations per frame | −10.03% | −10.51% (4,354 fewer; was 4,153) |
| …median app | −3.9% and −18.1% | −4.6% and −19.1% |
| snake | 2,773 instructions, 96 allocations | 2,754, 90 |
| Panel corpus through five edits (275 followed reloads): kept / rebuilt | 11,352 / 1,752 | 12,768 / 2,250 |
| Moses as written, `static_frame`: slots, checks a frame | 426, 34 | 451 (in 451 segments), 42 |
| …a later frame's static part | 2,674 instructions, 58 allocations, 2 records | 2,641, 45, 1 |
| …instructions a frame leaves out | 5,391 | 5,424 |
| …the frame that builds everything | 8,352 | 9,217 |
| Moses with the 74 cells as `const`: slots, checks | 529, 48 | 548, 54 |
| …a later frame | 2,566 instructions, 46 allocations | 2,557, 40 |
| …the frame that builds everything | 35,291 | 36,330 |
| Moses, the frame after an edit to one table (`reload_follow`) | 2,889 (214 over a frame with all kept) | 2,681 (39 over) |
| …the same with the 74 cells | 2,781 (214 over) | 2,600 (42 over) |
| …kept / rebuilt on that edit | 425 / 1 | 450 / 1 |

How to read it. The per-frame gain is small and comes from (2): stretches
of one allocation. The point of (1) is the frame after an edit, which on
Moses now costs the edited table and 39 instructions where it cost 214,
and a slow neighbour that is no longer rebuilt at all. The price is two
instructions per stored value on a frame that builds everything, the
first one and the one after a reload that clears the table: 865 more on
Moses as written (10% of that frame's static part, about 10 µs), 1,039
with the 74 cells. Most of Moses's 451 values are numbers that dynamic
code reads, each now a segment; merging those into a neighbour's segment
would give the instructions back and put a dragged number back in a slow
table's store, so it was not done. The corpus rebuilds a larger share of
its slots than before (15.0% against 13.4%) because the count has 346
more slots in it and the edit of that test nudges every seventh number;
each of them is rebuilt alone, where a rebuilt slot used to execute its
whole run. Reload times are unchanged within noise (452.8 ms followed
against 454.8 ms rebuilt, reload plus lowering, a function added).

Tests: `core/tests/static_reload.rs` has six more cases (45): a rebuilt
value holds its kept neighbour, a table two values are built from stays
shared (with dynamic code between them, without, and when the edit is
what makes it shared), an equal value holding another object is not
swapped for the old one, one rebuilt value does not rebuild its
neighbours, the dragged expression, and
`sharing_is_the_same_as_after_a_rebuild_of_everything`, a generator of
tables that hold one another with `freeze` through arbitrary names after
an edit to one of them (40 seeds by default; 2,000 in release, clean).
`core/tests/static_cache.rs` has the table between two function
declarations, and two cases changed what they assert (one value kept
before a raise; `GOOD` kept beside a pending `BAD`). Checks of the
checks: without the "kept only with what it read" rule,
`a_table_two_values_are_built_from_stays_shared` fails and the generator
fails within 400 seeds; with `replaces` accepting any identical value,
`an_equal_value_holding_another_object_is_not_swapped_for_the_old_one`
fails.

Oracles at `800ff90` (debug unless said): core `cargo test
--no-fail-fast` 63 binaries, 2,573 passed, 0 failed, 16 ignored
(`hot_reload` with its slot-by-slot comparison, `static_reload`,
`static_cache`, the audits); `static_fuzz` and `static_cache_fuzz` at
3,000 seeds each in release; the sharing generator at 2,000 in release;
`core-libs/petal-ui` `static_cache` (the 55-app corpus frame for frame,
cache on against off, four policies) and `static_reload` (the corpus
through five edits, followed against clear-all, three policies);
`integrations/petal-c-bridge` 20 and `make test-c-bridge` 4 of 4;
`integrations/petal-desktop-sdl` 54; `./tools/test-examples.ts` 33
(optimizer on and off, cache on and off, golden). The whole petal-ui suite
(20 binaries, 297) passed on the code as it was an hour before the commit
and was not repeated on the final tree; `--static-audit` was run only as
those suites run it, not over the example directories by hand.

Open:

- **A cold frame costs two instructions per stored value** (above). A
  segment per number is the simple rule, not the cheapest one.
- **Function declarations still end a stretch**; see (2). A program that
  alternates tables and functions pays a check per table.
- **A group is not timed**, only its segments; `build_time` of a slot is
  its own segment's. A slow table with no slot is charged to the one value
  that reads it, which is right for the debounce and means the report
  names the reader, not the table.
- **Deciding what to keep was not timed again**; the new rule is a fixed
  point over the segments, one pass per link of the longest chain of
  values built from a rebuilt one.
- **Not run**: the Garden workspace's tests and `petal-fantasy-nes`
  (disk), `tools/verify.ts`, vitest, clippy, the release-profile core
  suite as a whole, Moses under its own engine. Garden panels have the
  cache off by construction.
- Everything else under "Open after S2" and the "Later" list is as it was.

## Later, each on evidence

Do one of these only when a profile or an author report asks for it.

- **Garden.** Replay named observations for skipped runs, the way memo
  scopes do with `Dep::Observed`, so panels can use the cache. **Done
  2026-10-13**: "Later: Garden" below.
- **Constants inside function bodies.** 279 sites, mostly small. Needs a
  thunk per constant because registers are per call. **Done 2026-10-13**,
  after measuring: "Later: constants inside function bodies" below. (The
  279 was a text scan; the classifier finds 5,899 in the panel corpus and
  7,510 in the Cheesecake games.)
- **Per-input-class slots.** Values that depend only on the viewport, or
  only on the theme, cached until that class's revision changes. This is the
  rest of `docs/dev/reactive-rendering-plan.md` P2, and it is where UI
  panels would start to benefit.
- **Keeping slow intermediates.** Only if one expensive intermediate feeds
  several kept values and edits downstream of it are common.
- **A frozen generation that can release objects**, so static values can
  live in it.

### Later: Garden (2026-10-13)

**Status: done; the cache is on under observation.** Commit `a14daea` and
the two after it. The description is the "Observations" section of the
header of `core/src/static_cache.rs`.

What shipped:

- **A log per segment.** Where the lowering preserves observations
  (`StaticLayout::observes`) a segment that executes writes down every
  observation made while it does, at the five places the VM records one,
  the replay of a memoized scope included. Its `StaticStore` keeps the
  last value of each term (`StaticSlots::keep_log`); a taken `StaticSkip`
  records the log again where the segment stood, and a group skip those of
  its segments in order. The buffer is last write wins and nothing else
  runs inside a segment, so one value per term is exact. The log is one
  more thing a skip waits for (the per-run pending count), so a segment
  with values and no log executes. Logged values are GC roots.
- **Reloads.** A reload that leaves the lowering in place (moved text, a
  patched literal, `set_config_value`) hands on the log of a segment when
  every slot it stores and reads is kept and the edit reached nothing any
  statement in the segment depends on (`RunFollow`, per run, beside the
  per-slot `SlotFollow`). A segment whose rebuild a drag put off keeps its
  log with its old values. A recompile hands on none (a log names terms of
  the old program): each kept segment executes once on the next frame for
  its log, and the store puts the kept values back in the registers and the
  kept record of an enum back in the enum table. The reload report is the
  one given with observations off
  (`a_recompile_keeps_the_values_and_writes_the_logs_again`).
- **The per-term trace still turns the cache off** (`explain`, a Garden
  pane being traced by the Petal IDE). It did not fall out: the trace
  records every instruction with its inputs, not one value per name.
- **Garden.** Nothing to switch on: `PanelHost` enables observations, and
  that no longer disables the cache. A panel reload prints the
  `[hot-reload] constants: …` and `stale seed` lines (the call was already
  there and had nothing to say), `PanelHost::last_reload_report` returns
  them, and a value that cannot be kept is printed once per load
  (`[petal warning]`), as the SDL host does.

Two things found on the way, both by the differentials:

- **In-place edits.** The optimizer edits a container in place where the
  program can no longer reach the old value, but the observation buffer
  holds ids, so it shows the edit under the old name: `let BASE = [..]`,
  `let cur = drop_last(BASE)` shows the short list as `BASE` with the cache
  off. `BASE` is rebuilt every run (it reaches a write) while `cur` was
  kept, so a frame that skipped `cur` showed the whole list. Under
  observation a statement holding an in-place instruction whose container
  another statement wrote (back through moves and match bindings) is not
  skippable. This is the one thing kept without observation and not with
  it.
- **A name with no slot.** `let K10 = K9` stays in the code under
  observation for its name alone, has no slot, and sits in the segment of
  whatever is stored next. Handing a log on by what the segment's slots
  depend on served a stale `K10` (`static_fuzz` seed 60). Hence `RunFollow`.

Why equality holds, and what it rests on. The cache-on code is the
cache-off code plus the skip and store instructions, so the set of
observed terms is the same. A skipped segment differs from an executed one
in four ways only: registers (slots), the enum table (slots), observations
(the log), and in-place edits to containers. The last is exact for a
container the same statement built (the log names it in its final state)
and is ruled out otherwise. The other direction, code outside a segment
editing a kept object in place where the cache off would show the edit,
is closed at the root level by the existing rule that a register reaching
an in-place instruction has no slot. Inside functions it rests on the
escape analysis never editing in place an object the function did not
allocate; that was not proved from `escape.rs`, only tested: the routes of
`no_route_writes_through_to_a_kept_value` and 16 dead-name cases
(`a_container_edited_in_place_is_observed_the_same`) under four policies,
and both fuzzers. If a case turns up it will be a kept table shown
unedited under a name the program no longer reads, where the cache off
shows it edited: a difference in the debug view, not in any value the
program computes.

Tested (all with observations on, cache on against off, by name and term
by term):

- Core, debug: `cargo test --no-fail-fast`, 64 binaries, 2,584 passed, 0
  failed, 16 ignored. `same_on_and_off` (`static_cache.rs`) and
  `follow_modules` (`static_reload.rs`, now three stacks: followed,
  rebuilt, no cache) run every case of theirs observed as well;
  `core/tests/static_observe.rs` is 11 new cases.
- `static_fuzz` and `static_cache_fuzz`, observed and not, 1,500 seeds each
  in release.
- The 55-app corpus (`core-libs/petal-ui/tests/static_cache.rs`,
  `static_reload.rs`, debug): 45 frames under `replay`, `replay-memo`,
  `baseline` and `fast`, and the five-edit chain under `replay`, `fast` and
  `replay-memo` with the third instance.
- Garden's panels and drawers under `PanelHost` itself, 20 scripts, 12
  frames (`every_panel_observes_the_same_with_its_static_values_kept`);
  `cargo test -p garden-script` 182 passed; `cargo check --workspace` in
  `garden/`.
- Each part removed in turn fails a test: the replay, the log's GC roots,
  the in-place rule, the log handed on regardless, the kept value put back
  in the register, the kept enum record published again, the renaming of a
  logged value to the kept one.

Measured (instruction and allocation counts, which do not depend on the
build profile; "off" is what every panel ran before):

- Garden panels under Garden's host, 60 frames, every frame run, no input
  and no host data: 2.4% to 17.8% fewer instructions a frame, 9.8% to 28.9%
  fewer allocations; 30,573 to 28,555 instructions (−6.60%) and 2,438 to
  1,989 allocations (−18.43%) over the 20. plant 1,042 to 918 and 107 to
  83; sketch 1,206 to 1,110 and 88 to 66; git_panel 2,572 to 2,437 and 211
  to 179; main_menu 5,546 to 5,411 and 268 to 242. The table is in
  `docs/dev/performance.md`. The drawers have no data in this run, so a
  populated pane saves the same absolute amount and a smaller share.
- The 55-app corpus observed: 4,206,466 to 4,179,089 instructions a frame
  (−0.65%), 39,359 to 35,015 allocations (−11.04%), 2,945 slots (2,940
  without observation, where the same run gives −0.74% and −10.51%).
- Not measured: wall time, the frame after a recompile (it executes every
  kept segment once, so it costs about a cache-off frame), memory held by
  the logs.

Open:

- A recompile costs one frame of rebuilding under observation, though the
  values are kept. Mapping old terms to new ones would avoid it; the
  reload does not have that mapping.
- The log holds every local of every function a segment called, for as
  long as the segment is kept: a function that builds a large scratch
  table and returns a summary keeps the scratch table alive. Without the
  cache the buffer held it until the next run.
- The in-function half of the equality argument is tested, not proved
  (above).
- Not run: the petal-ui suite beyond its two static corpus files, clippy,
  vitest, `tools/verify.ts`, the release-profile core suite as a whole,
  `garden-app`'s tests and `petal-fantasy-nes` (disk), a real Garden
  session.

### Later: constants inside function bodies (2026-10-13)

**Status: measured, then done.** The description is the header of
`core/src/static_cache/in_fn.rs`.

#### The measurement that came first

The plan says to do a "Later" item only on evidence, and the survey's count
(279 sites) was a text scan. So this was measured before anything was
written, with a tool that is now in the tree:

- `core/src/static_survey.rs` finds, from the classifier's term classes,
  every **invariant expression** inside a function: static, and reading
  nothing of the call (no parameter, no loop or pattern variable of a loop
  that is not itself invariant, no capture of a per-call value, no `state`,
  no `var`). The classifier alone is not that: it classifies a function
  body *assuming its parameters are data*, so `[a, 1]` is static to it.
  Counted are the maximal ones that allocate, call or loop; left out are the
  ones whose value reaches a write (`let out = []` in front of a loop that
  appends).
- A counter on the profiler (`VmProfile::sites`, the hooked dispatch path
  only) charges every retired instruction and every allocation to the
  expression it is under, the functions it calls included, and counts how
  often each is entered. A slot per expression would leave out everything
  but the first entry, at one load each.
- `core-libs/petal-ui/tests/static_in_functions.rs`
  (`measure_constants_inside_functions_on_the_corpus`) runs that over the
  55-app corpus, 60 monkey frames, `replay`; `core/examples/static_survey.rs`
  counts from the text for a script whose host is not here.

Panel corpus (release, every frame run, the top-level cache on, nothing
kept for function bodies):

| | Invariant expressions | of them run | Instructions a frame | a slot each would save | Allocations a frame | a slot each would save |
|---|---|---|---|---|---|---|
| all 55 apps | 5,899 | 704 | 3,643,852 | 9,615 (0.26%) | 37,059 | 2,692 (7.26%) |
| dashboards/server-monitoring | 93 | 14 | 43,032 | 2,087 (4.85%) | 2,759 | 698 (25.29%) |
| productivity/markdown-editor | 109 | 16 | 73,869 | 1,244 (1.68%) | 911 | 250 (27.42%) |
| productivity/photo-adjust | 148 | 63 | 321,019 | 4,923 (1.53%) | 4,589 | 1,231 (26.82%) |
| productivity/spreadsheet | 112 | 16 | 177,972 | 349 (0.20%) | 760 | 116 (15.26%) |
| ui/bloom-gallery | 176 | 36 | 21,349 | 88 (0.41%) | 666 | 70 (10.56%) |
| garden panels: gallery, git_panel, db_view, screens-demo | | | | 0.2% to 0.8% | | 5.3% to 7.1% |

(1,047 more whose value reaches a write and 94,455 that allocate and call
nothing were left out. 83 of every app's expressions are the UI prelude's.)

The rule set for this step was: do not implement if the estimate is under
about 2% of instructions **and** under about 5% of allocations on every
program measured. It is under 2% of instructions on all but one
(server-monitoring, 4.85%) and **at or over 5% of allocations on 11 of the
55**, over 25% on three. So it was implemented. What those three do: a heat
map hands `color_lerp` two colour literals per cell, a markdown scanner
builds its `{kind: "none", ml: 0, close: -1, url: ""}` once per character
it looks at, and an image generator builds its palette with
`fc(0.90, 0.93, 0.96)` once per pixel.

The Cheesecake games cannot run here (their dynamic code calls the engine's
natives), so they have no weights, only counts
(`static_survey ~/cheesecake/games/*/game.ptl`, read only; a native the
engine would register counts as dynamic here, so these are floors):

| | Functions | Invariant expressions | terms in them | allocating terms | Kept: slots | stretches | instructions | allocating instructions |
|---|---|---|---|---|---|---|---|---|
| moses | 1,573 | 2,078 | 16,662 | 3,336 | 4,381 | 1,666 | 16,322 | 2,758 |
| moonbear | 909 | 1,380 | 13,203 | 1,903 | 2,011 | 1,028 | 12,478 | 1,809 |
| solitude | 1,058 | 910 | 5,907 | 924 | 1,100 | 782 | 5,086 | 875 |
| neon | 831 | 749 | 5,889 | 1,081 | 782 | 527 | 4,145 | 752 |
| all 13 | 8,330 | 7,510 | 63,730 | 11,011 | 11,217 | 5,858 | 57,972 | 9,652 |

For scale: a Moses frame was 83,119 instructions in the baseline profile.
If each of its 1,666 stretches ran once a frame the cache would leave out
14,656 of them and 2,758 allocations; how often they really run needs the
engine (follow-up 1). The static part of a Moses frame does not reach them
at all with the cache on: the functions it calls are behind kept values.

#### What shipped

- **`FnPlan`** (`in_fn.rs`): which terms inside functions are invariant.
  A fixed point from below (nothing is invariant until shown to be). A
  parameter never is; a capture is when it is a kept top-level value or a
  function at every place the closure is made, or invariant in the function
  that makes it; the function's own name is. A loop or pattern variable
  never is on its own: a loop, branch or match is invariant as a whole when
  everything under it is, given its own variables, and what is under it is
  then kept with it or not at all. A name a loop or branch rebinds is
  invariant behind it when the loop or branch is. `state`, `var`, `return`,
  `break` and a closure never are. A forward reference to a function
  declared further down goes through a cell and is left out with them.
- **The pass** (`in_fn::apply`, after the top-level one, over every
  function's code as it finally is). An instruction is a candidate when its
  term is invariant *and* it is clean: every register it reads is written
  only by candidates (or is an invariant capture, or the function's own
  name), and nothing else writes what it writes. A maximal sequence of
  candidates that control enters at its first instruction and leaves at its
  end, that allocates (or is more than one call), and none of whose stored
  registers reaches a write (the term-level set of `StaticPlan`, now also
  `StaticPlan::mutated`, and the register-level in-place check) gets a
  `StaticSkip` in front and a `StaticStore` behind: **the same two
  instructions as the top level, and no instruction moved**. What a
  stretch ends in that builds nothing (the constants of the dynamic code
  behind it) is trimmed off first.
- **The slot belongs to the place in the program.** Slots and runs are
  appended to the lowering's layout behind the top-level ones
  (`StaticLayout::root_slots`, `root_runs`), so they are in the stack's
  table, stamped, cleared and collected with it. In a function frame the
  skip loads into that call's registers and the store marks the value
  immutable and fills the slot; nothing is timed or logged, and a build in
  progress in the root code is left alone.
- **What a function returns for its caller to fill is not kept.** `let a =
  build()`, then `a[i] = v` in a loop: the escape analysis lets the caller
  write in place because the list is new. Kept, the first write of every
  call would copy it, which is the work the slot saved, a second copy held
  for good, and a copy where `core/tests/copy_elision.rs` requires none.
  So a function some caller writes to the result of (by name, at the call
  site: `WrittenResults`) keeps nothing that can come out as its result.
- **The escape analysis is not told about these values**, unlike the
  top-level ones. Where a caller writes to a kept value by a route the
  rule above does not see (through another name for the function, a
  callee that writes its parameter), the immutable bit makes the write a
  copy and the next write edits the copy in place. Telling the analysis
  would turn every such write into a copy.
- **Reloads.** Not followed: every reload rebuilds every function slot (no
  key and no dependency set, so `static_follow` finds each dirty, and it
  is seeded empty outright). A paused drag empties them with what it
  rebuilds. They are not counted in the reload report, which is about
  top-level declarations.
- **Not under observation.** Where the lowering preserves observations
  (Garden's panels) nothing is kept for function bodies: a skipped stretch
  would have to report the names bound inside it, per call. The top-level
  cache there is as it was.
- **A check, and a switch.** `Env::set_static_check(true)`
  (`PETAL_STATIC_CHECK=1` for every `Env`) makes every function skip fall
  through and every store compare what it built with what the slot holds
  (`static_cache::identical`); a difference is listed in
  `StaticValues::mismatches`. `OptFlags::static_in_functions` (the
  `-fnstatic` policy modifier, as in `replay-fnstatic`) switches these
  slots off alone. `StaticValues::in_functions` lists them;
  `StaticValues::slots`, `runs` and `segments` are the top level's, as
  before.

#### What it found: an in-place write into another value

The new fuzzer's seed 245 diverged with the cache **off**: a kept top-level
table came out edited. The cache was the side that was right. A name a
branch rebinds to *another* value in front of a loop that writes to it,

```petal
let b = sort([3, 1, 2])
if p % 2 == 0 then
  b = other
end
for i in range(0, 3) do
  b[i] = (b[i] + 17) % 97
end
```

wrote into `other` in place, under the optimizer, with or without this
plan's cache (`fast-static`): `other` could be a parameter, a captured
table or a top-level `let` with a second name on it, and its owner saw the
writes. The route-B escape analysis passed the arm as one that "carries
the value through" because it performs no mutation, and its closed-sources
rule looked at the web's phis and not at the copy in the arm. The copy is
checked now (`escape.rs`, condition 2). `core/tests/copy_elision.rs` has
the four shapes under four policies; the benchmark instruction and
collection counts are unchanged (life 3,913,944; spreadsheet 20,449,651
and 9; audio_synth 621; particles 4), so nothing that was in place stopped
being. With the cache on, a kept `other` had hidden it: the immutable bit
turned the write into a copy.

#### Measured

Panel corpus, release, 60 monkey frames, `replay`, the cache without
function slots (`replay-fnstatic`) against the cache with them
(`measure_what_keeping_them_saves_on_the_corpus`):

| | Function slots | Instructions a frame, without | with | Allocations a frame, without | with |
|---|---|---|---|---|---|
| all 55 apps | 6,254 | 3,643,851 | 3,633,409 (−0.29%) | 37,058 | 34,437 (−7.07%) |
| dashboards/server-monitoring | 99 | 43,032 | 40,595 (−5.66%) | 2,758 | 2,059 (−25.35%) |
| productivity/markdown-editor | 102 | 73,869 | 72,625 (−1.68%) | 910 | 663 (−27.19%) |
| productivity/photo-adjust | 153 | 321,018 | 315,744 (−1.64%) | 4,589 | 3,359 (−26.81%) |
| productivity/spreadsheet | 112 | 177,971 | 177,627 (−0.19%) | 759 | 645 (−15.11%) |
| productivity/email-client | 167 | 11,700 | 11,570 (−1.11%) | 572 | 536 (−6.31%) |
| garden/examples/panels/gallery | 89 | 5,204 | 5,182 (−0.41%) | 301 | 280 (−7.09%) |

That is the estimate (−0.26% and −7.26%): the survey run again with the
slots on finds 75 allocations a frame still under an invariant expression,
0.22%. The whole cache against no cache on the same run is now −1.03% of
instructions and −16.84% of allocations over the sum (it was −0.74% and
−10.51%), median app −5.4% and −25.3% (was −4.6% and −19.1%), 2,940
top-level slots as before.

What it costs:

- **Nothing is gained in 18 of the 55** (a frame runs no fewer
  instructions and makes no fewer allocations, in whole numbers), and 9
  reach 5% of allocations where the estimate said 11. In 12 apps the run
  as a whole executes a few instructions more (at most +0.02%) or makes a
  few more string allocations (below): a stretch that runs once pays its
  skip and its store and is never loaded.
- **A recompile**: 26 ms of 481 on Moses (455 with `fast-fnstatic`, 396
  with the cache off) and 8 of 186 on neon, half the plan and half the
  pass. It was 62 ms before the plan's tables were hashed with the fast
  hasher, the mutated set was shared with the top-level plan and functions
  with nothing heavy and invariant were skipped.
- **Slots**: 89 for an app that only imports the UI prelude, 4,381 for
  Moses. A slot nothing reaches costs its entry in the table.
- **String allocations move by a few either way** in some apps (layers
  179 to 207 over the run, sysmon 137 to 132) with the same number of
  collections. An interned string is allocated again after a collection
  that found it unheld, and collections now fall at other instructions;
  that is the reading, and it was not chased further.
- **`freeze` can tell**: `freeze(mk())` freezes the one kept list, and
  `frozen(mk())` is then true where every call used to make a new one. The
  same difference the top level has, pinned by
  `freezing_what_a_function_returned_freezes_the_kept_value`.

Not measured: wall time, memory held by the slots, the frame after a
reload (it builds each constant the frame reaches, once), anything on a
Cheesecake game under its engine.

#### Tested

- `core/tests/static_in_functions.rs`, 24 cases, each driving the same
  frames with the cache on, off, and on and checking itself, under five
  policies, and once more observed: what is kept (a record of literals,
  constant arguments of a dynamic call, a pure call on constants with what
  it calls, a constant built from a kept top-level value, a loop over
  constants) and how much it saves; ten things that look constant and are
  a value of the call (a parameter, through a local and a pure call, a
  loop variable in and out of an invariant loop, a pattern variable, a
  name a branch on a parameter rebinds, an accumulator, `state`, `var`, a
  default); a closure made per call; a capture of a top-level value that
  is not static; values written to inside the function; ten routes by
  which a caller or a callee writes to a kept value; code that raises;
  recursion, with a function that reaches its own constant while building
  it; memoized calls; a fork; collections; a literal edited inside a
  constant, a constant built from an edited top-level value and one that
  calls an edited function, each through patched and recompiling reloads
  and against the reload that rebuilds everything; a drag with a deferred
  slow table; the `freeze` difference; the switch.
- `core/src/backend/bytecode/static_fn_fuzz.rs`: a generator of function
  bodies (the header lists what it writes), three texts per seed through
  three reloads, the three sides under four policies. 3,000 seeds in
  release, clean, after the escape fix (seed 245 before it).
- `core-libs/petal-ui/tests/static_in_functions.rs`: the 55-app corpus, 30
  frames, the three sides under four policies
  (`function_constants_change_nothing_across_the_corpus`).
- Checks of the checks, each rule taken out in turn: parameters taken to
  be invariant fails 12 of the 24 cases and the fuzzer's first seed, and
  at the term level alone (the register check left in) still fails 2 and
  seed 12; loop variables taken to be invariant with the register check
  out fails; a rebound name invariant whatever rebinds it fails; a reload
  that patches a literal keeping function slots fails three reload cases
  and the fuzzer. Three removals fail nothing, because another rule
  covers them: loop variables invariant *with* the register check in (the
  loop's own instruction taints the register), a reload keeping these
  slots like any other (with no dependency set every slot is dirty
  anyway), and a paused drag not emptying them (they are in the redo set
  already). One could not be tested at all: a capture invariant at *one*
  of the places a closure is made instead of all, since no program here
  makes one function's closure in two places.
- Oracles on the final tree (`e284f9f`; debug unless said): core `cargo
  test --no-fail-fast`, 65 binaries, 2,612 passed, 0 failed, 16 ignored;
  every fuzzer in the library at 3,000 seeds in release (the general
  differential fuzzer and the two cell fuzzers, which is what the escape
  fix answers to, `static_fuzz`, `static_cache_fuzz`, `static_fn_fuzz`);
  `core-libs/petal-ui`, the whole suite, 21 binaries, 300 passed;
  `integrations/petal-c-bridge` 20 and `make test-c-bridge` 4 of 4;
  `integrations/petal-desktop-sdl` 54; `garden-script` 182;
  `./tools/test-examples.ts` 33; vitest, 75 files, 1,571 passed;
  `./tools/verify.ts --plan compiler` with
  the UI driver under `fast-static` on one side and the default on the
  other, 139 identical traces, 19 modules, 145 unsupported, nothing
  changed.

#### Open

- **Garden's panels keep nothing for function bodies** (observation). The
  top-level log could be extended per stretch; nothing was designed.
- **Reloads are not followed for these slots.** Every reload costs each
  constant one rebuild at its next use. S2's rule would need the slot
  matched across lowerings (a key from the function's text and the term's
  place in it) and the function's dependency set.
- **Calls of functions declared further down are not kept**: the forward
  reference is a cell read, and a cell read is never invariant here. The
  classifier already knows such a cell is written once.
- **One stretch is one run**, with every register something outside reads
  in its store: no cut per value as at the top level, so one value that
  cannot be kept (a handle) takes the stretch with it.
- **A function is "written to" by name.** A method named like any method
  some caller writes to the result of keeps nothing it returns.
- **The equality rests on the immutable bit** wherever a kept value
  leaves its function, as it does for top-level values handed to
  functions. Tested by ten routes and the fuzzer, not proved.
- **Not run**: the release-profile core suite as a whole and its ignored
  oracles (the whole-corpus reload oracle, the small-scope sweep, the
  sharing generator beyond its default seeds), any suite under
  `PETAL_STATIC_CHECK=1` beyond the three that set it themselves,
  `--static-audit` by hand, `tsc`, clippy, `garden-app`'s tests and
  `petal-fantasy-nes` (disk), a real Garden session, any Cheesecake game
  under its engine.

### Review of the phase after S2 (2026-10-13)

An independent pass over the four pieces above: a store per value
(`800ff90`), the cost of a reload (`f03dab6`, `489ea39`), the cache under
observation (`a14daea`, `0dcd9d2`) and constants inside function bodies
(`4dbced6`, `e284f9f`), aimed at a kept value that is stale or wrongly
shared. **No defect was found in the cache.** Two things were wrong around
it and are fixed; the measurements were run again and agree.

**Attacked**, each attempt against the cache off, against the cache
checking itself, and against a reload that rebuilds everything, under
`replay`, `replay-memo`, `fast`, `baseline+memo` and `baseline+static`,
with a host observing and without (`core/tests/static_review3.rs`, 16
cases, about 120 programs):

- *Function bodies.* A value of the call reaching a constant through a
  closure made per call, at one and two removes (`make(k)` returns `user`,
  which captured `inner`, which captured `k`: two closures of one function
  share its slots), through `map`, a function handed in, a function in a
  table or a list, a function another function returns, and a top-level
  function that captured a value of the frame. A parameter, a local, a
  loop variable and a default named like a kept table; a default that is
  written to. A name a branch or loop of the call rebinds. Writes through
  a loop variable that is an element of a kept list, through `state`,
  through a `var` written inside another function, through a method that
  writes its receiver, through enum data. A name rebound below the
  function that captured it. A host call of the function between frames
  and straight after a reload. A failed reload between two good ones. A
  reload that makes a constant depend on the call (a literal replaced by
  the parameter, the parameter renamed to the table's name, the table made
  dynamic, a local that shadows it). Two stacks and a fork of one program
  with a literal patched through each in turn, then a restore.
- *A store per value.* A kept table inside a rebuilt one, written through
  every name the program has for it, across four edits and a
  `set_config_value`. A frame that raises between two values, then edits
  to the one in front and to the index that raised.
- *The kept parse and classification.* Read, not only run: an entry of
  `ParseCache` is found by the whole text and the file id, compared byte
  for byte, so there is nothing to go stale; `KeptAnalysis` is dropped by
  the two reload paths that change a program in place and by
  `get_program_mut`, a recompile starts with none, and every way of
  changing the native table draws a new revision from the process-wide
  counter. The failed-reload and two-stack cases above go through both.
- *Observations.* One function called from three kept segments and from
  dynamic code between them, through six reloads (a patched table, a
  patched literal beside a call, an edit inside the function, a
  recompile) and a `set_config_value`: the locals a host reads are those
  of the last call, term by term. A host that starts and stops observing
  while the stack lives. 24 shapes that hand a table nothing reads again
  to something that may edit it where it is (a callee that writes its
  parameter, at one and two removes; `append`, `++`, `sort`, `reverse`,
  `map`, `filter`; through a holder, a second name, a branch, a loop),
  each with and without a later reader, through a recompile: the table is
  observed the same under its dead name.
- *The optimizer fix* (`4dbced6`). Fourteen relatives of the shape it was
  for (the accumulator rebound to an element of another value, a field,
  the result of a call; by both arms, a nested branch, a branch as an
  expression, a `match`, a `for`, a `while`; through a second name;
  appended to instead of written): the optimizer on prints what
  `baseline` prints.

**Checked that the new cases bite.** With every capture taken as
invariant at the closure site (the one rule the first tests could not
reach, since no program there made one function's closure in two places),
`a_value_of_the_call_reached_through_a_function_is_never_kept` fails.
Taking every *top-level* capture as invariant fails nothing, and should
not: a dynamic one is already `Dynamic` to the classifier, and a static
one that is written to is the same value every frame and is protected by
the immutable bit once a kept value holds it.

**Fixed.**

- `core/tests/lsp_tests.rs` had the hover expectations for a store per
  value sitting uncommitted in the working tree (each step took it for
  another's), so `main` as committed failed three hover tests. Committed.
- `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` failed
  in release when the ignored whole-corpus oracle ran beside it
  (`--include-ignored`): 3.4 ms against 11.5, where alone it is 1.2
  against 11. Nothing was slower; the minimum of three rounds of a 1 ms
  reload does not survive a loaded machine. It takes the minimum of 12
  for the two cheap edits now, and passed three runs beside the oracle.
- The docs said every reload empties the function-body slots. One that
  only moves text does not reach the table at all and leaves them, which
  is right (no code and no constant changed);
  `a_reload_that_only_moves_text_leaves_function_constants_and_any_other_empties_them`
  pins both halves, and `in_fn.rs`, `hot-reload.md`, `performance.md` and
  the changelog say so.

**Measurements run again** (release, load average 7 to 17, so counts and
not times):

| | Reported | Run again |
|---|---|---|
| Corpus, whole cache against none: instructions, allocations | −1.03%, −16.84% | −1.03%, −16.84% (3,671,125 to 3,633,409; 41,411 to 34,437) |
| Corpus, function slots alone | −0.29%, −7.07% | −0.29%, −7.07% (6,254 slots) |
| Corpus under observation | −0.65%, −11.04% (2,945 slots) | −0.65%, −11.03% (39,361 to 35,019; the allocation counts move by a few strings between runs) |
| snake, later frame | 2,754 instructions, 90 allocations | the same |
| Garden panels under `PanelHost`, sum | 30,573 to 28,555; 2,438 to 1,989 | the same |
| Moses as written: slots, checks, later frame | 451, 42, 2,641 / 45 / 1 | the same |
| Moses: frame that builds everything; frame after an edit to one table | 9,217; 2,681 | 9,217; 2,681 (450 kept, 1 rebuilt) |
| Classification alone, neon | 14.4 ms | 14.8 ms |
| neon, an edit to `config.ptl`: relocated, patched, recompiled | 0.87, 1.00, 186 ms | 0.86, 1.08, 185 ms |
| neon, `set_config_value`: file named, searched for | 1.03, 2.72 ms | 1.07, 2.73 ms |
| The timing test, release, alone | 1.20 to 1.29 ms against 10.5 to 11.3 | 1.17 to 1.32 against 10.7 to 11.7 |

**Tests on the final tree** (all green): core debug, 66 binaries, 2,626
passed, 16 ignored (before the last two cases were added); core
**release, whole suite**, 66 binaries, 2,628 passed, which nobody had run;
`hot_reload` in release with `--include-ignored` (49, the whole-corpus
reload oracle among them) three times; every fuzzer in the library at
3,000 seeds in release (`differential_fuzz`, `ir_round_trip_fuzz`, both
cell fuzzers, `static_cache_fuzz`, `static_fuzz`, `static_fn_fuzz`),
which closes "the 1,500-seed run predates the last two changes" under
"Later: Garden"; the sharing generator at 2,000 seeds in release (60 s);
petal-ui whole suite, debug, 21 binaries, 300 passed; petal-c-bridge 20;
petal-desktop-sdl 54; garden-script 182; `./tools/test-examples.ts` 33;
`petal run --static-audit --static-only` over the thirteen Cheesecake
games that load here, no finding.

**Still open after this review.**

- What the builders left open and this pass did not take: function-body
  slots are not followed through a reload that patches or recompiles, are
  not kept under observation, have no cut per value and no
  `--static-audit` coverage; a recompile under observation executes every
  kept segment once to write its log again; classification is from
  nothing on every recompile; the per-term trace turns the cache off.
- The equality of a kept value that leaves its function, and of an
  in-place edit inside a function under observation, rests on the
  immutable bit and the escape analysis. This pass added about sixty
  programs to the evidence and no proof.
- The sharing generator's default is still 40 seeds, where a removed rule
  needs up to 400 to show: 400 would cost about two minutes of a debug
  run. The unit test beside it catches the same removal at once.
- `examples/panels/shapes.ptl` raises under `PanelHost` in the Garden
  measurement, the same with the cache on and off; not looked into.
  (**Looked into 2026-10-13**: the test had removed the panel's store.
  Fixed in the test; "What the disk had kept back".)
- Not run: clippy, `tsc` and vitest (this phase changed no TypeScript),
  `tools/verify.ts`, `garden-app`'s tests and `petal-fantasy-nes` (disk),
  a real Garden session, any Cheesecake game under its engine, so the
  function-body slots of Moses (4,381 of them) have still never executed
  there.

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

## Results (12 October 2026)

Measured at `f3d6e67` with this repository's release build, on a machine at
load average 5 to 14. Times are minimums unless said; instruction and
allocation counts are exact. The rows the work of 13 October could move
(a store per value, the cost of a reload, the cache under observation,
constants inside function bodies) were measured again at `4e5b584`, at load
average 4 to 13, and carry that date; the other rows are as measured on the
12th.

### Baseline against now

| Benchmark | Baseline | Now | |
|---|---|---|---|
| Moses headless, 600 frames, under the Cheesecake engine | 49,871,337 instructions; 2.71 ms a frame; 14 collections; 406 `StateInit` a frame | **13 October, Cheesecake `fa36156` (Petal `4269f9f`), adopted and reviewed** (three `PB_FX_CONST` natives, 60 tables as `const`, `F_PEOPLE` dropped, the skins a constant): **46,469,822 instructions (−3,401,515, −6.8%; 77,450 a frame against 83,119)**; 2.60 to 2.64 ms a frame with the profile (three runs, load average 2 to 4); 11 collections; 353 `StateInit` a frame (211,680); record literals built by script code 196,762 against 516,335 with the cache off (−61.9%). **As the adoption left it, `fd51d84`:** 46,506,542 (−6.7%; 77,511 a frame); 2.60 to 2.74 ms with the profile and 2.02 to 2.13 without (the adoption's and the review's runs); 10 or 11 collections; 212,220 `StateInit`. **Before the adoption, Cheesecake `03319c2` (Petal `518c008`), no script changed:** 46,543,802 instructions (−3,327,535, −6.7%; 77,573 a frame against 83,119); 2.55 to 2.59 ms a frame with the profile and 2.00 to 2.03 without (5 runs each); 10 or 11 collections; 406 `StateInit` a frame (243,540, unchanged) | The cache is nearly all of it: the same binary under `PETAL_STATIC_AUDIT=watch`, which turns the cache off, runs 49,847,577. `LoadConst` 9,924,893 to 7,647,726. The time is not a same-session comparison: the pre-plan engine measured that day ran 4.24 to 11.60 ms at load average 5 to 21, and that binary no longer exists. The adoption itself is −37,260 of the total (58 fewer `StateInit` a frame); the constant natives moved nothing in Moses. With the cache off the adopted scripts run 58,933,077 (98,222 a frame), which is why the rewrite had to wait for the cache. "Cheesecake: the adoption" and "Review of the adoption" below |
| …the static part of a Moses frame (`static_frame`, the substitute) | 8,065 instructions, 1,230 allocations, 482 records (cache off) | **13 October:** 2,641 instructions, 45 allocations, 1 record; 451 slots in 451 segments, 42 checks a frame; 262 µs to 174 µs (12 October: 2,674, 58, 2; 426 slots in 34 runs) | −5,424 instructions and −1,185 allocations a frame against the −4.1k the proposal estimated by hand; 0 audit findings. The frame that builds everything is 9,217 against 8,284 with the cache off |
| …the same with the lint's rewrite applied (76 `state` to `const`, scratch copy) | 25,804 instructions, 4,687 allocations (cache off) | 2,566 instructions, 46 allocations; 529 slots | Shows why the rewrite must wait for an engine with the cache on |
| Example panels in the Cheesecake headless host | snake 2,617 instructions a frame; analytics-dashboard 11,005; network-graph 153,493; settings 31,624 | **13 October, same engine:** snake 2,396 (−8.4%); analytics-dashboard 10,703 (−2.7%); network-graph 152,125 (−0.9%); settings 30,371 (−3.9%). **The same on the adopted engine** (Cheesecake `fd51d84`, Petal `4269f9f`, taken by the review): every total and every collection count identical | Totals over 600 frames: 1,570,122 to 1,437,822; 6,603,120 to 6,421,680; 92,093,804 to 91,275,164; 18,952,538 to 18,222,458. Collections 34 to 28, 35 to 32, 100 to 89, 235 to 229. On the adopted engine: 0.16, 0.59, 2.58 to 2.62 and 1.87 to 1.88 ms a frame with the profile (one or two runs each). No panel calls an engine native that became constant, so the adoption could not move them. The last two `import garden`, which the engine does not provide; both sides ran against a package of four do-nothing functions, which puts their baselines 3 and 36 instructions a frame under this row's (153,490 and 31,588) |
| …the same four under petal-ui's driver, cache off against on (60 monkey frames) | snake 2,992; analytics-dashboard 29,817; network-graph 783,883; settings 36,727 | **13 October:** 2,754 (−8.0%); 29,477 (−1.1%); 782,380 (−0.2%); 35,389 (−3.6%) (12 October: 2,773; 29,513; 782,445; 35,571) | All 55 apps: −1.03% instructions (3,671,125 to 3,633,409), −16.84% allocations (41,411 to 34,437), 2,940 top-level slots and 6,254 in function bodies (12 October: −0.73%, −10.03%, 2,594 slots). The function slots alone: −0.29% and −7.07%. Under observation: −0.65% and −11.03%, 2,945 slots. Garden's 20 panels under `PanelHost`: 30,573 to 28,555 instructions (−6.60%), 2,438 to 1,989 allocations (−18.43%) |
| Enum reads (A1), ns over the empty loop | local 51.3; `K.v.code` 71.3; `K.all` 227.8 | 3.5; 6.5; 18.7 (record field 1.8, payload 11.3, walk of `all` 16.7) | |
| 50-variant enum with data, per run (S1) | 9.54 µs before A1, 6.67 µs after it | 2.46 µs (2.00 µs for the enum with no data) | |
| GC pause with a rooted 100,000-record table (A2) | 10.0 ms a collection in the proposal's script; 13.5 ms in `tools/bench-gc-pause.ts` | frozen 0.20 ms (rooted 15.1 ms in the same run); one collection alone 2.29 ms rooted, 0.009 ms frozen | A static value is not frozen: `freeze` is still the author's call |
| A `state` hit (A3) | 16.9 µs for 200 top-level; 57.0 µs for 200 in a function | 9.6 to 10.3 µs; 38.6 to 40.3 µs (medians 10.5 to 10.9 and 43.8 to 44.7) | |
| A 400-record `let` table as a memo argument | 58 misses in 30 frames | 0 misses, 29 hits in 29 frames (29 misses with the cache off) | `a_large_table_as_a_memo_argument_stops_missing` |
| `petal check` on Moses | 587 ms | 540 ms (542 ms on 13 October) | No `const` in the game as written, so no classification |
| Patched reload, `reload_timing` on `games/neon` | about 0.8 ms (when neon was 14 files, 62,733 terms) | **13 October:** 0.99 ms patched, 0.85 ms relocated, 187 ms recompiled; `set_config_value` 1.04 ms with the file named, 2.71 ms searched for (12 October: 1.5, 1.3, 183; 35 files, 77,853 terms) | Not comparable with the baseline: the game grew and no pre-plan binary exists. On today's binary the cache is nothing measurable of the patched reload (0.96 ms with it off) and 31 ms of the recompile (156 ms off; 175 ms without the function-body slots, so they are 12 of the 31, and the classification is 14.4). Moses: 1.88, 2.75 and 488 ms (386 ms off, 455 ms without the function-body slots, classification 47 ms) |
| Structural edit on Moses, reload + lowering + next frame (S2) | 152.9 + 382.9 + 0.58 ms, rebuilding all 426 | **13 October:** 494 + 0 + 0.36 ms, 451 kept; one table changed: 450 kept, 1 rebuilt, 2,681 instructions; its value alone changed (patched): 3.2 ms, 450 kept, 1 rebuilt (12 October: 540.5 + 0 + 0.39 ms, 426 kept) | The next frame runs 2,642 instructions instead of 9,218 (with follow off: 151 + 337 ms and every value rebuilt) |
| `test/benchmarks/*` under `petal run --seed 1`, minimum of 15 | A1's same-day pre-plan binary: append 6.1, arith 12.3, audio_synth 110.6, calls 29.8, life 15.0, match_dispatch 317.8, particles 11.2, spreadsheet 205.9 ms | 6.5, 11.9, 105.4, 29.1, 14.2, 312.0, 10.8, 209.2 ms; **13 October:** 6.6, 12.0, 106.2, 29.3, 14.3, 309.8, 11.0, 209.6 ms | No regression (the second run is within 2% of the first on every script, at load average 5). life 3,913,944 instructions, spreadsheet 20,449,651 and 9 collections, audio_synth 621 collections, particles 4: unchanged from before the plan, on both days |

The table in `docs/dev/performance.md` (append 5.2 ms … spreadsheet 201.4)
is older than the commit that made `petal run` check a program first, and
every script is about 1.2 ms above it for that reason, on the pre-plan
binary as well.

### Cheesecake, under its engine (13 October 2026)

**Status: re-vendored and measured; the adoption was not started when this
was written, and was done later the same day ("Cheesecake: the adoption",
below).**
Cheesecake `03319c2` vendors `518c008` (its tree is blob-for-blob this
repository's at that commit, apart from `garden/gpp/Cargo.toml`, the one
recorded local modification). The frozen-generation patch it carried is
gone, and so is `engine/script/packages/cheesecake/memory.ptl`. No engine
source and no game script changed. It does not have `fccbc12` (the in-place
miscompile found later that day, older than the plan) or `1fe4ea4`.

How the first two rows of the table above were taken: the Release engine
(`build/main/engine/app/cheesecake`), the command of "Benchmarks that gate
the work", 600 frames, the profile covering frames 60 on (540 frames; the
per-frame figures divide the total by 600, as the baseline did, so
`StateInit` is 451 for each frame profiled). "Before" is the engine at
Cheesecake `ee34761` (Petal `42142fd`), measured the same day by the step
that re-vendored: instruction counts identical over 5 runs of Moses and 3
of each panel. "After" was taken by the review, at load average 8 to 11:
identical counts over 6 runs of Moses and 4 of each panel. The pre-plan
binary was lost with the disk it was saved on, so the review could not
repeat "before", and no time here is an interleaved comparison.

| | Before | After |
|---|---|---|
| Moses: instructions | 49,871,337 | 46,543,802 (cache off, under the audit: 49,847,577) |
| Moses: `LoadConst`; user calls; native calls | 9,924,893; 2,058,633; 3,276,665 | 7,647,726; 2,021,350; 3,213,575 |
| Moses: collections | 14 | 10 or 11 (it varies from run to run) |
| Moses: ms a frame, profile on / off | 4.24 to 11.60 / 2.36 to 6.07 (load average 5 to 21); 2.71 with the profile when the plan was written | 2.55 to 2.59 / 2.00 to 2.03 |
| snake: ms a frame, profile on / off | 0.18 / 0.15 to 0.17 | 0.16 / 0.14 |
| analytics-dashboard | 0.61 to 0.62 / 0.48 | 0.58 to 0.59 / 0.46 to 0.47 |
| network-graph | 2.57 to 2.64 / 1.32 to 1.33 | 2.52 to 2.54 / 1.24 |
| settings | 2.06 to 2.09 / 1.77 to 1.80 | 1.86 to 1.89 / 1.61 to 1.63 |

Checked on Cheesecake's HEAD with that build:

- `ctest`, 347 tests (the 344 of A2, plus Rivet's): all pass in one run, 20
  minutes, load average 7 to 9. The two full runs of the re-vendor step, at
  load average 40 to 76, each failed a different handful of timing budgets.
- The 13 games with a `game.ptl`, headless for 300 frames with
  `--dump-state` (and `games/demo/basic.ptl` without the dump), three ways: as
  shipped, under `PETAL_STATIC_AUDIT=watch` (cache off, every write to a
  static value reported) and under `PETAL_STATIC_CHECK=1`. No script error,
  no audit finding, and the dumped state is the same in all three for every
  game (Moonbear's differs only in `bake_ms`, a wall-clock reading).
  `PETAL_STATIC_CHECK` at `518c008` does not print a mismatch, so its part
  of this rests on the state comparison.
- `petal check` (this repository's debug CLI, built a few commits after
  `518c008`) over the 274 tracked `.ptl` files outside `third_party`: the 7
  documented exceptions fail, nothing else.

Not done in Cheesecake, so nothing of it could be reviewed: no native is
declared `PB_FX_CONST` (`hash01` and `hex_color` are the candidates named;
neither implementation was read for this), so every value built with an
engine native is rebuilt each run and no Cheesecake `CONST` row has been
audited; no `state` cell is rewritten to `const`; `T`, `F`, `USE` and
`F_PEOPLE` are still in `games/moses/config.ptl`; the engine prints nothing
of `statics` and calls none of the new bridge functions; no reload was
timed (S2) and no allocation count taken (S1). The step meant to do them
stopped before starting, because the external disk its builds were to go on
was unmounted; the review built nothing for the same reason and ran only
the existing Release tree.

### Cheesecake: the adoption (13 October 2026)

**Status: done, with two things deliberately left (the map's source tables,
the int tables) and the reasons measured.** Cheesecake `de6bff3` (re-vendor
at `baf0328`), `b618c58` (the engine side, committed by the user while it
was in progress), `d8fcc2a` (re-vendor at `4269f9f`), `558ec98`, `c92466e`
(Moses), `52b80e2` (`F_PEOPLE`) and its docs commit. Everything ran on the
internal disk in the existing build trees. Its own account is
`~/cheesecake/docs/moses-debrief.md`, section 7.

**What was done there.**

- **Re-vendored three times**: at `baf0328`, then at `6a102e1` and `4269f9f`
  for the fixes below. Each tree is byte for byte `git archive` of the
  commit apart from `garden/gpp/Cargo.toml`.
- **Three natives are `fx::Const`**: `hash01`, `hex_color`, `sweep_hits`,
  the only three that were `fx::Pure`. Each implementation was read: they
  compute from their arguments and read no host state. Every other engine
  native carries `Effect` or a `Reads*` flag and was left alone. Moses's
  instruction count did not move with them (46,543,802 before and after).
- **The engine reports**: the reload line says `constants kept: N, rebuilt:
  M (NAMES)`; one line per stale seed (at most twelve); the warnings of
  `pb_vm_static_values_json` once per load; `pb_vm_flush_static_rebuilds`
  when a Tuning drag ends; the agent's `reload` reply carries `statics`;
  and a `const` that is not constant is logged with its reason chain.
- **Moses**: the lint offered 76 cells; 60 are `const`. `F_PEOPLE` is gone.

**What it found in this repository** (each fixed here with a test and
re-vendored):

- **A C host could not see a `const` that is not constant** (`6a102e1`).
  The check adds an error to the program's diagnostics and the program runs
  on, the binding a `let`. `petal check` and the SDL host print it; the
  bridge had no way to read it, and the engine is the only place it can be
  judged (the check goes by the natives registered `PB_FX_CONST`). Under
  Cheesecake `const X = time()` loaded and ran with no word.
  `pb_vm_diagnostics_text(vm, errors_only)`.
- **The stale-seed report named states it should not** (`4269f9f`). It went
  by every top-level write of the cell: `state n = 0` then `n = step(n)`
  was a stale seed after any edit to `step`. It goes by the initializer
  now, as every description of it says.
- **`PETAL_STATIC_CHECK=1` called a frozen kept value a failure**
  (`4269f9f`). Moses printed nine such lines: `bank_color`, `tile_far` and
  `city_box` return lists that end up inside tables the game freezes. With
  `freeze` taken out of a scratch copy the check printed nothing. It is a
  `static check note:` line now. "No failure line" is a criterion a host
  can use again.

**Two claims of the proposal that did not hold under the engine.**

- "The stale-seed report names the cell" for `moses/map.ptl`'s `if G ==
  nil` guard. It does not: the cells are `state var`, filled by `set` under
  the guard, and the report follows a `state`'s initializer. An edit to
  `LABELS_SRC` as a `const` (scratch copy) reported `rebuilt: 1`, 0 stale
  seeds, and the map did not change. So the 16 source tables of `map.ptl`
  stay in `state`, with `petal-lint-ignore-file state-as-constant` and the
  reason. Reporting a `state var` whose guarded write reads a changed
  constant is not built; it would need the same care about writes that
  happen on every run as the fix above.
- "Reading enum data used to be slow, so the derived tables can go" (A1's
  follow-up). `F.tenement` is one instruction and `Fabric.tenement.code`
  two. Writing all 220 reads through the enum left the play benchmark at
  46,501,142 (−5,400) and took the 460-frame autoplay run from 152.50 to
  152.80 million instructions to 153.58 million (three runs each; +0.5 to
  0.7%, about 2,000 a frame in `buildings.ptl`). `T`, `F` and `USE` stay.
  Folding a variant's constant data read to a constant load would close
  that.

**The S1 gate under the engine.** The command of "Benchmarks that gate the
work", Release engine, the profile over frames 60 on, per-frame figures the
total over 600 as the baseline's were. Instruction and allocation counts
were identical in every run (3 to 5 each); times are at load average 2 to 4.

| | Instructions | A frame | ms a frame, profile on / off | Collections | `StateInit` | Record literals built |
|---|---|---|---|---|---|---|
| Petal `42142fd` (the baseline) | 49,871,337 | 83,119 | 2.71 when the plan was written | 14 | 243,540 | not counted |
| Cache off (the audit), scripts as they were | 49,847,577 | 83,079 | 2.89 / — (one run) | 18 | 243,540 | 516,335 |
| `4269f9f`, no script changed | 46,543,802 | 77,573 | 2.59 to 2.66 / 2.00 to 2.13 | 10 | 243,540 | 196,762 |
| … and 60 tables as `const`, `F_PEOPLE` dropped | 46,506,542 | 77,511 | 2.60 to 2.74 / 2.02 to 2.03 | 10 or 11 | 212,220 | 196,762 |
| Cache off, scripts as they are now | 58,933,077 (58,964,397 while `F_PEOPLE` stood) | 98,222 | 3.47 to 3.62 / — (one run each) | 24 | — | 800,915 |

Against the expectation (83.1k to about 79k instructions a frame, record
allocations down about 40%): **83.1k to 77.5k (−6.7%), and −61.9% of the
record literals script code builds** (list literals and comprehensions
−90.7%, 454,507 to 42,242). The allocation figure is new: Cheesecake's
`--vm-profile` now prints the counts of `AllocMap`, `AllocMapSpread` and
`AllocList` instructions. It is not heap allocations (records built inside
natives are not in it), and its "before" is the same binary with the cache
off, since the pre-plan engine binary is gone. The rewrite itself is worth
37,260 instructions over the run (58 fewer `StateInit` a frame), and the
last row is the 25.8k-a-frame warning of the scratch measurement seen under
the engine: 98.2k a frame with the cache off.

**The live-coding check** (Moses under `--headless --agent`, a scratch copy
of the adopted scripts, no restart at any point):

| Edit | Reload | Constants | Seen in the game |
|---|---|---|---|
| A title in `history.ptl`'s `RAW`, which was `state` (a value) | 13 ms, patched | 528 kept, 2 rebuilt (`RAW`, `TIMELINE`) | the `history` tool returns the new title |
| The same edit on the scripts before the rewrite | 15 to 45 ms | — | the old title |
| An alias added to `projects.ptl`'s `ALIASES` (structural) | 550 ms, recompiled | 529 kept, 1 rebuilt | `project tribo` resolves; it was "no such project" |
| `util.pct` rewritten | 522 ms, recompiled | 530 kept, 0 rebuilt | |
| A key added to `hud.ptl`'s `POT_SHORT`, three times | 528 to 550 ms | 529 kept, 1 rebuilt | |
| A label in `map.ptl`'s `LABELS_SRC`, still `state` | 18 ms | 530 kept, 0 rebuilt | the log names it a stale seed |

S2 under the engine: a structural edit is 522 to 550 ms (six edits) with 529
or 530 of 530 values kept, against the 494 ms `reload_timing` gives the
recompile alone. The frame after is not a measure of the constants there:
the game declares its procedural assets again after any reload, and frames
run 12 to 98 ms for a few frames where a steady one is 1.6 ms.

**Checked.** Every game with a `game.ptl` (13, and `games/demo/basic.ptl`)
headless for 300 frames, three ways (as shipped, `PETAL_STATIC_AUDIT=watch`,
`PETAL_STATIC_CHECK=1`), after each step: no script error, no audit
finding, no check failure (nine notes, Moses), the same dumped state three
ways (Moonbear's `bake_ms` aside). Moses's dumped state after 600 frames of
1956 and after 460 frames of autoplay: equal key for key before and after
the rewrite, minus the 58 cells that are no longer state. Here: `core`
2,632, `petal-ui` 300 and the bridge's 21 tests pass with the stale-seed
change (debug); the static suites (`static_cache`, `static_reload`,
`static_in_functions`, `static_check_env`, the library's `static` tests)
pass with the check change; `make test-c-bridge` 4 of 4 at `6a102e1`.
Cheesecake's `ctest` on the final tree: 347 of 347 in one run (20 minutes),
and 347 of 347 after the first re-vendor; `petal check` over its 274 tracked
scripts outside `third_party`: the seven documented exceptions and nothing
else.

**Not done.** No game but Moses was profiled for what the constant natives
bought. The lint offers nothing in the other twelve games. The engine's
line for a value that cannot be kept has no test (a script cannot make one:
a table that holds a function is classified dynamic, and the bridge cannot
return a handle or a pending result from a native). The release core suite,
the fuzzers, `verify.ts`, vitest and the Garden workspace were not run again
on `4269f9f`.

### Review of the adoption (13 October 2026)

**Status: done; what the adoption reported holds, one figure was corrected,
one half-reload it had missed is fixed in the game (Cheesecake `fa36156`),
and the core suite it had left unrun was run.** An independent
review of Cheesecake `03319c2..fd51d84`, on the internal disk, in the
existing Release tree (`ninja` found nothing to build at `fd51d84`).

**Checked, and as reported.**

- **The vendored tree.** `git archive 4269f9f` of the eight vendored paths
  against Cheesecake's committed tree and against its working tree:
  `garden/gpp/Cargo.toml` differs (the three `*.workspace = true` lines
  made concrete), nothing else.
- **The constant natives.** `hash01` (a splitmix over the bits of its
  arguments), `hex_color` (`Color::parse_hex` on its string, a record of
  ints out) and `sweep_hits` (segment against sphere over the two lists it
  is given, the option check a pure throw) were read line by line with what
  they call: no clock, input, generator, file, host table or handle in any
  of them. Across the engine's hundred or so registrations these three are the
  only `fx::Const`; every other one carries `Effect` or a `Reads*` flag.
- **The rewrite.** 60 `state` to `const` in 13 files of Moses. None of the
  60 is assigned, indexed-assigned, `set` or appended to in any module (the
  hits a grep finds are other modules' own cells of the same name: `WORKS`
  in `map.ptl`, `NAMES` in `terrain.ptl`, `TONES` in `politics.ptl`); none
  is `pub` or read as `module.NAME`; none seeds another `state`; none is
  named by a test, a `--dump-keys` list or a `--set`. Every reader of each
  was listed by function. Three of them feed a cache held in `state`, which
  is "Found" below; no value a run computes depends on that. One, `seen` in
  `dev/core_check.ptl`, is read by nothing at all. Dumped state after the 600-frame run, scripts of `03319c2` against
  `fd51d84`: 451 keys to 393, the 58 gone are all rewritten cells, no key
  left has another value.
- **`F_PEOPLE`.** No reader left in any `.ptl`, test or tool; the one
  remaining mention is the comment that says where it went.
- **Every game.** The 13 games with a `game.ptl` and `games/demo/basic.ptl`,
  headless for 300 frames, as shipped, under `PETAL_STATIC_AUDIT=watch` and
  under `PETAL_STATIC_CHECK=1`: exit 0 and "0 script errors" 42 times of 42,
  no audit line, no `static check failure:` line, nine `static check note:`
  lines (Moses), the dumps byte for byte the same three ways for 12 games
  and Moonbear's differing in `bake_ms` alone.
- **`ctest`** on `fd51d84`: 347 of 347 in one run, 1,191 s, load average 2
  to 3. On `fa36156`, which changes one Moses script: the 70 Moses tests and
  the 12 of the script host, the agent and the player. `petal check` over the 274 tracked scripts outside `third_party`:
  the seven documented exceptions and nothing else. The lint offers nothing
  more in `games/moses`.
- **The numbers.** The benchmark command, five runs: 46,506,542
  instructions, 10 collections, 212,220 `StateInit`, 187,079 `AllocMap` +
  9,683 `AllocMapSpread` (196,762), 42,242 `AllocList`, every run. The
  scripts of `03319c2` on the same binary: 46,543,802 and 243,540, the
  same allocation counts; under the audit 49,847,577, 18 collections,
  506,652 + 9,683 (516,335) and 454,507. Times at load average 2.3: 2.69 to
  2.71 ms a frame with the profile, 2.07 to 2.13 without (three runs each),
  a little above the adoption's.
- **Live coding**, a fresh scratch copy under `--headless --agent`, one
  process throughout: a title in `history.ptl`'s `RAW` edited, reload 13.5
  ms, `values`, 528 kept and 2 rebuilt (`moses/history::RAW`,
  `moses/history::TIMELINE`), and the `history` tool's first 1934 entry
  carries the new title; `tribo` added to `ALIASES`, 539.4 ms,
  `recompiled`, 529 kept and 1 rebuilt, and `project tribo` goes from "no
  such project" to the Triborough Bridge; `util.pct` rewritten, 531.9 ms,
  530 kept and 0 rebuilt; a label in `map.ptl`'s `LABELS_SRC`, 17.7 ms, 530
  kept, 0 rebuilt, and the log line `state moses/map::LABELS_SRC keeps its
  old value: its declaration changed`. The frames after each reload ran 12
  to 82 ms, a steady one 1.85 ms.

**Corrected.** "Cache off, scripts as they are now" was 58,964,397. That is
the run at Cheesecake `c92466e`, before `F_PEOPLE` went (taken again here:
the same). At `fd51d84` it is 58,933,077: with nothing keeping it, the
comprehension that built `F_PEOPLE` was 58 instructions a frame. The table
above and Cheesecake's debrief say so now.

**Found.**

- **A constant that feeds a cache held in `state` is rebuilt and the game
  does not show it.** `widgets.set_skin` kept the merged palette of the look
  in force in `state var SK` and returned early while the look was the same.
  Editing the paper of the skin in force (scratch copy, the running game):
  `values`, 17.1 ms, 529 kept and 1 rebuilt (`moses/widgets::SKINS`), no
  stale seed, and `SK.paper` was the old colour. It is the map's half-reload
  in small: the report says rebuilt and the screen does not move. The
  stale-seed report cannot name it for the same reason: the cell is a
  `state var`, written inside a function.
  **Fixed in the game** (Cheesecake `fa36156`): the merged palettes are a
  constant, `SKIN_PALS`, and the state is the index of the one in force,
  found again on every `set_skin`. The same edit: 16.2 ms, 529 kept and 3
  rebuilt (`SKINS`, `SKIN_IDS`, `SKIN_PALS`), no stale seed, and
  `widgets.skin().paper` is the new colour on the next frame. Dumped state
  after 600 frames is equal key for key, `SK` and `SK_ID` gone and `SK_IX`
  new (392 keys); the benchmark is 46,469,822 instructions, 211,680
  `StateInit`, the allocation counts unchanged, three runs; the three-way
  run of Moses is clean and the same; 532 constants.
- **Two more such caches, left.** `projects.defs()` builds the named
  projects once into `state var DEFS_C`, each spread over its kind's
  template in the constant `GENERIC`: a template edit reaches the generic
  works at once and the named ones at a restart, as an edit to the named
  list itself always did. `panels.bake_overlay` colours a mesh from the
  constant `RAMPS` and is run again each quarter of game time. Neither is
  named by the reload report. They are what "report a `state var` whose
  guarded write reads a changed constant" (not built) would be for, and
  they say the guard is not always `if X == nil` at the top level: these
  two are a nil test and a key comparison inside functions.
- **A constant handed to `append` is built on every run, and nothing a host
  prints says so.** The first form of the fix was `const SKIN_PALS =
  append(for id in SKIN_IDS do ... end, PAL)`. `petal check` passed it, the
  engine's `const` check passed it, the slot of `SKIN_PALS` was filled, and
  the comprehension ran on every frame: four `AllocMap` and four
  `AllocMapSpread` a frame in the engine's profile, which is the only way it
  was seen. In this repository, four frames of a two-skin reduction: 48
  instructions a steady frame with the cache on against 56 with it off,
  where `concat([PAL], for ... end)` and the comprehension alone are kept
  whole. It is the first guard of "What keeps a kept value from changing"
  doing what it says (a value that reaches the container operand of a
  mutating builtin gets no slot). What is missing is the word: with the
  list named, `const T = for ...` then `append(T, PAL)`, `petal show-static`
  prints "! not kept: T is passed to append, which may update it in place"
  and neither `petal check`, the `const` check nor the static warnings do;
  with the comprehension inline nothing prints at all, because the part
  that is not kept has no declaration. `const` is the plan's guard against
  a silent fall back to per-frame, and here it does not guard. **Not fixed
  here** (follow-up 11): it wants a decision between a warning on such a
  `const` and keeping the value on the strength of the other two guards.
  The game uses `concat`.

**Run here that the adoption had not.** `core`'s whole debug suite on this
repository's head (`4269f9f` and two commits of documentation; `cargo test
--no-fail-fast`): 67 binaries, 2,632 passed, 0 failed, 16 ignored. That is
the check-note half of `4269f9f` under the whole suite for the first time.
Still not run on `4269f9f`: the release core suite, the fuzzers at their
3,000 seeds in release, `verify.ts`, vitest, `petal-ui`'s suite after the
check-note half, the Garden workspace.

**Not repeated.** The 220-read experiment behind keeping `T`, `F` and `USE`
(the scratch copy is gone; the reasoning, one instruction against two, was
read and holds) and the 460-frame autoplay state comparison. Cheesecake
`b618c58`, which the adoption did not make, is on `main` there and was not
pushed: the branch was nine commits ahead of `origin/main` and one behind
when the review began.

**Looked at and left.** `const G = build_grid()` for the map. It would move
the half-reload one layer down, not remove it: `terrain.ptl`, `sim.ptl`,
`cam.ptl` and `traffic.ptl` build their own state from the grid behind their
own guards, so a rebuilt grid would be read by a terrain baked from the old
one. It would also take the grid out of the frozen generation (`freeze` is
an effect, so a constant cannot be built with it), where A2 put it to keep
16,384 tiles out of every collection. The 16 source tables stay in `state`,
where the module changes together at a restart.

### The final test pass, 13 October 2026

**Status: done, on `4e5b584`; nothing fails.** The whole list of the 12
October pass below, run again after the work of 13 October (a store per
value, the cost of a reload, the cache under observation, constants inside
function bodies, and their review), one suite at a time, with a clean
working tree. The one test that failed before the plan passes: the release
core suite is green for the first time.

- **Rust, debug.** `core`: 66 binaries, 2,628 passed, 0 failed, 16 ignored.
  `core-libs/petal-ui`: 21 binaries, 300 passed, 5 ignored (the measuring
  tests). `core-libs/petal-query`: 33. `integrations/petal-c-bridge`: 20,
  and `make test-c-bridge` 4 of 4. `integrations/petal-desktop-sdl`
  (`LIBRARY_PATH=/opt/homebrew/lib`): 54. `garden-script`: 182, and `cargo
  check --workspace` in `garden/`.
- **Rust, release.** `core`: 66 binaries, 2,628 passed, **0 failed**;
  `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` is among
  them, and alone it measures 1.13 to 1.15 ms for the value edit against
  11.1 to 11.4 ms for the recompile over three runs (9.7 to 9.9 times,
  where it asserts 5). The ignored oracles: copy propagation against its
  reference; the 4-token small-scope sweep; `hot_reload` with
  `--include-ignored`, 49 of 49, which is the whole-corpus reload oracle
  over this repository beside the timing test; the same oracle with
  `PETAL_RELOAD_NO_RUN=1` over the Cheesecake games, worlds-fair,
  `~/.garden`, the website and `~/tools` (343 files: 160 relocated, 9
  patched, 154 recompiled). Every fuzzer in the library at 3,000 seeds
  (`differential_fuzz`, `ir_round_trip_fuzz`, `static_cache_fuzz`,
  `static_fuzz`, `static_fn_fuzz`, both cell fuzzers, and the five tests
  that hold the generators to their job); the sharing generator of
  `static_reload.rs` at 2,000 seeds. The four corpus sweeps of `make
  expensive_lint_tests`, in release: 4 of 4.
- **The rest.** vitest 1,571 in 75 files; `tsc`; `npm run stdlib:json`
  leaves no diff; `./tools/test-examples.ts` 33 (optimizer on and off, cache
  on and off over two runs, golden).
- **`./tools/verify.ts --plan compiler`**, four ways, each 139 identical
  traces, 19 modules, 145 unsupported of 303, nothing changed: against the
  golden hashes; the UI driver with the cache off against on (`fast-static`
  against the default); the optimizer off against on (`baseline` against
  the default); optimizer off with the cache off against on (`baseline`
  against `baseline+static`). The axis is the UI driver's policy in each;
  the console scripts' optimizer and cache differentials are
  `test-examples` above.
- **Audits.** `petal run --effect-audit`, `--static-audit` and
  `--static-only` over the 48 scripts of `examples/console`,
  `test/benchmarks` and `test/benchmarks/micro`, each under `fast`,
  `replay`, `replay-static` and `baseline` and over one and two runs (1,152
  runs): 0 under-declared natives, 0 failures of a static build, no static
  term that touched a dynamic input, no exit code that differs from the
  run without the audit. `frozen` is reported over-declared, as before.
  These audits watch top-level static terms; the values kept inside
  function bodies are outside them (their check is `PETAL_STATIC_CHECK`).
- **`./tools/native-effect-audit.ts` exits 1**, as before the plan, on
  `queue_on_host` alone.
- **The ecosystem sweep**, `petal check` and `petal show-static --json` on
  the same 372 files as below: the same 6 do not compile, for the same two
  reasons; 102 more fail `petal check` and compile, with 2,882 error lines,
  every one an unknown function or undefined variable the file's host
  provides (64 of them saved panels under `~/.garden/state`, 18 in
  worlds-fair, 9 on the website, 3 in `~/cheesecake/games`, all `hash11`
  in `rivet`); no diagnostic names `const`, `freeze`, `frozen` or a static
  value. (The 108 of the first sweep counted the 6 with the 102.)
- **Corrected on the way.** `docs/features/hot-reload.md` and `CHANGELOG.md` still
  gave a recompile on neon as 175 ms and on Moses as 461, and the cache's
  share as 19 and 61: the figures from before constants inside function
  bodies. They are 187 and 488 now, the cache 31 and 102 of them.
  `docs/dev/performance.md` gave the function-body share as 8 of 186 and 26
  of 481; measured over more runs it is 12 of 187 and 33 of 488.
- **Not run.** `petal-fantasy-nes` and the tests of the Garden workspace
  beyond `garden-script` (disk: 8.5 GB free); clippy;
  `tools/oracle-external.ts`; any suite as a whole under
  `PETAL_STATIC_CHECK=1` (the three that set it themselves ran); a real
  Garden session; anything under the Cheesecake engine. No wall time was
  taken for a frame of the panel corpus. (All but the last three ran later
  that day: the next section.)

### What the disk had kept back, 13 October 2026

**Status: run, on `518c008` and the five commits after it, until the
external disk it ran on went away; two things were lost to that and are
listed last.** Everything the two passes above list under "Not run" for
want of disk, built into target directories on an external SSD. One
miscompile was found and fixed, in the optimizer and older than this plan.
**Nothing was found in the cache.**

**Found and fixed.**

- **A reference taken inside a guard saw a later in-place rewrite**
  (`fccbc12`). `let v = [1, 2, 3]`, then inside an `if` (or the body of an
  enclosing loop) `let s = v`, a loop that appends to `v`, and `print(s)`:
  it printed the grown list with the optimizer on. The arm is part of the
  accumulator's region, so the copy `s` was a web carrier, condition 5 of
  `escape.rs` passed every user of a backbone term that was in the web,
  and the linearity check does not count the loop phi as a second reader.
  A web user that feeds nothing on the spine must now observe before the
  id is rewritten, as a user outside the web must. A binary of 21
  September has the bug, so it is not from this plan; the static cache is
  not involved. Counted over 706 `.ptl` files (this repository, the
  Cheesecake games, worlds-fair, `~/.garden`, the website, `~/tools`):
  13,357 in-place instructions before the fix and after it, in the same
  381 files, so no script had the shape and none lost a write. Eight
  shapes pinned in `inplace_does_not_fire_past_a_reference_taken_inside_the_guard`,
  with two that must keep firing.
- **The fuzzer that found it** is new: `bytecode/enum_all_fuzz.rs`, for
  the `Enum.all` read that copies (A1's `copies_enum_all`), which neither
  in-place fuzzer reached because neither declares an enum. 96 of 100 of
  its programs have a copying read; each runs three frames under the
  baseline, each in-place route alone, and `fast` and `replay` with the
  cache on, off, checking and observed. Seed 148 was the bug above, with
  no enum in it. The `Enum.all` path itself: nothing in 11,176 seeds.
- **`PETAL_STATIC_CHECK=1` over a whole suite could not report anything**
  (`1fe4ea4`): a mismatch went into `StaticValues::mismatches`, which no
  test of such a run reads. It prints a `static check failure:` line now
  (`core/tests/static_check_env.rs`).
- **`shapes.ptl` raised because the test took the panel's store away**
  (`0cfc09f`), not for anything in Petal or the script: the corpus test set
  `host.store = None`, the script counts its runs with `panel_store_set`,
  and "this panel has no store" compared equal under both hosts. Each host
  gets an empty store of its own, and a panel that raises fails the test
  unless it is one of `gpp-test-app`'s two that exist to. With it the
  Garden measurement is 18 panels that draw: 32,116 to 29,947 instructions
  a frame (−6.75%) and 2,579 to 2,088 allocations (−19.02%); `shapes`
  alone 1,542 to 1,391 and 142 to 101, 19 slots. (The 30,573 to 28,555
  given above is the same run without it.)
- **`tools/oracle-external.ts` did not run at all** (`e7ee738`):
  `petal-ui-run` refuses a script that calls a host native without
  importing its interface, and worlds-fair's sources predate that, so every
  fragment stopped at "unknown function `query`". The bundle is prefixed
  with `import garden`, `import ui`, `import color`; `--runner` and
  `--wf-bin` name binaries built elsewhere.

**Ran.**

- **Garden, the whole workspace** (`cargo test --workspace` in `garden/`,
  the seven GPP apps are members), debug, the first time against the cache
  under observation: 36 binaries, 1,444 passed, 0 failed, 1 ignored
  (`garden-app` 992, `garden-core` 106, `garden-diff` 49, `garden-render`
  19 and its integration tests, `garden-script` and the other apps the
  rest).
- **`petal-fantasy-nes`**: 56 and 2 (the carts) passed.
- **`PETAL_STATIC_CHECK=1`**, `--no-fail-fast -- --nocapture`. `core`: 66
  binaries, 2,625 passed, 3 failed, all in `static_in_functions.rs` and by
  design (two count skips, which the check does not take; one pins what
  `freeze` can tell, and its one `static check failure: … mk (line 2)` is
  the only such line in the run). `petal-ui`: 21 binaries, 300 passed, 0
  failed, no line.
- **`PETAL_STATIC_AUDIT=watch`**, the same way. `core`: 2,530 passed, 100
  failed, 11 findings. The audit turns the cache off, so 99 of the 100 are
  the tests that assert it kept or skipped something (`static_cache` 29,
  `static_reload` 36, `static_in_functions` 15, `static_observe` 11,
  `static_review3` 2, `const_decl` 1, `hot_reload` 1, three in the
  library, and the one that checks the variable is off by default). The
  hundredth was the new fuzzer's own generator test, broken at that moment
  by a scoping bug in the generator that was fixed before it was
  committed. The 11 findings
  are all printed by `tests/static_audit.rs`, whose natives lie on
  purpose. `petal-ui`: 295 passed, 5 failed the same way ("no static run
  was ever skipped"), 0 findings. `docs/dev/scripts.md` said one test fails
  under the audit; it says this now.
- **The fuzzers, release**, eight at once on a machine at load average 35
  to 80, each sized from a 300-seed calibration to nine minutes and
  finishing in four to seven because the load fell: `differential_fuzz`
  73,598 seeds (260 s), `static_cache_fuzz` 21,833 (312 s), `cell_fuzz`
  17,477 (365 s), `state_cell_differential_fuzz` 13,929 (397 s),
  `enum_all_fuzz` 11,176 (352 s), the sharing generator of
  `static_reload.rs` 4,501 (333 s), `static_fuzz` 2,946 (346 s),
  `static_fn_fuzz` 2,551 (328 s). All pass, with the escape fix in. The
  last two are below the 3,000 they had already been run at.
- **`tools/oracle-external.ts`**, once it ran: 12 of 12 worlds-fair
  fragments agree over 45 monkey frames, `baseline` against `fast-memo`,
  `replay` and `fast` (53 to 363 draw commands a frame, none vacuous), and
  the effect audit finds no under-declared native. `wf-ui` was built with
  `CARGO_TARGET_DIR` on the SSD; `git status` in `~/worlds-fair` is clean.
  It is worlds-fair's own vendored Petal that printed the fixtures; the
  frames ran on this repository's `petal-ui-run`.
- **The wasm build**: `wasm-pack build --target web` in
  `integrations/petal-web-canvas/rust` with its target and `--out-dir` on
  the SSD succeeds, `wasm-opt` included; `petal_web_canvas_bg.wasm` is
  3,582,214 bytes (the `pkg/` of 10 October in this repository is
  3,018,474 and was not touched). Not loaded in a browser, not vendored
  anywhere.
- **clippy** (`cargo-clippy` is in the toolchain and not on `PATH` here;
  run with the toolchain's `bin` in front). `core --all-targets`: 122
  warning lines, 29 of them on lines written since `281c802`, all fixed
  (`1fe4ea4`, `d901428`): 11 loops over instruction positions allowed for
  `static_cache` with the reason (one `ip` indexes several parallel
  tables), one `too_many_arguments` allowed on `in_fn::stretches`, the
  rest rewritten. One was a test bug of no consequence: a `.replace()` in
  `static_in_functions.rs` that replaced a string with itself. `petal-ui`:
  2 of 15 on plan lines (an unused helper, a `sort_by`), fixed.
  `petal-query` 6, `petal-c-bridge` 1, `petal-desktop-sdl` 23: none on
  plan lines. Left alone: 93 warnings in `core` on older lines, and a
  deny-by-default `approx_constant` in `petal-ui/tests/prelude.rs:2087`
  that stops `cargo clippy --all-targets` there.
- **After the fixes**, debug: `core` 66 binaries, 2,631 passed, 0 failed,
  16 ignored (three more than above: the two tests of the new fuzzer and
  the regression), then `static_check_env` alone; `petal-ui` 21 binaries,
  300 passed.

**Lost when the SSD unmounted** (about 06:55; every log was on it, so the
figures above are as read while it was there):

- The second fuzzer pass, four at a time for a full ten minutes each. It
  was still building. The pass above stands, at four to seven minutes.
- The whole `core` suite in the release profile on the fixed tree. The
  debug suite above is what ran.
- The Garden workspace a second time, after the escape fix: 1,443 passed
  and **1 failed**, `ticking_advances_the_script_clock_by_the_supplied_dt`
  in `garden-app`, `time()` advancing 0.982 where it allows 0.96 ± 0.01.
  The tolerance is for two frames timed by the wall clock, the machine was
  at load average 18 with a release build beside it, and the test passed
  in the first run; the five runs alone that would have settled it are
  what the disk took. Unconfirmed either way.
- clippy was not run again after the two `petal-ui` fixes, nor over
  `core/tests/static_check_env.rs` and the last comment added to the
  fuzzer.
- One line was written after the disk went and has not been compiled:
  `static_check_env.rs` also clears `PETAL_STATIC_AUDIT` for the `petal` it
  starts, so that it does not join the tests that fail under the audit.

Still not run: clippy over `garden/`; a real Garden session; a browser
with the new wasm; anything under the Cheesecake engine.

### The final test pass, 12 October 2026

Everything passes except the one test that failed before the plan.

- **Rust, debug.** `core`: 63 binaries, 2,558 passed, 0 failed, 16 ignored
  (the oracles below and doc examples). `core-libs/petal-ui`: 20 binaries,
  297 passed (the panel corpus cache on against off under four policies,
  the followed reload against clear-everything, the effect, static and
  classifier audits). `core-libs/petal-query`: 33.
  `integrations/petal-c-bridge`: 20, and `make test-c-bridge` 4 of 4.
  `integrations/petal-desktop-sdl`: 54. `garden-script`: 180, and `cargo
  check --workspace` in `garden/`.
- **Rust, release.** `core`: 2,557 passed, 1 failed (below). The ignored
  oracles: copy propagation against its reference; the 4-token small-scope
  sweep; the whole-corpus reload oracle over this repository and over the
  Cheesecake games, worlds-fair, `~/.garden`, the website and `~/tools` (343
  files: 160 relocated, 9 patched, 154 recompiled), each reload compared
  with a recompile and with the reload that rebuilds every static value;
  `static_fuzz` and `static_cache_fuzz` at 3,000 seeds each; the other
  fuzzers at their defaults. `make expensive_lint_tests`: 4 of 4.
- **The rest.** vitest 1,570 in 75 files; `tsc`; `npm run stdlib:json`
  leaves no diff; `./tools/test-examples.ts` 33 (optimizer on and off, cache
  on and off over two runs, golden).
- **`./tools/verify.ts --plan compiler`**, four ways, each 139 identical
  traces, 19 modules, 145 unsupported, nothing changed: against the golden
  hashes; the UI driver with the cache off against on (`fast-static` against
  the default); the optimizer off against on (`baseline` against the
  default); and optimizer off with the cache off against on (`baseline`
  against `baseline+static`).
- **Audits.** `petal run --effect-audit`, `--static-audit` and
  `--static-only` over the 48 scripts of `examples/console`,
  `test/benchmarks` and `test/benchmarks/micro`, each under `fast`,
  `replay`, `replay-static` and `baseline` and over one and two runs (1,152
  runs): no under-declared native, no static term that touched a dynamic
  input, no failed build. `frozen` is reported over-declared (it declares an
  effect so that nothing caches its answer, and has none), which is
  informational.
- **Still failing, as before the plan.**
  `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` in the
  release profile only: 3.39 ms against 12.41 ms, 3.7 times where it asserts
  5. It passes in debug.
- **`./tools/native-effect-audit.ts` exits 1**, as it did before the plan:
  its one suspect is `queue_on_host`, a native that a test in
  `integrations/petal-desktop-sdl/src/sound.rs` makes silent on purpose.
- **Fixed on the way.** `core/examples/reload_timing.rs` loaded its program
  linked and so refused every Cheesecake game, whose packages now require
  host interfaces. It compiles without linking, as it never ran anything.
- **Not run.** `petal-fantasy-nes` (it has no target directory and the disk
  has 5 GB free); the tests of the Garden workspace beyond `garden-script`;
  clippy; `tools/oracle-external.ts`; anything under the Cheesecake engine.

### The ecosystem sweep

`petal check` and `petal show-static --json` on 372 `.ptl` files outside
`examples/` and `test/`: `garden/` 27, `~/cheesecake/games` 232,
`~/worlds-fair/ui/ptl` 19, `~/.garden` 72, petal-lang.org 17, the two
experiment apps 2, `~/tools` 3 (`~/biz/hotlaps` is not on this machine).

- **Nothing the plan added breaks a script.** No diagnostic in the sweep
  names `const`, `freeze`, `frozen` or a static value. No binding,
  parameter or function is named `const`. Six Cheesecake files bind a local
  `freeze` or `frozen`, which shadows the builtin as any local does, and
  all six check clean.
- **6 files do not compile, all for reasons older than the plan**: five
  copies of `~/.garden/state/window-*/git.ptl` (a list without the commas
  required since 8 October) and the website's `snippets/match.ptl` (a bare
  variant as a pattern, an error since 11 October). S0's sweep counted 21
  of 744 with `examples/` and `test/` included.
- **108 files fail `petal check` and compile**: every error is an unknown
  function or variable that the file's own host provides (2,882 such
  lines). In `~/cheesecake/games` that is 3 of 232, all `hash11` in
  `rivet`.

### Follow-ups

As of 13 October 2026: in this repository 5 to 8 are done, 9 is partly
done, of 10 three "Later" items are left, none of which has a profile
asking for it, and 11 and 12 are new from the review of Cheesecake's
adoption and not started. What the work of the 13th left open is listed under "Open"
in each of its notes and gathered in "Still open after this review"; the
largest is that a recompile now pays the cache 31 ms of 187 on neon and 102
of 488 on Moses (classification 14 and 47, constants inside function bodies
12 and 33), against none of a patched or relocated reload.

Outside this repository, 1 is done:

1. **Cheesecake.** **Done 2026-10-13** ("Cheesecake: the adoption" above;
   reviewed the same day, "Review of the adoption"):
   re-vendored at `4269f9f`, `PB_FX_CONST` on its three constant natives
   and every game run under the audit and the check, the headless Moses
   profile, the lint's rewrite (60 of 76 cells; the 16 of `map.ptl` kept in
   `state` on purpose), `statics` printed and the flush called, `F_PEOPLE`
   dropped. **Left:** the map's guard (`const G = build_grid()` is the
   honest rewrite and was not attempted), and `T` / `F` / `USE`, which
   still pay. (`rivet`'s `hash11` is one of its shader-only functions, by
   design.)
2. **worlds-fair.** `node tools/vendor-petal.ts`; `wf-ui-ffi` knows neither
   `const` nor the cache until then. Its `.ptl` sources import nothing, and
   today's Petal refuses a host native called without its interface
   imported: they need `petal fix-imports` and an `import garden` by hand
   when it re-vendors, and `tools/oracle-external.ts` can then drop the
   three imports it prefixes.
3. **petal-lang.org and hotlaps.** Rebuild and re-vendor the WASM build;
   fix `snippets/match.ptl`.
4. **`~/.garden`.** The five saved `git.ptl` panels need their commas.

In this repository:

5. The release-only reload timing test: the value edit parses the edited
   file twice. **Done 2026-10-13**: "The cost of a reload" below.
6. Split static runs at function declarations and store per value, so one
   rebuilt slot does not re-execute its run (the open item of S1 and S2,
   and what would make `frozen` unable to tell a kept neighbour from a
   rebuilt one). **Done 2026-10-13**: "A store per value", after the review
   of S2.
7. Time a run that has no slot of its own, for the debounce. **Done
   2026-10-13**, same note: there is no such run any more.
8. Report stale seeds in Garden panels, where the cache is off. **Done
   2026-10-13** ("Later: Garden"): the cache is on there and the report
   with it.
9. The classification that lowering does costs 24 ms of a 183 ms recompile
   on neon; it could be served from the previous analysis when the diff
   touches no declaration. **Partly done 2026-10-13** ("The cost of a
   reload" below): it runs once whoever asks and costs 14 ms; it is not
   served from the previous program's analysis, for the reasons given
   there.
10. The "Later" items above, each on evidence: Garden observation replay
    (**done 2026-10-13**), constants inside function bodies (**done
    2026-10-13**, measured first: "Later: constants inside function
    bodies"), per-input-class slots, keeping slow intermediates, a frozen
    generation that can release objects.
11. **A `const` that is not kept says nothing** (found by the review of the
    adoption, 13 October; not started). A value handed to `append` or
    another builtin that may write in place gets no slot, by design, and is
    rebuilt on every run. `petal show-static` says "! not kept" when the
    value has a name; `petal check`, the `const` check and the static
    warnings do not, and for `const X = append(for ... end, Y)` nothing
    does. Either warn on a `const` any part of whose build is not skipped,
    or keep such a value (the second and third guards hold without the
    first, by S1's own test). `const X = append(for i in range(0, 3) do [i]
    end, [9])` is the reproduction: 100 frames under the Cheesecake engine
    run 2,809 instructions and three `AllocList` a frame, and 1,028 and
    none with `concat(for ... end, [[9]])`.
12. **The reload report does not name a cache.** A `state var` that a
    function fills from a constant behind a test of the cell itself (`if
    get X == nil`, or a remembered key) keeps a value built from the old
    constant, and the report says only that the constant was rebuilt. Three
    in Moses (`map.ptl`'s grid, `projects.ptl`'s `DEFS_C`, the overlay
    bake), and a fourth the review removed. Not started; it needs to tell
    such a write from one that happens on every run, which is what
    `4269f9f` had to stop reporting.

### The cost of a reload (2026-10-13)

Follow-ups 5 and 9, at `f03dab6` and `489ea39`. Release build, Apple
silicon, a machine at load average 4 to 20; every figure is the minimum of
9 rounds in each of 5 to 10 runs of `core/examples/reload_timing.rs`.

**A reload parses only the text that is new (5).** `source_diff` parsed the
running text and the new text of each changed file. The parse of the text a
reload ends on is now kept for the next one (`source_diff::ParseCache`, held
by the `Env`). It is found again by the text itself, so there is nothing to
invalidate: a module edited, a reload that failed, a reload through another
stack all ask with the text the program holds, and a hit is that text's
parse. A file is kept only once it has been edited, two versions of it at
most (which is what lets a reload that did not compile be followed by one
that parses once), twelve texts and 2 MiB in all;
`Env::clear_reload_parses` drops them and `Env::reload_parses_kept` counts
them. Also: the diff lexes without recovering trivia and does not clone the
token stream, and `plan_patch` finds a term's file in a table (0.38 to
0.07 ms on a 78k-term program).

- `a_value_edit_in_a_large_program_is_far_cheaper_than_a_recompile` passes
  in release: 1.20 to 1.29 ms against 10.5 to 11.3 ms over three runs (8.5
  to 9.5 times; it was 2.78 against 11.8, and asserts 5). The test was not
  weakened. It was made harder: every round reloads a text no reload has
  seen, where alternating between two texts would now measure a reload
  that parses nothing, and it asserts one parse per reload.
- New cases in `core/tests/hot_reload.rs`:
  `a_reload_parses_only_the_text_that_is_new`,
  `a_reload_that_fails_does_not_cost_the_next_one_a_parse`,
  `in_a_program_of_several_files_only_the_edited_file_is_parsed`, and
  `reloads_through_kept_parses_equal_reloads_that_parse_everything`.

**A program is classified once, and faster (9).** With the cache on, the
lowering classified the program, the audit classified it again when one was
watching, and a tool asking `Env::static_report` a third time; only a
`const` check kept its answer. `ensure_bytecode` now classifies before it
lowers and leaves the answer beside the program
(`Program::static_analysis`), which serves the lowering, the audit,
`Env::static_report` and `Env::static_analysis` (hover). It is dropped by
what changes a program in place: a relocating or patching reload, since the
report's lines and columns move (the classes would hold), and
`Env::get_program_mut`. A native table's revision now comes from one
counter for the process, so an answer kept by one `Env` is not served to
another with different natives (it was a per-table count, and two tables
with as many registrations had the same one).

**Not done: serving a recompile from the previous program's analysis.**
Looked at and left, as written on `static_classify::KeptAnalysis`. The
classes are a fixed point over the whole term graph, reached from above:
restarting from the previous answer is sound for an edit that makes
something dynamic and not for one that makes something static again, and
telling them apart needs every cross-declaration read of the transfer
function (callee summaries, captures, cells, method dispatch, enum data) as
an explicit dependency graph, which S2's per-declaration dependency sets
are not (they are an output of the pass, closed over names). The reason a
declaration is dynamic is the first one the solver met, so a correct
restart could still word a reason differently from a classification from
nothing, and the test below would have to be weakened to pass. And a
recompile renumbers every term after the edit. That is neither simple nor
obviously sound, so the pass was made cheaper instead: the fast hasher for
its id-keyed tables, block lists and native lookups indexed once, no
transfer for a term already dynamic, marks in place of a hash set per
dependency walk, a flat list of users.

Classification alone (`reload_timing` prints it now), the previous
classifier built into today's tree for the comparison:

| Program | Terms | Before | Now |
|---|---|---|---|
| `games/neon` | 77,853 | 23.6 to 25.9 ms | 14.4 ms |
| Moses as written | 192,534 | 69.2 ms | 47.9 ms |
| Moses with 74 tables as `const` (scratch copy) | 192,460 | 70.7 ms | 48.9 ms |
| example panels (snake, analytics-dashboard, settings) | 17,465 to 27,399 | not measured | 2.6, 3.5, 4.6 ms |

**The patched-reload row, measured again.** `games/neon`, edits to
`config.ptl`, 35 files, 15,211 lines, 77,853 terms:

| Edit | Outcome | 12 October | Now | Now, cache off (`fast-static`) |
|---|---|---|---|---|
| a comment added | relocated | 1.3 ms | 0.87 ms | 0.89 ms |
| one number changed | patched | 1.5 ms | 1.00 ms | 0.98 ms |
| a statement added | recompiled | 183 ms | 175 ms | 156 ms |
| `set_config_value`, file named | patched | 1.4 ms | 1.03 ms | 1.00 ms |
| `set_config_value`, file searched for | patched | 3.1 ms | 2.72 ms | 2.66 ms |

Moses as written (65 files with its packages, 33,848 lines, 192,534 terms),
edits to `config.ptl`: relocated 1.96 ms, patched 2.69 ms, recompiled 461 ms
(400 ms with the cache off; 483 ms with the previous classifier),
`set_config_value` 4.04 and 4.66 ms. First load: compile 287 ms, lowering
324 ms (247 ms with the cache off).

How to read it. The baseline's "about 0.8 ms" was neon at 14 files and
62,733 terms on a binary that no longer exists; on today's 77,853 terms a
patched reload is 1.0 ms, with the cache on or off (the 0.02 ms between
them is inside the noise). What the 1.0 ms is made of was not profiled
again after the second parse went. `set_config_value` with no file named
costs 1.7 ms more than with one, as before. The cache costs 19 ms of a
recompile on neon, 14.4 of them the classification, and 61 ms on Moses, 48
of them the classification.

The cheaper classifier says what the previous one said: `petal show-static`
and `show-static --json` from a build with the previous classifier and from
this one, over 643 `.ptl` files (this repository's `examples/`, `test/`,
`garden/` and `core/tests`, the Cheesecake games, worlds-fair, `~/.garden`,
the website, `~/tools`), are byte for byte the same; 21 of the files do not
compile and print the same error.

Oracles (the tree also held the store-per-value work, uncommitted for the
first three and committed as `800ff90` for the rest): core `cargo test
--no-fail-fast`, debug, 63 binaries, 2,572 passed, 0 failed, 16 ignored;
`hot_reload` in release 48 of 48, the timing test among them;
`static_fuzz` and `static_cache_fuzz` at 3,000 seeds in release;
`core-libs/petal-ui` `cargo test`, debug, 297 passed, 0 failed (the panel
corpus cache on against off, the followed reload against clear-everything,
the audits); the whole-corpus reload oracle in release over this
repository, and with `PETAL_RELOAD_NO_RUN=1` over the Cheesecake games,
worlds-fair, `~/.garden`, the website and `~/tools` (343 files: 160
relocated, 9 patched, 154 recompiled), each file's kept classification
compared with one from nothing. Every case of `hot_reload.rs` does the same
under three policies and once more with a `const` in both versions: before
the reload, after it, after running, and against the recompiled side
(declarations, reasons, readers, positions, every term's class, every
dependency set), and counts the classifications: at most one for a
recompile, none for a relocated or patched reload.

Open:

- **The classification is still paid in full by every recompile**: 14 ms of
  175 on neon, 48 of 461 on Moses. Serving it incrementally needs the
  transfer function's reads recorded as dependencies first.
- **The rest of what the cache adds to a lowering** is not classification
  and was not looked at here: about 5 ms on neon and 13 ms of a recompile
  on Moses (29 ms of its first load), in `lower_with_statics` and the
  follow step.
- **A patched or relocated reload drops the kept classification**, so the
  first hover or `static_report` after one classifies again (once per ask:
  `Env::static_report` takes `&self` and keeps nothing). The language
  server keeps its own per document version, as before.
- **Not run by this step**: `tools/verify.ts`, `./tools/test-examples.ts`,
  vitest, `tsc`, clippy, the release-profile core suite as a whole, the C
  bridge and SDL suites, `petal-query`, the Garden workspace,
  `petal-fantasy-nes`.

## Risks to watch

- **A wrong `CONST` flag is a stale value forever.** The audit only covers
  natives a test run calls. Every embedder's natives default to dynamic.
- **Silent fall back to per-frame.** One dynamic read makes a whole table
  dynamic. `const` is the guard; the lint must rewrite to `const`, not
  `let`.
- **Half-reload.** A source table reloads while a derived value held in
  `state` behind an `if X == nil` guard does not (`moses/map.ptl:1472`). The
  stale-seed report does **not** name such a cell (a `state var` set under
  a guard; found in the adoption), so Cheesecake kept those source tables in
  `state`. The rewrite is manual. The review of the adoption found the same
  shape inside functions (a palette cached by `set_skin`, fixed; two more
  left): follow-up 12.
- **Another lowering flag doubles the differential-oracle matrix.**
- **`petal check` and the LSP compile without host natives**, so they may
  report less as static than a host does. They must never report more.

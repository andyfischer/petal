# Static value optimization: plan

Status: planned, 12 October 2026. Nothing here is implemented.

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
- A loop that grows a list starting from `K.all` (`xs = append(xs, v)` with
  `xs` first bound to `K.all`) now copies on each append, because its root
  is no longer a fresh allocation. None in the corpus.

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
is revisited there.

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

## S0. The classifier, with no behaviour change

**Status: step 1 of 3 (the `CONST` flag and the audit) done 2026-10-12.** The
classifier, `show-static`, `--static-audit` and the go/no-go gate are not
started.

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

## S2. Following the diff

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

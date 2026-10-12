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

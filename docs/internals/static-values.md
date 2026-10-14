# Static values: how they are implemented

A static value is a value that is a function of the source text alone. Petal
builds it once, keeps it on the stack between runs, and rebuilds it when an
edit reaches it. What that means for a script author and a host (`const`,
what is kept, the reload report, the tools) is in
[features/static-values.md](../features/static-values.md). This page is for
people changing Petal itself: where the code is, what each piece decides, and
what must stay true.

Three pieces do the work:

| Piece | Code | Decides |
|---|---|---|
| The classifier | `core/src/static_classify/` | Which terms are static, why the others are not, and what each declaration depends on. |
| The cache | `core/src/static_cache.rs`, `core/src/static_cache/in_fn.rs` | Which static code a run may jump over, and where its values are kept. |
| Following the diff | `core/src/env/static_follow.rs` | Which kept values a reload may put back. |

The module docs at the top of each of those files are the detailed
reference. This page is the map.

- [The classifier](#the-classifier)
- [The cache: stretches, segments and slots](#the-cache-stretches-segments-and-slots)
- [The skip lowering](#the-skip-lowering)
- [What keeps a kept value from changing](#what-keeps-a-kept-value-from-changing)
- [The table and its lifetime](#the-table-and-its-lifetime)
- [Following the diff on reload](#following-the-diff-on-reload)
- [Values inside function bodies](#values-inside-function-bodies)
- [Observation mode](#observation-mode)
- [When it is on](#when-it-is-on)
- [Tests, fuzzers and oracles](#tests-fuzzers-and-oracles)
- [Invariants](#invariants)

## The classifier

`static_classify::analyze(program, natives)` is a dataflow analysis over the
term graph. It changes nothing about how a program runs. Each term gets one
of three classes (`TermClass`):

- **data**: a function of the source text alone, holding no closure;
- **function**: a known function value, as a set of possible targets;
- **dynamic**: anything else, including anything the pass cannot resolve.

A fourth value, `Top`, is the optimistic starting point and never survives
the pass. A user function is **static-pure** when every term of its body is
static under the assumption that its parameters are data. That assumption is
checked at each call the pass calls static (the arguments must be data).
Callees are resolved the two ways a function is actually bound: a direct
closure capture, and the write-once cell a hoisted forward reference goes
through. Recursion is solved as a fixpoint that starts from "pure".

The structural rules (a region is static only as a whole, a name bound more
than once is dynamic at every definition, a value that would hold a function
is dynamic, a method call needs every method of that name, a field read that
is some enum's constant-data key needs that data) are listed in the module
doc and, as the user sees them, in the
[feature doc](../features/static-values.md#what-is-static).

**Natives.** A native is static only when its row is `CONST`
(`native_fn::NativeConstness`: `Const`, `Dynamic`, or `ConstIfCallbacksAre`
for `map`, `filter`, `reduce`, `sort`, `sort_by`). A name nothing registered
is dynamic. So the answer depends on the native table of the `Env` that asks,
and a process without the host's natives (`petal check`, the language
server) reports less as static than the host does, never more.

**The output** is a `StaticAnalysis`: the class of every term, and a
`StaticReport` with one `StaticDecl` per top-level declaration. A
declaration carries its class, the reason chain when dynamic (`ReasonStep`,
built from a small `Copy` `Why` per term), who reads it (`Reader`: dynamic
code, or only other static declarations), and its **dependency set**: every
declaration its value can depend on, by module-qualified name, closed
transitively. The dependency set is what a reload compares. `petal
show-static` prints the report, and `Env::static_report` returns it for a
host's own natives.

**`const`.** `Env::check_consts` runs at the end of a compile, only for a
program that declares a `const`, and turns `ConstCheck::NotConstant` into a
compile error carrying the reason chain. To tell "dynamic under every host"
from "dynamic only because a native is not registered here", `analyze` runs
the pass a second time with `assume_missing_const` set, which takes every
unregistered native and every method name nothing defines to be constant. A
`const` that is static only under that assumption is `ConstCheck::Unverified`
and is left for its host to report. The term classes stay those of the first
pass.

**The classification never reads a literal's value.** It reads the shape of
the graph and the names the compiler wrote into it. A literal-only edit
therefore cannot change any answer, which is why a reload that only patches
literals does not classify again.

**It runs once per recompile.** The answer is kept beside the program
(`Program::static_analysis`, a `KeptAnalysis` stamped with the native table's
revision and the term count) by whichever asks first, the `const` check or
`Env::ensure_bytecode`, and `static_classify::analyze_kept` serves it to the
lowering, the audit and `Env::static_report`. An incremental re-analysis
after an edit was considered and not built; the reasons are in the doc
comment on `KeptAnalysis`.

## The cache: stretches, segments and slots

The cache pass (`static_cache::apply`) works on the **lowered root code,
after every other pass**, because that is where it can see what it must not
break: registers and labels as they finally are. It adds instructions and
moves or removes none.

- A **candidate** instruction belongs to a top-level statement whose term is
  static data and is in the `StaticPlan`, and is of a kind that is safe to
  leave out (`skippable_inst`): no `state`, no cell, no `return`, no call of
  a native that is not constant. A statement with one instruction that fails
  is out as a whole. So is one that reads a register some dynamic instruction
  writes. The classifier says that cannot happen; the pass checks the
  lowering instead of trusting it.
- A **stretch** is a maximal sequence of candidates that control flow enters
  only at its first instruction and leaves only at its end. A function
  declaration ends a stretch, because its `MakeClosure` runs on every run
  where it stands.
- A stretch is cut into **segments** (`cut`), one behind each value that
  something outside its own segment reads. Each segment builds, stores and
  is rebuilt alone. A cut is never placed inside a top-level statement or
  across a jump, so a loop and its result are one segment.
- A register gets a **slot** when anything outside the segment that writes
  it reads it: dynamic code, a closure capture, a static statement that is
  not skippable (a top-level `assert`), the program's result, or another
  segment. The last case is what keeps sharing intact: with `let A = [P, 1]`
  and `let B = [P, 2]`, a rebuilt `B` must be handed the `P` the kept `A`
  holds.
- A stretch whose values nothing reads always executes, so an unused binding
  that raises keeps raising. So does a stretch lighter than `MIN_WEIGHT`
  (one allocation): loading two numbers costs less than testing for them.
- **Enum constant data** is published into the stack's enum table by a call
  the declaration makes on every run. Where the data is static that call is a
  candidate, and the record it publishes gets a slot whoever reads it. The
  enum table and the static table are emptied together, so a kept slot always
  has its record published.

A segment that raises never reaches its store, so its value is not kept and
it raises again on the next run at the same place.

`PETAL_OPT_STATS=1` prints what the pass did for a lowering
(`static_cache::describe`):

```text
$ PETAL_OPT_STATS=1 petal run --runs 2 app.ptl
static cache: 3 candidate runs, 2 skippable in 2 segments, 2 slots; a frame that skips them all leaves out 8 of 16 root instructions (1 of them allocations) for 2 checks
  s0: TILES at line 4
  s1: (unnamed) at line 7
```

## The skip lowering

Two instructions (`backend/bytecode/isa.rs`) wrap each segment:

```text
      static_skip run 0 [r7 = s0, r9 = s1] -> L     ; the group: every value kept? load, jump
      static_skip run 1 [r7 = s0] -> A              ; one value: kept? load, jump
      ...the instructions that build r7, exactly as they lower without the cache...
      static_store [s0 = r7]
  A:  static_skip run 2 [r9 = s1] -> L
      ...the instructions that build r9...
      static_store [s1 = r9]
  L:  ...
```

`StaticSkip` loads each of its slots into its register and jumps when every
slot the run *feeds* is filled; otherwise it falls through. What a run feeds
is its own slots plus the slots of every later run that reads one of its
registers (`StaticRun::feeds`), so a run is skipped only when the runs that
read it will be skipped too. A stretch of several segments has one more
`StaticSkip` in front, the **group**, which is the only test a frame makes
when every value of the stretch is kept.

`StaticStore` keeps each register in its slot and marks the value immutable.
A value that cannot be kept rejects every slot of the instruction for the
life of the table, and the segment then executes on every run as it would
without the cache.

The VM's handlers are in `backend/bytecode/vm/dispatch.rs`, one pair for the
root frame and one for function frames.

The lowering entry point is `backend::bytecode::lower_with_statics(program,
flags, analysis, natives)`. `lower_with_flags` has no analysis and so never
applies the cache; `Env::lower_with` picks between them from
`OptFlags::static_cache`. Because no instruction is moved or removed, **the
lowering without the flag is the oracle**: the same code minus the skips and
stores.

## What keeps a kept value from changing

Petal's optimizer rewrites a write as an in-place edit where it can prove the
container is not shared. A kept value is shared with every later run, so
three layers keep such an edit away from it. The first two are static and
the last is at run time.

1. **A value that is mutated is never kept.** `StaticPlan::of` leaves out
   every top-level term whose value reaches the container operand of a write
   or of a mutating builtin (`mutated_terms`), and the pass refuses a slot to
   any register that reaches an in-place instruction.
2. **A slot load is never a fresh root.** The escape analysis and the
   last-use pass are handed the plan (`escape::analyze_with_static_roots`,
   `lastuse::apply_with_static_roots`) and will not rewrite a mutation in
   place on a term a slot may fill.
3. **The immutable bit.** `StaticStore` calls `Heap::mark_immutable`, which
   sets a bit on the value and every container under it. The in-place heap
   operations copy an object with the bit instead of editing it. This is what
   holds when a kept table is handed to a function that writes its parameter,
   or an element of it is edited through an alias.

`mark_immutable` is also what finds a value that cannot be kept: it fails on
a function, a `var` cell, a host handle or a pending result, and the store
rejects the slots (`StaticSlots::reject`, reported through
`StaticSlots::rejected` and `Env::take_static_warnings`).

The immutable bit is distinct from `freeze`. An immutable object is traced
and swept like any other and is released when nothing holds it.

## The table and its lifetime

`StaticSlots` belongs to the `Stack` (`stack.statics`) and is a GC root. A
slot is `Empty`, `Filled(value)` or `Rejected`.

The table is stamped with the lowering it was filled under
(`StaticLayout::id`, new for every lowering, so for every program and every
flag set) and the revision of the program's constant table (moved when a
literal is patched in place). `StaticSlots::sync` empties a table that meets
any other stamp. `transfer_stack_state` clears it together with the closures,
the enum data and the memo records. A fork starts with an empty table.

`Env::static_values(stack)` returns the table slot by slot with the counters
(`StaticStats`: skips, skipped instructions, stores, rejections, clears,
values carried across a reload). `Env::clear_static_values` empties it by
hand.

## Following the diff on reload

`Env::reload_program` and `Env::set_config_value` clear the table like any
state transfer, and then put back the values the edit cannot have changed.
`begin_static_follow` takes what it needs from the old program and the table
before they are replaced; `finish_static_follow` decides and calls
`StaticSlots::install`.

Everything is asked in terms of **where things are written**:

1. `source_diff` says which top-level statements of each file an edit
   changed, added or removed, with their spans in the old and the new text.
   For a literal-only edit it says which statement each literal is in.
2. A **declaration is changed** when a term that binds it is written inside
   one of those spans (`changed_decls`). Asking by position, not by the
   diff's `(kind, name)`, is what makes a class with its methods, an
   overloaded function and a destructuring `let` come out right without a
   rule each.
3. A slot is **kept** when all of these hold:
   - none of the terms that write its register (`SlotFollow::origins`) is
     inside a changed span of the old program;
   - none of the declarations it depends on (`SlotFollow::deps`, the
     classifier's set, its own declaration included) is changed in the old
     program;
   - the new lowering has a slot with the same key (`SlotFollow::key`, the
     module-qualified name), and that slot depends on the same *names*;
   - none of those is changed in the new program.
4. A slot that passes is still rebuilt when a value its segment read
   (`StaticRun::reads`), or one the same store holds, is rebuilt. It was
   built from that very object. A rebuilt number, boolean or `nil` does not
   count, since it has no identity to share.

The comparison of names is what makes this sound without hashing anything.
An edit that changes what a name resolves to (a new import, a shadowing
declaration, an added overload or method) changes the set of names or adds a
statement that binds one of them.

A slot comes out of a reload as one of three things (`Seed`):

- **kept**: filled with the value it held.
- **rebuilt**: empty, with the old value remembered beside it. When the
  segment's `StaticStore` finds the new value `identical` to the remembered
  one (same kinds, numbers bit for bit so `1` is not `1.0`, records with the
  same keys in the same order, the same answer to `frozen`), the slot and the
  register take the old heap value, provided the new one holds no kept object
  the old one does not (`replaces`).
- **deferred**: filled with the old value although the edit changed it,
  because its last build was slow and the edit is one tick of a drag
  (`StaticDebounce`, 4 ms and 150 ms by default). It is rebuilt when the drag
  pauses or `Env::flush_static_rebuilds` is called. A slot that holds one
  literal (`SlotFollow::literal`) is given the new value directly.

Other cases:

- **A literal-only edit** recompiles nothing, so the old lowering and its
  dependency sets are still the program's. Slots are matched by position and
  only the old-program conditions apply.
- **A value with no declaration** (a table written inside a call) is matched
  across a recompile by its statement's text and shape (`unnamed_keys`). One
  whose statement is written twice has no key and is rebuilt by every reload
  that recompiles.
- **An enum's constant data** is kept or rebuilt whole and never compared.
  The segment publishes the record it builds, so a kept record is published
  again by the reload and a rebuilt one replaces it.
- **Everything is rebuilt** when the diff cannot say what changed, when
  top-level statements were reordered, when a file could not be read, when
  the program's file table changed, and under `Env::set_static_follow(false)`.

A reload that follows static values lowers the new program itself, so that it
has the new layout to compare with. That lowering used to be the next
frame's.

The reload also reports **stale seeds**: `FollowDecl` records, for each
top-level `state`, what its initializer depends on
(`StaticAnalysis::state_initializer_dependencies`, which is narrower than the
dependencies of every write to the cell), and a cell whose initializer read a
changed declaration is named in `StaticReload::stale_seeds`.

## Values inside function bodies

`static_cache/in_fn.rs` gives a function's code the same two instructions
around a value the function builds the same on every call
(`OptFlags::static_in_functions`). Registers are per call, so the value lives
in a slot of the stack's table, behind the top-level ones
(`StaticLayout::root_slots`, `root_runs`). The first execution fills the
slot, in whichever call that is, and every later one loads it.

Two things have to hold, and neither is enough alone:

- **The term is invariant** (`FnPlan`). The classifier calls a term inside a
  function static *assuming the parameters are data*, so `[a, 1]` is static
  there and a different list on every call. A term is invariant when it is
  static and reads nothing that belongs to the call: only constants, other
  invariant terms, the function itself, and captures that are a kept
  top-level value or a function at every place the closure is made. A
  parameter never is. A loop or pattern variable never is on its own; a loop,
  branch or `match` is invariant as a whole when everything under it is, and
  is then kept with everything under it or not at all.
- **The instruction is clean.** Every register it reads is written only by
  kept candidates, or is an invariant capture or the function's own name, and
  every register it writes is written by nothing else.

Where these differ from the top-level cache:

- The escape analysis is *not* told about these values. A caller that edits
  in place the "fresh" list a function returns pays one copy (the immutable
  bit) and edits the copy from then on. Telling the analysis would turn every
  such edit into a copy. For the same reason a function some caller writes to
  the result of keeps nothing that can come out as its result.
- They are **not followed**. Every reload that patches a literal or
  recompiles empties all of them (`StaticSlots::empty_function_slots`),
  whatever the edit was. They are emptied again when a paused drag rebuilds,
  since one may have been built from a value the drag left stale. They are
  not in the reload report's counts.
- They are **not kept where the lowering observes**: a skipped stretch would
  have to report the names bound inside it per call.
- The slot belongs to the place in the program, not to the call. A recursive
  call that reaches the code while an outer call is building the value builds
  it too, and the first to finish fills the slot. A closure made per call
  shares its slots with every other closure of that function, which is why a
  capture has to be invariant at every place the closure is made.

**The check.** `StaticSlots::set_check` (`Env::set_static_check`,
`PETAL_STATIC_CHECK=1`) makes every skip in front of such a value fall
through. The code runs on every call and the store compares what it built
with what the slot holds. A difference is recorded in
`StaticValues::mismatches`; one that is only `frozen`
(`identical_but_for_frozen`) is a note, because `freeze` reaching the one
kept value is the documented difference and not a bug.

`static_survey.rs` and `cargo run --release --example static_survey` count
the invariant expressions of a program without changing how it runs.

## Observation mode

A host that reads the last value bound to every named term after each frame
(`Env::observations_mut().enable()`: every Garden panel, `petal run
--observe`) must read the same thing from a frame that skipped a segment as
from one that ran it. So where the lowering preserves observations
(`StaticLayout::observes`):

- A segment that executes writes down what was observed while it did, in the
  functions it called too, one value per term. `StaticStore` keeps that log
  with the stack (`StaticSlots::keep_log`), and a `StaticSkip` that is taken
  records it again where the segment stood. This is what a memoized scope
  does with `Dep::Observed`.
- The log is part of what a skip needs: a segment counts as built only when
  it has one.
- A reload that leaves the lowering in place (moved text, a patched literal)
  hands on the log of a segment the edit did not reach (`RunFollow`). A
  reload that recompiles hands on none, since a log names terms of the old
  program. A segment left with its values and no log executes once more on
  the next frame to write it, and its store puts the kept values back, so
  nothing that holds one sees a copy.
- A statement that edits in place a container another statement built (`let
  cur = drop_last(BASE)` where nothing reads `BASE` again) is not skipped
  under observation, because the buffer still names `BASE` and shows the
  edit.
- The values a log holds are GC roots for as long as the segment is kept.

Values inside function bodies are not kept at all in this mode.

## When it is on

`OptFlags::static_cache` and `OptFlags::static_in_functions`
(`core/src/backend/mod.rs`) are the switches. The second is inert without the
first.

| | Top-level values | Function bodies |
|---|---|---|
| `fast`, `replay` | on | on |
| `baseline`, `explain` | off | off |

A [run policy](../CLI.md#run--execute-a-program) name takes modifiers
(`RunPolicy::parse` in `core/src/policy.rs`): `-static` / `+static` switch
the cache and `-fnstatic` / `+fnstatic` the function-body part alone, so
`fast-static` is the default without the cache and `fast-fnstatic` is the
cache for top-level values only. `+opt` turns the cache on with the
optimizer unless the policy preserves the trace or observations, and `-opt`
turns it off. `Env::set_static_cache` sets the same flag on the `Env`'s
policy.

`Env::effective_opt_flags` then turns the cache off for a run where it cannot
hold:

- **the per-term trace is on** (`preserve_trace`): a skipped run records
  nothing for it, and what it would have recorded is every instruction;
- **the static audit is on**, since the audit watches static code run. The
  exception is the static-only build with its values kept
  (`keeps_static_values`).

`petal run` without `--policy`, `PETAL_POLICY` or `--runs` switches the cache
and the memo off (`cli/handlers.rs`): a script that runs once has no second
run to keep a value for, and classifying it is a cost on every start.

The effective flags are the bytecode cache key, so toggling any of these
re-lowers the program on its next run, and the new `StaticLayout::id` empties
the table.

## Tests, fuzzers and oracles

Every test of the cache is a differential. The cache must never change what
a program does, only how much of it runs, so each case drives the same frames
through two or more worlds and compares everything a host could tell apart:
prints, the run's value or error, state, and for a reload the tables slot by
slot.

| Oracle | Against |
|---|---|
| The cache off (`with_static_cache(false)`, `replay-static`) | The cache on. |
| A reload that rebuilds everything (`set_static_follow(false)`) | A reload that follows the diff. |
| The cache checking itself (`set_static_check(true)`) | Values kept inside function bodies. |
| The static audit (`static_audit.rs`) | The classifier, and each native's `CONST` row. |

Where they are:

- `core/src/static_classify/tests.rs`: the classifier's rules.
- `core/tests/const_decl.rs`: `const` parses, formats, exports, hoists and
  runs as a `let`, and errors when not static.
- `core/tests/static_cache.rs`: the top-level cache, on against off.
- `core/tests/static_reload.rs`: following the diff, against rebuilding
  everything.
- `core/tests/static_in_functions.rs`: function bodies, three ways.
- `core/tests/static_observe.rs`: what a host reads from the observation
  buffer, with the cache on and off, across every kind of reload.
- `core/tests/static_review3.rs`: lists of frames, reloads and host calls
  through four worlds at once.
- `core/tests/static_audit.rs`, `static_audit_env.rs`,
  `static_check_env.rs`: the audit against natives that lie about being
  constant, and the two environment variables.
- `core/tests/lint_state_const.rs`: the `state-as-constant` rule.
- `core-libs/petal-ui/tests/static_classify.rs`, `static_audit.rs`,
  `static_cache.rs`, `static_reload.rs`, `static_in_functions.rs`: the same
  oracles over the whole panel corpus under a monkey scenario. Their
  `--ignored` tests print the measurements recorded in
  [dev/performance.md](../dev/performance.md#static-values).
- `const_builtins_are_exactly` in `core/src/builtins/mod.rs` pins which
  builtins are `CONST`.

Fuzzers, all in `core/src/backend/bytecode/` (`cargo test --lib <name>`,
`PETAL_FUZZ_ITERS=<n>` for a soak):

- `static_fuzz.rs`: top-level tables of every kind with dynamic declarations
  between them, every route of writing to a value that may be kept, and three
  reloads, each with the cache on and off under four policies.
- `static_fn_fuzz.rs`: function bodies that mix what may be kept with what
  looks the same and may not, under the check.
- `fuzz.rs` (`static_cache_fuzz`): the general generator's programs, run
  three times each, cache on against off.
- `enum_all_fuzz.rs`: lists that begin as `Enum.all`, the one list the stack
  keeps for an enum.

Whole suites can be run under the two environment variables:
`PETAL_STATIC_AUDIT=watch cargo test -- --nocapture` and
`PETAL_STATIC_CHECK=1`, then search the output for `static audit failure:`
and `static check failure:`. `tools/test-examples.ts` runs every example
twice on one stack with the cache on and off. The wider description of these
harnesses is in [dev/testing.md](../dev/testing.md).

Measuring tools: `core/examples/static_frame.rs` (the static part of a frame
with the cache off and on, for a script whose host is not here),
`reload_follow.rs` and `reload_timing.rs` (what a reload costs), and
`static_survey.rs`.

## Invariants

A change to any of this must keep the following true.

1. **The cache changes nothing a program can observe**, except `frozen` on a
   kept value. Printing, returning, raising, storing and what a host reads
   from the observation buffer are the same with `static_cache` on and off.
   A new feature that a program can use to tell a kept object from a rebuilt
   one (an identity test, for example) breaks this.
2. **The pass adds instructions and moves or removes none.** The lowering
   without the flag has to remain the same code minus the skips and stores,
   or the differential stops being an oracle. Run the pass last.
3. **Dynamic is the safe answer.** The classifier may call less static than
   is true and never more. A new `TermOp`, a new way to bind or call a
   function, or a new builtin must be dynamic until it is taught otherwise.
4. **A native is constant only by an explicit `CONST` row.** A new builtin
   that reads anything but its arguments (a seed, the clock, a handle, the
   source) must not be `CONST`; `const_builtins_are_exactly` has to be
   updated deliberately.
5. **The classification never reads a literal's value.** A reload that
   patches literals relies on the old analysis still being right.
6. **A kept value is never edited in place.** A new in-place heap operation
   has to test the immutable bit and copy, and a new mutating term or builtin
   has to be known to `mutated_terms`. The immutable bit is the last guard,
   not the only one.
7. **A kept value holds no function, `var` cell, handle or pending result.**
   A new value kind that carries identity, host state or a reference into a
   program has to fail `Heap::mark_immutable`.
8. **Code that must run on every run is never skipped**: a raise in an
   unused binding, a top-level `assert`, a `MakeClosure`, anything with an
   effect.
9. **A table is only read under the stamp it was filled under.** Anything
   that changes the lowering or a constant has to change `StaticLayout::id`
   or the constant table's revision, and every path that swaps a stack's
   program has to clear `stack.statics` along with the enum table and the
   memo.
10. **A reload keeps a value only with the values it was built from**, and
    swaps a rebuilt value for an identical old one only where that puts no
    second copy of a kept object beside the first. Which objects two values
    share must be the same after a followed reload as after rebuilding
    everything.
11. **When the diff cannot say what changed, rebuild everything.** A new kind
    of edit that `source_diff` reports, or a new way for a name to change
    meaning, has to either show up as a changed span or a changed name set,
    or fall into the rebuild-all path.
12. **Function-body values read nothing of the call.** The classifier's
    "static" is not enough inside a function; a new source of per-call values
    (a new kind of binding, an implicit parameter) has to be excluded in
    `FnPlan`.

Each of these has a differential that fails when it is broken. When adding a
feature that touches one, extend the matching fuzzer's generator so that it
writes the new construct.

## See also

- [features/static-values.md](../features/static-values.md): what a script
  author and a host see.
- [The static-values proposal](../proposals/static-values.md) and
  [the work record](../tasks/static-value-optimization.md): the design, the
  alternatives, and what each stage found.
- [dev/performance.md](../dev/performance.md#static-values): measurements.
- [dev/Architecture.md](../dev/Architecture.md): the compiler and the VM
  around this.

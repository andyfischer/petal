# Plan: Isolated / speculative script execution

## Goal

Petal aims for *cheap experimentation*: take a running script, alter it (or its
inputs), and run the variant **without disturbing the original execution** — so
the original and the variant can run side-by-side and be compared. Each
execution already gets its own `Stack`. The remaining problem is the **shared
heap**: heap objects are mutable and shared, so a variant run corrupts the
objects the original still points at.

This document analyzes the root cause and proposes an implementation.

## Status / start here (handoff)

**Approach chosen:** Option B — make collections immutable-by-default (value
semantics), so heap objects are never mutated and sharing between executions is
safe. See [Decision: Option B](#decision-option-b) and the
[incremental roadmap](#incremental-roadmap-toward-immutable-by-default).

**⭐ Core goal ACHIEVED (Increments 1–3 + 5 accessor removal).** The heap is
immutable by construction, and the plan's root-cause isolation bug is fixed:
because no heap object is ever mutated, a speculative frame that "mutates" a
state collection cannot corrupt the object the committed state still points at.
Proven by
`env.rs::speculative_tests::speculative_run_does_not_corrupt_committed_state_objects`.
This delivers single-timeline speculative isolation *without* the Option-A
copy-on-write/`Rc` slot machinery.

**⭐⭐ `ExecutionContext` system SHIPPED (2026-06, steps 3–6).** The
two-concurrently-live machinery is now built and on `main`. Execution-local
state (heap + the registries that reference it) lives in a standalone
`ExecutionContext` struct (`rust/src/execution_context.rs`); `Env` holds a
`contexts: HashMap<ContextKey, ExecutionContext>` map plus a `default_context`,
and each `Stack` links to its context by a `ContextKey` (key, not pointer).
`Env::fork_execution(src)` forks a stack into a fully isolated context+stack
copy that shares no mutable heap state with the source, and `run_speculative` is
now re-expressed on top of it (fork → run the fork → drop the fork), so a
speculative frame leaves the source *entirely* untouched — state, heap objects,
and even print output. Per-context GC, the fork, and the stronger speculative
isolation are all unit-tested (`env.rs::fork_tests`, `speculative_tests`). What
remains is only the host/CLI/WASM surface (step 7) and Increment 4 (perf).

**Design as built — simpler than the pin (no threaded `StackContext` view).**
The earlier pin called for a transient `StackContext` borrow view threaded
through the evaluator to keep parameter counts down. In the event that proved
unnecessary: the existing `Evaluator<'a>` struct (`eval/mod.rs`) *already*
bundles exactly the seven execution-local borrows (`heap`, `closures`,
`overload_sets`, `output`, `output_buffers`, `bindings`, `counters`), so it
*is* the borrow view. Moving those fields into `ExecutionContext` meant
`Env::step`/`call_function` simply borrow them out of the resolved
`&mut ExecutionContext` (disjoint from the `&mut Stack` and `&Program`, since
`stacks`/`contexts`/`programs` are distinct fields of `Env`) — and **all of
`eval/*.rs` was left unchanged**. The "don't nest stacks inside the context"
rule from the pin still held and is why the disjoint-field borrow works.

**Shipped (Increment 1, on `main`):** lists are immutable. `Heap::list_append`
returns a new list; the `append` builtin is immutable; `push` is a deprecated
immutable alias. All `examples/` and `apps/` scripts migrated off in-place
`push`; tests + docs updated; garden scripts checked (no list-mutation, nothing
to migrate). Commits: `feat: make list append immutable …` → `refactor: migrate
apps to immutable append`.

A companion compiler fix was required and shipped: reassigning a `state`
variable inside a loop (`xs = append(xs, x)`) now persists across runs
(`compiler/phi.rs::emit_body_phi_ins` tags the loop carry Copy with the state
key). Without it, value-semantic accumulation into a state list silently
vanished each frame.

**Shipped (Increment 2):** index/field assignment is immutable. `xs[i] = v` and
`obj.f = v` (incl. nested `grid[y][x] = v`, `obj.items[i].f = v`) recompile into a
functional rebuild of the path + rebind of the root variable. `Heap::list_set` /
`map_set` / `f64_array_set` return new collections; `SetIndex`/`SetField` now
*produce* the updated collection instead of mutating + returning Nil. Both
assignment forms route through a shared `compiler/stmt.rs::rebind_name`, so the
Increment-1 in-loop-state-persistence machinery applies for free. Two companion
fixes were required:
- `compiler/phi.rs`: `collect_assigned_names_stmts` now treats the *root* of an
  index/field assignment (via `assign_target_root`) as a reassignment, so an
  in-loop `grid[i] = v` registers as a loop carry and its `StateWrite` reaches
  the base state slot.
- `eval/exec.rs::exec_set_index`: list index-assign now resolves negative
  indices from the end, symmetric with `GetIndex` — required so a negative
  index at a *non-leaf* level of a nested path (`grid[-1][0] = v`) rebuilds the
  same slot it read.

Non-variable-rooted assignment (`foo()[0] = v`) is a dead store under value
semantics and now emits a compile-time `Error` term rather than silently
dropping. All `examples/`/`apps/` scripts run headless; only `tetris.ptl` needed
migrating — it routed grid writes through a `set_cell(grd, …)` helper and relied
on by-reference mutation, so `set_cell`/`lock_piece`/`clear_lines` now take the
grid and return the updated grid (`grid = lock_piece(grid)`).

**Shipped (Increment 3):** the remaining in-place collection builtins are
immutable. f64-array `set`/`swap` return a new array (callers rebind
`a = set(a, i, v)`); `pop` is a deprecated immutable alias of the new
`drop_last` (returns the shortened list, not the removed element); new builtins
`last` (read final element), `drop_last`, and `remove` (map key removal) were
appended to the registration table (order is load-bearing — phantom term
indices). Heap gained `list_drop_last`/`f64_array_swap`/`map_remove` mirroring
the existing immutable ops, each unit-tested for non-mutation. Only
`noc_fractal_tree.ptl` used the value-returning `pop`; migrated to
`last`+`drop_last`. Commits: `feat: add immutable heap ops …` → `feat: make
pop/set/swap/remove immutable …`.

**Former limitation `let g = <state_var>` — RESOLVED (could not reproduce).**
Earlier this was flagged as: a `let` alias of a state var, reassigned by index
(`g[i] = v`), is silently dropped. As of Increments 2–3 + the nested-`if`
loop-carry fix below, it no longer reproduces under any tested pattern
(single/multi-frame, write-back across frames, nested index, inside an `if`, via
a function parameter). Pinned by
`env.rs::speculative_tests::let_alias_of_state_var_mutated_by_index_persists`.
Reopen only with a concrete failing repro. (The old lead —
`compiler/expr.rs::compile_ident` not carrying the source `state_key` — was a
red herring for these patterns; value semantics make `g` an independent local,
which is the desired behavior, and the path-assign root registration from
Increment 2 routes the rebind correctly.)

**Compiler bug found + fixed during Increment 3:** multiple reassignments of a
loop-carried variable inside a nested `if` block within a loop used to lose all
but the *first* write per iteration (`for i in range(0,3) do if true then s =
append(s,i); s = append(s,i*10) end end` gave `[0,0,1,2]` instead of
`[0,0,0,1,10,2,20]`). Cause: `rebind_name` only logged the *first* in-block
rebind into `block_rebinds` (the phi-out source map); subsequent in-block
reassignments took the `existing_block == current_block` fast path and updated
only the scope, so the conditional's phi-out wired from the stale first binding.
Fix (`compiler/stmt.rs::rebind_name`): also route through
`rebind_name_in_current_block` when the name already has a rebind logged in the
current block, keeping `block_rebinds` on the latest binding. Pre-existing since
Increment 1; exposed by `noc_fractal_tree.ptl`, which now builds the full tree
(1023 branches / 512 leaves at depth 9) instead of one child per node. Regression
test in `ts/test/loop-carry-limitations.test.ts`. Commit: `fix: persist every
rebind of a loop-carried var inside a nested if-block`.

**Next: optional/secondary work** — Increment 4 (persistent backing for
performance) and the host/CLI/WASM surface (step 7) that exposes
`fork_execution` + per-fork output/diff to the SDL and web apps. The
`ExecutionContext`/`fork_execution` core (steps 3–6) is now **shipped** — see
the ⭐⭐ note in the handoff above. The single-timeline speculative-isolation
goal was already met before that (see ⭐).

**How to verify (live, not just `cargo test`):** the bug above only surfaced
under multi-frame execution. Build `apps/petal-sdl` and run headless:
`petal-sdl --headless <file.ptl>` then pipe `{"cmd":"step","n":50}` +
`{"cmd":"state"}` to inspect state-list lengths; or `--screenshot out.png
--frames N <file>` (no window; runs the program multiple times, so it exercises
cross-frame state persistence). Sweep all `apps/petal-sdl/examples/*.ptl`.

## Current state (as of this writing)

- One `Env` owns exactly one `Heap` (`env.rs`), shared by every `Stack`.
- `Value` is `Copy`. Heap-backed variants (`String`, `List`, `F64Array`, `Map`,
  `Element`, `EnumVariant`) are just `u32` IDs indexing `Vec`s in the heap
  (`heap.rs`, `value.rs`).
- Collections still have **reference / in-place mutation** semantics for the
  ops not yet migrated: `pop`, `swap`, `set`, `SetField`, `SetIndex` go through
  `get_*_mut` and mutate the object behind the ID; every alias observes the
  change (`builtins/collections.rs`, `eval/exec.rs`). (As of Increment 1, list
  `append`/`push` are immutable and return a new list — those no longer mutate.)
- Persistent script state lives in `stack.state: HashMap<RuntimeStateKey, Value>`
  (`stack.rs`). These Values are often heap IDs.
- `run_speculative` (`env.rs`) snapshots/restores **only `stack.state`** and
  resets the stack. It does **not** isolate the heap.
- GC is a stop-the-world mark/sweep over *all* stacks + Env-level roots
  (`env.rs::collect_garbage`). Heap slots are reused via free-lists, so a freed
  ID can be handed back out.
- Other execution-local state also lives on the `Env`, not the stack:
  `closures`, `overload_sets`, `output`, `output_buffers`, `counters`,
  `bindings`, plus the `symbols` intern table.

## Root cause

The isolation goal is broken by exactly one thing: **in-place mutation of heap
objects that are shared between executions.** `stack.state` snapshotting restores
the *IDs*, but the object behind an ID is mutated permanently. Example that
already breaks today:

```
state items = [1, 2, 3]      # state var holds ListId(7)
# speculative run:
items[0] = 99                # SetIndex still mutates list slot 7 in place
# restore_state puts ListId(7) back into state — but slot 7 is now [99,2,3]
```

(`push`/`append` no longer mutate after Increment 1; the remaining in-place
ops like `SetIndex` above are what Increment 2 addresses.)

Two ways to make sharing safe:

1. **Never mutate shared objects** — copy a shared object before writing
   (copy-on-write), or
2. **Never mutate objects at all** — immutable / persistent data structures;
   "mutations" produce new IDs.

## Design options

### Option A — Copy-on-write heap + execution fork (RECOMMENDED)

Keep today's reference/mutable semantics inside a single execution. Make the
*heap itself* forkable with copy-on-write so each execution has an isolated view
that shares unmutated objects with its parent.

Mechanism:
- Wrap each heap slot's payload in `Rc<_>` (single-threaded) — `Rc<Vec<Value>>`
  for lists, `Rc<IndexMap<..>>` for maps, etc.
- `fork()` clones the slot vectors. That clones `Rc` *pointers* only (refcount
  bumps), not payloads — O(slots), no deep copy.
- `get_*_mut` becomes `Rc::make_mut`: if the slot is uniquely owned, mutate in
  place (today's fast path); if it's shared with a parent fork, clone *that one
  slot* then mutate. Mutation cost is paid only on first write to a shared
  object.
- IDs stay stable across a fork (both views index by the same `u32`), so
  existing `Value`s remain valid in both.

Pros: **zero language-semantics change, zero script breakage**; cheap fork;
genuinely supports two live side-by-side executions. Cons: per-slot `Rc` adds an
indirection and a refcount; `fork()` is O(number of live slots) in pointer
clones (cheap, but not free for very large heaps — mitigated by the layered
variant below if needed).

### Option B — Immutable persistent data structures (value semantics)

Replace `Vec`/`IndexMap` with persistent structures (e.g. the `im` crate's
`Vector`/`OrdMap`/`HashMap`) and make collections immutable. `push(list, x)`
returns a *new* list ID; the old object is never touched, so sharing is
automatically safe and forking is free (just share IDs).

Pros: the cleanest isolation; structural sharing makes "copies" cheap; aligns
with the "immutable objects" framing. Cons: this is a **breaking semantics
change** — every `push`/`set`/`SetField` used for its side effect (the common
case in current scripts and the sample apps) stops working as written; callers
must rebind (`items = push(items, x)`), and aliasing semantics flip from
reference to value. Large migration across `examples/`, `apps/*`, and
`~/garden`.

### Option C — Snapshot/restore the whole heap (no sharing)

Extend `run_speculative` to deep-clone the entire heap before the run and
restore it after.

Pros: trivial to implement. Cons: O(heap) every speculative step; no real
side-by-side (only snapshot→run→restore on one timeline); doesn't scale to the
"interactive, per-frame" experimentation Petal wants.

### Decision: Option B

We are pursuing **Option B — default immutable values with value semantics.**
The language is changing so that collection operations never mutate in place;
"mutations" produce new values and callers rebind (`xs = append(xs, x)`). Once
no heap object is ever mutated, sharing between executions is automatically safe
and forking a speculative run is free. Option A was the lower-churn alternative
but keeps reference semantics; we chose B because immutable-by-default is the
end state we want for the language itself, not just for speculation.

Backing strategy: get the *semantics* right first with copy-on-write
(allocate-a-new-collection) implementations, keeping the heap's `Vec`/`IndexMap`
storage. Later, swap the backing to persistent/structural-sharing structures so
the immutable ops are cheap; the heap API (`list_append`, …) is shaped so that
swap won't touch call sites.

## Incremental roadmap toward immutable-by-default

Each increment is independently shippable and keeps the test suite green.

1. **Immutable lists via `append` — DONE.** `Heap::list_append` returns a new
   list; the `append` builtin is immutable; `push` is a deprecated immutable
   alias. Migrated every statement-form `push(x, v)` → `x = append(x, v)` across
   `examples/` and `apps/` and updated the embedded-snippet tests + docs. Garden
   scripts (`~/garden`, `~/.garden`) checked — they use no list-mutation builtins,
   so nothing to migrate there.

   Required a companion compiler fix: reassigning a `state` variable inside a
   loop (`xs = append(xs, x)`) compiled to a plain Copy and never persisted, so
   the value-semantic migration silently broke every per-frame "build a state
   list in a loop" pattern. The loop body-entry carry Copy now inherits the
   carried name's state key, so in-loop reassignment emits a `StateWrite` to the
   base slot (see `compiler/phi.rs::emit_body_phi_ins`). Verified headless: all
   31 SDL examples run and their state lists accumulate. **Increment 2 will need
   the same care** — index/field assignment desugared to rebind must likewise
   persist when the root is an in-loop state variable.
2. **Immutable index/field assignment — DONE.** `xs[i] = v` and `obj.f = v`
   (the `SetIndex` / `SetField` term ops) recompile into functional-update-and-
   rebind of the root variable (`compiler/stmt.rs::compile_path_assign` +
   `rebind_name`); `SetIndex`/`SetField` now produce a new collection via
   `Heap::list_set`/`map_set`/`f64_array_set`. Nested paths (`grid[y][x] = v`)
   nest the rebuild. Companion fixes: `phi.rs` registers the assignment root as
   a loop carry (in-loop state persistence), and `exec_set_index` resolves
   negative indices symmetric with `GetIndex`. Scripts needed no change except
   `tetris.ptl` (routed grid writes through a helper that relied on by-reference
   mutation). (The `let g = <state_var>` follow-up once flagged here is now
   resolved / non-reproducing — see handoff.)
3. **Immutable `pop` / f64-array `set` / `swap` / map field-set & remove —
   DONE.** f64-array `set`/`swap` now return a new array (callers rebind
   `a = set(a, i, v)`) instead of mutating + returning Nil. `pop` is a
   deprecated immutable alias of the new `drop_last` (returns the shortened
   list, not the popped element); the `pop`-the-value pattern migrates to
   `last(xs)` + `drop_last(xs)`. New builtins `last`/`drop_last`/`remove`
   (map key removal) appended to the registration table (order is load-bearing).
   Heap gained `list_drop_last`/`f64_array_swap`/`map_remove` mirroring the
   existing immutable ops. Only `noc_fractal_tree.ptl` used the value-returning
   `pop`; migrated to `last`+`drop_last`. Map field-set was already immutable
   (Increment 2's `SetField`).
4. **Persistent backing — DEFERRED (optional, profiling-gated).** Swapping
   list/map/array storage to structural-sharing structures (e.g. `im`) would
   make the immutable ops stop copying whole containers — a *pure performance*
   change; semantics are already correct with the clone-based copies. Deferred
   deliberately: it is a high-churn, high-risk refactor (~110 call sites use
   `get_list`/`get_map`/`get_f64_array`, many relying on contiguous
   `&[Value]`/`&IndexMap` semantics — slice comparisons, indexing,
   pattern-destructuring — which `im::Vector`/`im::OrdMap` can't provide by
   borrow), and nothing currently profiles collection-copy cost as a
   bottleneck. The plan's own guidance gates this on profiling ("revisit with a
   chunked/persistent array if profiling shows it matters"). Pick it up only
   when a real workload shows the copies hurt; the heap API
   (`list_append`/`list_set`/…) is already shaped so the swap won't touch call
   sites of the *mutators*, only the *readers*.
5. **Remove `get_*_mut` from the heap — accessor removal DONE.** With no
   in-place mutation left after Increment 3, `get_list_mut`/`get_f64_array_mut`/
   `get_map_mut` were dead (verified: no callers in core or apps) and are
   deleted. Heap objects are now immutable by construction (documented on the
   `heap` module). This is the milestone that unlocks free speculative forking
   (no shared object can be mutated behind an alias). **`let`-alias limitation: RESOLVED / could
   not reproduce.** The `let g = <state_var>` then `g[i] = v` "silently dropped"
   symptom no longer reproduces under any tested pattern — single-frame,
   multi-frame with write-back (`count = g` per frame), nested index
   (`g[1][0] = v`), inside an `if`, or via a function parameter all behave with
   correct value semantics. Most likely closed by the Increment 2 companion fix
   (`assign_target_root` registering the path-assign root as a reassignment)
   together with the nested-`if` loop-carry fix above. Pinned by
   `env.rs::speculative_tests::let_alias_of_state_var_mutated_by_index_persists`.
   Reopen only with a concrete failing repro.
6. **Garden migration — DONE (no-op).** Re-checked `~/garden` and `~/.garden`
   (3 `.ptl` files total): none use list-mutation builtins, `push`, the changed
   `set`/`swap`/`pop`, or index/field assignment — nothing to migrate. (They do
   reference host builtins like `editor(...)` that only exist inside the Garden
   app, so they're not runnable from the core `petal` CLI; that's unrelated.)

The remaining sections describe the **`ExecutionContext` fork** machinery that
delivers *two concurrently-live* executions. Under Option B the heap is immutable
by construction, so the copy-on-write slot mechanics are unnecessary — but the
execution-context fork, per-context GC, side-effect, ID-reuse, symbol-table, and
hot-reload concerns are all still live and are addressed below.

## Recommended architecture

### 1. Heap copy-on-write — NOT NEEDED under Option B (Rc is the deferred Increment 4)

Under Option B the heap is immutable by construction (no `get_*_mut`), so there
is no in-place write to make copy-on-write — the CoW-slot machinery the original
Option-A design called for is **not needed** on the correctness path.

The only thing left here is a *pure performance* optimization, **Increment 4
(deferred, profiling-gated)**: wrap each heap slot's payload in `Rc`
(`Rc<Vec<Value>>`, `Rc<IndexMap<..>>`, `Rc<str>`) so `fork()` (below) becomes
O(live slots) pointer-clones instead of a deep copy. Semantics are already
correct without it; pick it up only if a real workload shows the copies hurt.

### 2. `Heap::fork()` — DONE

`Heap::fork()` is in place as a plain deep clone (`Heap: Clone`): the child
shares no mutable state, and pre-fork ids resolve to equal objects in both. The
intern table is cloned too — interning across forks is fine: a string allocated
post-fork in the child simply isn't visible to the parent, and pre-fork shared
strings keep their IDs. (The Increment-4 `Rc` payloads above would later make
this O(live slots) without changing the API.)

### 3. Bundle execution-local state into an `ExecutionContext`

> **As built (2026-06):** this section is the original design rationale and is
> accurate about *what* gets bundled and *why* the two-map disjoint borrow
> works. One thing changed in implementation: the transient `StackContext`
> borrow view and `with_stack_context` closure helper below were **not needed**
> — the existing `Evaluator<'a>` struct already serves as the borrow view (it
> bundles the same seven fields), so the registries are borrowed straight out of
> the resolved `&mut ExecutionContext` and `eval/*.rs` was left untouched. Read
> the rest for the ownership reasoning; treat the `StackContext` code blocks as
> historical.

The `Stack` is *not* the whole execution. A `Stack` holds only the call-stack
machinery (frames, registers, `state`, status/loop bookkeeping). The mutable
state a run actually *grows* lives on `Env` today, shared by every stack:
`heap`, `closures`, `overload_sets`, `output`, `output_buffers`, `counters`,
`bindings`. Because immutability (Increments 1–3) means a clone can't *corrupt
existing* heap objects, but two concurrently-live runs would still interleave
allocations into one heap, share closure/overload ids, and mix output —
isolating a run means isolating *this whole bundle*, not just cloning a `Stack`.

So give each `Stack` an `ExecutionContext` that owns that bundle — a `Stack`
belongs to exactly one context (many-to-one; a context may back more than one
stack), and the fork boundary is the context. The fork clones:

- `heap` — immutable by construction, so fork is a plain `Heap::fork()` deep
  clone (already in place; a later `Rc`-payload optimization can make it
  O(live slots), see Increment 4).
- `closures: Vec<RuntimeClosure>` and `overload_sets` — their captures hold heap
  IDs; appending in a child must not perturb the parent, and IDs must stay
  stable. Clone on fork.
- `counters` — per-run, clone (or reset) on fork.
- `output` / `output_buffers` — side-effect sinks. Fork gets fresh/empty buffers
  so the variant's output is captured separately and can be compared (see
  hazards below).
- `bindings` (host→script uniforms) — clone; the variant may want different
  inputs (that's often the *point* of the experiment).

Shared, read-only-after-build, NOT forked: `programs`, `native_fns`, and the
`symbols` intern table (see hazard 5 for the symbol-table caveat).

Concretely, introduce:

```rust
/// One isolated execution's mutable bundle: the heap + the runtime registries
/// that reference it. Does NOT own the Stack. Forking yields an isolated copy.
struct ExecutionContext {
    heap: Heap,
    closures: Vec<RuntimeClosure>,
    overload_sets: Vec<Vec<OverloadEntry>>,
    output: Vec<String>,
    output_buffers: HashMap<SymbolId, Vec<Value>>,
    counters: HashMap<SymbolId, u64>,
    bindings: HashMap<SymbolId, Value>,
}
impl ExecutionContext { fn fork(&self) -> ExecutionContext { .. } }
```

**Pinned ownership & borrowing (Rust):**

- `ExecutionContext` is a standalone struct owning the forkable bundle above. It
  does **not** own the `Stack`. Keeping the stack separate lets `fork_execution`
  clone the heap bundle and attach a *fresh or separately-cloned* stack, and lets
  one context back more than one stack if ever wanted (today's "one heap, many
  stacks" model, now scoped per context).
- `Env` owns two maps plus the shared, read-only-after-build parts:

  ```rust
  struct Env {
      programs: HashMap<ProgramId, Program>,   // shared, read-only-after-build
      native_fns: NativeFnTable,               //   "
      symbols: SymbolTable,                    //   "
      contexts: HashMap<ContextKey, ExecutionContext>,
      stacks: HashMap<StackKey, Stack>,
  }
  ```
- Each `Stack` links to its context by **key, not pointer**: a
  `context: ContextKey` field. Rust aliasing rules make a `Stack`→
  `ExecutionContext` back-pointer impractical; an integer key sidesteps it and is
  the idiomatic choice. Many stacks may share one `ContextKey` (many-to-one).
- A transient borrow view bundles what the evaluator needs into one parameter, so
  functions take a single `&mut StackContext` instead of five-plus args:

  ```rust
  struct StackContext<'a> {
      stack: &'a mut Stack,
      ctx: &'a mut ExecutionContext,
      programs: &'a HashMap<ProgramId, Program>,
      native_fns: &'a NativeFnTable,
  }
  ```
  It is built once per step and threaded down — never stored. Because the `Stack`
  and its `ExecutionContext` live in *separate* maps, both are mutably borrowable
  at once (disjoint fields of `Env`). Hand the view out through a closure, not a
  returned `StackContext<'_>` — a method returning the view would make the borrow
  checker treat *all* of `self` as borrowed:

  ```rust
  fn with_stack_context<R>(
      &mut self, sk: StackKey, f: impl FnOnce(StackContext) -> R,
  ) -> Result<R, String> {
      let ck = self.stacks.get(&sk).ok_or("no stack")?.context;   // Copy key
      let stack = self.stacks.get_mut(&sk).unwrap();
      let ctx = self.contexts.get_mut(&ck).ok_or("no context")?;
      Ok(f(StackContext { stack, ctx,
          programs: &self.programs, native_fns: &self.native_fns }))
  }
  ```
  Inside the evaluator, `sc.stack` and `sc.ctx.heap` are disjoint fields and can
  be sub-borrowed simultaneously; only a whole-`self` *method* on `StackContext`
  would block that, so prefer field access in hot paths.
- **Do not** nest the stacks map inside `ExecutionContext`: reaching a stack would
  then borrow the whole context, killing the `&mut ctx` + `&mut stack` pair that
  makes the view work.

### 4. New public API

```rust
impl Env {
    /// Fork an existing execution into a new, isolated stack + ExecutionContext.
    /// The new stack shares no mutable heap state with the source.
    pub fn fork_execution(&mut self, src: StackKey) -> Result<StackKey, String>;
}
```

Re-express `run_speculative` in terms of a fork: fork → run the fork → read
results / diff → drop the fork. Dropping a fork drops its `ExecutionContext`,
releasing its heap and registries.

### 5. GC must be per-context

`collect_garbage` currently marks across all stacks and Env roots into the one
heap. With multiple contexts:
- Each `ExecutionContext` GCs its own heap, rooted by its stack plus that
  context's closures/overload_sets/output_buffers/bindings.
- Each context owns its heap outright (the fork deep-clones it), so its ID space
  is independent after fork — a context must only ever sweep its own slot
  vector. Keep the marking logic; just scope roots to one context. (If Increment
  4 later makes payloads `Rc`-shared across a fork, "free" becomes "drop the
  `Rc`" and a slot's memory is reclaimed only when the last context drops it —
  the per-context root scoping is unchanged.)

## Phased implementation

1. **CoW slots (no behavior change). — NOT NEEDED under Option B.** The heap is
   immutable by construction (no `get_*_mut`), so there is no in-place write to
   make copy-on-write. Skipped.
2. **`Heap::fork()` + unit test — DONE.** Because objects are immutable, fork is
   a plain deep clone (`Heap: Clone`); the child shares no mutable state, and
   pre-fork ids resolve to equal objects in both. Test:
   `heap.rs::tests::fork_yields_an_isolated_heap_sharing_pre_fork_objects`. A
   later `Rc`-payload optimization can make fork O(live slots) (see Increment 4).
3. **`ExecutionContext` extraction — DONE.** Execution-local registries moved
   off `Env` into a standalone forkable `ExecutionContext`
   (`rust/src/execution_context.rs`: `heap`, `closures`, `overload_sets`,
   `output`, `output_buffers`, `counters`, `bindings`); `Env` gained a
   `contexts: HashMap<ContextKey, ExecutionContext>` map plus a
   `default_context`, and each `Stack` got a `context: ContextKey`.
   **No `StackContext` view was needed** — the existing `Evaluator<'a>` struct
   already bundles those seven borrows, so `step`/`call_function` borrow them out
   of the resolved `&mut ExecutionContext` and `eval/*.rs` was untouched (see the
   "Design as built" note in the handoff). Existing behavior is unaffected by
   defaulting every stack to a single context; the full cargo suite stayed green.
4. **`ExecutionContext::fork()` + `Env::fork_execution()` — DONE.** `fork()`
   deep-clones the heap (`Heap::fork()`) and the closures/overload_sets/
   bindings/counters registries, and gives the fork *fresh* empty output +
   output_buffers so the variant's output is captured separately.
   `Env::fork_execution(src) -> Result<StackKey, String>` forks the source
   context into a new `ContextKey` and clones the source stack into a new
   `StackKey` rebound to it. Proven by
   `env.rs::fork_tests::fork_runs_independently_and_leaves_source_untouched`.
5. **Per-context GC — DONE.** `collect_garbage(ck)` marks roots only from stacks
   bound to that context plus that context's own registries, and sweeps only
   that context's heap; `run` collects the running stack's context. Each context
   owns its heap outright, so cross-context isolation is structural — pinned by
   `env.rs::fork_tests::gc_in_one_context_does_not_free_objects_live_in_another`.
6. **Rebuilt `run_speculative` on `fork_execution` — DONE.** It now forks → runs
   the fork → drops the fork (releasing its context+heap), keeping the same
   public signature. The source is left *entirely* untouched, including its
   print output (the old snapshot/restore leaked speculative prints into the
   shared buffer). Pinned by
   `env.rs::speculative_tests::speculative_run_does_not_leak_output_or_heap_into_source`.
7. **Host/CLI/WASM surface — TODO.** Expose fork + per-fork output and a
   state/heap diff so the SDL and web apps can drive side-by-side runs (out of
   scope for the core; `fork_execution`'s `Result<StackKey, String>` signature
   already anticipates it).
8. **Docs + examples — DONE (this doc).** The handoff + steps above record the
   shipped design. The "forked run leaves the original untouched" demonstration
   lives as runtime tests (`env.rs::fork_tests`, `speculative_tests`) rather than
   a `.ptl` example, since forking is a host-API capability with no script-level
   syntax.

## Other hazards with parallel / speculative scripts

These are issues independent of the heap that will bite once two executions run
against shared state. Calling them out per the design ask:

1. **Side effects aren't transactional.** `print`, draw-command output buffers,
   and any native function with external effect (file/network/SDL draw) *do*
   happen during a speculative run. Forking buffers (step 3) captures *script*
   output cleanly, but genuinely external effects (I/O) can't be rolled back —
   speculative runs should either forbid effectful natives or run them in a
   record/replay sandbox. Decide a policy and enforce it at the native-fn
   boundary.

2. **Closures / overload sets on the `Env`.** Today they're a shared `Vec`
   indexed by ID. *Even without speculation*, two stacks in one Env already
   share this Vec — a latent bug for side-by-side runs. Forking the registries
   (step 3) fixes it; ensure `ClosureId`/`OverloadSetId` values captured before
   the fork still resolve in both contexts (they will, since the Vec is cloned).

3. **Heap ID reuse via free-lists.** A freed slot's `u32` is reused. If a stale
   `Value` (e.g. cached in host code, or in dropped-but-not-cleared state)
   outlives a GC, it can alias a *different* object after reuse. This is a
   pre-existing footgun that gets worse with multiple contexts. Consider
   generational IDs (`{index, generation}`) so a stale ID fails fast instead of
   silently aliasing.

4. **GC determinism / IDs in comparisons.** If side-by-side comparison ever
   compares heap IDs directly (rather than structural value equality), GC timing
   and free-list order make IDs non-deterministic between two runs. Compare by
   value, never by ID.

5. **Symbol table interning.** `symbols` is append-only interning shared on the
   `Env`. If two forks intern *new, different* symbols concurrently they could be
   assigned the same ordinal and diverge. Either (a) keep the symbol table shared
   and require interning to be monotonic/coordinated, or (b) snapshot it per
   context. Simplest: treat the symbol table as shared+append-only and accept that
   a symbol interned in a child is globally visible (symbols are identity, not
   mutable state, so this is usually fine — but document it).

6. **`transfer_state` (hot reload) interaction.** Hot reload clears closures and
   retains state keyed by compile-time `StateKey`. With contexts, decide whether a
   reload forks a fresh context or mutates in place. The state Values carry heap
   IDs, so a reload that swaps the program but keeps the heap must keep the same
   context.

7. **`F64Array` / large buffers.** CoW copies a whole f64 array on first write.
   For big numeric buffers mutated element-by-element in a speculative run, that
   first-write copy can be expensive (though still paid once). Acceptable to
   start; revisit with a chunked/persistent array if profiling shows it matters.

8. **Memory growth from many forks.** Each live fork pins the objects it has
   copied. Long-lived side-by-side sessions that keep forking need a way to drop
   old forks; make fork lifetime explicit and ensure dropping an
   `ExecutionContext` releases its heap/`Rc`s promptly.

## Out of scope / explicit non-goals

- True OS-thread parallelism. `Rc` keeps this single-threaded; if real threads
  are ever needed, switch `Rc`→`Arc` (and the interior types to thread-safe
  ones), which the design allows but doesn't require now.
- Moving to value semantics for collections (Option B). Reconsider only as a
  deliberate language-design decision.

# Plan: Isolated / speculative script execution

## Goal

Petal aims for *cheap experimentation*: take a running script, alter it (or its
inputs), and run the variant **without disturbing the original execution** — so
the original and the variant can run side-by-side and be compared. Each
execution already gets its own `Stack`. The remaining problem is the **shared
heap**: heap objects are mutable and shared, so a variant run corrupts the
objects the original still points at.

This document analyzes the root cause and proposes an implementation.

## Current state (as of this writing)

- One `Env` owns exactly one `Heap` (`env.rs`), shared by every `Stack`.
- `Value` is `Copy`. Heap-backed variants (`String`, `List`, `F64Array`, `Map`,
  `Element`, `EnumVariant`) are just `u32` IDs indexing `Vec`s in the heap
  (`heap.rs`, `value.rs`).
- Collections have **reference / in-place mutation** semantics. `push`, `append`,
  `pop`, `swap`, `set`, `SetField`, `SetIndex` all go through `get_*_mut` and
  mutate the object behind the ID; every alias observes the change
  (`builtins/collections.rs`, `eval/exec.rs`).
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
push(items, 4)               # mutates list slot 7 in place
# restore_state puts ListId(7) back into state — but slot 7 is now [1,2,3,4]
```

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
2. **Immutable index/field assignment.** Recompile `xs[i] = v` and `obj.f = v`
   (the `SetIndex` / `SetField` term ops) into functional-update-and-rebind of
   the root variable, so existing assignment *syntax* keeps working with value
   semantics and scripts need no change. Two-level paths (`grid[y][x] = v`) nest
   the rebuild. This is the main compiler change.
3. **Immutable `pop` / f64-array `set` / `swap` / map field-set & remove.**
   Decide the `pop` shape (it returns both a value and a shorter list — likely
   split into `last` + `drop_last`, or return a pair). `pop` is currently unused
   by any script, so this is low-risk.
4. **Persistent backing.** Swap list/map/array storage to structural-sharing
   structures (e.g. `im`) so the immutable ops stop copying whole containers.
5. **Remove `get_*_mut` from the heap.** With no in-place mutation left, delete
   the mutable accessors; heap objects become immutable by construction. This is
   the milestone that unlocks free speculative forking (see Option A/§fork for
   the now-trivial isolation).
6. **Garden migration.** Apply the same `push`→`append` and assignment changes
   to `~/garden` and `~/.garden` scripts.

The remaining sections below were written for the Option-A copy-on-write design.
They are retained because the *fork / World / per-world-GC* machinery and the
**hazards list** apply regardless of which option provides isolation — under
Option B the heap becomes immutable so the "copy-on-write slot" mechanics are
unnecessary, but the execution-context fork, side-effect, ID-reuse, symbol-table,
and hot-reload concerns are all still live.

## Recommended architecture

### 1. Make heap slots copy-on-write

In `heap.rs`, change the payload fields to reference-counted:

```rust
struct HeapList   { elements: Rc<Vec<Value>>, alive: bool, gc_mark: bool }
struct HeapMap    { entries:  Rc<IndexMap<String, Value>>, .. }
struct HeapF64Array { data:   Rc<Vec<f64>>, .. }
// strings: Rc<str>; elements struct is already small/Copy-ish
```

- `get_list(id) -> &[Value]`: `&self.lists[i].elements` (unchanged at call sites).
- `get_list_mut(id) -> &mut Vec<Value>`: `Rc::make_mut(&mut self.lists[i].elements)`.
  Every existing mutation site keeps compiling and gets CoW for free.
- Allocation wraps payloads in `Rc::new`.

This step is internal to `heap.rs`; mutation call sites in `eval/exec.rs` and
`builtins/collections.rs` are unchanged.

### 2. Add `Heap::fork()`

```rust
impl Heap {
    /// Cheap copy-on-write clone. Shares all live payloads via Rc; the first
    /// write to any object in either heap copies just that object.
    pub fn fork(&self) -> Heap { /* clone the Vecs of Rc + intern table */ }
}
```

The intern table is cloned too. Note: interning across forks is fine — each
fork interns into its own table; a string allocated post-fork in the child
simply isn't visible to the parent. (Pre-fork shared strings keep their IDs.)

### 3. Bundle execution-local state and fork it together

The heap is not the only shared mutable state. Move the execution-local
registries off `Env` into a struct that forks as a unit (or fork them alongside
the heap). The fork must clone:

- `heap` (CoW, step 1–2)
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

Concretely, introduce something like:

```rust
/// An isolated execution world: one heap + the runtime registries that
/// reference it. Forking a World yields a fully isolated copy.
struct World {
    heap: Heap,
    closures: Vec<RuntimeClosure>,
    overload_sets: Vec<Vec<OverloadEntry>>,
    output: Vec<String>,
    output_buffers: HashMap<SymbolId, Vec<Value>>,
    counters: HashMap<SymbolId, u64>,
    bindings: HashMap<SymbolId, Value>,
}
impl World { fn fork(&self) -> World { .. } }
```

`Env` then holds the shared parts plus one-or-more `World`s and the `Stack`s,
with each stack tagged by which world it runs in.

### 4. New public API

```rust
impl Env {
    /// Fork an existing execution into a new, isolated stack+world.
    /// The new stack shares no mutable heap state with the source.
    pub fn fork_execution(&mut self, src: StackKey) -> Result<StackKey, String>;
}
```

Re-express `run_speculative` in terms of a fork: fork → run the fork → read
results / diff → drop the fork. Dropping a fork reclaims its uniquely-owned
slots immediately (Rc drop); shared slots survive in the parent.

### 5. GC must be per-world

`collect_garbage` currently marks across all stacks and Env roots into the one
heap. With multiple worlds:
- Each `World` GCs its own heap, rooted by the stacks bound to that world plus
  that world's closures/overload_sets/output_buffers/bindings.
- Because payloads are `Rc`, a sweep in one world that drops its last reference
  to a *shared* object must NOT free the slot the parent still references. With
  `Rc`, "free" = drop the `Rc`; the payload's memory is reclaimed only when the
  last world drops it. The free-list/`alive` bookkeeping becomes per-world (each
  world's ID space is independent after fork — but IDs were equal at fork time,
  so a world must only ever sweep its own slot vector). Keep marking logic; just
  scope roots to one world.

## Phased implementation

1. **CoW slots (no behavior change).** Convert heap payloads to `Rc`, switch
   `get_*_mut` to `make_mut`. All existing tests must still pass; this is a
   pure internal refactor. (Measure: confirm no perf regression on the sample
   apps — unique-ownership writes should stay in-place.)
2. **`Heap::fork()` + unit test** that fork → mutate child → assert parent
   object unchanged, and vice-versa.
3. **`World` extraction.** Move execution-local registries off `Env` into
   `World`; thread a `WorldId` (or fold world into the stack). Keep a single
   default world so existing call sites are unaffected.
4. **`World::fork()` + `Env::fork_execution()`.** Fork heap + registries +
   bindings; give the new world fresh output buffers.
5. **Per-world GC.** Scope `collect_garbage` roots to a world's stacks.
6. **Rebuild `run_speculative` on top of `fork_execution`**; keep the old
   signature as a thin wrapper for compatibility.
7. **Host/CLI/WASM surface.** Expose fork + per-fork output and a state/heap diff
   so the SDL and web apps can drive side-by-side runs (out of scope for the core
   but the API in step 4 should anticipate it).
8. **Docs + examples.** Update `docs/dev` and add an example demonstrating a
   forked run that leaves the original untouched.

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
   the fork still resolve in both worlds (they will, since the Vec is cloned).

3. **Heap ID reuse via free-lists.** A freed slot's `u32` is reused. If a stale
   `Value` (e.g. cached in host code, or in dropped-but-not-cleared state)
   outlives a GC, it can alias a *different* object after reuse. This is a
   pre-existing footgun that gets worse with multiple worlds. Consider
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
   world. Simplest: treat the symbol table as shared+append-only and accept that
   a symbol interned in a child is globally visible (symbols are identity, not
   mutable state, so this is usually fine — but document it).

6. **`transfer_state` (hot reload) interaction.** Hot reload clears closures and
   retains state keyed by compile-time `StateKey`. With worlds, decide whether a
   reload forks a fresh world or mutates in place. The state Values carry heap
   IDs, so a reload that swaps the program but keeps the heap must keep the same
   world.

7. **`F64Array` / large buffers.** CoW copies a whole f64 array on first write.
   For big numeric buffers mutated element-by-element in a speculative run, that
   first-write copy can be expensive (though still paid once). Acceptable to
   start; revisit with a chunked/persistent array if profiling shows it matters.

8. **Memory growth from many forks.** Each live fork pins the objects it has
   copied. Long-lived side-by-side sessions that keep forking need a way to drop
   old forks; make fork lifetime explicit and ensure dropping a `World` releases
   its `Rc`s promptly.

## Out of scope / explicit non-goals

- True OS-thread parallelism. `Rc` keeps this single-threaded; if real threads
  are ever needed, switch `Rc`→`Arc` (and the interior types to thread-safe
  ones), which the design allows but doesn't require now.
- Moving to value semantics for collections (Option B). Reconsider only as a
  deliberate language-design decision.

# Proposal: FFI expansion for game-engine embedding (Unreal)

Status: **M1 in progress**. Shipped so far: `Value::Handle(HandleVal)`
(`rust/src/handle.rs`), `Env::register_handle_class` / `make_handle`
(`rust/src/env/mod.rs`), the per-class `call_method` dispatcher (§5),
`PetalCxt::get_handle` (`rust/src/native_fn.rs`), the `is_valid` builtin
(`rust/src/builtins/handle.rs`), handle-receiver method dispatch in the VM
(`rust/src/backend/bytecode/vm/calls.rs`), and equality/hash by identity.
Remaining in M1: `$handle` JSON state encoding and the Headless mock entity
table; M2–M4 not started. Companion to [../ffi.md](../ffi.md), which
documents the existing surface. The motivating scenario: Petal scripting game
logic inside Unreal Engine 5 — retained references to engine objects, hot
reload during play, and a safety story for the fact that engine objects are
mutable while Petal's machinery assumes immutability.

## Grounding: how Unreal wants to be held

Facts that shape the design (consistent across UnLua, Hazelight AngelScript,
Verse, and Blueprint):

- **Raw `UObject*` is never safe to cache.** UE's GC only sees references
  declared via `UPROPERTY` or reported through
  `FGCObject::AddReferencedObjects`; anything else dangles silently. The
  sanctioned weak reference (`TWeakObjectPtr`) stores an **index + serial**
  into the global object array — it detects both collection and slot reuse,
  and also reads invalid for pending-kill actors (destroyed, not yet swept).
- Every mature script integration converges on the same pattern:
  **weak-by-default handles, validity-checked at each dereference, with an
  explicit opt-in pin** for keeping something alive (UnLua's `UnLua.Ref`, a
  strong set reported via `FGCObject`). Verse surfaces staleness as a checked
  runtime failure; Blueprint guards with `IsValid`.
- **Game thread only**: spawning/destroying actors and most world APIs must
  run on the game thread; GC can run between frames. Run the VM on the game
  thread; treat off-thread execution as pure computation.
- Actor names/paths are not identity (PIE prefixes, rename/reuse).
  `FObjectKey` (index+serial) is in-session identity; GUID/soft-path refs are
  the cross-session story.
- Reflection (`UFunction` + `ProcessEvent`, `FProperty`) gives the whole
  Blueprint-visible API surface without hand-written glue, at per-call
  marshaling cost. UnLua's hybrid — reflection by default, hand bindings for
  hot paths — is the proven middle ground.

Petal's existing integer-id pattern is *almost* right (host-side table, ids by
value) but has no staleness detection, no type tag, and no lifetime contract.
The proposal is essentially: promote that pattern into the language with the
safety rails the pattern can't provide on its own.

## 1. `Value::Handle` — a first-class opaque foreign reference

Add one `Copy` value variant:

```rust
Value::Handle(HandleVal)   // { class: HandleClassId (u16), slot: u32, serial: u32 }
```

- **Weak, generation-checked, host-owned.** The handle is an index+serial into
  a host-side table — deliberately the same shape as `FWeakObjectPtr`. The
  runtime never dereferences it; only natives do, through a host-registered
  handle class:

  ```rust
  let actor_class = env.register_handle_class(HandleClass {
      name: "Actor",
      is_valid: fn(&HostTables, slot, serial) -> bool,
      describe:  fn(...) -> String,          // for print/tracing/error messages
  });
  let h = env.make_handle(actor_class, slot, serial);   // host mints handles
  ```

- **Staleness is a checked failure, not UB.** `PetalCxt::get_handle(i, class)`
  validates class and calls `is_valid`; a stale handle makes the native return
  `Err`, which surfaces as an ordinary Petal runtime error with the handle's
  `describe` output — the Verse model. Scripts that expect churn guard with a
  builtin: `if is_valid(h) { ... }`. `Value::Nil` remains "no object".
- **Value semantics for free.** A handle is a `Copy` leaf: storable in lists,
  maps, `state`, and enum payloads; equality/hash by `(class, slot, serial)`
  so handles work as map keys and `state(h)` per-entity keys; no heap
  involvement, so `Heap::fork`, GC, and state snapshots need zero changes.
  Immutability is preserved: *the handle is an immutable name for a mutable
  thing*, like a file descriptor.
- **Serialization**: `get_state_json` emits `{ "$handle": [class_name, slot,
  serial] }`. On `set_state_from_json` the host revalidates; stale entries
  become nil (hook for GUID-based rehydration later, see §8).

Why not keep plain ints? Because the two failure modes ints can't catch —
stale slot reuse and wrong-table confusion — are exactly the crashes that make
script/engine integrations miserable. The serial + class tag close both, and
the cost is one word.

### The Unreal side of the handle

The VM host is a `UGameInstanceSubsystem` implementing `FGCObject`:

- `TArray<FSlot>` where `FSlot { TWeakObjectPtr<UObject> obj, uint32 serial }`;
  a `TMap<FObjectKey, int32>` dedups so one UObject ⇒ one live handle.
  Freeing a slot bumps its serial.
- `is_valid` = serial match **and** `obj.IsValid()` (which already covers GC'd
  and pending-kill).
- **Pinning** (the strong escape hatch): `pin(h)` / `unpin(h)` natives move
  the slot into a strong set reported via `AddReferencedObjects`. Default is
  weak — the script never fights level teardown; pinning is for objects the
  script logically owns (a spawned projectile mid-flight, a dynamically
  created data asset).

## 2. The mutability compromise: reads snapshot, writes are effects

Petal's immutability is enforced by construction (no mutable IR, COW
collections), and three features lean on it: `Heap::fork`/speculative runs,
state snapshots, and re-run-per-frame. Engine objects are irreducibly mutable.
The compromise that keeps the benefits:

**Rule 1 — Handles are immutable, the world is not a value.** No Petal value
ever *contains* engine state. `get_actor_location(h)` returns a fresh
`Vec2`/map snapshot — plain immutable data, safe to store and compare. You
never hold a live view into the engine, so nothing in the heap can be mutated
behind Petal's back.

**Rule 2 — Writes are buffered by default.** World mutations
(`set_actor_location(h, pos)`, `spawn_actor(class, t)`, `destroy(h)`) don't
touch the engine mid-run: they `emit` into a `world_commands` output buffer —
the exact mechanism draw commands already use — and the host applies the
buffer after `run` returns, on the game thread. Consequences:

- A frame is a pure function `(uniforms, state, world-snapshot-reads) →
  (new state, command buffer)`. Speculative runs and heap forks stay sound:
  fork a stack, run a what-if frame, *drop its command buffer* — the world is
  untouched. This is the property that would otherwise die first.
- Reordering/validation happens in one place: the host can reject commands on
  stale handles, batch spawns, or record the buffer for replay/debugging.
- Known cost: read-after-write within one frame doesn't see the write. That's
  the same discipline as `draw_commands` and as retained-mode UI; game logic
  that needs the new value already has it (it computed the value it wrote).

**Rule 3 — An explicit immediate tier, capability-gated.** Some calls can't
buffer (line traces, queries whose results the frame needs, `spawn` when the
script must reference the new actor this frame). These run immediately but
are gated by an Env-level **world access mode** set by the host per run:

| Mode | reads | immediate calls | buffered writes |
|---|---|---|---|
| `Live` (normal tick) | ✓ | ✓ | ✓ (applied) |
| `ReadOnly` (speculative run, what-if frame) | ✓ | reads only | ✓ (dropped) |
| `Sealed` (validation replays, off-thread) | error | error | error |

Mechanically this is one enum on the `ExecutionContext` checked by
`PetalCxt::get_handle` / a `require_immediate()` helper — natives opt into a
tier, the mode enforces it. Speculation safety stops being a convention and
becomes a checked property.

`spawn_actor` in buffered mode still returns a usable handle: the host
pre-allocates the slot when the command is emitted and binds the UObject when
it applies the buffer — the handle is simply not `is_valid` until next frame
(scripts naturally store it in `state` and use it next tick). Decision: this
is acceptable — Petal already has run modes where the script is "running but
not really running" (speculative frames, `ReadOnly`), and a provisionally
invalid handle is the same category of thing.

## 3. Retained state holding engine references

This mostly falls out of §1 + existing machinery:

- `state enemies = []` holding handles survives `reset_stack`, and
  `state(h) hp = 100` gives per-entity script state keyed by the handle
  itself (handles hash by identity — this is `FObjectKey` semantics).
- Stale handles in state are inert until dereferenced; `sweep_untouched_state`
  already reclaims per-entity state once the script stops iterating a dead
  entity. Optionally add `env.sweep_stale_handles(stack)` for hosts that want
  eager cleanup after level transitions.
- Hot reload: `transfer_state` preserves handles for free — unlike closures,
  a handle references nothing in the old program, so it needs no clearing.

## 4. Native registration upgrades

Two current restrictions bite a reflection-driven binding layer:

1. `NativeFn` is a bare `fn` pointer — a generated binding can't close over
   its `UFunction*`/marshaling descriptor.
2. Natives must all be registered before `load_program`.

Rather than lifting both (registering natives post-load disturbs the
root-frame register scheme), the binding surface rides **method-call syntax
on handles** — see §5, which exists already at the syntax/IR level. Each
`HandleClass` carries a dispatcher; `h.set_actor_location(p)` resolves
through it, and the Unreal host implements one dispatcher per class over UE
reflection (`FindFunction` + `ProcessEvent` param marshaling). No generated
wrapper module is required for the core surface; a small hand-written
`unreal` prelude module (the `ui`-prelude trick) can still supply free-
function conveniences and multi-step helpers. Hot paths later get
hand-written natives (the UnLua hybrid). Independently worth doing: change
`NativeFnTable` entries to `Box<dyn Fn(&mut PetalCxt) -> NativeResult +
Send>` — a contained change (the table is the only consumer) that removes
the biggest papercut for *all* embedders.

Engine→script events (overlaps, hits, input actions) do **not** become
callbacks (natives can't call closures, and we shouldn't change that): the
host queues events and binds them as a per-frame list uniform, exactly like
`petal_ui::InputState` edges. Scripts pattern-match the event list each tick.

## 5. Method-call syntax on handles — build it early

`h.set_location(p)` needs **no parser or IR work**: `obj.method(args)`
already parses, compiles to `TermOp::MethodCall` / `Inst::MethodCall`, and the
bytecode VM dispatches it (`rust/src/backend/bytecode/vm/calls.rs`
`do_method_call`):

1. a callable field on a record receiver, else
2. **UFCS fallback** — look the method name up in the native table and call
   it with the receiver prepended.

So `h.set_location(p)` would work *today* against any registered native named
`set_location` — but through one flat global namespace, where `Actor` and
`Texture` handles would collide and stale-handle checks live in every native.
The right increment is a third resolution step:

3. if the receiver is `Value::Handle`, dispatch through the handle class:

```rust
pub struct HandleClass {
    name: &'static str,
    is_valid: ...,
    describe: ...,
    call_method: Box<dyn Fn(&mut PetalCxt, method: SymbolId) -> NativeResult>,
}
```

One boxed dispatcher **per class** (not per method) keeps `NativeFn`'s
plain-fn-pointer scheme untouched, sidesteps the register-before-load
restriction entirely (the class's method set can grow at any time — exactly
what reflection rebuilds after Live Coding need), gives per-class namespaces,
and centralizes the stale-handle check and world-access-mode gate at the one
dispatch point. Unknown methods reuse the existing "No method 'x' on type y"
error with the class's method list as the hint.

This goes in **M1, with the handle itself**, not later: it is small (the
machinery exists; the increment is one match arm in the VM plus the vtable
field), and deciding it late would mean building a generated-wrapper prelude
module in M3 and then discarding it — the dispatcher *is* the binding layer.

## 6. Hot reload in-engine

Petal's story already fits UE's Live Coding culture:

- Script edit → `compile_program_at` + `transfer_state` on the next tick.
  State (including handles, per-entity slots) survives by name-hash key;
  closures recapture. petal-sdl's watcher generalizes directly; in-editor, an
  `FAssetRegistry`/directory-watcher triggers the same path.
- C++ Live Coding / engine hot reload invalidates `UFunction*` caches, not
  handles: the host rebuilds its reflection method tables; serial-checked
  handles remain valid. Handle *class* registrations are name-keyed, so they
  survive too.
- PIE session boundaries: end-PIE tears down the world; every handle goes
  stale (correctly). Hosts that want state to survive across PIE restarts use
  `get_state_json` + GUID rehydration (§8).

## 7. Threading & frame contract

- VM runs on the game thread, in tick. `run_bounded` already provides the
  budget mechanism if a script overruns; `Sealed` mode makes any accidental
  off-thread run fail loudly instead of racing the GC.
- The per-tick contract is petal-sdl's, verbatim: bind uniforms (dt, input,
  event list) → `reset_stack` → `run` → drain `world_commands` (+ optional
  debug-draw buffer feeding `DrawDebugHelpers`). The petal-ui `Headless`
  harness pattern gives Unreal-free unit tests for game logic: a mock handle
  table + recorded command buffers.
- petal-sdl's JSON protocol (pause/step/state/screenshot) ports to an editor
  Slate panel or remote-control endpoint for live inspection of script state.

## 8. Deferred (explicitly out of v1)

- **Cross-session identity** — GUID/soft-object-path rehydration of handles in
  saved state. The `$handle` JSON encoding leaves room for it.
- **Property access via FProperty reflection** (`h.health` as field access on
  a handle) — same dispatcher shape as method calls; add when needed.
- **Natives calling closures** — keep inverting callbacks into data.
- **Off-game-thread pure execution** — `Sealed` mode is the prerequisite;
  scheduling is future work.

## Phasing

1. **M1 — handles, engine-agnostic.** ✅ `Value::Handle`, ✅ `register_handle_class`
   (with per-class `call_method` dispatcher — §5), ✅ `make_handle`, ✅ `is_valid`
   builtin, ✅ `PetalCxt::get_handle`, ✅ handle-class method dispatch in the
   VM, ⬜ JSON encoding, ✅ equality/state-key support. ⬜ Prototype in
   petal-sdl or the `Headless` harness with a mock entity table (e.g. handles
   to host-side "sprites") — proves the design with no Unreal dependency and
   gives petal-sdl retained resources (textures, sounds) as a side benefit.
2. **M2 — effect discipline.** `world_commands` buffer conventions, world
   access mode on `ExecutionContext`, mode enforcement in `get_handle` /
   immediate-tier natives, speculative-run integration (`ReadOnly` + dropped
   buffers).
3. **M3 — the Unreal host.** `UGameInstanceSubsystem` + `FGCObject` handle
   table, pin/unpin, reflection-backed `call_method` dispatchers per class,
   an optional hand-written `unreal` prelude for conveniences, event-list
   uniforms, tick integration, watcher-based hot reload.
4. **M4 — tooling.** Editor panel over the JSON protocol; closure-capable
   `NativeFnTable` (`Box<dyn Fn>`); perf pass (hand natives for hot paths,
   read-snapshot caching if profiling demands it).

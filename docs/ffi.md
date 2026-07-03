# Petal FFI and embedding

How a host application talks to the Petal runtime today. This documents the
*existing* surface; ideas for expanding it (opaque foreign handles, mutable
host objects) live in [dev/unreal-ffi-proposal.md](./dev/unreal-ffi-proposal.md).

The design is Lua-inspired: native functions are registered by name against a
stack-style calling convention (`rust/src/native_fn.rs` opens with "Native
function FFI — Lua-inspired plugin system"). Everything crosses the boundary
**by value**; there is no userdata / opaque-pointer value type. Host resources
are referenced from scripts as plain integers, with the host keeping an
id→resource table on its side.

## The embedding lifecycle

Every embedder (petal-sdl, the wasm runtimes, the headless test harness)
follows the same shape:

```rust
let mut env = Env::new();
env.register_native("spawn_particle", native_spawn_particle); // host functions
env.register_module("ui", include_str!("ui.ptl"));            // Petal-source prelude
env.set_implicit_imports(&["ui"]);
let pid = env.load_program_at(&source, &path)?;               // compile (walks imports)
let stack = env.create_stack(pid)?;

loop {                                                        // per frame / tick
    env.set_binding(dt_sym, Value::Float(dt));                // host → script uniforms
    env.reset_counter(canvas_id_sym);
    env.reset_stack(stack);
    env.run(stack)?;                                          // re-run the whole program
    let cmds = env.take_output_buffer(draw_sym);              // script → host commands
}
```

Key entry points, all on `Env` (`rust/src/env/mod.rs`):

| Concern | API |
|---|---|
| Native functions | `register_native(name, func) -> NativeFnId` |
| Modules / prelude | `register_module`, `add_module_path`, `set_implicit_imports` |
| Programs | `load_program`, `load_program_at`, `compile_program_at`, `load_program_ir` |
| Execution | `create_stack`, `run`, `run_bounded`, `reset_stack`, `call_function` |
| Host→script data | `intern_symbol`, `set_binding`, `clear_binding` |
| Script→host data | `take_output_buffer`, `output_buffer`, `take_output` (print lines) |
| Id allocation | `reset_counter`, `next_counter` |
| State tooling | `get_state_json`, `set_state_from_json`, `snapshot_state`, `diff_state` |
| Speculation | `fork_execution`, `run_speculative`, `drop_fork` |
| Hot reload | `module_manifest`, `transfer_state` |

`run_bounded` returns `RunOutcome::Done | Yielded` so a 60fps host can slice
long computations cooperatively. `call_function(stack, "name", args)` invokes a
top-level Petal function by (possibly module-qualified) name after at least one
`run` — the host-side call-in direction.

## Native functions

```rust
pub type NativeFn = fn(&mut PetalCxt) -> NativeResult;   // rust/src/native_fn.rs
pub type NativeResult = Result<u32, String>;             // Ok(count of pushed results)
```

A native is a **plain non-capturing `fn` pointer** — no closures, no per-fn
context. `Env::register_native` appends it to the `NativeFnTable` (the id is
the table index) and **must be called before `load_program`**: at load time
every native is materialized as a `Value::NativeFunction(id)` in the root
frame's registers, index == id, so scripts resolve natives through ordinary
scope lookup (and can shadow them).

`PetalCxt` is the per-call context handle. Argument readers are 1-indexed like
Lua (`get_int(1)`, `get_string(2)`, `get_value`, `get_symbol`, …); results are
pushed (`push_int`, `push_value`, `push_nil`, …). It also exposes the three
host channels (below), `heap`/`heap_mut`, and an `in_place` flag (see
Immutability). Both backends dispatch natives identically — graph engine in
`rust/src/backend/graph/call.rs`, bytecode VM in
`rust/src/backend/bytecode/vm.rs` — so a native never knows which backend
called it.

**Method-call syntax reaches natives.** `obj.method(args)` compiles to a
`MethodCall` op, and both backends resolve it the same way: a callable field
on a record receiver first; then, on a handle receiver, the handle class's own
`call_method` dispatcher — which wins over any same-named native and rejects
stale handles before dispatch; otherwise **UFCS fallback** — the method name is
looked up in the native table and called with the receiver prepended
(`exec_method_call` in `rust/src/backend/graph/call.rs`, `do_method_call` in
the bytecode VM). So registering `set_location` makes both
`set_location(obj, p)` and `obj.set_location(p)` work. The namespace is flat —
one native table for all receiver types.

The compiled-in builtins (`rust/src/builtins/`) are registered through the same
table by `register_builtins`. Registration **order is load-bearing** (phantom
term indices in the IR are assigned in registration order — append, never
reorder). `map`/`filter`/`reduce`/`forEach` register placeholder fns but are
dispatched specially by the evaluators because they call back into closures;
ordinary natives **cannot invoke Petal closures**.

## Values and the heap

`Value` (`rust/src/value.rs`) is a `Copy` enum; anything bigger than a machine
word lives in the `Heap` behind a typed u32 id:

```
Nil, Bool, Int(i64), Float(f64), Vec2(f64, f64), Dual,
String(StringId), List(ListId), F64Array(F64ArrayId), Map(MapId),
Closure(ClosureId), OverloadSet(..), NativeFunction(NativeFnId),
EnumVariant { tag: StringId, data: ListId }, Element(ElementId),
Symbol(SymbolId)
```

Notes for embedders:

- **There is no foreign-handle / userdata variant.** You cannot stash a host
  pointer behind a `Value`. The closest things are `Value::Symbol` (an interned
  key shared with the host) and plain `Int` ids.
- The heap (`rust/src/heap.rs`) is mark-and-sweep (allocation-count triggered,
  no refcounting). GC roots include stack registers, persistent state, closure
  captures, **bindings, and output buffers** — so values parked in the host
  channels stay alive.
- Heap collection ops are **copy-on-write**: `list_append`, `map_set`, etc.
  clone the backing store and return a new id. `Heap::fork` deep-clones the
  whole heap for speculative execution — sound precisely because objects are
  immutable-by-construction.

## The three host channels

All host↔script data flows through symbols (`env.intern_symbol(name)` — the
`SymbolTable` is explicitly "shared with the embedding host"):

1. **Bindings** — GLSL-uniform-style host→script values. Host calls
   `env.set_binding(sym, value)` before a run; scripts (or natives) read them
   via the `binding` builtin / `PetalCxt::binding_named`. Used for input
   snapshots, `dt`, `frame_count`, screen dimensions.
2. **Output buffers** — script→host command streams. A native calls
   `cxt.emit(sym, tag, data)`, which appends an `EnumVariant { tag, data }` to
   the buffer for `sym`; the host drains it after the run with
   `env.take_output_buffer(sym)` and decodes tags into typed commands. This is
   how all rendering works: draw natives don't draw, they emit.
3. **Counters** — per-run monotonic id allocators (`reset_counter` /
   `next_counter`), used to hand scripts fresh integer ids (offscreen canvas
   ids, element ids).

## Referencing host resources today

The established pattern: **allocate an integer id, pass it as `Value::Int`,
keep the id→resource table host-side.**

- Offscreen canvases (petal-ui): `create_canvas()` returns an int from a
  per-frame counter; `draw_to(id)` / `draw_canvas(id, …)` reference it. The
  host materializes real render targets from the command stream — the id is an
  index into command order, never a live host pointer, and it resets every
  frame.
- DOM elements (petal-web): `next_id()` ids round-trip through `data-eid`
  attributes and come back via a `clicked_id` binding.
- petal-sdl's example browser and file I/O pass strings through bindings and
  output buffers.

This works because these resources are frame-scoped or looked up by the host
on demand. Nothing today holds a *retained* cross-frame reference to a host
object — that is the main gap for a game-engine embedding.

## Retained state

Scripts keep state across runs with the `state` keyword:

```petal
state score = 0            // initialized on first run only
state(item.id) hp = 100    // explicit key: per-entity state inside a loop
```

- Storage is `Stack::state: HashMap<RuntimeStateKey, Value>` — on the stack,
  surviving `reset_stack` + `run`, one map per stack.
- `StateKey` is a **hash of the variable name** (module-qualified for module
  state, e.g. `"ui::scroll"`); declaration order doesn't matter. A `state`
  inside a loop is additionally keyed by loop index or an explicit
  `state(key)` expression, giving per-iteration / per-entity slots.
- After each run, `sweep_untouched_state` drops keys the run didn't touch — so
  state for deleted code or removed list items doesn't leak.
- `get_state_json` / `set_state_from_json` serialize it for tooling, and
  `fork_execution` + `run_speculative` run what-if frames against a forked
  copy.

## Hot reload

`Env::module_manifest(pid)` lists every source file a program was compiled
from (name, filesystem origin, content hash); petal-sdl's file watcher watches
those directories, so editing an imported module reloads its importer. On
change:

```rust
let new_program = env.compile_program_at(pid, &source, &path)?;
let result = env.transfer_state(stack, new_program)?;  // { state_preserved, state_dropped }
```

`transfer_state` keeps every state value whose name-hash key still exists in
the new program, drops the rest, clears all closures and the cached function
table (they reference old code; recaptured on next run), and invalidates
cached bytecode. Because `StateKey` is a name hash, reordering declarations
preserves state; renaming a variable or moving it between modules drops it
(`diff_state` exists for hosts that want a migration affordance).

## Immutability and the in-place gate

Petal values are immutable **by construction**, not by runtime check:

- The IR has no mutable variables — reassignment emits a new term and rebinds
  the name (SSA/phi). There is no "set" op to police.
- Collections are value types; `append`/`set`/`remove` return new collections
  (the `@` rebind operator is sugar: `append(@nums, 4)`).
- The one exception is an optimization: when the bytecode backend's escape
  analysis proves a container uniquely owned and non-escaping, it sets
  `PetalCxt::in_place` and builtins mutate the backing store directly
  (`list_append_in_place` etc.). The graph engine never sets it.

Several load-bearing features assume this: `Heap::fork` / speculative runs,
cheap state snapshots, and the general "re-run the whole program every frame"
model. Any future mutable foreign objects must not silently break these — see
the proposal doc.

## Existing embedders (worked examples)

- **petal-sdl** (`apps/petal-sdl/`) — the reference native embedder. Per frame:
  translate SDL events into `petal_ui::InputState`, bind the input snapshot +
  frame info as uniforms, `reset_stack` + `run`, drain `draw_commands`, and
  rasterize. File-watcher hot reload via `module_manifest` + `transfer_state`.
  Its `protocol.rs` JSON protocol (pause/step/state/screenshot over
  stdin/stdout) drives the same contract headlessly for agents and tests.
- **petal-ui** (`petal-ui/`) — the reusable layer: the input vocabulary
  (`InputEvent`, `InputState` with level/edge semantics), the `DrawCommand`
  enum (with a `Host { tag, data }` pass-through so embedder-specific natives
  keep their place in the command stream), the Petal-source `ui` prelude
  registered as an implicit import, and a `Headless` harness that mirrors the
  frame contract exactly for tests.
- **petal-web / petal-diagram-canvas** (`apps/`) — wasm-bindgen `PetalRuntime`
  structs owning an `Env`; the same channels, marshalled as JSON strings
  across the wasm boundary. petal-web returns a retained element tree instead
  of draw commands; diagram-canvas reimplements the draw-command loop and
  exposes `run_speculative` for isolated what-if frames.

## Current limitations (the gaps an expanded FFI would fill)

1. **No opaque foreign handle.** Host objects can only be referenced as
   integers with all safety left to the host's table discipline; nothing
   detects a stale id.
2. **Natives are bare `fn` pointers** — no captured per-function context, so a
   binding generator can't close over e.g. a reflection method descriptor;
   each native must rendezvous with host state through the channels.
3. **Natives must be registered before `load_program`** (ids become root-frame
   register indices at load time), so the native set can't grow dynamically.
4. **Natives can't call Petal closures** (only the blessed intrinsics can), so
   host-driven callbacks must be inverted into data (command buffers).
5. **Everything is by value** — fine for commands and snapshots, unbuilt for
   large or intrinsically mutable host objects.

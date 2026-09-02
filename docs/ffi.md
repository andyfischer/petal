# Petal FFI and embedding

How a host application talks to the Petal runtime: the API on `Env`, native
functions, the value model, the host channels, and retained state. For
task-oriented patterns built on these primitives, see
[embedding-guide.md](embedding-guide.md).

The design is Lua-inspired. Native functions are registered by name against a
stack-style calling convention (`rust/src/native_fn.rs`). Everything crosses
the boundary **by value**, except for [handles](#handles), which are opaque
references to host-owned objects.

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
    env.reset_counter(canvas_id_sym, 0);
    env.reset_stack(stack);
    env.run(stack)?;                                          // re-run the whole program
    let cmds = env.take_output_buffer(draw_sym);              // script → host commands
}
```

Entry points, all on `Env` (`rust/src/env/`):

| Concern | API |
|---|---|
| Native functions | `register_native(name, func) -> NativeFnId`, `set_native_class` |
| Handles | `register_handle_class`, `make_handle` |
| Modules / prelude | `register_module`, `add_module_path`, `set_implicit_imports` |
| Programs | `load_program`, `load_program_at`, `compile_program_at`, `load_program_ir` |
| Execution | `create_stack`, `run`, `run_bounded`, `reset_stack`, `call_function` |
| Host→script data | `intern_symbol`, `set_binding`, `clear_binding` |
| Script→host data | `take_output_buffer`, `output_buffer`, `take_output` (print lines) |
| Id allocation | `reset_counter`, `next_counter` |
| State tooling | `get_state_json`, `set_state_from_json`, `snapshot_state`, `restore_state`, `diff_state` |
| Observation | `observations_mut().enable()`, `get_observations_json` — the last value bound to every named term (see [embedding-guide.md](embedding-guide.md#reading-arbitrary-named-values-observation)) |
| Emit tracing | `enable_emit_trace`, `take_output_origins` (see [direct-manipulation.md](direct-manipulation.md)) |
| Speculation | `fork_execution`, `run_speculative`, `drop_fork` |
| Hot reload | `module_manifest`, `transfer_state` |

`run_bounded` returns `RunOutcome::Done | Yielded`, so a 60fps host can slice a
long computation across frames. `call_function(stack, "name", args)` calls a
top-level Petal function by (possibly module-qualified) name after at least one
`run`; this is the host-to-script call direction.

## Native functions

```rust
pub type NativeFn = fn(&mut PetalCxt) -> NativeResult;   // rust/src/native_fn.rs
pub type NativeResult = Result<u32, String>;             // Ok(count of pushed results)
```

A native is a plain, non-capturing `fn` pointer. `Env::register_native`
appends it to the native table (the id is the table index) and **must be
called before `load_program`**: at load time every native becomes a
`Value::NativeFunction(id)` in the root frame, so scripts resolve natives
through ordinary scope lookup and can shadow them.

`PetalCxt` is the per-call context. Argument readers are 1-indexed like Lua
(`get_int(1)`, `get_string(2)`, `get_value`, `get_symbol`, `get_handle`, …);
results are pushed (`push_int`, `push_value`, `push_nil`, …). It also exposes
the host channels (`binding_named`, `push_output`, `emit`, `next_counter`),
`heap`/`heap_mut`, the call's origin term (`origin()`, used by emit tracing),
and the `in_place` flag (see [Immutability](#immutability-and-the-in-place-gate)).

**Method-call syntax reaches natives.** `obj.method(args)` resolves, in order:
a callable field on a record receiver; a handle class's own `call_method`
dispatcher for a handle receiver (which also rejects stale handles); otherwise
**UFCS fallback**, where the method name is looked up in the native table and
called with the receiver prepended. So registering `set_location` makes both
`set_location(obj, p)` and `obj.set_location(p)` work. The namespace is flat:
one native table for all receiver types.

**Native classes.** After registration, mark a native with
`env.set_native_class(id, NativeClass::…)` to say how it treats a
`Value::Pending` argument: `Strict` (default) absorbs and returns the pending
value, `Effectful` makes the call a no-op that emits nothing, and
`AllowPending` runs normally. Emitters should be `Effectful`.

The compiled-in builtins (`rust/src/builtins/`) go through the same table.
`map`/`filter`/`reduce`/`forEach` register placeholders and are dispatched
specially by the VM because they call back into closures; ordinary natives
**cannot call Petal closures**.

## Values and the heap

`Value` (`rust/src/value.rs`) is a `Copy` enum; anything bigger than a machine
word lives in the `Heap` behind a typed u32 id:

```
Nil, Bool, Int(i64), Float(f64), Vec2(f64, f64), Dual { value, derivative },
String(StringId), List(ListId), F64Array(F64ArrayId), Map(MapId),
Closure(ClosureId), OverloadSet(..), NativeFunction(NativeFnId),
EnumVariant { tag: StringId, data: ListId }, Element(ElementId),
Symbol(SymbolId), Cell(CellId), Handle(HandleVal), Pending(PendingId)
```

Notes for embedders:

- `Cell` is the box behind a `var`; it never reaches host code, because every
  read dereferences it (see [var.md](var.md#containment)).
- `Pending` is an unresolved resource (loading or errored). Ordinary operations
  absorb it and return it.
- The heap (`rust/src/heap.rs`) is mark-and-sweep, triggered by allocation
  count. GC roots include stack registers, persistent state, closure captures,
  **bindings and output buffers**, so values parked in the host channels stay
  alive.
- Heap collection ops are copy-on-write: `list_append`, `map_set`, etc. return
  a new id. `Heap::fork` deep-clones the whole heap for speculative execution,
  which is sound because objects are immutable by construction.

## The three host channels

All host↔script data flows through symbols. `env.intern_symbol(name)` returns
a `SymbolId`; the host and the script share an id by interning the same name.

1. **Bindings** — host→script values, like GLSL uniforms. The host calls
   `env.set_binding(sym, value)` before a run; scripts read them with the
   `binding` builtin, natives with `PetalCxt::binding_named`. Used for input
   snapshots, `dt`, `frame_count`, screen dimensions.
2. **Output buffers** — script→host command streams. A native calls
   `cxt.emit(sym, tag, data)`, which appends `EnumVariant { tag, data }` to the
   buffer for `sym` (or `cxt.push_output(sym, value)` for an untagged value).
   The host drains it after the run with `env.take_output_buffer(sym)`. This is
   how all rendering works: draw natives do not draw, they emit.
3. **Counters** — per-run monotonic id allocators (`reset_counter(sym, start)`
   / `next_counter(sym)`), used to hand scripts fresh integer ids (offscreen
   canvas ids, element ids).

## Referencing host resources

There are two ways for a script to refer to something the host owns.

### Integer ids

Allocate an id from a counter, pass it as `Value::Int`, and keep the
id→resource table host-side. This suits frame-scoped resources:

- Offscreen canvases (petal-ui): `create_canvas()` returns an int from a
  per-frame counter; `draw_to(id)`, `draw_canvas(id, …)`, `snapshot_to(id, …)`
  reference it. The host materializes render targets from the command stream;
  the id is an index into command order and resets every frame.
- DOM elements (petal-web-html): `next_id()` ids round-trip through `data-eid`
  attributes and come back via a `clicked_id` binding.

Nothing detects a stale integer id; safety is the host table's discipline.

### Handles

For retained, host-owned objects (a game entity, an actor, a file), use a
**handle**: `Value::Handle(HandleVal { class, slot, serial })`. The host owns
the object; the handle is an opaque address into the host's own storage,
typically a slot map with generation counters.

```rust
use petal::{HandleClass, HandleClassId, HandleVal};

let actor_class: HandleClassId = env.register_handle_class(HandleClass {
    name: "Actor".into(),
    is_valid: Box::new(|slot, serial| world.is_live(slot, serial)),
    describe: Box::new(|slot, serial| format!("Actor {slot}#{serial}")),
    call_method: Box::new(|cxt, method| { /* dispatch on `method`, receiver is arg 1 */ }),
});

// Hand one to a script (e.g. push it from a native, or set it as a binding):
let h: Value = env.make_handle(actor_class, slot, serial);

// Read one back inside a native, checked for class and liveness:
let h: HandleVal = cxt.get_handle(1, actor_class)?;
```

- `register_handle_class` may be called at any time; unlike natives, handle
  classes are not referenced by compiled programs.
- Handles compare and hash by identity and are GC leaf values.
- `obj.method(args)` on a handle receiver goes to the class's `call_method`,
  which wins over any same-named native; stale handles are rejected before
  dispatch.
- The `is_valid(v)` builtin lets scripts guard against host-side object churn.
  It returns false for nil, non-handles and stale handles, never an error.

## Retained state

Scripts keep state across runs with the `state` keyword:

```petal
state score = 0            // top level: one slot, initialized on first run only
state(item.id) hp = 100    // explicit key: one slot per entity, wherever it is reached from
```

- Storage is `Stack::state`, a map on the stack that survives `reset_stack` +
  `run`. One map per stack.
- A runtime key is `RuntimeStateKey { base, path }`. `base` is the
  **declaration id**: a hash of the declaration's full name path (module
  qualifier, enclosing function chain, variable name: `"score"`,
  `"ui::scroll"`, `"ui::draw/row"`). Declaration order does not matter.
- `path` is the chain of callsites and loop iterations that reached the
  declaration, so each callsite, recursion depth and caller iteration gets its
  own slot ([one slot per call path](language-guide.md#one-slot-per-call-path)).
  A top-level declaration runs on the empty path: one declaration, one slot.
  `state(key)` keys by the value and ignores the path.
- After each run, `sweep_untouched_state` drops keys the run did not touch, so
  state for deleted code, removed list items, or paths an edit no longer
  produces does not leak.
- `get_state_json` / `set_state_from_json` serialize state for tooling.
  `Env::get_state`/`set_state` and the JSON setters address **top-level slots
  only**; a pathed slot has no name to address it by and renders in
  `get_state_json` under its path (`[3]/row/hovered`).

**Host-invoked functions get a root path of their own.** `Env::call_function`
runs with no caller frame, so it starts a path derived from the function's
qualified name. Repeated host calls of the same function share slots with each
other, but not with in-program calls of that function. To share a value across
both, put it in a top-level `state var` and read it with `get`.

## Hot reload

`Env::module_manifest(pid)` lists every source file a program was compiled
from (name, filesystem origin, content hash). petal-sdl's file watcher watches
those directories, so editing an imported module reloads its importer. On
change:

```rust
let new_program = env.compile_program_at(pid, &source, &path)?;
let result = env.transfer_state(stack, new_program)?;  // { state_preserved, state_dropped }
```

`transfer_state` keeps every state value whose declaration id still exists in
the new program and drops the rest. It clears closures and the cached function
table (they reference old code and are recaptured on the next run) and
invalidates cached bytecode. Because the id is a name hash, reordering
declarations preserves state; renaming a variable, or moving it between
functions or modules, drops it. See
[program-modification.md](program-modification.md#state-preserving-hot-reload-transfer_state)
for the call-structure caveats.

## Immutability and the in-place gate

Petal values are immutable **by construction**, not by runtime check:

- Ordinary bindings are not mutable variables. Reassignment emits a new term
  and rebinds the name.
- Collections are value types; `append`/`set_at`/`remove` return new
  collections (the `@` rebind operator is sugar: `append(@nums, 4)`).
- The one mutable slot is a `var`, and it is contained: no expression ever
  evaluates to a cell, and the cell slab lives in the heap, so `Heap::fork`
  isolates it like any other object. A `set` replaces the cell's contents; it
  does not edit a value in place.
- The one exception is an optimization. When the VM's escape analysis proves a
  container uniquely owned and non-escaping, it sets `PetalCxt::in_place` and
  builtins mutate the backing store directly. With `--no-opt` the flag is
  never set.

`Heap::fork`, speculative runs, cheap state snapshots, and the
"re-run the whole program every frame" model all depend on this.

## Existing embedders

- **petal-sdl** (`integrations/petal-desktop-sdl/`) — the reference native
  embedder. Per frame: translate SDL events into `petal_ui::InputState`, bind
  the input snapshot and frame info, `reset_stack` + `run`, drain
  `draw_commands`, rasterize. Hot reload via `module_manifest` +
  `transfer_state`. Its JSON protocol (pause/step/state/screenshot over
  stdin/stdout) drives the same contract headlessly for agents and tests.
- **petal-ui** (`petal-ui/`) — the reusable layer: the input vocabulary
  (`InputEvent`, `InputState`), the `DrawCommand` enum (with a
  `Host { tag, data }` pass-through so embedder-specific natives keep their
  place in the command stream), the Petal-source `ui` prelude registered as an
  implicit import, and a `Headless` harness that mirrors the frame contract
  for tests.
- **petal-web-html** (`integrations/petal-web-html/`) and **diagram-canvas**
  (`examples/custom-integrations/diagram-canvas/`) — wasm-bindgen
  `PetalRuntime` structs owning an `Env`, with the same channels marshalled as
  JSON strings across the wasm boundary. petal-web-html returns a retained
  element tree instead of draw commands; diagram-canvas exposes
  `run_speculative` for isolated what-if frames.

## Current limitations

1. **Natives are bare `fn` pointers.** No captured per-function context, so a
   binding generator cannot close over a descriptor; each native reaches host
   state through the channels or a handle class's boxed callbacks.
2. **Natives must be registered before `load_program`**, so the native set
   cannot grow while a program is loaded. Handle classes can.
3. **Natives cannot call Petal closures.** Host-driven callbacks must be
   inverted into data (command buffers) or go through `Env::call_function`
   between runs.
4. **Everything but handles is by value.** Fine for commands and snapshots;
   large or intrinsically mutable host objects belong behind a handle.

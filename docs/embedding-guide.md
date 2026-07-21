# Embedding Guide

Practical patterns for embedding Petal in a Rust host — owning an [`Env`], running
scripts, and getting data across the host↔script boundary **without host-side
globals**.

- **Primitives reference:** [ffi.md](ffi.md) — the native-fn API, `Value`, and the
  three symbol-keyed channels (bindings, output buffers, counters).
- **Where embedding fits among the app-building paths:**
  [building-on-integrations.md](building-on-integrations.md).
- **Persisting settings back to a script file:**
  [goal-based-editing.md](goal-based-editing.md).

This guide is task-oriented: it shows the *patterns* you compose from those
primitives. The running example is Garden (`~/garden/garden-script`), a text
editor whose whole layout, theme, and color scheme are declared by a user's
`init.ptl`.

---

## The golden rule: no globals

An `Env` owns everything a run touches — heap, symbols, bindings, output
buffers, counters. Anything a native fn needs to read or write should come from
its [`PetalCxt`], and anything the host needs to hand off should live on the
`Env`. Do **not** stash run state in a `thread_local!` or `static`.

Why it matters:

- **Forks.** `env.fork_stack` gives a script its own [`ExecutionContext`] (heap +
  registries). A global would be shared across forks and corrupt them; the
  per-`Env` channels are addressed by stack (`take_output_buffer_for`,
  `set_binding_for`), so each fork stays isolated.
- **Multiple hosts / threads.** Two `ScriptHost`s on two threads each own an
  `Env`. A thread-local capture slot silently couples them; owned state does not.
- **Testability.** Owned state means a test constructs an `Env`, runs, and reads
  back — no global reset between cases.

The three channels, all keyed by an interned **symbol** (`env.intern_symbol(name)`
— the host and script share an id by interning the same name):

| Direction | Channel | Host API | Script/native API |
|-----------|---------|----------|-------------------|
| host → script | **binding** (a uniform) | `set_binding` / `binding` / `clear_binding` | `binding(sym)` builtin, `cxt.binding_named` |
| script → host | **output buffer** (a stream) | `take_output_buffer` / `output_buffer` / `clear_output_buffer` | `push_output(sym, v)` builtin, `cxt.push_output` / `cxt.emit` |
| per-run ids | **counter** | `reset_counter` / `next_counter` | `cxt.next_counter` |

---

# Observing Function Calls

**Goal:** let a script *declare* something by calling a function — `layout(...)`,
`color_theme(...)`, a `spawn(...)` in a game — and have the host read back what
was declared after the run, without a global.

The mechanism is the **output buffer**. A native fn that you want to observe does
not compute a result or mutate host state; it just pushes its argument into a
symbol-keyed buffer. After the run the host drains that buffer and interprets the
values it finds.

This replaces the older "capture slot" anti-pattern (a `thread_local!
Option<T>` the native writes and the host takes). The buffer version is a drop-in
improvement: it survives forks, needs no reset dance beyond a clear, and keeps
all state on the `Env`.

### 1. The native fn just emits

Push the raw argument value into a buffer named for the call. Validate nothing
here — recording the call is the native's only job; interpretation happens
host-side where errors are easier to surface.

```rust
// Shared name so host and native intern the same SymbolId.
const LAYOUT_SYM: &str = "app.layout";

fn native_layout(cxt: &mut PetalCxt) -> NativeResult {
    let value = cxt.get_value(1)?;              // the record the script passed
    let sym = cxt.intern_symbol(LAYOUT_SYM);
    cxt.push_output(sym, value);               // record the call
    cxt.push_nil();                            // layout(...) returns nil
    Ok(1)
}
```

If the call carries a *tagged* payload (many command kinds through one buffer,
e.g. draw commands), use `cxt.emit(sym, tag, data)` instead — it builds a
`Value::EnumVariant { tag, data }` for you. For a single observed call like
`layout`, a plain `push_output` of the argument is enough.

### 2. Classify emitters as `Effectful`

A native that only emits should be a no-op when handed a `Value::Pending`
(loading/errored) argument — it should emit *nothing* rather than have the
pending value absorbed as its result. Mark it after registration:

```rust
let id = env.register_native("layout", native_layout);
env.set_native_class(id, NativeClass::Effectful);
```

`NativeClass::Strict` (the default) is for pure natives (`sqrt(pending)` →
`pending`). `Effectful` is for emitters (`print`, `push_output`, and your
observed calls). `AllowPending` is for natives that inspect pendings themselves.
See [`NativeClass`] in [ffi.md](ffi.md).

### 3. The host drains and interprets after the run

Intern the same names (ids are idempotent), clear stale values, run, then read
the buffers. **Last write wins** falls out naturally: take `.last()` of the
drained `Vec`.

```rust
fn run_and_extract(&mut self) -> Result<Layout, String> {
    let layout_sym = self.env.intern_symbol(LAYOUT_SYM);

    // A run that errored after emitting could leave stale values; clear first so
    // "the script never called layout()" reads as an empty buffer.
    self.env.clear_output_buffer(layout_sym);

    self.env.run(self.stack_id)?;

    // The drained Values reference the Env heap — decode against `env.heap()`
    // BEFORE the next run mutates it.
    match self.env.take_output_buffer(layout_sym).last().copied() {
        Some(value) => convert_layout(value, self.env.heap()),   // your interpreter
        None => Ok(Layout::default()),                           // not called → default
    }
}
```

### Things to get right

- **Decode before the next run.** Buffer `Value`s are heap ids into the `Env`
  heap. They are valid after the run and until the next run mutates or GCs the
  heap. Drain and convert in the same turn; don't stash a `Value` for later.
- **Buffer values are GC roots.** While a run is in progress, anything in an
  output buffer is marked live (see `env/gc.rs`), so an emitted value won't be
  collected before you drain it.
- **Forks emit into their own context.** If you `fork_stack`, drain the fork's
  buffer with `take_output_buffer_for(fork, sym)` and decode against
  `heap_for(fork)`, not the default heap.
- **Errors surface host-side.** Because the native emits without validating, a
  malformed argument becomes an error (or warning) when your interpreter runs
  after the drain — not a mid-run abort. If you *need* a mid-run abort, validate
  in the native and return `Err`.
- **Multiple calls.** Every call appends. `.last()` gives last-wins; iterate the
  whole `Vec` if the calls form a sequence (e.g. one `spawn(...)` per entity).

### Worked example in the tree

Garden's `layout` / `color_theme` / `color_scheme` are exactly this pattern:

- Natives: `~/garden/garden-script/src/native_fns.rs` (push into
  `garden.layout` / `garden.color_theme` / `garden.color_scheme`).
- Host drain + interpret: `ScriptHost::run_and_extract` in
  `~/garden/garden-script/src/lib.rs`.
- Interpreters: `convert_layout` / `convert_theme` in
  `~/garden/garden-script/src/convert.rs`.

The same pattern powers every renderer in the repo: draw natives don't draw,
they `emit` into the `draw_commands` buffer, and the host decodes it each frame.

---

## Related patterns

### Feeding inputs in (host → script)

The mirror image. Bind per-run uniforms before `env.run`:

```rust
let dt = env.intern_symbol("dt");
env.set_binding(dt, Value::Float(frame_dt));   // script reads binding("dt") or dt()
```

Use bindings for scalars/lists the script reads this frame (time, screen size,
input snapshot). See [ffi.md](ffi.md) for the input-state and clicked-id
examples.

petal-ui hosts also publish an **absolute clock** each frame via
`petal_ui::input::bind_time(env, seconds)` — a monotonic value read straight
from the host's clock (e.g. `start.elapsed().as_secs_f64()`), *never* a running
sum of `dt`. Scripts read it as `time()`, and the `ui` prelude's `elapsed()`
captures it once into `state` to report seconds since its first call without the
rounding drift of accumulating `dt` every frame.

### Allocating stable per-run ids

Use a **counter** when the host needs to hand out sequential ids that are stable
across a per-frame re-run (offscreen canvas ids, element ids):
`env.reset_counter(sym, 0)` at frame start, `cxt.next_counter(sym)` in the
native.

### Live host callbacks during a run

Output buffers observe *data*. When a native must call *back into host logic*
synchronously mid-run — e.g. an async data provider answering `query(kind, arg)`
— a buffer can't carry the `Box<dyn Trait>`. Today that still uses a
scoped-swap `thread_local!` (install the provider around `env.run`, reclaim it
after), as in `~/garden/garden-script/src/query.rs`. This is the one case the
buffer pattern does not cover; prefer bindings/buffers whenever the host side is
plain data rather than a callback.

### Persisting an observed call back to source

Once you can observe that a script called `color_scheme("dark")`, you often want
a menu action to *rewrite* that call. That is goal-based editing: declare "there
is a call `color_scheme("light")`" and the engine updates the existing call in
place or appends one, preserving comments and layout. See
[goal-based-editing.md](goal-based-editing.md); Garden's `save_setting` /
`save_layout` are worked examples.

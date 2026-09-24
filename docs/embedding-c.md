# Embedding Petal in C and C++

`integrations/petal-c-bridge/` embeds Petal in a C or C++ host: a Rust static
library that owns the Petal VM (with [petal-ui](../petal-ui/) for input and
drawing), a hand-written C ABI over it (`petal_bridge.h`), and a header-only
C++20 wrapper (`petal.hpp`). It was built for the Cheesecake C++ game engine,
which scripts its game logic in Petal, and moved here so that changes to the
Rust embedding API are built and tested against it in this repository.

For Rust hosts, read [embedding-guide.md](embedding-guide.md) and
[ffi.md](ffi.md) instead. The bridge is written on top of the APIs they
describe, and the sections below link to them where the C API forwards to
one.

```
integrations/petal-c-bridge/
  Cargo.toml             crate petal-c-bridge (lib petal_bridge: staticlib + rlib)
  src/vm.rs              pb_vm: Env + program + stack, most of the C API
  src/natives.rs         host natives: one boxed Petal native per C callback / emitter
  src/scenario.rs        pb_scenario: petal-ui JSON input replay
  src/view.rs            Petal values -> flat pb_value trees
  src/builder.rs         pb_builder: host-built values -> Petal values
  src/draw.rs            petal-ui DrawCommand -> pb_draw_cmd
  src/ffi.rs             status codes, structured errors, panic firewall
  include/petal_bridge.h C ABI (the contract)
  include/petal.hpp      C++20 RAII wrapper
  CMakeLists.txt         target petal::bridge (cargo build + link + includes)
  tests/                 C++ test suite (+ tiny header-only runner), plain-C smoke test
  examples/hello.cpp     smallest useful host
```

The C symbols all start with `pb_` and the C++ wrapper lives in namespace
`petal`.

## Building

`CMakeLists.txt` defines **`petal::bridge`**, an INTERFACE target that
carries the include directory, the static library and its system libraries,
and depends on a custom target that runs cargo. Link it and include
`<petal.hpp>` (or `<petal_bridge.h>` from C):

```cmake
add_subdirectory(path/to/petal/integrations/petal-c-bridge petal-c-bridge)
target_link_libraries(my_engine PRIVATE petal::bridge)
```

Cargo runs on every build (it is incremental, so an up-to-date build costs a
fraction of a second) with `CARGO_TARGET_DIR=<build>/cargo-target`. The crate
depends on the in-repo `rust/`, `petal-ui/` and `petal-query/` crates by path,
so a host that adds this directory always builds against the Petal checkout
it lives in.

| Cache variable | Default | Meaning |
|---|---|---|
| `PETAL_BRIDGE_PROFILE` | `release` | Cargo profile. Stays `release` for Debug C++ builds: the VM is far slower unoptimized. `dev` is available. |
| `PETAL_CARGO_TARGET_DIR` | `<build>/cargo-target` (standalone: the crate's own `target/`) | Where cargo builds. |
| `PETAL_BRIDGE_BUILD_TESTS` | `ON` | Build `petal_bridge_tests`, `petal_bridge_c_smoke`, `petal_bridge_hello` and register them with `add_test`. |

Cargo features: `query` (default on) links petal-query and reports its
protocol version in `pb_version()`; build with `--no-default-features` to
leave it out.

Standalone, from the repository root (this is what CI and `make
test-c-bridge` run; needs CMake, Ninja and a C++20 compiler):

```sh
cmake -S integrations/petal-c-bridge -B integrations/petal-c-bridge/build -G Ninja
cmake --build integrations/petal-c-bridge/build
ctest --test-dir integrations/petal-c-bridge/build --output-on-failure
integrations/petal-c-bridge/build/petal_bridge_tests reload   # a subset, by name
```

`ctest` runs the C++ suite, the plain-C smoke test (the header must compile
as C), the `hello` example, and the crate's Rust unit tests
(`cargo test --lib`).

A Rust crate can also depend on `petal-c-bridge` directly (it is built as an
`rlib` too): [`vm::Vm`](../integrations/petal-c-bridge/src/vm.rs) wraps an
`Env` + program + stack + input state with the same load / run / reload /
scenario behavior as the C API, and the `view`, `draw` and `builder` modules
are the value decoding and draw flattening behind it. That is the path for a
host that already exposes its own C ABI from Rust.

## The model

One `petal::Vm` is one Petal `Env` with one loaded program and one execution
stack. The script's whole top level re-runs every frame (immediate mode);
`state` variables persist across runs and across hot reloads.

```
 host → script                          script → host
 ─────────────                          ─────────────
 bindings      set_float/…/set_value    output buffers   drain(buffer)
 input         mouse_move/key/…         draw commands    drain_draw()
 frame info    begin_frame(dt, n, t)    mouse grab       take_mouse_grab()
 host natives  native(name, fx, fn) ◄── synchronous calls with return values
                                        print lines      take_output()
 recorded input apply_scenario(sc, n)
```

The frame contract:

```cpp
// as SDL events arrive
vm.mouse_move(x, y); vm.mouse_motion(dx, dy); vm.key("space", true); ...

// each frame
vm.begin_frame(dt, frame_index, seconds_since_start);  // input edges, dt(), time()
vm.set_value("physics", [&](petal::BuilderRef b) { ... });  // host data
try {
    vm.run();                          // clears all output buffers first
} catch (const petal::Error& e) {      // runtime error: show overlay, keep going
    overlay.show(e);
}
for (petal::Value cmd : vm.drain("scene")) { ... }
for (const pb_draw_cmd& d : vm.drain_draw()) { ... }
if (auto grab = vm.take_mouse_grab()) window.set_relative_mouse(*grab);
for (auto& line : vm.take_output()) log(line);

// every ~0.25 s
if (vm.sources_changed()) {
    try { vm.reload(); } catch (const petal::Error& e) { overlay.show(e); }
}
```

## Errors

The C++ wrapper throws `petal::Error` for every failure; there is no
expected-style variant. An error carries:

- `code()` — a `pb_status` (`PB_ERR_COMPILE`, `PB_ERR_RUNTIME`, `PB_ERR_IO`,
  `PB_ERR_NOT_LOADED`, `PB_ERR_INVALID_ARG`, `PB_ERR_NOT_FOUND`,
  `PB_ERR_PANIC`, `PB_ERR_REENTRANT`; `PB_ERR_LIMIT` is reserved);
- `what()` — the full message, including Petal's source excerpt with a caret;
- `phase()` — `"lex"`, `"parse"`, `"module"`, `"compile"`, `"lower"` or
  `"runtime"`;
- `items()` — each diagnostic with `message`, `file`, `line`, `column`
  (1-based), and `file()/line()/column()` for the first.

A failed load or reload leaves the previous program loaded and running.
Runtime errors abort the frame; whatever the script emitted before the error
is still drainable, and `state` written before the error is kept. Rust panics
never cross the boundary: every entry point catches them and reports
`PB_ERR_PANIC`.

From C, functions return `pb_status` and `pb_vm_last_error(vm)` returns the
details (valid until the next call on that VM).

## Values

### Script → host: `petal::Value`

Everything the script hands back — drained buffers, native arguments,
`call()` results, `state()` — arrives as a **decoded view**: a tree of
`pb_value` nodes the bridge owns. No JSON is involved. A `Value` is a
non-owning handle to one node:

```cpp
petal::Value v = vm.drain("scene")[0];   // spawn("crate", {pos: {...}, tags: [...]})
v.tag();                  // "spawn"            (enum variant tag)
v[0].str();               // "crate"            (string_view)
v[1]["pos"].num("x");     // 1.5                (map field, numeric with fallback)
v[1]["tags"].size();      // 2
for (petal::Value t : v[1]["tags"]) t.str();
v[1]["missing"]["deeper"].is_nil();      // true: misses chain safely
```

| Petal | `pb_kind` | Access |
|---|---|---|
| `nil` | `PB_NIL` | `is_nil()` |
| `true` | `PB_BOOL` | `boolean()` (also `number()` = 0/1) |
| `42` | `PB_INT` | `integer()`, `number()` |
| `1.5` | `PB_FLOAT` | `number()`, `integer()` (truncated) |
| `vec2(x, y)` | `PB_VEC2` | `x()`, `y()`; also `num("x")`/`num("y")` |
| `vec3(x, y, z)` | `PB_VEC3` | `x()`, `y()`, `z()`; also `num("x")`…`num("z")`, so host code written for `{x, y, z}` records accepts vectors (`[key]`/`has` stay map-only) |
| `"text"` | `PB_STRING` | `str()` |
| `[a, b]` | `PB_LIST` | `size()`, `[i]`, iteration |
| `{k: v}`, colors, class instances | `PB_MAP` | `[key]`, `num(key)`, `has(key)`, iteration (`key()` on each field); `str()` = class name |
| `Circle(3)` | `PB_ENUM` | `tag()`, payload via `[i]` |
| `symbol("s")` | `PB_SYMBOL` | `str()` |
| handles | `PB_HANDLE` | `integer()` = slot |

Color literals such as `#ff8800` are records `{r, g, b}` (0–255). A native
`vec3` decodes as `PB_VEC3`, with its components in `number`, `y` and `z`
(full `f64` precision); an `{x, y, z}` *record* is still an ordinary
`PB_MAP`.

**Lifetime:** views stay valid until the next `run()`, `call()`, `load_*()`,
`reload*()` or `clear_views()` on the same VM. Several buffers can be drained
in one frame; all of their views live until the next run. Copy out anything
you need to keep.

### Host → script: bindings and builders

Bindings are host-set values the script reads with
`binding(symbol("name"))` (a host prelude usually wraps these in nicer names).
They persist until overwritten or cleared.

```cpp
vm.set_float("gravity", -9.81);
vm.set_vec3("sun_dir", 0.3, -1, 0.2);          // a native vec3
vm.set_floats("heights", std::span<const double>(h));
vm.set_value("bodies", [&](petal::BuilderRef b) {
    b.list([&](petal::BuilderRef l) {
        for (auto& body : world.bodies())
            l.map([&](petal::BuilderRef m) {
                m.field("id", body.id);
                m.key("pos").vec3(body.pos.x, body.pos.y, body.pos.z);
                m.field("sleeping", body.sleeping);
            });
    });
});
```

`petal::Builder` (owning) / `petal::BuilderRef` (non-owning) build arbitrary
values: `value(bool|int|int64|double|string)`, `nil()`, `vec2`, `vec3`,
`symbol`, `floats`, `list(fn)`, `map(fn)`, `enum_(tag, fn)`, `field(key, v)`,
or the explicit `begin_*/end_*` + `key()` calls. Builders record a plain tree
and are only turned into Petal heap values at the moment they are bound,
passed or returned, so they are reusable and independent of any VM. Misuse
(a key outside a map, unbalanced containers) surfaces as `PB_ERR_INVALID_ARG`
when the builder is consumed.

`vec3` (`pb_builder_vec3`, `pb_vm_set_vec3`) produces a native Petal `vec3`,
so scripts get vector arithmetic, `.x/.y/.z`, `mag`, `dot`, `cross` and the
rest on it directly. *Changed in 0.2.0:* earlier versions (and Cheesecake's
original bridge) built the record `{x, y, z}` instead. Scripts that only read
`.x/.y/.z` are unaffected; a script that treats the value as a record (adds
fields to it, spreads it, iterates it with `keys`) must build its own record.

## Host natives

### Callbacks: synchronous, with return values

```cpp
vm.native("raycast", petal::fx::ReadsHostData, [&](petal::Call& c) {
    petal::Value from = c[0], dir = c[1];
    auto hit = physics.raycast({from.x(), from.y(), from.z()},  // PB_VEC3 args
                              {dir.x(), dir.y(), dir.z()}, c[2].number(100));
    if (!hit) return;                          // empty result = nil
    c.result().map([&](petal::BuilderRef m) {
        m.field("dist", hit->dist);
        m.key("point").vec3(hit->p.x, hit->p.y, hit->p.z);
        m.field("body", hit->body_id);
    });
});
```

```petal
let hit = raycast(pos, vec3(0, -1, 0), 2.0)
let grounded = hit != nil && hit.dist < 0.6
```

- Register **before the load that uses the native** (natives are resolved by
  name at compile time; a later reload also sees natives registered since).
- `Call` gives `name()`, `size()`, `c[i]` (views valid during the call), and
  `result()`, a builder for at most one value (empty = `nil`).
- Throw any `std::exception` to fail the script call; it becomes a
  `PB_ERR_RUNTIME` from `run()` with your message and the call's line/column.
- Do not call back into the same VM from inside a native (`PB_ERR_REENTRANT`).
- The `std::function` is owned by the VM and destroyed with it. It survives
  reloads: the native belongs to the VM, not to one program.
- There is no limit on how many natives a VM registers.

**Effects.** Petal's reactive layers trust each native's declared effect row,
so declare honestly (combine with `|`; [Embedder pitfalls](ffi.md#embedder-pitfalls)
shows what a dishonest row breaks):

| Flag | Meaning |
|---|---|
| `fx::Pure` (0) | pure function of its arguments |
| `fx::Probe` | pure function of its arguments and its reads; safe to re-evaluate |
| `fx::Emits` | pushes into an output buffer. Only [emitters](#emitters-the-command-stream-fast-path) do that (they set it themselves); a callback has no way to push, so a callback that queues work for the host is `fx::Effect` |
| `fx::Effect` | does something no replay reproduces (mutates host state, plays a sound) |
| `fx::ReadsHostData` | answers from host state (body transforms, raycasts). The bridge also calls `note_host_read()` for you. |
| `fx::ReadsPointer/Keyboard/Clock/Viewport/Resources/Rng/Bindings` | other input classes |
| `fx::PendingEffectful` / `fx::PendingAllow` | how a `Pending` argument is treated (default: absorbed) |

### Emitters: the command-stream fast path

Most engine calls are declarations the host acts on after the frame (spawn
a mesh, declare a body, play a sound). An emitter records the call and nothing
else; no host code runs mid-script:

```cpp
vm.emitter("draw_mesh", "scene");                 // tag = "draw_mesh"
vm.emitter("point_light", "scene");
vm.emitter("sfx", "audio");
vm.emitter("body", "physics", "declare_body");    // custom tag
```

```petal
draw_mesh("cube", vec3(0, 1, 0), rot, 1.0, {color: #cc3333})
sfx("jump", {volume: 0.8})
```

Each call pushes `tag(args...)` — a `PB_ENUM` whose payload is the call's
arguments — into the named buffer, in call order, and returns `nil`:

```cpp
for (petal::Value cmd : vm.drain("scene")) {
    if (cmd.tag() == "draw_mesh") renderer.submit(cmd[0].str(), {cmd[1].x(), cmd[1].y(), cmd[1].z()}, ...);
    else if (cmd.tag() == "point_light") ...
}
```

Emitters are declared `Emits` with the Pending-no-op policy. Emitting into
petal-ui's own `"draw_commands"` buffer is supported: such commands come back
from `drain_draw()` as `PB_DRAW_HOST` in their place in the draw order (see
below) — useful for HUD extensions like sprites.

### How it works (no globals)

Each host native is one boxed Petal native
([`Env::register_native_boxed`](ffi.md#native-functions)): a closure that
owns the C callback, its userdata and the userdata's free function (or, for
an emitter, the buffer and tag). The closure belongs to the VM's `Env`, so
all dispatch state lives on the `Env`, as Petal's
[embedding rules](embedding-guide.md#the-golden-rule-no-globals) ask. Two VMs
never share it (this is tested), and the userdata is freed exactly once, when
the `Env` drops the closure on `pb_vm_destroy`. A forked or speculative
execution of the VM calls the same closure. Petal's effect audit sees only
what the native does through its `PetalCxt`, not what the host callback does
to host state, so the `fx::` flags must say that.

## petal-ui: input and draw commands

Each VM registers every petal-ui native and makes the `ui` prelude an
implicit import, so scripts use `mouse_x()`, `key_pressed("space")`,
`mouse_dx()`, `grab_mouse()`, `draw_rect(...)`, `button(...)` etc. with no
imports.

Input goes in as events; `begin_frame` promotes edges (so `key_pressed` is
true for exactly one frame) and binds `dt()`, `frame_count()` and `time()`.
Key names must be petal-ui's canonical names (`"a"`–`"z"`, `"0"`–`"9"`,
`"space"`, `"return"`, `"escape"`, `"tab"`, `"backspace"`, arrows
`"up"/"down"/"left"/"right"`, `"shift"/"ctrl"/"alt"/"cmd"`, `"f1"`–`"f12"`,
`"minus"`, `"equals"`, `"comma"`, `"period"`, `"slash"`, `"backslash"`,
`"semicolon"`, `"quote"`, `"backquote"`, `"leftbracket"`, `"rightbracket"`,
`"pageup"`, `"pagedown"`, `"home"`, `"end"`, `"delete"`, `"insert"`);
anything else is rejected with `PB_ERR_INVALID_ARG`
(`Vm::is_canonical_key()` checks). Mouse buttons: `PB_MOUSE_LEFT` (0),
`PB_MOUSE_RIGHT` (1), `PB_MOUSE_MIDDLE` (2).

| C++ | Script sees |
|---|---|
| `mouse_move(x, y)` | `mouse_x()`, `mouse_y()`, hover/drag |
| `mouse_motion(dx, dy)` | `mouse_dx()`, `mouse_dy()` (summed per frame; works while grabbed) |
| `mouse_button(b, down)` | `mouse_down(b)`, `mouse_pressed(b)`, `mouse_released(b)`, `click_count()`, drags |
| `scroll(dx, dy)` | `scroll_x()`, `scroll_y()` (lines; fractions carry) |
| `key(name, down)` | `key_down`, `key_pressed`, `key_released` |
| `text(utf8)` | `text_input()` |
| `modifiers(bits)` | `mod_shift()`, `mod_ctrl()`, `mod_alt()`, `mod_cmd()` (bits `PB_MOD_SHIFT/CTRL/ALT/CMD`) |
| `begin_frame(dt, frame, t)` | `dt()`, `frame_count()`, `time()` |
| `set_dimensions(w, h)` | `screen_width()`, `screen_height()` |
| `set_text_metrics(ratio)` / `set_text_vertical_metrics(...)` | `text_width()`, `text_metrics()` match the host font |
| `set_text_advances(table)` | proportional `text_width()`: `table[codepoint]` is that glyph's advance ÷ size |
| `set_seed(n)` | reproducible `random(...)` |
| `apply_scenario(sc, frame)` | whatever the scenario schedules for `frame` (see below) |

`take_mouse_grab()` returns the last `grab_mouse()` (`true`) /
`release_mouse()` (`false`) request of the frame, or `nullopt`.

### Draw commands

`drain_draw()` returns a `std::span<const pb_draw_cmd>` in draw order. Each
petal-ui `DrawCommand` is one flat struct with a `kind` tag; only the fields
listed for that kind are meaningful (the rest are zero). Coordinates are
logical pixels from the top-left; colors are sRGB 0–255.

| `kind` | Fields |
|---|---|
| `PB_DRAW_CLEAR` | `color` |
| `PB_DRAW_RECT` | `x y w h`, `color`, `radius` (0 = square) |
| `PB_DRAW_RECT_OUTLINE` | `x y w h`, `color`, `width`, `radius` |
| `PB_DRAW_LINE` | `x1 y1 x2 y2`, `color`, `width` |
| `PB_DRAW_CIRCLE` | `cx cy`, `radius` (= `rx` = `ry`), `color` |
| `PB_DRAW_ELLIPSE` / `_OUTLINE` | `cx cy rx ry`, `color` (+ `width`, stroked inside) |
| `PB_DRAW_ARC` | `cx cy`, `r_in r_out`, `a0 a1` (radians, clockwise from +x, y down), `color` |
| `PB_DRAW_TRIANGLE` | `x1 y1 x2 y2 x3 y3`, `color` |
| `PB_DRAW_POLY` | `points[point_count*2]` (convex fan from point 0), `color` |
| `PB_DRAW_POLYGON` | `points` (simple polygon, concave allowed), `color` |
| `PB_DRAW_FAN` | `cx cy` + `points` (fan from center, not closed), `color` |
| `PB_DRAW_POLYLINE` | `points` (open path, round joins/caps), `color`, `width` |
| `PB_DRAW_TEXT` | `text`/`text_len`, `x y` (top-left), `size`, `color`, `font` (NULL = default), `weight`, `italic`, `spacing` |
| `PB_DRAW_RECT_GRADIENT` | `x y w h`, `radius`, `color` → `color2` along `angle` |
| `PB_DRAW_CIRCLE_GRADIENT` | `cx cy`, `radius`, `color` (center) → `color2` (rim) |
| `PB_DRAW_SHADOW` | casting shape `x y w h radius`, `blur`, `spread`, `dx dy`, `color` |
| `PB_DRAW_CLIP` | `x y w h radius` — replaces the clip |
| `PB_DRAW_CLIP_NONE` | — |
| `PB_DRAW_CLIP_PUSH` / `PB_DRAW_CLIP_POP` | push (intersect) / pop |
| `PB_DRAW_IMAGE` | `text` = asset source, `x y w h`, `color.a`, `radius` |
| `PB_DRAW_CREATE_CANVAS` | `id`, `w h` |
| `PB_DRAW_SET_TARGET` | `id` (0 = framebuffer) |
| `PB_DRAW_DRAW_CANVAS` | `id`, `x y`, `color.a` = opacity, `w h` (0 = native size) |
| `PB_DRAW_SNAPSHOT` | `id`, `x y` |
| `PB_DRAW_BLUR_CANVAS` | `id`, `radius` |
| `PB_DRAW_HOST` | `text` = tag, `data[data_count]` = decoded args |

A host that cannot do something should degrade it (square corners, one
gradient stop, skip canvases) rather than drop it; see petal-ui's README.

### Input scenarios (replay)

A scenario is petal-ui's declarative input script
(`petal-ui/src/scenario.rs`): JSON listing input events keyed by frame, plus
an optional window size and frame count. It is the format `petal-ui-run` and
`ts/bin/verify.ts` use, so a repro recorded there replays in a C++ host, and
a C++ host's headless tests can be written as data:

```json
{ "size": [800, 600], "frames": 120,
  "events": [ {"at": 5, "click": [100, 200]}, {"at": 9, "key": "space"},
              {"at": 12, "text": "hi"}, {"at": 20, "scroll": [0, -3]},
              {"at": 30, "key_down": "a"}, {"at": 40, "key_up": "a"} ] }
```

```cpp
petal::Scenario sc = petal::Scenario::from_file("tests/menu_clicks.json");
if (auto size = sc.size()) vm.set_dimensions(size->first, size->second);
size_t frames = sc.frames().value_or(sc.end_frame());
for (size_t f = 0; f < frames; ++f) {
    vm.apply_scenario(sc, f);                  // before begin_frame
    vm.begin_frame(1.0 / 60, f, f / 60.0);
    vm.run();
}
```

- `apply_scenario(sc, n)` feeds the events scheduled for frame `n` into the
  VM's input state exactly as the `mouse_*`/`key`/`text` calls would; call it
  before `begin_frame(…, n, …)` so that frame sees their edges. Applying is a
  pure function of (scenario, frame), and a `Scenario` is independent of any
  VM.
- `click: [x, y]` expands to move + press at `n` and release at `n + 1`;
  `key: "name"` to press + release at `n`. `event_count()` counts the
  expanded events; `end_frame()` is one past the last event's frame.
- `Scenario::monkey(seed, frames, w, h)` generates a deterministic
  pseudo-random scenario (clicks, canonical keys, short text), and
  `to_json()` writes any scenario back out, for a repro file.
- A malformed scenario, or one naming a non-canonical key, throws
  `PB_ERR_INVALID_ARG` (`PB_ERR_IO` for an unreadable file). From C,
  `pb_scenario_load_json` / `_load_file` return the status and
  `pb_scenario_error` says why; the previous contents are kept.

## Hot reload

```cpp
vm.load_file("games/marble/game.ptl");
...
if (vm.sources_changed()) {                 // stat() of every source file
    auto edited = vm.changed_sources();     // which ones, e.g. {".../tuning.ptl"}
    try {
        auto r = vm.reload();               // recompile + transfer_state
        log("reloaded {}: kept {} state slots, dropped {}", edited, r.state_preserved, r.state_dropped);
    } catch (const petal::Error& e) {
        overlay.show(e);                    // old program keeps running
    }
}
```

- `source_files()` lists the entry file and every imported module with a
  filesystem origin (`Env::program_source_paths`). `sources_changed()`
  compares each file's modification time and length with the last
  load/reload attempt, and counts a deleted or newly appeared file as a
  change (`petal::source_watch`, the same watcher petal-desktop-sdl and
  Garden use). A failed reload counts as an attempt, so a broken file is not
  recompiled every poll. After a successful reload the watched set follows
  the new program's imports.
- `changed_sources()` names the files `sources_changed()` counts, in
  `source_files()` order and spelling (a deleted file is included). Read it
  *before* `reload()`, which re-stamps the watch: a host uses it for its
  "reloaded tuning.ptl" message, or to keep caches that depend only on
  modules that did not change. Empty when nothing changed or the program
  came from memory. From C, `pb_vm_changed_sources` (the list stays valid
  until the next call to it).
- **While the program is broken** — a `load_file()` that failed, or a failed
  reload of a file-loaded program — the program's own file list is missing
  or stale: the broken edit may import a module that does not exist yet, or
  one that exists but does not compile. Until a load or reload succeeds,
  `sources_changed()` / `changed_sources()` also watch every `.ptl` file
  under the entry file's directory (hidden directories skipped, the scan
  bounded to 4096 entries), and a `.ptl` file that appears there counts as a
  change. After a failed first load, `reload()` retries the load (fresh
  state), so one loop covers both cases:

  ```cpp
  try { vm.load_file("game.ptl"); } catch (const petal::Error& e) { overlay.show(e); }
  // every ~0.25 s
  if (vm.sources_changed()) {
      try { vm.reload(); } catch (const petal::Error& e) { overlay.show(e); }
  }
  ```
- `reload()` re-reads the entry file (imports are re-resolved and re-read
  too), compiles it once with `Env::compile_program_diag` (so a compile error
  arrives with its structured diagnostics), and calls Petal's
  `transfer_state`: state whose declaration
  still exists is kept (matched by name path, so reordering is fine; renaming
  drops it). `state_preserved` also counts the `ui` prelude's internal state
  slots.
- `reload_source(text)` does the same from memory (editors, tests).
- A new program sees natives registered since the original load.

## Modules and packages

```cpp
vm.register_module("engine", engine_prelude_source);  // `import engine`
vm.add_implicit_import("engine");                      // ...or no import at all
vm.add_module_path("games/marble/lib");                // search path (+ package discovery)
vm.add_package("path/to/petal/petal-libs/bloom");     // `import bloom/menu`
```

The implicit-import list starts as `["ui"]`; `add_implicit_import` appends.
All of these affect later loads.

## Other tooling

- `call("fn_name", args)` calls a top-level Petal function after at least one
  `run()`. A function that does not exist is `PB_ERR_NOT_FOUND`; one that
  exists and fails is `PB_ERR_RUNTIME` (the bridge matches Petal's typed
  `CallError`, not the message text). `has_function("fn_name")` asks first,
  for optional hooks.
- `restart()` starts the loaded program over with empty `state`, without
  recompiling (a fresh stack on the same program).
- `state("name")` reads a top-level state variable as a view;
  `state_json()` dumps all state (debug use only).
- `take_output()` returns the lines the script `print`ed. Echo to stdout is
  off by default (`set_echo(true)` turns it on).
- `set_profiling(true)` / `profile_report(top_n)` (`pb_vm_set_profiling`,
  `pb_vm_profile_report`) turn on the VM profiler and read its report:
  opcodes, functions (self instructions and the time in the natives they
  call, as `name file:line`), natives by wall time, collections and the memo
  counters, all since profiling was turned on. Profiling roughly doubles
  script time, so turn it on after warm-up and read the report once
  (docs/dev/performance.md).
- `set_memo(false)` / `memo()` (`pb_vm_set_memo`, `pb_vm_memo`) switch call
  memoization off or on for later runs (on by default, or as `PETAL_POLICY`
  says). Memo replays a call whose inputs repeat, which pays off for UI-like
  programs; a game whose inputs change every frame runs a few percent faster
  with it off. Output is the same either way.
- `pb_version()` names the bridge, petal-ui contract and prelude level, and
  petal-query protocol version.

## Using the C API directly

`petal.hpp` is a thin layer; anything it does can be done from C:

```c
pb_vm* vm = pb_vm_create();
pb_vm_register_emitter(vm, "spawn", "scene", NULL);
if (pb_vm_load_file(vm, "game.ptl") != PB_OK) {
    const pb_error* e = pb_vm_last_error(vm);
    fprintf(stderr, "%s:%u:%u: %s\n", e->file, e->line, e->column, e->message);
}
pb_vm_begin_frame(vm, dt, frame, t);
pb_vm_run(vm);
pb_values cmds;
pb_vm_drain(vm, "scene", &cmds);
for (size_t i = 0; i < cmds.count; ++i) {
    const pb_value* c = &cmds.items[i];                 /* PB_ENUM, str = tag */
    double x = pb_value_num(pb_value_get(pb_value_at(c, 1), "x"), 0.0);
}
pb_vm_destroy(vm);
```

Host natives in C: `pb_vm_register_native(vm, name, fn, userdata, free_fn,
effects)` with `int fn(pb_call*, void* userdata)`; read arguments with
`pb_call_arg(call, i)`, build the result into `pb_call_result(call)` with
`pb_builder_*`, and fail with `pb_call_set_error` + a nonzero return.

## Layering an engine's own C ABI in Rust

Some hosts want a few C entry points of their own on top of the bridge —
concepts that belong to the game, not to Petal (a screen registry, a data
model, the game's action vocabulary). Such a crate depends on
`petal-c-bridge` as an `rlib` and builds one static library: the bridge's
`pb_*` symbols come along, so the host includes `petal_bridge.h` next to its
own header and uses the bridge for everything generic.

```rust
use petal_bridge::vm::{Vm, VmHandle};

let mut vm = Vm::new();                       // petal-ui registered, `ui` imported
vm.env_mut().register_native("game_model", ...);
vm.load(&source, None, "hud".into())?;
let ptr = VmHandle::into_raw(vm);             // the host's pb_vm*; pb_vm_destroy frees it

// inside the crate's own entry points:
let vm = unsafe { VmHandle::vm_mut(ptr) }.unwrap();  // None while a native is running
vm.begin_frame(dt, frame, t);
vm.run()?;
vm.set_last_error(Some(err));                 // surfaces through pb_vm_last_error
```

The host then feeds input (`pb_vm_input_*`), reads draw commands
(`pb_vm_drain_draw`) and reads errors (`pb_vm_last_error`) on that same
`pb_vm*`. WorldsFair's Unreal UI (`ui/crates/wf-ui-ffi`) is built this way.

## Performance notes

Measured on an M-series Mac, release build, in Cheesecake: a frame that makes
200 emitter calls, 200 host-callback calls and 2 draw calls, then drains and decodes
everything, costs about 0.4 ms. Each host-callback native reuses its own
argument, view and result buffers from call to call, so a call allocates
nothing on the bridge side beyond its result. Loading or reloading a program takes about
0.1 s, most of it compiling the `ui` prelude.

## Limitations

- Runtime error positions are recovered from the text of Petal's message
  (`[line N, column M]` in the entry file, `[module.ptl line N, column M]` in
  an imported module), since Petal's runtime errors are plain strings.
- `petal-query` is linked by default (its protocol version is reported by
  `pb_version()`) but its provider/cache layer is not exposed yet; host data
  queries use host natives instead.
- Any value the bridge does not know decodes as `PB_OTHER` with its type
  name.
- Emit tracing / provenance (Petal's "which line drew this?") and forked
  speculative runs are not exposed yet.

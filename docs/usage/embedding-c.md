# Embedding Petal in C and C++

`integrations/petal-c-bridge/` embeds Petal in a C or C++ host: a Rust static
library that owns the Petal VM (with [petal-ui](../../core-libs/petal-ui/) for input and
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
  src/source.rs          pb_source: read and edit source text as a config file
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
depends on the in-repo `core/`, `core-libs/petal-ui/` and `core-libs/petal-query/` crates by path,
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
`rlib` too): [`vm::Vm`](../../integrations/petal-c-bridge/src/vm.rs) wraps an
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
  `PB_ERR_PANIC`, `PB_ERR_REENTRANT`; `PB_ERR_LIMIT` is reserved, and
  `PB_ERR_NEEDS_RELOAD` is only ever the `false` that
  [`set_config`](#setting-a-value-without-writing-the-file) returns);
- `what()` — the full message, including Petal's source excerpt with a caret;
- `phase()` — `"lex"`, `"parse"`, `"module"`, `"compile"`, `"lower"`,
  `"link"` (a declared native or a required interface this VM does not have)
  or `"runtime"`;
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
| `Shape.circle(3)` | `PB_ENUM` | `tag()` = `"Shape.circle"`, payload via `[i]` |
| `symbol("s")` | `PB_SYMBOL` | `str()` |
| handles | `PB_HANDLE` | `integer()` = slot |

Color literals such as `#ff8800` are records `{r, g, b}` (0–255). A native
`vec3` decodes as `PB_VEC3`, with its components in `number`, `y` and `z`
(full `f64` precision); an `{x, y, z}` *record* is still an ordinary
`PB_MAP`.

An enum variant crosses as its tag and its payload. The tag is qualified
(`Shape.circle`, with the enum's declared name whatever module it came
through) and the payload is positional. The variant's constant data
(`sides` of `circle(r, sides: 0)`) and the names of its payload fields belong
to the declaration, not to the value, so they are not in the view: read them
in the script and hand the host what it needs. A command pushed with `emit`
is also a `PB_ENUM`, under the plain tag the host registered (`"spawn"`).

**Lifetime:** views stay valid until the next `run()`, `call()`, `load_*()`,
`reload*()` or `clear_views()` on the same VM. Several buffers can be drained
in one frame; all of their views live until the next run. Copy out anything
you need to keep.

### Host → script: bindings and builders

Bindings are host-set values the script reads with
`binding(symbol("name"))` after `import io` (a host's interface package
usually wraps these in nicer names, so only it needs the import).
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

`enum_("Shape.box", fn)` builds a variant of the script's own enum: under the
tag `Enum.variant`, with the payload in declaration order, it *is* that
variant to the script — it equals `Shape.box(2, 3)`, matches
`when Shape.box(w, h)`, and reads the constant data and field names its
declaration gives it. The bridge does not check the tag against the program,
which may not be loaded yet: a payload of the wrong size, or a variant the
enum does not declare, is a value no pattern matches. A tag with no enum in
front (`"box"`) belongs to no declared enum.

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

```petal ignore
import vec2
import engine        // your interface package: it declares raycast
let hit = raycast(pos, vec3(0, -1, 0), 2.0)
let grounded = hit != nil && hit.dist < 0.6
```

- A script reaches the native through a `native fn` declaration, so register
  [an interface package](#your-natives-need-an-interface-package) that
  declares it, and have the script import that package.
- Register **before the load that uses the native** (a load links every
  declared native the program refers to; a later reload also sees natives
  registered since).
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
| `fx::Const` (`PB_FX_CONST`) | a **constant function**: the same arguments give the same result for as long as the VM lives, reading no host state of any kind. Opt-in; see below |

**`fx::Const` is not `fx::Pure`.** `fx::Pure` is `0`, the default, and says
only that the native touches nothing Petal tracks. `fx::Const` is a stronger
promise that Petal acts on differently: a top-level table whose values come
from constant natives is a static value, which Petal may compute **once** and
keep until the script's source changes — your callback is not called again
for it. A wrong `fx::Const` is therefore a stale value forever, where a wrong
`fx::Pure` is a stale replay until the next invalidation. Give it only to a
callback that computes from its arguments or from a table compiled into the
host (`tile_code(name)`, a unit conversion, a fixed palette lookup), never to
one that answers from anything loaded, configured or simulated (assets,
settings, fonts, the scene, the clock), and leave it off when unsure: a
native without it is simply re-evaluated every run, as before. Combined with
`fx::Probe`, `fx::Emits`, `fx::Effect` or any `fx::Reads*` flag it is ignored.
The full checklist is in [ffi.md](ffi.md#constant-natives).

```cpp
vm.native("tile_code", petal::fx::Const, [](petal::Call& c) {
    c.result().value(tile_code_of(c[0].string()));   // a compiled-in table
});
```

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

```petal ignore
import vec2
import engine        // your interface package: it declares draw_mesh and sfx
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

Each VM registers every petal-ui native and the `ui` package that declares
them. A script starts with `import ui` and then uses `mouse_x()`,
`key_pressed("space")`, `mouse_dx()`, `grab_mouse()`, `draw_rect(...)`,
`button(...)` etc. bare (or as `ui.button`). Nothing is imported implicitly.

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
| `PB_DRAW_IMAGE` | `text` = asset source, `x y w h`, `color.a`, `radius`; `color.rgb` = tint (white = none); `x1 y1 x2 y2` = source rect (`x2 == 0`: whole image); `x3 y3 cx cy` = nine-slice insets |
| `PB_DRAW_CREATE_CANVAS` | `id`, `w h` |
| `PB_DRAW_SET_TARGET` | `id` (0 = framebuffer) |
| `PB_DRAW_DRAW_CANVAS` | `id`, `x y`, `color.a` = opacity, `w h` (0 = native size) |
| `PB_DRAW_SNAPSHOT` | `id`, `x y` |
| `PB_DRAW_BLUR_CANVAS` | `id`, `radius` |
| `PB_DRAW_HOST` | `text` = tag, `data[data_count]` = decoded args |

| `PB_DRAW_RECTS` | `points` = `point_count` rects, five ints each: `x y w h` and the colour as `r << 24 \| g << 16 \| b << 8 \| a` |
| `PB_DRAW_CACHE_BEGIN` / `PB_DRAW_CACHE_END` | `id`, `w h` / `id`: the commands between are retained layer `id`'s content, in its own coordinates |
| `PB_DRAW_CACHE_DRAW` | `id`, `x y`: draw the retained layer there |

A host that cannot do something should degrade it (square corners, one
gradient stop, skip canvases) rather than drop it; see petal-ui's README.

The last three kinds are sent only to a host that asks:

```cpp
pb_vm_set_draw_caps(vm.raw(), PB_DRAW_CAP_BATCH_RECTS | PB_DRAW_CAP_RETAINED_LAYERS);
```

Without that, a `draw_rects` batch arrives as one `PB_DRAW_RECT` per entry
and a `cached_layer` as its content, moved and clipped, on every frame: the
script's saving is the same, the host's is not. With it, the host keeps a
texture per layer id and calls `pb_vm_invalidate_layers` if it loses one. The
rules are in [ui-host-contract.md](ui-host-contract.md#cache_begin-cache_end-cache_draw-only-with-the-retained-layer-capability).

Draw layers (`ui_layer`) need nothing: `pb_vm_drain_draw` returns the frame
already sorted.

### Targets: acting on a widget by name

Every rect a script registers for hit testing can carry an id: a `button` its
label, anything passed to `interact(r, id)` or `ui_target(id, r)`. The host
reads them after a run:

```cpp
pb_ui_target t;
if (pb_vm_ui_target(vm.raw(), "End turn", &t) == 1)
    click(t.x + t.w / 2, t.y + t.h / 2);

const pb_ui_target* all; size_t n;
pb_vm_ui_targets(vm.raw(), &all, &n);        // everything registered last run
```

The rect is in logical pixels and already clipped to what was visible. This
is what a test harness or an agent tool uses in place of coordinates, and it
is what a scenario's `{"at": N, "click_target": "End turn"}` resolves against.

### Input scenarios (replay)

A scenario is petal-ui's declarative input script
(`core-libs/petal-ui/src/scenario.rs`): JSON listing input events keyed by frame, plus
an optional window size and frame count. It is the format `petal-ui-run` and
`tools/verify.ts` use, so a repro recorded there replays in a C++ host, and
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
        auto r = vm.reload();               // only the work the edit calls for
        log("reloaded {}: kept {} state slots, dropped {}", edited, r.state_preserved, r.state_dropped);
        log_error(vm.new_diagnostics_text());   // what the edit introduced, warnings too
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
- `reload()` re-reads the entry file and its modules and calls
  `Env::reload_program`, which does only the work the edit calls for
  ([../features/hot-reload.md](../features/hot-reload.md)):

  | The edit | `r.kind` | What happens |
  |---|---|---|
  | nothing | `PB_RELOAD_UNCHANGED` | nothing |
  | whitespace, comments, layout | `PB_RELOAD_RELOCATED` | source positions move |
  | literal values, each keeping its type | `PB_RELOAD_PATCHED` | the values are written into the running program |
  | anything else | `PB_RELOAD_RECOMPILED` | recompile, then `transfer_state` |

  The first three compile nothing, and whichever path is taken, the program
  and its state afterwards are what the full recompile would have left; what
  each outcome keeps and what it costs is in
  [../features/hot-reload.md](../features/hot-reload.md#what-each-outcome-leaves-alone).
  A compile error arrives with its structured diagnostics while the old
  program keeps running. `r.kind` is also available afterwards as
  `last_reload_kind()` (`pb_vm_last_reload_kind`, `-1` in C before the first
  reload). After a recompile, state whose declaration still exists is kept
  (matched by name path, so reordering is fine; renaming drops it).
  `state_preserved` also counts the `ui` package's internal state slots.
- `last_reload_json()` (`pb_vm_last_reload_json`) describes the last reload:
  the outcome, the files that changed, each literal that changed with its
  binding path (`{"path": "POST.bloom", "old": 0.2, "new": 0.4}`), or the
  top-level constructs that differ (`{"kind": "fn", "name": "draw", "edit":
  "changed"}`). Under `statics` it says what the reload did to the program's
  static values and names the stale seeds: `{"kept": 41, "rebuilt": 2,
  "rebuilt_names": ["TILES", "WIDTHS"], "deferred": [], "stale_seeds":
  [{"name": "grid", "read": "TILES"}], "cleared": null}`
  ([../features/hot-reload.md](../features/hot-reload.md#static-values-across-a-reload)). The format
  is in `petal_bridge.h`.
- `set_static_follow(false)` (`pb_vm_set_static_follow`) makes a reload
  rebuild every static value instead of keeping the ones the edit cannot have
  changed, which is the default.
- `reload_source(text)` does the same with new text for the entry file
  (editors, tests).
- A new program sees natives registered since the original load.

### Setting a value without writing the file

A slider drag should not write a file every frame. `set_config`
(`pb_vm_set_config`) sets one value of a top-level binding in the running
program directly:

```cpp
// while dragging
if (!vm.set_config("POST.effects[2].amount", "0.35")) {
    // a different type or shape: write the file and reload instead
}
// on release: the running text is the file with every edit applied
write_file(config_path, vm.source_text(config_path));
vm.reload();                                // PB_RELOAD_UNCHANGED: nothing to do
```

- The path and the JSON are those of `pb_source_set_value` (below): a bare
  number is a float, an integer is `{"int": 3}`. An optional third argument
  names the source file; without it the bridge finds the one file that binds
  the name at its top level.
- The edit is made to the text the running program holds for that file, the
  way `pb_source_set_value` would make it, and is taken up without
  recompiling
  ([../features/hot-reload.md](../features/hot-reload.md#setting-a-value-without-writing-the-file)
  has the rules).
- `source_text(file)` (`pb_vm_source_text`) returns that text. Writing it on
  release makes the following reload a no-op; reloading without writing it
  abandons the drag and brings the file's own values back.
- It returns `false` (`PB_ERR_NEEDS_RELOAD`) and changes nothing when the
  value has a different type or shape from the one written: an integer
  literal given `0.5`, a list of another length, a field that is not there.
- Like a reload, a successful call ends the lifetime of the last run's views,
  and the program's functions are callable again after the next `run()`.
- A static value built from the dragged literal is rebuilt on each tick,
  unless it is slow to build: then it waits until the drag pauses
  ([A slow table under a drag](../features/hot-reload.md#a-slow-table-under-a-drag)).
  Call `flush_static_rebuilds()` (`pb_vm_flush_static_rebuilds`) on release
  to rebuild at once; it returns how many values were waiting.
  `set_static_debounce(threshold_ms, quiet_ms)` (`pb_vm_set_static_debounce`)
  changes the two times from their defaults of 4 and 150, and a negative
  threshold rebuilds on every tick.

## Editing source: a script as a config file

A host with its own editing UI — an inspector, a panel of sliders — often
needs to write a change back into the `.ptl` file, so that the edit is real
code and hot reload carries it into the running program. `petal::Source`
(`pb_source_*`) does that without running anything, and without reformatting
the file:

```cpp
petal::Source src(read_file("games/neon/config.ptl"));

// What the file sets, as JSON: name, `config let` or not, the comment above
// it, the right-hand side as written, and its static value.
std::optional<std::string> bindings = src.bindings_json();

// Change values where they are written. Nothing else in the file moves.
// Values are JSON, in the format bindings_json() writes.
src.set_value("POST.exposure", "1.2");
src.set_value("POST.effects[2].amount", "0.35");
src.set_value("MOON_DIR[1]", "-0.8");        // into a call's arguments: vec3(…, -0.8, …)
src.set_value("NEON_PINK", R"({"color": "#ff2e88"})");
src.set_value("DROPS", R"({"int": 500})");

// Change the shape of a list or record, in place.
src.append("POST.effects", R"({"rec": {"effect": "halftone", "amount": 0.7}})");
src.insert("POST.effects[0]", R"({"rec": {"effect": "crt"}})");
src.remove("POST.effects[1]");
src.insert("POST.bloom", "0.4", "tint");     // a new field, before `tint`

// Or hand over a whole value: only what differs from the file is rewritten.
src.set_value("POST", post_as_json);
write_file("games/neon/config.ptl", src.text());
```

- A **path** is a top-level name followed by `.field` steps into record
  literals and `[index]` steps into list literals or call arguments. The name
  alone is the binding's whole right-hand side. A path can only see literals:
  it stops (`PB_ERR_NOT_FOUND`) at a name, a spread or a computed value.
- **The value edits** — `set_value`, `insert`, `append`, `remove` — take a
  value as JSON and edit the literal in place. What already reads as the new
  value is not touched, a changed number or color keeps the old token's
  spelling habits, and new elements are formatted like their siblings; the
  guarantees are spelled out in
  [../features/source-preservation.md](../features/source-preservation.md).
  What is particular to this API:
  - Int against float is the value's say: `{"int": 2}` writes `2`, a bare `2`
    writes `2.0`.
  - `set_value` on a record field that does not exist adds it. `insert` on a
    field that exists sets it. `remove` of a field that is not there succeeds.
  - `insert(path, json, before)` adds a record field before the sibling
    `before`, or last without one.
- `set(path, text)` is the lower-level form: it takes Petal source for exactly
  one expression and replaces exactly the expression the path names with that
  text, for a host that wants to decide the spelling itself. Setting a value
  to the text it already has changes nothing.
- `bindings_json()` is [`static_bindings`](config-files.md#reading-the-whole-file-including-what-it-cannot-read) as JSON (the
  format is documented at `pb_source_bindings_json` in `petal_bridge.h`).
  Bindings written [`config let`](../syntax/config-let.md) carry `"config": true`: those are the knobs
  the author declared, and the ones an editor should offer. Values are JSON:
  floats are numbers, strings strings, lists arrays, `nil` is `null`, and the
  rest are tagged — `{"int": 3}`, `{"rec": {"key": value}}` (keys in source
  order), `{"call": "vec3", "args": [...]}`, `{"color": "#ff2e88"}`, and for
  a variant of an enum the file declares `{"variant": "Mode.dark"}` (with
  `"args": [...]` when it carries a payload). A
  color is always `#rrggbb` or `#rrggbbaa` in lowercase there; `expr(path)`
  returns an expression as written (`#F80`, `0.020000`).
- A failed call returns false (the edits) or `nullopt` and leaves the text as
  it was; `error()` says why. The text is never left unparseable. A `Source`
  is independent of any `Vm`.

## Modules and packages

```cpp
vm.register_module("engine", engine_prelude_source);  // `import engine`
vm.register_package("kit", {{"kit", kit_src},          // a package from memory:
                            {"shapes", shapes_src}});  // `import kit`, `import kit/shapes`
vm.add_module_path("games/marble/lib");                // a directory of packages
vm.add_package("path/to/petal/core-runtime/bloom");     // a package directory
```

All of these affect later loads. Nothing is in a script's scope but the core
builtins: a script names what it uses (`import ui`, `import engine`, and
`import io` or `import vec2` for those builtins). `bloom`, `text_layout`
and `glsl` are bundled in the runtime, so `import bloom` needs no
registration; a package registered here wins over the bundled copy. How an
import is resolved is in [../module-system.md](../module-system.md), and
manifests, dependencies and the lookup order in
[../packages.md](../packages.md#where-a-package-name-is-looked-up).

For an engine that ships scripts without a filesystem, `petal bundle <entry>`
writes a project's installed dependency graph as JSON: each
`packages[i].modules` object is the name/source list `register_package`
takes, with the package's `petal.toml` among them
([../packages.md](../packages.md#bundles)). Scripts loaded from disk find
their project's git dependencies in `vendor/` or the package cache, as
`petal.lock` pins them; the bridge never fetches, so run `petal install` (or
check `vendor/` in) before shipping.

### Your natives need an interface package

A script reaches a native only through a `native fn` declaration in scope
([ffi.md](ffi.md#native-interfaces-native-fn)). So a host that registers
natives also registers the package that declares them, and its scripts
import it:

```cpp
vm.native("gravity_at", [](petal::Call& c) { c.result().value(-9.81 * c[0].number()); });
vm.emitter("spawn", "scene");
// Natives first, then the package: registering it gives them their
// declared parameter names (`gravity_at(frame: 3)`).
vm.register_package("game", {{"game", R"(
pub native fn gravity_at(frame: num) -> float
pub native fn spawn(kind: string, props: record)
)"}});
// The drift check, for a test: "" when every registered native is declared
// and every declared native is registered. Name every interface package the
// VM answers for; `ui` is petal-ui's.
assert(vm.verify_interface({"ui", "game"}).empty());
```

```petal ignore
import game
spawn("ball", {y: gravity_at(frame: 3)})
```

Keep the same source in a directory with a `petal.toml`
(`packages/game/petal.toml`, `packages/game/game.ptl`) and
`petal check -I packages script.ptl` checks your scripts with the stock CLI.
`pb_vm_add_package(vm, "packages/game", …)` registers it from that directory
instead of from memory. In C: `pb_vm_register_package`,
`pb_vm_verify_interface`, `pb_vm_set_check_only` (compile without linking,
to check a script written for another host) and
`pb_vm_set_inline_import_dir` (where `import ./x` leads for a program loaded
from a string).

`vm.override_file(path, source)` (`pb_vm_override_file`) compiles as if the
file at `path` held `source`: every later import that resolves to that file
reads the text given instead of the one on disk. A relative import is a file,
so a module registered in memory cannot stand in for it; this can. It is how
an engine applies a `--set NAME=VALUE` to a `config let` in an imported module
without writing the file. A reload compiles the override again, so edits to
the file itself do not reach the program while it stands. The entry file is
not an import: `load_source` / `reload_source` run other text in its place.

A script that uses a declared native the VM does not register fails at
`load_*` with `PB_ERR_COMPILE`, phase `link`, before any line runs.

**Manifests and required interfaces.** Script loaded from a file finds the
nearest `petal.toml` by walking up from it, and a `[dependencies]` entry there
(`game = { path = "../engine/packages/game" }`) is importable by its key with
no call on the VM — the same lookup the stock CLI and the language server do,
so they check the project's scripts with no flag. A package states the host
interfaces it needs under [`[requires]`](../packages.md#requires)
(`game = ">=0.2"`), and the VM says what it provides:

```cpp
vm.register_package("game", {{"game", game_src},
                             {"petal.toml", game_manifest}});  // the manifest rides along
vm.provides_package("game");        // at the version in that manifest
// or: vm.provides("game", "0.2.0");
```

In C: `pb_vm_provides(vm, name, version)` and
`pb_vm_provides_package(vm, name)`. A `pb_vm_create` VM already provides `ui`
and `core`. Loading a program whose packages require an interface the VM
does not provide, or provides at a version the requirement rejects, fails
with `PB_ERR_COMPILE`, phase `link`, before any line runs.

## Other tooling

- `call("fn_name", args)` calls a top-level Petal function after at least one
  `run()`. A function that does not exist is `PB_ERR_NOT_FOUND`; one that
  exists and fails is `PB_ERR_RUNTIME` (the bridge matches Petal's typed
  `CallError`, not the message text). `has_function("fn_name")` asks first,
  for optional hooks.
  A call that fails leaves nothing on the script's stack: the trace of a
  later failure lists that call's frames only, however many calls failed
  before it, and the same holds for the first call after a frame that
  errored.
- `call_json("fn_name", json)` is the same call with the arguments as JSON
  text, checked against the function's declaration: see
  [Calling with JSON arguments](#calling-with-json-arguments).
- `restart()` starts the loaded program over with empty `state`, without
  recompiling (a fresh stack on the same program).
- `state("name")` reads a top-level state variable as a view;
  `state_json()` dumps all state for reading (debug use only: it is keyed by
  display name and lossy). `save_state()` / `load_state()` are the pair that
  round-trips: see [Saving and loading state](#saving-and-loading-state).
- `take_output()` returns the lines the script `print`ed. Echo to stdout is
  off by default (`set_echo(true)` turns it on).
- `set_profiling(true)` / `profile_report(top_n)` (`pb_vm_set_profiling`,
  `pb_vm_profile_report`) turn on the VM profiler and read its report:
  opcodes, functions (self instructions and the time in the natives they
  call, as `name file:line`), natives by wall time, collections and the memo
  counters, all since profiling was turned on. Profiling roughly doubles
  script time, so for steady-state numbers turn it on after warm-up and read
  the report once ([../dev/performance.md](../dev/performance.md)). To
  measure boot instead, turn it on before `load_file()`: it may be set on a
  VM with nothing loaded, and the report then covers the first run, where a
  game builds its tables and `state` initializers execute.
  The report also lists native time by calling line, the native calls that
  took 1 µs or longer by calling line, and allocations by line;
  `set_profile_slow_native_us(us)` (`pb_vm_set_profile_slow_native_us`)
  moves that threshold.
- `set_memo(false)` / `memo()` (`pb_vm_set_memo`, `pb_vm_memo`) switch call
  memoization off or on for later runs (on by default, or as `PETAL_POLICY`
  says). Memo replays a call whose inputs repeat, which pays off for UI-like
  programs; a game whose inputs change every frame runs a few percent faster
  with it off. Output is the same either way.
- `set_static_cache(false)` / `static_cache()` (`pb_vm_set_static_cache`,
  `pb_vm_static_cache`) switch the static-value cache off or on for later
  runs (on by default, or as `PETAL_POLICY` says). With it on, a
  [static value](../features/static-values.md) is built by the first run
  after a load or a reload and kept for the runs after it. Output is the same
  either way. A native counts as
  constant only if it was registered with `PB_FX_CONST`, so a host's natives
  are dynamic until it says otherwise ([Constant natives](ffi.md#constant-natives)).
  `clear_static_values()` (`pb_vm_clear_static_values`) drops what is kept,
  for a host that changed something a `PB_FX_CONST` native answers from.
  `static_values_json()` (`pb_vm_static_values_json`) returns the counters
  (`slots`, `filled`, `skips`, `skipped_instructions`, `clears`), the values
  that could not be kept (`rejected`: one holding a function, a `var` cell,
  a handle or a pending result is rebuilt on every run), and `warnings`, the
  same in words and only those no earlier call handed out since the last
  load or reload: log them after a frame and each is said once.
- `diagnostics_text()` (`pb_vm_diagnostics_text(vm, errors_only)`) returns
  what the compile of the loaded program found and did not stop for, as
  `petal check` prints it (`""` when there is nothing; `errors_only`, the
  wrapper's default, leaves the warnings out). Print it after a load and
  after a reload. The error that matters is a `const` that is not constant:
  the program loads and runs, with that binding built on every run as a
  `let` is, and since the check goes by the natives this VM registered
  `PB_FX_CONST`, the host is the only place it can be reported.
  After a reload, print `new_diagnostics_text()`
  (`pb_vm_new_diagnostics_text`) instead: only the findings the edit
  introduced, warnings included, so a large program's standing warnings are
  not repeated on every save and the mistake just typed is seen at once. It
  is empty when the reload did not recompile, and `last_reload_json()` has
  the same list as `new_diagnostics`.
  `diagnostics_json()` (`pb_vm_diagnostics_json`) is the same list as data,
  warnings included: `{"errors": 0, "warnings": 1, "items": [{"severity":
  "warning", "message": "...", "file": "game.ptl", "line": 3, "column":
  5}]}`.
- `pb_version()` names the bridge, petal-ui contract and prelude level, and
  petal-query protocol version.

## Calling with JSON arguments

A host that takes commands from outside (an agent's tool call, a debug
console, a socket) has its arguments as JSON. `call_json`
(`pb_vm_call_json`) calls a top-level function with them directly:

```petal no-check
fn toll(id: string, cents: int = 25, rate: float = 0.5)
  ...
end
```

```cpp
vm.call_json("toll", R"({"id": "north", "rate": 2})");   // by parameter name
vm.call_json("toll", R"(["north", 40, 0.75])");          // by position
vm.call_json("status");                                  // no arguments
std::string schema = vm.function_json("toll");           // its parameters
```

- **An object is a call by name**: each key is a parameter, in any order, and
  a parameter with a default may be left out. This is the language's own
  named-argument binding (`toll(id: "north", rate: 2)`), so its errors are the
  language's: an unknown name, a parameter given twice or a required one
  missing is `PB_ERR_RUNTIME` naming it. **An array is a call by position.**
- **The declared types settle what JSON leaves open.** JSON has one number
  type, and a JavaScript client writes the float `3.0` as `3`. The bridge
  reads the function's declaration (the data `fn_info()` returns) and
  converts per argument:

  | Declared | Sent | The function receives |
  |----------|------|-----------------------|
  | `float` | a whole number (`3`) | the float `3.0` |
  | `int` | a float with no fraction (`3.0`, `1e3`) | the int |
  | `int` | any other float (`3.5`) | nothing: `PB_ERR_INVALID_ARG` |
  | `int`, `float`, `num` | not a number | nothing: `PB_ERR_INVALID_ARG` |
  | `string`, `bool`, `list`, `record` | another kind | nothing: `PB_ERR_INVALID_ARG` |
  | any other type, or none | anything | it as sent |

  The error names the function, the parameter, the declared type and what
  arrived (`toll(): parameter 'cents' is declared int, got a string
  ("abc")`), and the function is not run, so a tool does not have to
  validate its own arguments. `null` always passes, as `nil`. Only the
  argument itself is looked at, not what a list or record holds, and an
  overloaded function (resolved by argument count) takes its arguments as
  sent. Without an annotation nothing is converted: a whole number is an
  int, as everywhere else JSON enters Petal.
- **Values are spelled as in a state document** (below): plain JSON is
  itself, and `{"$petal": "enum", "tag": "Kind.road", "data": [2]}` is how a
  caller sends what JSON has no spelling for.
- `function_json("toll")` (`pb_vm_function_json`) returns the parameters a
  host needs to describe the function to a caller: `{"name": "toll",
  "params": [{"name": "cents", "type": "int", "has_default": true,
  "default_source": "25"}, ...]}`. `type` and `default_source` are `null`
  where the declaration has none; `params` is `null` for an overloaded
  function.

`call` with a builder is unchanged: a host that builds `pb_builder_int(3)`
has said what it means, and nothing is converted.

## Running the script's tests

A script's `test "title" do … end` blocks ([testing.md](testing.md)) run under
the host, with its natives present. `petal test` cannot do that for a script
that calls a host native.

```cpp
vm.run();                                   // functions exist once the program has run
std::string tests = vm.tests_json();
// [{"name": "the door opens", "function": "__test_0", "module": null,
//   "file": "game.ptl", "path": "/abs/game.ptl", "line": 12}]

vm.test_begin("__test_0");
for (;;) {
    pb_test_step step = vm.test_resume();   // or test_resume(max_steps)
    if (step == PB_TEST_SUSPENDED) { run_frames(wanted); continue; }
    break;
}
std::string result = vm.test_result_json();
// {"function": "__test_0", "name": "the door opens", "passed": false,
//  "checks": 3, "failed": 1, "error": null,
//  "failures": [{"check": "expect_eq", "file": "game.ptl", "line": 14, "column": 3,
//                "message": "expected 3, got 4", "actual": "4", "expected": "3"}]}
```

| C | C++ | |
|---|---|---|
| `pb_vm_tests_json` | `vm.tests_json()` | The program's tests: the entry file's and every module's (`"module"` is null for the entry file) |
| `pb_vm_test_begin(vm, function)` | `vm.test_begin(f)` | Start one. The program must have completed a run; one test at a time |
| `pb_vm_test_resume(vm, max_steps)` | `vm.test_resume(n = 0)` | Run until `PB_TEST_FINISHED`, `PB_TEST_SUSPENDED` or (with a bound) `PB_TEST_OUT_OF_STEPS` |
| `pb_vm_test_abort(vm, why)` | `vm.test_abort(why)` | Give up on it; the result is what it recorded, with `why` as its error |
| `pb_vm_test_result_json` | `vm.test_result_json()` | The last finished test's result |
| `pb_vm_run_test(vm, function)` | `vm.run_test(f)` | Begin and resume in one call; returns the result |
| `pb_vm_test_in_progress` | `vm.test_in_progress()` | The function of the test begun and not finished |
| `pb_vm_take_expect_failures_json` | `vm.take_expect_failures_json()` | Expectations that failed outside tests, in ordinary runs and host calls, since the last take |

**A test that spans frames.** Register a native that notes how long to wait
and suspends the test; `test_resume` then answers `PB_TEST_SUSPENDED`, the
host runs its frames exactly as it always does, and the next `test_resume`
carries on from the line after the call:

```cpp
vm.native("wait_frames", petal::fx::Effect, [&](petal::Call& c) {
    if (!c.suspend()) return;     // not in a test the host is stepping: the call fails
    wanted = int(c.arg(0).integer());
});
```

`c.suspend()` (`pb_call_suspend`) fails in an ordinary run, in a function
called with `pb_vm_call`, and inside a callback of
`map`/`filter`/`reduce`/`forEach`/`sort`. A test run with `pb_vm_run_test`
has nobody to resume it: if it suspends it is aborted, with an error that
says it needs frames. After a resume, a `var`
or `state var` read with `get` is live; a `let` or plain `state` the test
captured is the value from when it began.

**A host's own expectations.** `c.expect(ok, message, check)`
(`pb_call_expect`) records a check as the script's `expect` does, at the
position of the script call, and `c.location()` (`pb_call_location`) gives
that position (file, line, column) to any native that wants to say where it
was called from.

**Isolation is the host's.** A test sees the program as the last run left it
and what it changes stays changed. For a fresh program per test,
`vm.restart(); vm.run();` before each `test_begin`, or save the state once
and `load_state` + `run` before each. A test's function is an ordinary entry
of the function table (`vm.has_function("__test_0")` is true), so a host that
lists a script's functions should leave out the names that start with
`__test_`. A reload while a test waits ends that test with an error.

## Saving and loading state

`save_state()` (`pb_vm_save_state`) returns the program's whole `state` as
one JSON document, and `load_state(document)` (`pb_vm_load_state`) puts it
back. That is a save point, an undo step, a fork, or a session that outlives
the process, without replaying anything:

```cpp
std::string point = vm.save_state();       // between two runs
...                                        // frames, input, tool calls
vm.load_state(point);                      // back to the save point
vm.run();                                  // this frame sees the loaded state

petal::Vm fork;                            // or carry on somewhere else
fork.load_file("game.ptl");
fork.load_state(point);
```

`save_state_file(path)` / `load_state_file(path)` do the same through a
file. The format is the one `petal run --state-storage` writes
([CLI.md](CLI.md#state-between-runs)): `"format": "petal-state"`, a version,
and one entry per slot.

- **Every slot is in it**: top-level and module state, and the slots that a
  `state` inside a function, a loop or a `state(key)` group owns, one per
  call path.
- **Loading replaces the state.** A slot the document does not hold is
  removed and starts from its initial value on the next run, which is what
  an undo wants: state that first appeared after the save point is gone
  again. A document with no slots clears the state.
- **Values keep their types.** Ints and floats stay apart, records keep
  their field order, and enum variants, class instances, `vec2`/`vec3`, f64
  arrays, symbols and `inf`/`nan` come back as themselves (written as
  objects tagged `"$petal"`).
- **The program may have changed.** Slots are matched by the ids a hot
  reload uses, so a document saved before a code edit loads after it under
  the reload's rules: state whose declaration was renamed or removed is not
  loaded, and newly declared state starts from its initializer.
- **A document that cannot be used is `PB_ERR_INVALID_ARG` and changes
  nothing**: not JSON, not a state document, a version this build does not
  read, or slots of which none is declared by the loaded program (it was
  saved by a different script).

What a document does not carry:

| Not saved | What happens |
|-----------|--------------|
| A value that is or holds a function, a host handle, a pending result or a UI element | The whole slot is left out. After a load, that `state` starts from its initializer. |
| Whether a value was frozen (`freeze()`) | It comes back equal and unfrozen. |
| Anything that is not `state` | Bindings, output buffers, input, petal-ui's registries and whatever your natives keep are the host's to save. |

`state_report_json()` (`pb_vm_state_report_json`) says what the last save or
load did, and is where the host learns about the first row:

```json
{"op": "save", "slots": 12, "skipped": [{"name": "on_done", "holds": "a function"}]}
{"op": "load", "slots": 11, "dropped": ["old_name"]}
```

Two things to know when loading into a VM that has already run:

- **Run a frame before calling into the script.** A function reads the
  top-level state its closure captured at the last run, so `call()` between
  a `load_state()` and the next `run()` still sees the state from before the
  load.
- **A handle in state does not survive**, by the first row above. Keep the
  durable part in `state` (an id, a path) and look the handle up again.

Saving costs a walk of the state and a JSON encode (about what
`state_json()` costs); it is meant for save points, not for every frame.

A hot reload follows the same rule about functions: a `state` that holds one
is dropped by a reload that recompiles or patches the program (its closure
belonged to the old one), and its initializer runs again.

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

To let scripts pass a native's arguments by name, declare them once after
registering it (a callback or an emitter):

```c
pb_vm_declare_native_params(vm, "spawn", "kind, x, y, speed?");
/* script: spawn(kind: "spark", x: 10, y: 20) */
```

The list is in the order the callback reads its arguments; `?` marks a
trailing optional one. Names are put in that order before the callback runs,
so it still reads by index. Without a declaration a named argument is a script
error. In C++: `vm.native_params("spawn", "kind, x, y, speed?")`. See
[Native functions](ffi.md#native-functions) for the rules.

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
0.1 s, most of it compiling the `ui` package.

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

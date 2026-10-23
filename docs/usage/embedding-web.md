# Embedding in a web page

Petal runs in the browser as WebAssembly. Two wasm builds exist, each a
`PetalRuntime` class that `wasm-bindgen` exposes to JavaScript, wrapping one
`Env` ([ffi.md](ffi.md)) with one active program and stack:

| Runtime | Built from | What a script does | Used by |
|---|---|---|---|
| canvas | `integrations/petal-web-canvas/rust/src/lib.rs` | `import ui`: draws, reads pointer and keys, every frame | petal-web-canvas, the diagram canvas, the petal-lang.org snippets |
| element tree | `core/src/wasm.rs` (the core crate, `--features wasm`) | `import web`: returns an element tree, re-run on each click | petal-web-html |

Each integration also ships a TypeScript class that drives its runtime
(`PetalCanvas`, `PetalWeb`), which is what most pages use. This page is the
`PetalRuntime` API under them, for a page that drives the runtime itself.

## Building

```bash
integrations/petal-web-canvas/build-wasm.sh   # wasm-pack build of rust/ into petal-web-canvas/pkg/
integrations/petal-web-html/build-wasm.sh     # wasm-pack build --features wasm of core/ into petal-web-html/pkg/
```

Both use `wasm-pack build --target web`, so the module is an ES module with a
default `init()` export to await before constructing a runtime. Projects
outside this repository (such as the website) vendor a built copy and see a
change only when it is rebuilt and vendored again. Each build writes
`pkg/BUILD_INFO` (`commit=`, `dirty=`, `profile=`, `built=`), so a vendored
copy can say which Petal it is.

Both builds use LTO. `build-wasm.sh --size` also compiles at
`opt-level = "z"`, for a page that cares more about the download than about
frame time. Measured on petal-web-canvas, 2026-10-23, median frame time in
Node:

| build | raw | gzip | flow_field frame | snake frame |
|---|---|---|---|---|
| default | 3.69 MB | 1.29 MB | 6.0 ms | 1.4 ms |
| `--size` | 2.26 MB | 0.85 MB | 9.3 ms | 2.7 ms |

`opt-level = "s"` falls between them (2.71 MB, 1.00 MB gzip, frames ~1.3x
slower). The rest of the size is mostly core's own code; the embedded
library sources (the ui prelude, bloom, glsl, text_layout) are about 300 KB.

`tools/wasm-smoke.ts` runs scripts through both built pkgs in Node, a few
frames each; CI runs it after the builds. A Rust panic inside the runtime is
logged to the console (message, location and stack) before the page sees
`RuntimeError: unreachable`, and leaves that runtime unusable.

## The lifecycle

```ts
import init, { PetalRuntime } from "./pkg/petal_web_canvas.js";

await init();
const rt = new PetalRuntime();                  // natives and `ui` registered
rt.register_package("kit", JSON.stringify(kitModules)); // libraries, before load
const pid = rt.load_program(source);            // compile; throws on an error
const sid = rt.create_stack(pid);

function frame(now: number) {
  rt.set_frame_info(dt, now / 1000, frameCount, width, height);
  rt.begin_frame();                             // promote input edges
  if (rt.frame_needed(sid)) {                   // the frame gate
    rt.reset_and_run(sid);                      // state persists
    paint(JSON.parse(rt.take_draw_commands()));
  }
  for (const line of JSON.parse(rt.take_output())) console.log(line);
  requestAnimationFrame(frame);
}
```

Every method that can fail throws a JavaScript exception whose message is the
Petal error text, positions included. A page catches it around `load_program`
and `reset_and_run`, shows it, and keeps the last good frame on screen.
`PetalCanvas.runFrame` does exactly this.

## Methods both runtimes have

| Method | Does |
|---|---|
| `new PetalRuntime()` | an `Env` with this runtime's natives and interface package registered and provided |
| `register_module(name, source)` | an in-memory module: later programs can `import name` |
| `register_package(name, modulesJson)` | a whole package from memory. `modulesJson` is a JSON object of package-relative module name (`menu`, `ui/menu`) to source, with an optional `"petal.toml"` entry: the form `petal bundle` emits ([packages.md](../packages.md#bundles)). Throws on a bad manifest |
| `provides(name, version)` | announce a host interface, so a package whose `[requires]` names it loads |
| `provided_interfaces()` | JSON object of interface name to version; always includes `core` |
| `verify_interface(packages)` | `packages` is comma-separated; `""` when every declared native is registered and every registered one declared, else what differs |
| `load_program(source)` | compile and make it the active program; returns its id |
| `create_stack(programId)` | a stack for it, made the active stack; returns its id |
| `run(stackId)` | run once; returns the program's result as JSON |
| `reset_and_run(stackId)` | reset the stack, keeping `state`, and run; returns the result as JSON |
| `take_output()` | JSON array of the `print` lines since the last call |

There is no filesystem, so `load_program` takes the source with no path: an
`import ./x` reaches only a module registered in memory as `x`, and every
library arrives through `register_module` or `register_package`
([module system](../module-system.md#embedding)). A program is not reloaded
in place: loading new source makes a new program and stack, and `state` starts
over unless the page carries it across with `get_state_json` /
`set_state_json`.

## The canvas runtime

Registers the whole petal-ui layer (input, draw commands, offscreen canvases,
`host_data` and the `ui` package) and provides `ui`, so `bloom` and
`text_layout` load as well.

**Each frame**

| Method | Does |
|---|---|
| `set_frame_info(dt, time, frame, width, height)` | stage this frame's `dt()`, `time()` (absolute seconds, read fresh from `performance.now()`, never a sum of `dt`), `frame_count()` and `screen_width()`/`screen_height()` |
| `begin_frame()` | advance the input clock by `dt` and promote the input edges since the last frame. Call after `set_frame_info`, before running |
| `frame_needed(stackId)` | the [frame gate](../dev/frame-gate.md): binds the frame's inputs and answers whether a run could differ from the last. `false` means the last frame's commands are this frame's |
| `invalidate_frame(stackId)` | make the next `frame_needed` answer `true` (a resize, new fonts, host data the runtime cannot see) |
| `reset_and_run(stackId)` | clear the draw buffer and canvas ids, bind the frame's inputs, reset and run |
| `take_draw_commands()` | JSON array of the frame's draw commands, petal-ui's `DrawCommand` shape (`{"op": …}`), for the renderer ([`ui` host contract](../ui-host-contract.md)) |

**Input**, called as browser events arrive; `InputTracker` in
`petal-web-canvas/src/input.ts` does the mapping:

| Method | Does |
|---|---|
| `set_mouse_position(x, y)` | pointer position in canvas pixels |
| `set_mouse_button(button, down)` | a DOM `MouseEvent.button` (0 left, 1 middle, 2 right), mapped to petal-ui's ids so `mouse_down(0)` is the left button on every host |
| `scroll(dx, dy)` | wheel delta in lines; fractions carry over |
| `set_key_state(key, down)` | `key` must already be a canonical petal-ui key name |
| `type_text(text)` | this frame's typed text, read by `text_input()` |

Events latch until `begin_frame`, so a click that goes down and up between two
frames still arrives as a `mouse_pressed` edge.

**Text metrics.** The browser draws the text, so the browser measures it: the
page measures glyphs with `measureText` and hands the ratios down, so
`text_width` and `text_metrics` agree with what `fillText` draws
(`bindFontMetrics` in `src/text-metrics.ts`). `set_default_font_metrics(advances,
fallback)` and `set_default_font_vertical(baseline, descent, line_height,
cap_height, x_height)` cover the default face, `set_font_metrics(name, …)` a
named one (a role such as `ui` or a variant key such as `ui@700i`), and
`set_default_font_name(name)` says which role the default is. All values are
ratios of the font size. See [text and fonts](text-and-fonts.md).

**Debugging**

| Method | Does |
|---|---|
| `get_state_json()` | the active stack's `state` as a JSON object |
| `set_state_json(name, json)` | set one top-level `state` by name; throws for a name the program has no `state` for |
| `run_speculative()` | run a fork of the active stack once with this frame's inputs and return its draw commands as JSON, leaving the live state alone ([time travel](../features/time-travel.md)) |

`PetalCanvas.setProp(name, value)` is built on `set_state_json`: it stages a
JSON value and writes it into the like-named top-level `state` just before the
next frame, so a page can feed a script data without a native
([petal-web-canvas README](../../integrations/petal-web-canvas/README.md#host-to-script-data-feed)).

## The element-tree runtime

Registers two natives, declared by the `web` package (`core/packages/web`),
which a script imports:

| Native | Script side |
|---|---|
| `next_id()` | a unique element id; ids restart at 1 on every `run` |
| `clicked(id)` | whether the element with that id is the one clicked |

The program's result is the element tree, which `run` / `reset_and_run`
return as JSON for the page to turn into DOM (`src/renderer.ts`). There is no
frame loop: the page runs the program once, and again on each click, after
`set_clicked_id(id)` tells it which element was clicked (`0` for none).
`PetalWeb.handleEvent` sets the id, re-renders and clears it.

## What a web host does not have

- **Hot reload.** No `reload_program`; load the new source instead.
- **The C bridge's extras.** No scenarios, no `call`, no saved state beyond
  `get_state_json`.
- **Fetching.** No pending-value fetcher; `host_data` is the pull channel
  ([pending values](../features/pending-values.md)).
- **Natives of your own from JavaScript.** Both runtimes register their
  natives in Rust. A page that needs another native builds its own runtime
  crate the way `petal-web-canvas/rust` does, on [ffi.md](ffi.md).

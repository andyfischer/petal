# petal-web-canvas

Run Petal scripts that draw interactive graphics into an HTML canvas, in the
browser.

The Petal runtime runs as a WebAssembly module loaded by Vite. Each animation
frame the host:

1. feeds mouse and keyboard events into the WASM runtime,
2. resets the script's stack and re-executes it,
3. drains the queued draw commands and plays them back through
   `CanvasRenderingContext2D`.

The runtime shares the standard interactivity layer, [petal-ui](../../petal-ui/),
with [petal-desktop-sdl](../petal-desktop-sdl/) and Garden: the same input
contract, draw-command vocabulary, offscreen canvases, and `ui` prelude, so
petal-sdl sample scripts run unchanged. Browser events are translated to
`InputEvent`s as they arrive and latched by `InputState`, so a press edge
(`mouse_pressed`, `key_pressed`) fires even for a click that goes down and up
between two frames.

## Development

```bash
./build-wasm.sh      # one-time (and after Rust changes): wasm-pack build of rust/ into pkg/
npm install
npm run dev          # Vite dev server on http://localhost:4017
npm run build        # production build to dist/
```

Requires Rust and `wasm-pack` (`cargo install wasm-pack`). `build-wasm.sh`
builds the crate in `rust/` (which depends on `../../petal-ui` and the Petal
core) and copies the output into `pkg/`; `npm run build:wasm` is the same
step.

Vite serves `.ptl` files as `text/plain`, so `src/main.ts` fetches them and
hands the source to the runtime.

## The script API

The full vocabulary is petal-ui's; see the [petal-ui README](../../petal-ui/README.md)
and, for the draw calls, Garden's
[panel reference](../../garden/docs/petal-graphical-panels.md#draw-surface).
The essentials:

```
clear(r, g, b)
draw_rect(x, y, w, h, r, g, b)
draw_rect_outline(x, y, w, h, r, g, b)
draw_line(x1, y1, x2, y2, r, g, b)
draw_circle(cx, cy, radius, r, g, b)
draw_text(text, x, y, size, r, g, b)

# Offscreen canvases (layers and compositing)
let c = create_canvas(w, h)   # returns a canvas handle
draw_to(c)                    # redirect drawing into the canvas (returns the previous target)
draw_to_screen()              # redirect back to the main canvas
draw_canvas(c, x, y)          # blit the offscreen canvas onto the current target
draw_canvas(c, x, y, a)       # at opacity a (0–255): group opacity for the whole layer
snapshot_to(c, x, y)          # copy the current target's pixels under the canvas rect into c
blur_canvas(c, radius)        # Gaussian-blur c in place (CSS blur() semantics)

# The prelude's layer helpers built on them
layer(rect, fn() ... end)                 # draw the body into a canvas, composite it at rect
layer(rect, {a: 128, blur: 4}, fn() ... end)
draw_backdrop_blur(rect, radius)          # blur what is already under rect
draw_material(rect, {kind: "regular", radius: 12, tint: #ffffff})  # translucent bar
```

Snapshot and blur use the 2D context's own `drawImage` and `filter: blur()`,
so the backdrop material looks the same here as on the GPU host.

Input:

```
mouse_x(),  mouse_y()
mouse_down(button),  mouse_pressed(button),  mouse_released(button)   # 0=left, 1=right, 2=middle
key_down("space"),   key_pressed("up"),      key_released("a")        # key name map in src/input.ts
scroll_x(),  scroll_y()                      # wheel/trackpad lines this frame
text_input()                                 # typed text this frame
```

Drag, click count, and modifiers are also available; see
[petal-ui/src/input.rs](../../petal-ui/src/input.rs).

Frame info: `dt()` (seconds since the last frame), `frame_count()`,
`screen_width()`, `screen_height()`.

## Host-to-script data feed

A host can push named data into a running script: app state, a fetched
record, sensor values, anything JSON-serializable. The host owns the value,
the script reads it.

```ts
const canvas = new PetalCanvas();
await canvas.init();
canvas.start(canvasEl);
canvas.load(source);

// Push a prop whenever it changes (deduplicated by value; safe to call every frame):
canvas.setProp("cubeState", cube);      // any JSON value
canvas.setProps({ score, level });      // several at once

// Read script-owned state back (for debug panels or two-way sync):
const { score } = canvas.getState();
```

The script reads the prop as a like-named `state` variable:

```
state cubeState = {}          # the host overrides this default on frame 1
draw_from(cubeState)
```

Each prop is flushed into committed state just before the frame runs, so a
value set before the first frame wins over the `state x = <default>`
initializer and the script never flashes a default. Props are one-way; if
the script also writes the same `state` variable, the host only re-pushes
when its own value changes. A prop with no matching `state` declaration is
skipped with a one-time console warning.

Props bind to top-level `state` declarations only. A `state` inside a
function is keyed by the call path that reaches it, so it has no bare name:
`getState()` returns it under a pathed key like `counter#1/count`, and
`setProp` cannot address it. Keep host-driven values at top level.

## Examples

`examples/*.ptl`, listed in `src/main.ts`:

- `bouncing_balls.ptl`: gravity and walls; click to add balls
- `paint.ptl`: drawing app with palette and brush size
- `starfield.ptl`: 3D star projection with motion trails
- `flow_field.ptl`: particles steered by a layered noise field
- `snake.ptl`: arrow-key snake game
- `typography.ptl`: text sizes, weights, and measurement

Add a script by dropping it into `examples/` and listing it in `src/main.ts`.

## Notes

- A WASM panic (for example, converting a non-finite float to int) traps with
  `RuntimeError: unreachable` and poisons the module; reload the page to
  recover. Keep coordinates bounded.
- `random()` is seeded deterministically on this target because
  `wasm32-unknown-unknown` has no system clock. Sequences repeat across page
  reloads but differ across calls within a session.

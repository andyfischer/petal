# petal-web-canvas

Run Petal scripts that draw interactive graphics into an HTML canvas, in the browser.

The Petal compiler runs as a WebAssembly module loaded by Vite. Each frame, the
canvas event loop:
1. feeds mouse + keyboard events into the WASM runtime
2. resets the script's stack and re-executes it
3. drains the queued draw commands and plays them back through `CanvasRenderingContext2D`

The runtime shares the standard interactivity layer (`petal-ui`) with
[petal-sdl](../petal-sdl/): the same input contract, draw-command vocabulary,
offscreen canvases, and `ui` prelude. So petal-sdl sample scripts run unchanged.
Browser events are translated to `petal_ui::input::InputEvent`s as they arrive
and latched by `InputState`, so a press edge (`mouse_pressed` / `key_pressed`)
fires even for a click that goes down and up between two animation frames.

## Drawing API

```
clear(r, g, b)
draw_rect(x, y, w, h, r, g, b)
draw_rect_outline(x, y, w, h, r, g, b)
draw_line(x1, y1, x2, y2, r, g, b)
draw_circle(cx, cy, radius, r, g, b)
draw_text(text, x, y, size, r, g, b)

# Offscreen canvases (PGraphics-style layers / compositing)
let c = create_canvas(w, h)   # returns a canvas handle
draw_to(c)                    # redirect drawing into the canvas (returns the previous target)
draw_to_screen()              # redirect back to the main canvas
draw_canvas(c, x, y)          # blit the offscreen canvas onto the current target
draw_canvas(c, x, y, a)       # …at opacity a (0–255): group opacity for the whole layer
snapshot_to(c, x, y)          # copy the current target's pixels under the canvas rect into c
blur_canvas(c, radius)        # Gaussian-blur c in place (CSS blur() semantics)

# …and the prelude's layer helpers built on them:
layer(rect, fn() ... end)                 # draw the body into a canvas, composite it at rect
layer(rect, {a: 128, blur: 4}, fn() ... end)
draw_backdrop_blur(rect, radius)          # blur what is already under rect
draw_material(rect, {kind: "regular", radius: 12, tint: #ffffff})  # iOS-style translucent bar
```

Snapshot and blur use the 2D context's own `drawImage` and `filter: blur()`,
so the backdrop material looks the same here as on the GPU host.

## Input

```
mouse_x(),  mouse_y()
mouse_down(button),  mouse_pressed(button),  mouse_released(button)   // button: 0=left, 1=right, 2=middle
key_down("space"),   key_pressed("up"),      key_released("a")        // see input.ts for the key name map
scroll_x(),  scroll_y()                      // wheel/trackpad lines this frame
text_input()                                 // typed text this frame
```

The full `petal-ui` input vocabulary (drag, click-count, modifiers, …) is also
available — see [`petal-ui/src/input.rs`](../../petal-ui/src/input.rs).

## Frame info

```
dt()                // seconds since last frame
frame_count()       // monotonic frame counter
screen_width(),  screen_height()
```

## Development

```bash
# One-time: build the WASM module
./build-wasm.sh

# Install JS deps
npm install

# Dev server (port 4017)
npm run dev
```

Vite serves `.ptl` files as `text/plain`, so `main.ts` fetches them and hands
the source to the runtime.

## Host → script data feed

Beyond input events, a host can push arbitrary named data into a running script
— app state, a fetched record, sensor values, anything JSON-serializable. This
is the "controlled prop" model: the host owns the value, the script reads it.

```ts
const canvas = new PetalCanvas();
await canvas.init();
canvas.start(canvasEl);
canvas.load(source);

// Push a prop whenever it changes (dedup'd by value — safe to call every frame):
canvas.setProp("cubeState", cube);      // any JSON value
canvas.setProps({ score, level });      // several at once

// Read script-owned state back (for debug panels / two-way sync):
const { score } = canvas.getState();
```

The script reads the prop as a like-named `state` variable:

```
state cubeState = {}          # host overrides this default on frame 1
draw_from(cubeState)
```

Each prop is flushed into committed state just before the frame runs, so a
value set before the first frame wins over the `state x = <default>`
initializer — the script never flashes a default. Props are one-way (host →
script); if the script also writes the same `state` var, the host only
re-pushes when its own value changes. A prop with no matching `state`
declaration is skipped with a one-time console warning.

Under the hood this is `PetalRuntime.set_state_json` — no per-frame recompile,
no WASM reload.

Props bind to **top-level** `state` declarations only. A `state` inside a
function is keyed by the call path that reaches it (one slot per callsite / loop
iteration), so it has no bare name: `getState()` returns it under a pathed key
like `counter#1/count`, and `setProp` cannot address it — that key is skipped
with the same warning as any unmatched prop. Keep host-driven values at top
level, which is where every prop-bound declaration already lives.

## Examples

`examples/*.ptl`:
- `bouncing_balls.ptl` — gravity + walls, click to add balls
- `paint.ptl` — drawing app with palette and brush size
- `starfield.ptl` — 3D star projection with motion trails
- `flow_field.ptl` — particles steered by a layered noise field
- `snake.ptl` — arrow-key snake game

Add new scripts by dropping them into `examples/` and listing them in
`src/main.ts`.

## Notes

- WASM panics (e.g. converting a non-finite float to int) trap with
  `RuntimeError: unreachable` and poison the module — reload the page to
  recover. Keep coordinates bounded.
- The PRNG used by `random()` is seeded deterministically on this target
  because `wasm32-unknown-unknown` has no system clock. Sequences repeat
  across page reloads but differ across calls within a session.

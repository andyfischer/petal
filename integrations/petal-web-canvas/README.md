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
draw_to(c)                    # redirect drawing into the canvas
draw_to_screen()              # redirect back to the main canvas
draw_canvas(c, x, y)          # blit the offscreen canvas onto the current target
```

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

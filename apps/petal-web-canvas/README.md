# petal-web-canvas

Run Petal scripts that draw interactive graphics into an HTML canvas, in the browser.

The Petal compiler runs as a WebAssembly module loaded by Vite. Each frame, the
canvas event loop:
1. feeds mouse + keyboard state into the WASM runtime
2. resets the script's stack and re-executes it
3. drains the queued draw commands and plays them back through `CanvasRenderingContext2D`

The drawing API is the same as [petal-sdl](../petal-sdl/), so most petal-sdl
sample scripts run unchanged.

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
mouse_down(button),  mouse_pressed(button)   // button: 1=left, 2=middle, 3=right
key_down("space"),   key_pressed("up")       // see input.ts for the key name map
```

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

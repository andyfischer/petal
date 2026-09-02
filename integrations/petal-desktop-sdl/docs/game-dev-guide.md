# Petal Game Development Guide

How to write a game or sketch for `petal-sdl`. The drawing and input
functions here come from `petal-ui`, so the same script also runs under
`petal-web-canvas` in the browser.

## Getting started

### Running a game

```bash
cd integrations/petal-desktop-sdl
LIBRARY_PATH=/opt/homebrew/lib cargo run -- examples/pong.ptl   # LIBRARY_PATH on macOS/Homebrew
```

Options:

- `--width <n>` / `--height <n>` — window size (default 800 x 600)
- `--title <str>` — window title
- `--no-hot-reload` — disable live reloading
- `--agent` — accept JSON commands on stdin (see [agent-protocol.md](agent-protocol.md))
- `--headless` — no window; frames advance on `step` commands (implies `--agent`)
- `--screenshot <file> --frames <n>` — run N frames headlessly, save a PNG, exit

### How it works

Your `.ptl` file runs every frame (about 60 fps). The engine runs the whole
script top to bottom each time. Use `state` variables to keep data between
frames.

```petal
state x = 100.0          // initialized once, persists across frames
x += 100.0 * dt()        // move 100 pixels per second
draw_rect(int(x), 100, 20, 20, 255, 0, 0)
```

### Where a `state` cell lives

A `state` slot is identified by its declaration and by the call path that
reached it: the chain of callsites and loop iterations from the top of the
file down to the declaration.

- **At the top level**, which is what every example in this guide uses, the
  path is empty. `state score = 0` is *the* score, readable and writable from
  anywhere in the file.
- **Inside a function**, each callsite gets its own cell, and a call made
  inside a `for` or `while` gets one cell per iteration:

  ```petal
  fn enemy(x)
    state hp = 100                       // one cell per caller, automatically
    draw_rect(int(x), 200, 16, 16, 255, 0, 0)
  end
  enemy(100.0)                           // its own hp
  enemy(300.0)                           // its own hp
  for i in range(0, 8) do
    enemy(float(i) * 40.0)               // eight more, one cell each
  end
  ```

  Each enemy gets its own health with no manual keying.
- **`state(expr) name = ...` overrides the path.** An explicit key is
  absolute: every callsite that asks for the same key value reaches the same
  cell. Use it when a cell belongs to a domain object rather than a position.
  `state(e.id) hp = 100` follows the enemy when the list is reordered; the
  unkeyed form stays with the loop index.
- Cells shared across functions on purpose go at the top level as
  `state var`, read and written with `get` / `set`.

See the [language guide](../../../docs/language-guide.md#state) for the full
rules.

### Hot reload

Edit your `.ptl` file while the game is running. Changes apply immediately
and `state` variables keep their values as long as they keep their names.

A `state` inside a function also needs its call path to survive the edit.
The callsite id comes from the callee's name and its position among calls to
that same name in the enclosing function, never from line numbers. Editing
elsewhere in the file is free, but renaming the called function, or inserting
an earlier call to it in the same function, moves those cells and they
re-initialize. Orphaned cells are swept after the next frame.

## Game API reference

### Drawing

Coordinates are in pixels with the origin at the top-left. Colors are RGB
integers 0-255.

```petal
clear(r, g, b)                              // fill the background
draw_rect(x, y, width, height, r, g, b)     // filled rectangle
draw_rect_outline(x, y, w, h, r, g, b)      // rectangle outline
draw_line(x1, y1, x2, y2, r, g, b)          // line segment
draw_circle(cx, cy, radius, r, g, b)        // filled circle
fill_triangle(x1, y1, x2, y2, x3, y3, r, g, b)
fill_poly(points, r, g, b)                  // points: list of vec2 or [x, y] pairs
draw_text(text, x, y, font_size, r, g, b)   // text string
```

`petal-ui` has more: rounded rectangles, ellipses, arcs, gradients, shadows,
clipping, images, and the `ui` widget prelude. See
[`petal-ui/README.md`](../../../petal-ui/README.md).

### The canvas persists between frames

The framebuffer is only wiped when you call `clear()`. Games call `clear(...)`
at the top of every frame and start from a blank screen.

For generative art where the accumulated trace *is* the art (attractors,
Lissajous figures, particle trails, brush strokes), don't call `clear()`
every frame. Clear once on the first frame, guarded by a `state` flag, then
let the image build up:

```petal
state started = false
if !started then
  clear(0, 0, 0)   // paint the background once
  started = true
end

// Each frame draws a few more dots that stay on screen.
draw_circle(int(x), int(y), 2, 255, 200, 80)
```

See `examples/cc_lissajous_trails.ptl`.

### Offscreen canvases

For layered compositing, masks, and per-layer trails, draw into an offscreen
canvas and blit it onto the screen later (like Processing's `PGraphics`).

```petal
// Build a stamp in a 24x24 offscreen canvas.
let stamp = create_canvas(24, 24)   // returns a canvas handle (an int)
draw_to(stamp)                       // redirect drawing into the canvas
draw_rect(9, 2, 6, 20, 240, 220, 120)
draw_rect(2, 9, 20, 6, 240, 220, 120)
draw_to_screen()                     // back to the main framebuffer

// Composite it wherever you like. Transparent pixels show the background.
draw_canvas(stamp, 100, 50)
draw_canvas(stamp, 200, 80)
```

An offscreen canvas starts fully transparent. Canvases are rebuilt from the
draw stream every frame, so call `create_canvas` each frame like any other
draw call. See `examples/cc_offscreen_layers.ptl`.

### Input

```petal
key_down("left")       // true while the key is held
key_pressed("space")   // true only on the frame the key went down
key_released("space")  // true only on the frame the key came up
mouse_x()              // mouse X position (pixels)
mouse_y()              // mouse Y position (pixels)
mouse_down(0)          // button held: 0 = left, 1 = right, 2 = middle
mouse_pressed(0)       // true only on the frame the button went down
mouse_released(0)      // true only on the frame the button came up
text_input()           // text typed this frame
```

Key names are lowercase strings: `a`-`z`, `0`-`9`, `up`, `down`, `left`,
`right`, `space`, `return`, `escape`, `tab`, `backspace`, `delete`,
`insert`, `home`, `end`, `pageup`, `pagedown`, `shift`, `ctrl`, `alt`, `cmd`,
`f1`-`f12`, and punctuation names such as `minus`, `equals`, `comma`,
`period`, `slash`, `leftbracket`, `rightbracket`. An unknown name is simply
never down.

Gamepads feed the same key names, so `key_down("left")` also answers to a
d-pad. See [design.md](design.md#gamepads) for the mapping.

### Timing and screen

```petal
dt()              // seconds since last frame (float, about 0.016 at 60 fps)
frame_count()     // total frames rendered (int)
screen_width()    // window width in pixels
screen_height()   // window height in pixels
```

### Built-in functions from Petal core

Always available:

```petal
// Math
abs(n)  sqrt(n)  floor(f)  ceil(f)  round(f)  min(a, b)  max(a, b)
random(min, max)    // random float in [min, max)

// Type conversion
int(x)  float(x)  str(x)  type(x)

// Collections
len(list)  append(list, val)  pop(list)   // append returns a NEW list: xs = append(xs, val)
contains(list_or_str, val)  range(start, end)
slice(list, start, end)  reverse(list)  sort(list)
flat(list)  enumerate(list)  zip(a, b)
map(list, fn)  filter(list, fn)  reduce(list, init, fn)

// Strings
split(str, sep)  join(list, sep)

// Records
keys(record)  values(record)

// I/O
print(...)    // goes to stderr, so it shows in the terminal
```

The full list is in the [language guide](../../../docs/language-guide.md).

## Petal quick reference

Just enough syntax to read the examples. The
[language guide](../../../docs/language-guide.md) has the rest.

```petal
// Variables
let speed = 200.0         // local, recomputed every frame
state score = 0           // persists across frames

// Control flow: blocks end with `end`
if x > 5 then
    print("big")
elsif x > 2 then
    print("medium")
else
    print("small")
end

for item in list do ... end
for i in range(0, 10) do ... end
while running do ... end      // break and continue work in loops

// Functions: the last expression is the return value
fn clamp(val, lo, hi)
    max(lo, min(val, hi))
end

let double = fn(x) -> x * 2   // lambda

// Collections
let items = [1, 2, 3]
items[0]
let player = { x: 100, y: 200, health: 3 }
player.x

// Strings: ++ concatenates, {} interpolates
"Score: " ++ str(score)
"Score: {score}"

// Pattern matching (match is an expression)
let dy = match direction
    when "up"   -> -speed
    when "down" -> speed
    when _      -> 0.0
end
y += dy * dt()

// Enums
enum GameState
    Playing,
    Paused,
    GameOver,
end
state current = Playing

match current
    when Playing  -> update_game()
    when Paused   -> draw_text("PAUSED", 350, 300, 32, 255, 255, 255)
    when GameOver -> draw_text("GAME OVER", 300, 300, 32, 255, 0, 0)
end
```

## Common patterns

### Game loop structure

```petal
// 1. State
state x = 400.0
state y = 300.0
state vx = 0.0
state vy = 0.0

// 2. Input
let delta = dt()
if key_down("left") then vx = -200.0 end
if key_down("right") then vx = 200.0 end

// 3. Physics
x += vx * delta
y += vy * delta

// 4. Drawing (later draws are on top)
clear(0, 0, 0)
draw_rect(int(x), int(y), 20, 20, 255, 255, 255)
```

### Collision detection (AABB)

```petal
fn rects_collide(x1, y1, w1, h1, x2, y2, w2, h2)
    x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2
end
```

### Wrapping around screen edges

```petal
x = x % float(screen_width())
if x < 0.0 then x += float(screen_width()) end
```

### Spawning entities with lists

```petal
state enemies = []
state spawn_timer = 0.0

spawn_timer += dt()
if spawn_timer > 1.0 then
    spawn_timer = 0.0
    enemies = append(enemies, { x: random(0.0, 800.0), y: 0.0 })
end

// Move every enemy down
enemies = map(enemies, fn(e) -> { x: e.x, y: e.y + 100.0 * dt() })

// Drop the ones that left the screen
enemies = filter(enemies, fn(e) -> e.y < 600.0)
```

### Blinking text

```petal
state frame = 0
frame += 1

if frame % 60 < 30 then
    draw_text("PRESS START", 300, 400, 24, 255, 255, 255)
end
```

### Random colors

```petal
let r = int(random(0.0, 256.0))
let g = int(random(0.0, 256.0))
let b = int(random(0.0, 256.0))
```

## Testing with the agent protocol

Run with `--headless` to drive frames from a script:

```bash
echo '{"cmd":"step","n":60}
{"cmd":"state"}
{"cmd":"capture_draw_commands"}' | cargo run -- --headless examples/your_game.ptl
```

See [agent-protocol.md](agent-protocol.md) for the command reference.

## Tips

- Drawing functions take ints; physics wants floats. Convert with `int()`
  and `float()`.
- Multiply velocities by `dt()` so movement is frame-rate independent.
- Draw order matters: `clear()` first, background next, foreground last.
- Use `state` for everything that must persist: positions, velocities,
  scores, entity lists, timers, game phase.
- Strings concatenate with `++`, not `+`.
- Lists are immutable. `append`, `map`, and `filter` return new lists, so
  write `xs = append(xs, val)`.

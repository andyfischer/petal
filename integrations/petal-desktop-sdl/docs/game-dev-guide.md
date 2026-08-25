# Petal Game Development Guide

## Getting Started

### Running a game

```bash
cd petal-sdl
cargo run -- examples/pong.ptl
```

Options:
- `--width <n>` — Window width (default: 800)
- `--height <n>` — Window height (default: 600)
- `--title <str>` — Window title
- `--no-hot-reload` — Disable live code reloading
- `--agent` — Enable agent debugging protocol (JSON over stdin/stdout)
- `--headless` — Headless mode, no window (implies --agent)

### How it works

Your `.ptl` file runs **every frame** (~60fps). The engine calls your entire
script once per frame. Use `state` variables to persist data between frames.

```petal
state x = 100.0          // initialized once, persists across frames
x += 100.0 * dt()        // move 100 pixels per second
draw_rect(int(x), 100, 20, 20, 255, 0, 0)
```

### Where a `state` cell lives

A `state` slot is identified by its declaration **and by the call path that
reached it** — the chain of callsites and loop iterations running from the top
of the file down to the declaration.

- **At the top level** — what every example in this guide uses — the path is
  empty, so a declaration is one cell for the whole game. `state score = 0` is
  *the* score, readable and writable from anywhere in the file.
- **Inside a function**, each callsite gets its own cell, and a call made inside
  a `for`/`while` gets one cell per iteration. So

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

  needs no manual keying to give each enemy its own health. Two functions that
  each declare `state t` never share a cell either.
- **`state(expr) name = …` overrides the path.** An explicit key is *absolute*:
  every callsite asking for the same key value reaches the same cell, in or out
  of a loop. Reach for it when a cell belongs to a domain object rather than to
  a position — `state(e.id) hp = 100` follows the enemy when the list is
  reordered, where the unkeyed form above stays with the loop index.
- Cells that are deliberately shared across functions go at the top level as
  `state var`, read and written with `get` / `set`.

### Hot reload

Edit your `.ptl` file while the game is running. Changes apply immediately and
`state` variables are preserved (as long as they have the same name).

For a `state` inside a function the call path has to survive the edit too. The
callsite id is derived from the callee's spelling and its position among calls
to that same name in the enclosing function — never from line numbers — so
editing elsewhere in the file is free, but renaming the called function, or
inserting an *earlier* call to it in the same function, moves those cells and
they re-initialize. That is the same accepted loss as renaming a `state`
variable, and the orphaned cells are swept after the next frame.

## Game API Reference

### Drawing

All coordinates are in pixels. Origin (0, 0) is the top-left corner.
Colors are RGB integers 0-255.

```petal
clear(r, g, b)                              // fill background
draw_rect(x, y, width, height, r, g, b)     // filled rectangle
draw_rect_outline(x, y, w, h, r, g, b)      // rectangle outline
draw_line(x1, y1, x2, y2, r, g, b)          // line segment
draw_circle(cx, cy, radius, r, g, b)        // filled circle
draw_text(text, x, y, font_size, r, g, b)   // text string
```

### Input

Key names: `a`-`z`, `0`-`9`, `up`, `down`, `left`, `right`, `space`, `return`,
`escape`, `tab`, `shift`, `ctrl`, `alt`, `backspace`.

```petal
key_down("left")       // true while key is held
key_pressed("space")   // true only on the frame the key was first pressed
mouse_x()              // mouse X position (pixels)
mouse_y()              // mouse Y position (pixels)
mouse_down(1)          // mouse button held (1=left, 2=middle, 3=right)
mouse_pressed(1)       // true only on the frame the button was first pressed
```

### Timing

```petal
dt()              // seconds since last frame (float, ~0.016 at 60fps)
frame_count()     // total frames rendered (int)
screen_width()    // window width in pixels
screen_height()   // window height in pixels
```

### Built-in functions (from Petal core)

These are always available:

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
print(...)    // prints to stderr (visible in terminal)
```

## Petal Language Quick Reference

### Variables and state

```petal
let speed = 200.0         // local, reset every frame
state score = 0           // persistent across frames
state player_x = 400.0    // initialized once
```

At the top level each `state` name is one cell. Inside a function it is one
cell per callsite (and per loop iteration around the call) — see
[Where a `state` cell lives](#where-a-state-cell-lives).

### Control flow

```petal
if condition { ... }
if condition { ... } else { ... }
if a { ... } else if b { ... } else { ... }

for item in list { ... }
for i in range(0, 10) { ... }

while condition { ... }

// break and continue work in loops
```

### Functions

```petal
fn clamp(val, lo, hi) {
    max(lo, min(val, hi))
}

// Lambdas
let double = fn(x) { x * 2 }
```

### Collections

```petal
let items = [1, 2, 3]
items[0]                  // index access

let player = { x: 100, y: 200, health: 3 }
player.x                  // field access
```

### String concatenation

```petal
"Score: " ++ str(score)   // use ++ to concat strings
```

### Pattern matching

```petal
match direction {
    "up" -> y -= speed * dt()
    "down" -> y += speed * dt()
    _ -> {}
}
```

### Enums

```petal
enum GameState { Playing, Paused, GameOver }
state current = Playing

match current {
    Playing -> { /* update game */ }
    Paused -> { draw_text("PAUSED", 350, 300, 32, 255, 255, 255) }
    GameOver -> { draw_text("GAME OVER", 300, 300, 32, 255, 0, 0) }
}
```

## Common Patterns

### Game loop structure

```petal
// 1. State declarations
state x = 400.0
state y = 300.0
state vx = 0.0
state vy = 0.0

// 2. Input handling
let delta = dt()
if key_down("left") { vx = -200.0 }
if key_down("right") { vx = 200.0 }

// 3. Physics / game logic
x += vx * delta
y += vy * delta

// 4. Drawing (order matters — later draws are on top)
clear(0, 0, 0)
draw_rect(int(x), int(y), 20, 20, 255, 255, 255)
```

### Collision detection (AABB)

```petal
fn rects_collide(x1, y1, w1, h1, x2, y2, w2, h2) {
    x1 < x2 + w2 && x1 + w1 > x2 && y1 < y2 + h2 && y1 + h1 > y2
}
```

### Wrapping around screen edges

```petal
x = x % float(screen_width())
if x < 0.0 { x += float(screen_width()) }
```

### Spawning entities with lists

```petal
state enemies = []
state spawn_timer = 0.0

spawn_timer += dt()
if spawn_timer > 1.0 {
    spawn_timer = 0.0
    enemies = append(enemies, { x: random(0.0, 800.0), y: 0.0 })
}

// Update all enemies
enemies = map(enemies, fn(e) {
    { x: e.x, y: e.y + 100.0 * dt() }
})

// Remove off-screen
enemies = filter(enemies, fn(e) { e.y < 600.0 })
```

### Simple animation

```petal
state frame = 0
frame += 1

// Blink every 30 frames
if frame % 60 < 30 {
    draw_text("PRESS START", 300, 400, 24, 255, 255, 255)
}
```

### Random colors

```petal
let r = int(random(0.0, 256.0))
let g = int(random(0.0, 256.0))
let b = int(random(0.0, 256.0))
```

## Debugging with Agent Protocol

Run with `--headless` for automated testing:

```bash
echo '{"cmd":"step","n":60}
{"cmd":"state"}
{"cmd":"capture_draw_commands"}' | cargo run -- --headless examples/your_game.ptl
```

See [agent-protocol.md](agent-protocol.md) for the full command reference.

## Tips

- Use `float()` and `int()` to convert between types — drawing functions need
  ints, physics calculations need floats.
- `dt()` makes movement frame-rate independent. Always multiply velocities by
  `dt()`.
- Draw order matters: call `clear()` first, draw background elements, then
  foreground elements last.
- Use `state` for everything that needs to persist: positions, velocities,
  scores, entity lists, timers, game phase.
- String concatenation uses `++`, not `+`.
- Lists are immutable: `append(list, val)` (and the deprecated `push`) return a
  **new** list rather than mutating — write `xs = append(xs, val)`. `map()` /
  `filter()` also return new lists.

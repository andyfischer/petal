# Agent Protocol

petal-sdl includes a JSON-over-stdin/stdout protocol for programmatic control,
designed for LLM agents, automated testing, and debugging tools.

## Modes

### Normal mode (default)
```
petal-sdl examples/pong.ptl
```
Standard interactive game. No protocol, no stdin reading.

### Agent mode (hybrid)
```
petal-sdl --agent examples/pong.ptl
```
Opens an SDL window that runs interactively **and** accepts commands via stdin.
The game runs at normal speed until paused. An LLM can observe and intervene
while a human watches the window.

### Headless mode
```
petal-sdl --headless examples/pong.ptl
```
No SDL window. Starts paused. Every frame is driven by `step` commands.
Ideal for CI, automated testing, and LLM-driven game development.

## Protocol

One JSON object per line on stdin (commands) and stdout (responses).
Stderr is used for logs and errors from the game itself.

### Ready Signal

On startup, the engine sends a ready message:
```json
{"ok": true, "paused": false, "frame": 0}
```
In headless mode, `paused` is `true` (the LLM drives all frames).

## Commands

### pause

Stop frame advancement. The SDL window (if open) stays responsive but the game
freezes.

```json
{"cmd": "pause"}
```
```json
{"ok": true, "paused": true}
```

### resume

Resume normal frame advancement (agent mode only — in headless, use `step`).

```json
{"cmd": "resume"}
```
```json
{"ok": true, "paused": false}
```

### step

Advance exactly N frames. In headless mode, `dt()` returns a fixed 1/60s.
Input state set by `input` commands persists across steps (sticky keys).

```json
{"cmd": "step"}
{"cmd": "step", "n": 10}
```
```json
{"ok": true, "frame": 42}
{"ok": true, "frame": 52, "output": ["debug: hit wall"]}
```

The `output` field is included only if the game called `print()` during the
stepped frames.

### state

Dump all Petal `state` variables as a JSON object. Variable names are keys,
values are serialized to JSON (numbers, strings, booleans, null, arrays, objects).

```json
{"cmd": "state"}
```
```json
{"ok": true, "state": {"ball_x": 403.33, "ball_y": 302.5, "score": 0}}
```

### capture_draw_commands

Run one frame **speculatively** — capture what would be drawn without advancing
game state. Uses Petal's `run_speculative()` which snapshots state, runs the
frame, then restores the snapshot.

This is the primary inspection tool for LLMs: see exactly what's on screen as
structured data, not pixels.

```json
{"cmd": "capture_draw_commands"}
```
```json
{
  "ok": true,
  "draw_commands": [
    {"op": "clear", "r": 0, "g": 0, "b": 40},
    {"op": "rect", "x": 20, "y": 250, "w": 10, "h": 80, "r": 255, "g": 255, "b": 255},
    {"op": "circle", "cx": 400, "cy": 300, "radius": 8, "r": 255, "g": 200, "b": 50},
    {"op": "text", "text": "Score: 5", "x": 350, "y": 20, "size": 24, "r": 255, "g": 255, "b": 255}
  ],
  "output": []
}
```

Draw command types: `clear`, `rect`, `rect_outline`, `line`, `circle`, `text`.

### input

Set input state. Keys are sticky — they stay down until the next `input` command.
Mouse position and buttons persist similarly.

```json
{"cmd": "input", "keys_down": ["up", "space"], "mouse": {"x": 400, "y": 300, "buttons": [1]}}
```
```json
{"ok": true}
```

Key names: `a`-`z`, `0`-`9`, `up`, `down`, `left`, `right`, `space`, `return`,
`escape`, `tab`, `shift`, `ctrl`, `alt`, `backspace`. Unrecognized names are
silently ignored.

`mouse` accepts two forms:

- Object (preferred): `{"x": int, "y": int, "buttons": [int, ...]}` where
  `buttons` is a list of SDL mouse button codes (1 = left, 2 = middle, 3 = right).
- Legacy tuple: `[x, y]` — sets position only, clears all button state.

### screenshot

Render the current frame (speculatively — state is not advanced) into a PNG
and return it as a base64-encoded data URL. Useful for visual diffs in CI.

```json
{"cmd": "screenshot"}
```
```json
{"ok": true, "screenshot": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAA..."}
```

The same PNG encoder is used by the `--screenshot out.png --frames N` CLI flag.

### set_state

Directly set a Petal state variable by name. Supports null, booleans, numbers,
and strings.

```json
{"cmd": "set_state", "name": "score", "value": 42}
{"cmd": "set_state", "name": "ball_x", "value": 400.0}
```
```json
{"ok": true}
```

## Error Handling

If a command fails, the response has `ok: false` with an error message:
```json
{"ok": false, "error": "No state variable named 'nonexistent'"}
```

Invalid JSON on stdin produces an error response without crashing:
```json
{"ok": false, "error": "Invalid command: missing field `cmd`"}
```

## Example: LLM Testing Session

```
→ {"cmd":"step","n":60}                          # Run 1 second of gameplay
← {"ok":true,"frame":60}
→ {"cmd":"state"}                                # Check game state
← {"ok":true,"state":{"ball_x":600.0,"score":3}}
→ {"cmd":"set_state","name":"ball_x","value":25} # Move ball near paddle
← {"ok":true}
→ {"cmd":"input","keys_down":[],"mouse":[0,300]} # Position paddle
← {"ok":true}
→ {"cmd":"step","n":5}                           # Let physics run
← {"ok":true,"frame":65}
→ {"cmd":"capture_draw_commands"}                 # See what's drawn
← {"ok":true,"draw_commands":[...]}
→ {"cmd":"state"}                                # Verify collision worked
← {"ok":true,"state":{"score":4,...}}
```

## Architecture Notes

**Thread-local command buffer**: Native drawing functions (`draw_rect`, etc.)
push to a `thread_local!` `Vec<DrawCommand>`. The protocol drains this buffer
to serialize draw commands. This means `capture_draw_commands` works by running
the Petal program and collecting side effects, not by querying a scene graph.

**Speculative execution**: `capture_draw_commands` uses `Env::run_speculative()`,
which clones the state HashMap (cheap — `Value` is `Copy`), runs the frame,
then restores the snapshot. Heap allocations from the speculative frame persist
but are garbage-collected naturally.

**Fixed dt in headless**: When stepping, `dt()` returns 1/60s (0.01667) for
deterministic behavior. The frame counter increments normally.

# Agent Protocol

`petal-sdl` can be driven by JSON commands on stdin, with one JSON response
per command on stdout. It is meant for LLM agents, automated tests, and
debugging tools.

The command and response schema is shared with other Petal hosts (the
diagram-canvas sample app speaks the same protocol over WebSocket). The
canonical description, including the shape of `state` keys, is
[`docs/dev/debug-protocol.md`](../../../docs/dev/debug-protocol.md). This page
is the `petal-sdl` view of it.

## Modes

**Normal** — `petal-sdl examples/pong.ptl`. Interactive window, no protocol,
stdin is not read.

**Agent** — `petal-sdl --agent examples/pong.ptl`. Opens the window *and*
accepts commands on stdin. The game runs at normal speed until you `pause`
it, so an agent can observe and intervene while a person watches.

**Headless** — `petal-sdl --headless examples/pong.ptl`. No window. Starts
paused; every frame is driven by `step`. Use this for CI and scripted tests.

## Framing

One JSON object per line on stdin (commands) and stdout (responses). The
game's own `print()` output and any logs go to stderr.

On startup the engine sends a ready message:

```json
{"ok": true, "paused": false, "frame": 0}
```

In headless mode `paused` is `true`.

Every response has `ok`. On failure it is `false` with an `error` string:

```json
{"ok": false, "error": "No state variable named 'nonexistent'"}
```

Invalid JSON on stdin produces an error response without crashing:

```json
{"ok": false, "error": "Invalid command: missing field `cmd`"}
```

## Commands

### pause / resume

`pause` stops frame advancement; the window (if any) stays responsive.
`resume` restarts it. In headless mode there is nothing to resume; use
`step`.

```json
{"cmd": "pause"}
{"ok": true, "paused": true}

{"cmd": "resume"}
{"ok": true, "paused": false}
```

### step

Advance exactly `n` frames (default 1). While stepping, `dt()` returns a
fixed 1/60 s so runs are deterministic. Input set by `input` stays down
across steps.

```json
{"cmd": "step"}
{"cmd": "step", "n": 10}

{"ok": true, "frame": 42}
{"ok": true, "frame": 52, "output": ["debug: hit wall"]}
```

`output` is present only if the game called `print()` during the stepped
frames.

### state

Dump every `state` variable as a JSON object. Values serialize to JSON
numbers, strings, booleans, null, arrays, and objects.

```json
{"cmd": "state"}
{"ok": true, "state": {"ball_x": 403.33, "ball_y": 302.5, "score": 0}}
```

Top-level cells appear under their bare name. A `state` declared inside a
function appears under a pathed key such as `counter#1/count` or
`[3]/row/hovered`.

### set_state

Set a top-level `state` variable by name. Accepts null, booleans, numbers,
and strings.

```json
{"cmd": "set_state", "name": "score", "value": 42}
{"cmd": "set_state", "name": "ball_x", "value": 400.0}
{"ok": true}
```

Pathed keys (state inside functions) cannot be set this way and are rejected
with `No state variable named '...'`. Anything an agent needs to drive from
outside belongs in a top-level `state var`.

### capture_draw_commands

Run one frame speculatively and return what it would draw, without advancing
game state. This is the main way for an agent to see the screen as
structured data instead of pixels.

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
  ]
}
```

The ops are the `petal-ui` draw vocabulary: `clear`, `rect`, `rect_outline`,
`line`, `circle`, `triangle`, `poly`, `text`, `clip`, `clip_none`, the
offscreen-canvas ops (`create_canvas`, `set_target`, `draw_canvas`), and the
newer shapes, gradients, and shadows. `output` is included if the frame
printed anything.

### screenshot

Render the current frame speculatively to a PNG and return it as a base64
data URL. State is not advanced.

```json
{"cmd": "screenshot"}
{"ok": true, "screenshot": "data:image/png;base64,iVBORw0KGgo..."}
```

The `--screenshot out.png --frames N` CLI flag uses the same encoder.

### input

Set the input state as an absolute snapshot: these keys and buttons are down
now. Keys stay down until the next `input` command. Press and release edges
(`key_pressed`, `mouse_pressed`, ...) are derived by diffing consecutive
snapshots and reach the next stepped frame.

```json
{"cmd": "input", "keys_down": ["up", "space"], "mouse": {"x": 400, "y": 300, "buttons": [0]}}
{"ok": true}
```

Fields, all optional:

- `keys_down` — list of key names. The names are the `petal-ui` canonical
  set: `a`-`z`, `0`-`9`, `up`, `down`, `left`, `right`, `space`, `return`,
  `escape`, `tab`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`,
  `pagedown`, `shift`, `ctrl`, `alt`, `cmd`, `f1`-`f12`, and punctuation
  names (`minus`, `equals`, `comma`, `period`, `slash`, `backslash`,
  `semicolon`, `quote`, `backquote`, `leftbracket`, `rightbracket`).
  Unrecognized names are ignored.
- `mouse` — either `{"x": int, "y": int, "buttons": [int, ...]}`, where
  buttons are `0` = left, `1` = right, `2` = middle and the list is
  authoritative (an empty list releases every button); or the legacy tuple
  `[x, y]`, which sets position only and leaves held buttons alone.
- `text` — a string delivered to the next stepped frame's `text_input()`,
  the same channel real typing uses. Lets you type into a script without
  simulating key scancodes.
- `mouse_delta` — `{"dx": int, "dy": int}`, raw relative pointer motion for
  the next stepped frame, read by `mouse_dx()` / `mouse_dy()`. Drives
  mouselook while the pointer is grabbed.

```json
{"cmd": "input", "text": "hello"}
{"cmd": "input", "mouse_delta": {"dx": 3, "dy": -2}}
```

### pending_report

Report every live pending resource (state, age, origin, absorption count) in
the `pending` field. This is the query behind the dev overlay; it reads the
live resource table and does not run a frame.

```json
{"cmd": "pending_report"}
{"ok": true, "pending": [...]}
```

### draw_stats

Optional per-frame draw statistics in the `stats` field, for hosts built on
this crate that implement `Host::draw_stats`. The shipped `petal-sdl` binary
does not, and answers:

```json
{"ok": false, "error": "draw_stats is not supported by this host"}
```

## Example session

```
→ {"cmd":"step","n":60}                          # run one second of gameplay
← {"ok":true,"frame":60}
→ {"cmd":"state"}                                # check the game state
← {"ok":true,"state":{"ball_x":600.0,"score":3}}
→ {"cmd":"set_state","name":"ball_x","value":25} # move the ball near the paddle
← {"ok":true}
→ {"cmd":"input","mouse":{"x":0,"y":300,"buttons":[]}}   # position the paddle
← {"ok":true}
→ {"cmd":"step","n":5}                           # let physics run
← {"ok":true,"frame":65}
→ {"cmd":"capture_draw_commands"}                 # see what is drawn
← {"ok":true,"draw_commands":[...]}
→ {"cmd":"state"}                                # verify the hit
← {"ok":true,"state":{"score":4,...}}
```

## How speculative commands work

`capture_draw_commands`, `screenshot`, and `draw_stats` run the frame in a
fork of the live execution: the heap and registries are deep-cloned, the
frame runs in the fork, the host drains the fork's own draw buffer and print
output, and the fork is dropped. Nothing leaks back into the live state, and
`Host::end_frame` is not called for these frames.

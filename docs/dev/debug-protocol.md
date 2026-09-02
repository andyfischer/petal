# Petal Debug Protocol

The JSON debug protocol shared by **petal-sdl** (`--agent` / `--headless`
modes, newline-delimited JSON on stdin/stdout) and **petal-diagram-canvas**
(WebSocket at `ws://.../debug`). Both transports accept the same command
shapes and produce the same response shape, so an agent written against one
works against the other.

Implementations:

- **petal-sdl:** `integrations/petal-desktop-sdl/src/protocol.rs`
  (`Command`, `Response`, `handle_command`).
- **petal-diagram-canvas:** `examples/custom-integrations/diagram-canvas/src/debug.ts`
  (`PetalDebugAPI.handleCommand`).

This document is the source of truth. When an implementation drifts, fix the
implementation.

## Commands (client → engine)

Every command is a JSON object with a `cmd` field. Unknown fields are
ignored, so additions stay backwards-compatible.

```json
{ "cmd": "pause" }
{ "cmd": "resume" }
{ "cmd": "step", "n": 5 }
{ "cmd": "state" }
{ "cmd": "set_state", "name": "player_x", "value": 100.5 }
{ "cmd": "capture_draw_commands" }
{ "cmd": "input",
  "keys_down": ["w", "a"],
  "mouse": { "x": 400, "y": 300, "buttons": [0] },
  "mouse_delta": { "dx": 3, "dy": -2 },
  "text": "hello" }
{ "cmd": "screenshot" }
{ "cmd": "pending_report" }
{ "cmd": "draw_stats" }
```

| Command | Fields | Notes |
|---------|--------|-------|
| `pause` | — | Freeze the frame loop. |
| `resume` | — | Resume real-time playback. |
| `step` | `n: int` (default `1`) | Advance N frames at fixed `dt = 1/60`. |
| `state` | — | Dump every runtime `state` variable as JSON (see keying below). |
| `set_state` | `name: string`, `value: json` | Set one **top-level** state var. |
| `capture_draw_commands` | — | Speculative run with no side effects. |
| `input` | `keys_down?: string[]`, `mouse?: MouseInput`, `mouse_delta?: {dx, dy}`, `text?: string` | Inject input for the next stepped frame. `text` is delivered to `text_input()`; `mouse_delta` to `mouse_dx()`/`mouse_dy()`. |
| `screenshot` | — | Return the current frame as a PNG data URL. |
| `pending_report` | — | Every live pending resource, with state, age, and origin. Replies in `pending`. |
| `draw_stats` | — | Optional per-frame draw statistics. A host that does not implement it replies with an error. |

### `state` keys and the call path

A `state` slot is keyed by its declaration *and* the call path that reached
it, so a `state` inside a function holds one value per callsite and loop
iteration. That shows up here only in the key strings `state` returns: a
top-level declaration is its bare (module-qualified) name (`score`,
`ui::theme`), while a pathed slot renders its path with the name last:
`counter#1/count`, `[3]/row/hovered`, `k1234…/leaf` (a call step, a loop
step, an explicit `state(key)` step). A pathed key always contains a `/`; a
top-level one never does.

`set_state` addresses top-level names only. A pathed key is rejected with the
usual `No state variable named '…'` error. A value the host means to drive
from outside belongs in a top-level `state var`.

Within one loaded program the key strings are stable, so diffing two dumps
across frames is meaningful. Across a hot reload they are not a stable
identity: the callee names and `#n` ordinals are display labels recovered
from the program, and an edit that adds a call can renumber them. Match on
the trailing variable name (after the last `/`) when following one
declaration across an edit.

### `MouseInput`

```json
{ "x": 400, "y": 300, "buttons": [0, 1] }
```

`buttons` lists the held **petal-ui button ids** (`0` left, `1` right,
`2` middle), the same ids scripts read with `mouse_pressed(0)`. It is
authoritative: an empty list releases every button. petal-sdl also accepts
the legacy tuple form `[400, 300]` (position only, buttons untouched); new
agents should use the object form.

## Responses (engine → client)

```json
{
  "ok": true,
  "paused": false,
  "frame": 42,
  "state": { "x": 100, "y": 50 },
  "draw_commands": [ ... ],
  "output": [ "..." ],
  "screenshot": "data:image/png;base64,...",
  "pending": [ ... ],
  "error": null
}
```

Only `ok` is always present; the other fields appear when relevant to the
command. On failure the engine returns `{ ok: false, error: <message> }`.

| Field | Type | When present |
|-------|------|--------------|
| `ok` | bool | always |
| `error` | string | on failure |
| `paused`, `frame` | bool, int | after any command that changes them |
| `state` | object | `state`, `set_state` |
| `draw_commands` | DrawCommand[] | `step`, `capture_draw_commands` |
| `output` | string[] | `step`, `capture_draw_commands`, when stdout was captured |
| `screenshot` | string (data URL) | `screenshot` |
| `pending` | PendingEntry[] | `pending_report` |

## PendingEntry

One entry per live pending resource in a `pending_report` response. This is
the data behind the dev overlay and "why is this region blank" debugging;
see [pending-values-plan.md](pending-values-plan.md).

```json
{ "id": 0,
  "key": 12345,
  "state": "loading",
  "age_frames": 12,
  "origin": { "line": 3, "col": 11, "text": "fetch(\"/api/user/7\")" },
  "absorbed_count": 4 }
```

| Field | Type | Meaning |
|-------|------|---------|
| `id` | int | Resource-table index (`PendingId`). |
| `key` | int | Cache key; two fetches of the same key share one entry. |
| `state` | `"loading" \| "errored" \| "ready"` | Resolution state. |
| `age_frames` | int | Frames since the resource was first requested. Only advances when the host calls `ExecutionContext::advance_frame()` once per frame, which petal-sdl does and the CLI does not. |
| `origin` | `{ line, col, text } \| null` | Originating call site; `null` when a native created it with no reachable term. |
| `absorbed_count` | int | How many ops absorbed this resource **this frame** (reset each frame). |

## DrawCommand

Every transport serializes draw commands the same way: an `op`-tagged object
whose other fields depend on the op, with fields at their default value
omitted. The core ops:

```json
{ "op": "clear|rect|rect_outline|line|circle|text",
  "r": 0, "g": 0, "b": 0, "a": 255,
  "x": 0, "y": 0, "w": 0, "h": 0, "radius": 0,
  "cx": 0, "cy": 0,
  "x1": 0, "y1": 0, "x2": 0, "y2": 0, "width": 1,
  "text": "", "size": 16 }
```

Every colored primitive takes an optional `a` (alpha, 0–255, default 255).
`rect` also takes a corner `radius`; `line` and `rect_outline` take a stroke
`width`. The full vocabulary (ellipses, polygons, arcs, gradients, clipping,
offscreen canvases, images, ...) is the `DrawCommand` enum in
`petal-ui/src/draw.rs`, which is the reference for field names.

## Transport differences

| | petal-sdl | petal-diagram-canvas |
|---|-----------|---------------------|
| Transport | stdin/stdout, newline-delimited JSON | WebSocket (`ws://.../debug`) |
| Startup | Engine emits one ready message `{ok:true, frame:0, paused}` | Client connects on demand |
| Headless mode | `--headless` starts paused | N/A, always has a canvas |
| Screenshot | Software rasterizer; also `--screenshot out.png --frames N` one-shot | `canvas.toDataURL()` |
| `input.text`, `input.mouse_delta` | Supported | Not wired (keys and mouse only) |

# Petal Debug Protocol

Canonical schema for the shared debug protocol used by **petal-sdl** (`--agent`
/ `--headless` modes, stdin/stdout JSON) and **petal-diagram-canvas**
(WebSocket `ws://.../debug`). Both transports accept the same JSON command
shapes and produce the same response shape. Agents writing against one
transport should work against the other.

Implementations:
- **petal-sdl:** `apps/petal-sdl/src/protocol.rs`, dispatched in `game_loop.rs::handle_command`
- **petal-diagram-canvas:** `apps/petal-diagram-canvas/src/debug.ts`
  (`PetalDebugAPI.handleCommand`)

---

## Commands (client → engine)

Every command is a JSON object with a `cmd` field. Unknown fields are ignored
so future additions stay backwards-compatible.

```json
{ "cmd": "pause" }
{ "cmd": "resume" }
{ "cmd": "step", "n": 5 }
{ "cmd": "state" }
{ "cmd": "set_state", "name": "player_x", "value": 100.5 }
{ "cmd": "capture_draw_commands" }
{ "cmd": "input",
  "keys_down": ["w", "a"],
  "mouse": { "x": 400, "y": 300, "buttons": [1] } }
{ "cmd": "screenshot" }
```

### Field reference

| Command | Fields | Notes |
|---------|--------|-------|
| `pause` | — | Freeze frame loop. |
| `resume` | — | Resume real-time playback. |
| `step` | `n: int` (default `1`) | Advance N frames at fixed `dt = 1/60`. |
| `state` | — | Dump all runtime state vars as JSON. |
| `set_state` | `name: string`, `value: json` | Mutate one state var. |
| `capture_draw_commands` | — | Speculative run, no side effects. |
| `input` | `keys_down?: string[]`, `mouse?: MouseInput` | Inject input. |
| `screenshot` | — | Return current frame as PNG data URL. |

### `MouseInput`

The canonical shape is an object:
```json
{ "x": 400, "y": 300, "buttons": [1, 3] }
```
For backwards compatibility, petal-sdl also accepts the legacy tuple form
`[400, 300]`. New agents should use the object form. `buttons` is an array of
SDL mouse button codes (1 = left, 2 = middle, 3 = right).

---

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
  "error": null
}
```

All fields except `ok` are optional and are only present when relevant to the
command. On failure the engine returns `{ ok: false, error: <message> }` with
no other fields set.

| Field | Type | When present |
|-------|------|--------------|
| `ok` | bool | always |
| `error` | string | on failure |
| `paused` | bool | always after state changes |
| `frame` | int | always after state changes |
| `state` | object | `state`, `set_state` |
| `draw_commands` | DrawCommand[] | `step`, `capture_draw_commands` |
| `output` | string[] | `step`, `capture_draw_commands` when stdout captured |
| `screenshot` | string (data URL) | `screenshot` |

---

## DrawCommand

Canvas and SDL emit draw commands in the same shape. Fields are optional per
`op`.

```json
{ "op": "clear|rect|rect_outline|line|circle|text",
  "r": 0, "g": 0, "b": 0,
  "x": 0, "y": 0, "w": 0, "h": 0,
  "cx": 0, "cy": 0, "radius": 0,
  "x1": 0, "y1": 0, "x2": 0, "y2": 0,
  "text": "", "size": 16 }
```

---

## Transport differences

| | petal-sdl | petal-diagram-canvas |
|---|-----------|---------------------|
| Transport | stdin/stdout (newline-delimited JSON) | WebSocket (`ws://.../debug`) |
| Startup | Engine emits one ready message `{ok:true, frame:0, paused}` | Client connects on demand |
| Headless mode | `--headless` starts paused | N/A — always has a canvas |
| Screenshot | PNG via software rasterizer, also `--screenshot out.png --frames N` one-shot | `canvas.toDataURL()` |

The command/response schemas above are identical across transports; the only
variation is the delivery mechanism.

---

## Versioning

This document is the source of truth. When either implementation drifts, fix
the implementation to match this doc — don't fork the schema.

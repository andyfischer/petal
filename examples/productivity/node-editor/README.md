# 38 — Node-based editor (Patchwork)

A dataflow patch editor drawn entirely by a Petal script inside a Garden
panel. Nodes sit anywhere on an infinite, pannable, zoomable canvas; each
carries input ports on its left edge and one output port on its right; wires
are cubic beziers between them. The whole patch is evaluated every frame, the
wires carry animated flow dots, and the four sink kinds — Display, Scope,
Gauge, Color — render what arrives at them. The patch persists across
restarts through the panel store.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/node-editor
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel pane 1268×778. The palette (176 px) and inspector (288 px) are fixed;
the canvas takes the rest, and the camera starts fitted to the patch, so
other sizes work — they just land at a different zoom.

The patch is saved to the panel store after every edit, so a second launch
shows the patch you left, not the demo. `GARDEN_PANEL_STORE_DIR=/tmp/x` gives
a run its own scratch store; the canvas context menu's **Reset demo patch**
restores the seed.

While playing, the panel asks for a frame every frame (`request_frame`), so
the sines and the scope keep moving. Press `P` to pause; the clock stops and
the panel is allowed to sleep. Under the headless harness `POST /tick` steps
the clock by exactly `dt` per frame.

## Node kinds

| Category | Kinds | Notes |
|---|---|---|
| Sources | Number, Time | no inputs; Time is the clock × speed |
| Generators | Sine, Noise | `amp·sin(2π·freq·t + phase)`, Perlin noise in −1..1 |
| Math | Add, Subtract, Multiply, Divide, Clamp, Lerp, Map range | Divide by 0 yields 0 |
| Sinks | Display, Scope, Gauge, Color | every sink also passes its value through its output port |

Every input port has a default used while it is unwired, editable with a
slider in the inspector. Parameters (Sine's freq/amp/phase, Gauge's min/max,
…) are never wired. Wiring that would close a loop is refused with a message
in the status bar.

## Controls

| Input | Effect |
|---|---|
| **Drag a node** | Move it (and every other selected node). `shift` snaps to a 16 px grid. |
| **Click a node** | Select it; `shift`-click adds or removes it from the selection. |
| **Drag empty canvas** | Marquee-select everything the box touches (`shift` adds). |
| **Drag from an output port** | Pull a wire; release on an input port to connect. An input holds one wire, so dropping onto a wired input replaces it. |
| **Drag from an unwired input port** | Pull a wire backwards to an output. |
| **Drag from a wired input port** | Pick the wire up; drop it on another input to move it, or anywhere else to remove it. |
| **Right-click a node** | Duplicate · Disconnect all · Bring to front · Delete. |
| **Right-click the canvas** | Add any kind at that spot · Fit to view · Reset demo patch. |
| **Palette row** | Click to add at the view centre, or drag onto the canvas to place it. |
| **Wheel** | Pan. `⌘`/`ctrl`-wheel zooms about the pointer. |
| **Space-drag, alt-drag, middle-drag** | Pan. |
| `⌘Z` · `⇧⌘Z` | Undo · redo (60 levels; covers moves, wires, edits, adds, deletes). |
| `⌘D` · `⌘A` | Duplicate the selection (internal wires included) · select all. |
| `⌫` · `delete` | Delete the selection. |
| `←` `→` `↑` `↓` | Nudge by 4 px, `shift` for 24. |
| `F` · `0` · `+` · `−` | Fit to view · 100% · zoom in · zoom out. |
| `P` | Play / pause the clock. |
| `esc` | Cancel the current gesture, else clear the selection. |

Header: play/pause, rewind the clock, undo, redo, fit, and the zoom pill
(click to return to 100%). Inspector: the selected node's description,
parameter sliders, input sliders (or the wire feeding each input, with an ×
to cut it), the live output value, fan-out count, and duplicate/delete.

## What it exercises

**Language**

- Records for nodes, links, and the drag gesture, updated immutably with
  spread inside collecting `for` loops; `continue` as the filter.
- `state` for what persists across frames (patch, selection, camera, clock,
  undo stacks, drag state, menu); `let` for every per-frame derivation.
- `var`/`set`/`get` exactly once: the evaluator's memo table, which a
  recursive `ev_node` writes through while walking upstream.
- **Per-key `state(id)`** for the Scope history — one sample list per node
  id, keyed by identity rather than call path, so reordering or deleting
  nodes never mixes traces.
- A `match` with `do … end` arms in the evaluator; `elsif` chains for the
  hit-test and the hint bar.
- Hand-rolled text serialization (`split`/`join`/`parse_float`/`??`) because
  Garden has no JSON builtins.

**Host / petal-ui**

- Draw surface: `draw_polyline` for the bezier wires (one stroke each, no
  double-blended joins), `draw_shadow` under nodes, `fill_arc` for the gauge,
  `draw_rect_rounded` header bands composed with `over` so the tint is
  opaque, `clip`/`clip_none` for the canvas.
- Text: `draw_text` with style records (`ui` and `mono` faces), `text_width`
  for right-aligned values, `wrap_px` for the description.
- Prelude widgets: `slider` (positional `state` inside the prelude gives each
  inspector row its own drag slot), `menu_state`/`menu_show`/`menu_blocking`/
  `context_menu`, `rect`/`point_in`/`hovered`/`clicked`, `Rect.inset`.
- Input: `mouse_pressed`/`mouse_down`/`mouse_released` on buttons 0, 1 and 2,
  `scroll_x`/`scroll_y`, `key_down("space")` as a pan modifier, the four
  `mod_*` reads, `claim_key` for `⌘Z`/`⌘D`/`⌘A`, `request_frame` while
  playing, `panel_store_get`/`panel_store_set`.

**Debug server** — every logical value is observable in
`GET /state → panes[0].panel.values`: `nodes`, `links`, `sel`, `cam`,
`clock`, `playing`, `drag`, `hit` (what the pointer is over, in world space),
`hist`/`fut` lengths, `ops` (edit counter, also the save trigger), `flash`.
Gestures are `/mouse {"op":"drag"}` in window coordinates; the pane origin is
`panes[0].rect`.

## Known limits

- `⌘`-wheel zoom cannot be exercised headless: the debug server delivers
  `mods` on clicks but a `scroll` arrives with `modifiers: 0`. Use `+`/`−`.
- Nodes cannot be renamed; they are identified as *Kind #id*.
- Wires cannot be selected directly; pick one up at its input port instead.

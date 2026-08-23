# 33 — Petal Paint

A drawing / paint app written entirely as a Garden Petal panel.

The artboard is a **vector document**: every mark is a record in a `strokes`
list, with its points stored in artboard-normalized `0..1` coordinates, so the
whole drawing rescales cleanly when the pane resizes. Each frame re-renders the
list from scratch — there is no pixel buffer and no offscreen canvas native.

The document opens on a seeded sketch (*morning-ridge*: a wash, a sun, two
ridges, a cabin, water and grass — 19 strokes) so there is something real on the
paper to erase, reorder and undo.

## Run it

```bash
cd examples/productivity/paint
GARDEN_HEADLESS_SIZE=1280x850 \
  /Users/andy/petal/garden/target/debug/garden --headless --debug-port 0 --init layout.ptl
```

or windowed:

```bash
cd examples/productivity/paint
/Users/andy/petal/garden/target/debug/garden --init layout.ptl
```

**Designed for a 1280×850 viewport** (the panel pane is 1268×778 inside it).
The layout is computed from `screen_width()`/`screen_height()` and degrades
sensibly at other sizes, but the spacing scale was tuned at that size.

## Controls

### Mouse

| Gesture | Effect |
|---|---|
| Drag on the paper | Paint with the current tool |
| Click a rail button | Pick a tool |
| Click a swatch | Pick an ink |
| Drag the Size / Opacity slider | Change the brush |
| Click a **History** row | Select that stroke — a marquee appears around it on the canvas (click again to deselect) |
| **Right-click** a History row | Context menu: *Select on canvas · Duplicate · Bring to front · Delete stroke* |
| Click **Grid / Undo / Redo / Clear** | Header actions |
| Hover a rail button | Tooltip with the tool's name and gesture |
| Hover the paper | Live brush-size cursor ring (a hollow disc for the eraser, a drop for the bucket) |

### Keyboard

| Key | Effect |
|---|---|
| `1`–`6` | Brush · Line · Rectangle · Ellipse · Eraser · Flood fill |
| `[` / `]` | Brush size − / + (1–48 px) |
| `U` or `⌘Z` | Undo |
| `R` or `⇧⌘Z` | Redo |
| `C` | Clear the canvas (undoable) |
| `G` | Toggle the artboard grid |
| `Esc` | Dismiss the context menu |

Undo/redo is snapshot-based (up to 60 steps) and covers every document edit,
including *Clear* and every context-menu action.

## Tools

- **Brush** — freehand polyline, sampled while the button is down (2 px minimum
  spacing, capped at 900 points per stroke).
- **Line / Rectangle / Ellipse** — two-point rubber-band shapes; the preview is
  drawn live during the drag and the stroke is discarded if the drag was under
  4 px.
- **Eraser** — a brush at 3× width in the paper color. Because the document is
  vector and layered, this is exactly how a real paint program's opaque eraser
  behaves, and it undoes as one stroke.
- **Flood fill** — pushes a full-artboard wash of the current ink at the current
  opacity, so translucent washes stack.

## What it exercises

**Host / panel runtime**

- Pointer input end to end: `mouse_pressed`, `mouse_down`, per-frame
  `mouse_x/y` sampling for freehand capture, and `mouse_pressed(1)` for the
  right-click gesture.
- The full draw vocabulary: `clear`, `draw_rect`, `draw_rect_rounded`,
  `draw_rect_outline`, `draw_line` with alpha *and* stroke width, `draw_circle`,
  `fill_poly`, `fill_triangle`, `clip`/`clip_none` (the artboard is a clip
  region, and so is the brush-preview swatch).
- Styled text (`{size, color, spacing}` records) and `text_width`-exact right
  and center alignment.
- Translucent compositing — the seeded wash, the flood fill, and any stroke
  below 100% opacity.

**`ui` prelude**

`rect` / `Rect` methods (`inset`), `point_in`, `hovered`, `clicked`,
`draw_text_right`, `draw_text_center`, `fit_parts`, and the whole context-menu
family (`menu_state`, `menu_blocking`, `menu_open_on_right_click`,
`menu_item`, `menu_sep`, `context_menu`).

**Language**

`state` for the document and every tool setting, `let` dataflow everywhere else,
function overloading (`mk/4` and `mk/5`), records-as-structs, list rebuilding
with `append` / `drop_last` / `last` / `slice`, `continue` as a filter inside a
`for`, string interpolation, `elsif` chains, and color literals.

## Notes

- A panel sleeps 10 s after the last input. This app is event-driven, not
  animated, so sleeping costs nothing — but if you are driving it from the debug
  server, inject any event to wake it.
- `panel.values` exposes the whole logical state by name: `tool`, `ink`,
  `brush`, `opacity`, `grid`, `sel`, `ops`, `strokes`, `hist`, `fut`.

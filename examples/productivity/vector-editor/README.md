# 34 — Vector graphics editor

A small but real vector illustration editor, written entirely as a Garden Petal
panel: an artboard with a z-ordered object list, live selection handles that
scale and rotate, a numeric inspector you can drag-scrub, alignment tools, and a
layers panel with visibility and lock toggles.

The document is a plain list of shape records in **artboard coordinates**
(760 × 540 pt). Every shape carries the same four numbers — position, size and a
rotation about its own center — so *one* transform path drives hit testing,
handle placement, scaling, rotation, marquee bounds and alignment. There is no
pixel buffer; the whole document is re-rendered from the list every frame.

It opens on a seeded 19-object poster (*aurora-poster* — a night backdrop with a
three-ring moon glow, two ridges, a lake with a receding moonpath, three stars, a
pine, a rule and two text objects), so there is something worth selecting,
restacking and transforming from the first frame.

## Run it

```bash
cd examples/productivity/vector-editor
GARDEN_HEADLESS_SIZE=1400x940 \
  /Users/andy/petal/garden/target/debug/garden --headless --debug-port 0 --init layout.ptl
```

or windowed:

```bash
cd examples/productivity/vector-editor
/Users/andy/petal/garden/target/debug/garden --init layout.ptl
```

**Designed for a 1400 × 940 viewport** (the panel pane is 1388 × 868 inside it).
The layout is computed from `screen_width()` / `screen_height()` and degrades
sensibly at other sizes; the spacing scale was tuned at that one.

## Controls

### Canvas

| Gesture | Effect |
|---|---|
| Hover a shape | Its true outline (not the box) ghosts in |
| Click a shape | Select it — rotated bounding box, 8 scale handles, rotation knob |
| ⇧-click | Add / remove a shape from the selection |
| Drag a shape | Move the whole selection (snapped to the 8 pt grid) |
| Drag a corner / edge handle | Scale about the opposite handle, in the shape's own rotated frame |
| ⇧-drag a corner handle | Scale keeping the aspect ratio |
| ⌥-drag a handle | Scale about the center |
| Drag the knob above the box | Rotate about the center (⇧ snaps to 15°) |
| Drag on the backdrop | Rubber-band marquee — everything it touches is selected |
| ⌥-drag a shape | Move ignoring the snap grid |

The *Night backdrop* ships **locked**, which is what makes a marquee possible
anywhere on the artboard: locked objects are transparent to the pointer. Unlock
it from its layer row and it becomes draggable like anything else.

### Tools (left rail)

`V` Select · `R` Rectangle · `E` Ellipse · `T` Triangle · `S` Star · `L` Line.
Every shape tool draws by dragging; the Line tool takes the drag as its two
endpoints and stores the result as a thin rotated box. After a shape is placed
the tool returns to Select and the new object is selected.

### Inspector (right)

- **X / Y / W / H / Rotation / Opacity** — press and drag horizontally on a
  field to scrub the value (rotation scrubs at half speed). One drag is one
  undo step.
- **Fill** — twelve swatches; clicking one recolors the whole selection.
- **Align** — left / center / right / top / middle / bottom, computed against
  the selection's *rotated* bounds. Enabled with 2+ objects selected.
- **Layers** — top of the list is top of the stack. Click to select, ⇧-click to
  extend, wheel to scroll. Each row has a **lock** and an **eye** toggle.
  `Back ↓ ↑ Front` restack the selection. Right-click a row for the full menu.

### Keyboard

| Key | Effect |
|---|---|
| `V R E T S L` | Pick a tool |
| Arrows | Nudge 1 pt (⇧ = 10 pt) |
| `[` / `]` | Send backward / bring forward |
| `⇧[` / `⇧]` | Send to back / bring to front |
| `D` | Duplicate the selection, offset 16 pt |
| `Backspace` / `Delete` | Delete the selection |
| `K` | Lock / unlock the selection |
| `G` | Toggle the artboard grid |
| `N` | Toggle 8 pt snapping |
| `A` | Select all |
| `Esc` | Deselect |
| `U` | Undo |
| `Y` | Redo |

⌘ never reaches a panel — Garden claims it for its own menu accelerators and
for the direct-manipulation cmd-click — so every shortcut here is unmodified or
⇧-modified.

Undo is snapshot-based (60 steps) and covers every document edit: moves, scales,
rotations, scrubs, creation, deletion, duplication, recolor, align, restack, and
the visibility / lock toggles.

## What it exercises

**Host / panel runtime**

- The full pointer contract: `mouse_pressed(0)`, `mouse_down(0)` sampled per
  frame for the drag state machine, `mouse_pressed(1)` for the context gesture,
  `scroll_y()` through `scroll_update`, and `mod_shift()` / `mod_alt()` as
  drag modifiers.
- The draw vocabulary end to end: `clear`, `draw_rect` (with alpha),
  `draw_rect_rounded`, `draw_rect_outline`, `draw_line` with alpha *and* stroke
  width, `draw_circle`, `fill_triangle` (every rotated shape is a triangle fan
  from its center), `fill_poly`, `clip` / `clip_none` for the artboard and the
  layers list.
- Styled text records (`{size, color, spacing}`) measured with the *same*
  record through `text_width`, which is what lets the text objects auto-fit
  their boxes.
- Translucent compositing — the three-ring moon glow and the moonpath are pure
  alpha stacking.

**`ui` prelude**

`rect` / `Rect` methods (`inset`), `point_in`, `hovered`, `clicked`,
`ellipsize`, `draw_text_right`, `draw_text_center`, `scroll_update`,
`draw_scrollbar`, and the whole context-menu family (`menu_state`,
`menu_blocking`, `menu_open_on_right_click`, `menu_item`, `menu_sep`,
`context_menu`).

**Language**

`state` for the document, the selection, the drag machine and the tool
settings; `let` dataflow everywhere else; pure list-rebuilding helpers
(`put`, `del_sel`, `apply_order`, `apply_align`) instead of mutation; record
field assignment as rebinding; `for`-as-expression for point lists; function
overloading, `elsif` chains, string interpolation, color literals, and
`atan2` / `radians` / `degrees` for the rotation math.

## Notes

- A panel sleeps 10 s after the last input. This app is event-driven, so
  sleeping costs nothing — but when driving it from the debug server, any
  injected event wakes it.
- `panel.values` exposes the whole logical state by name: `doc`, `sel`, `tool`,
  `drag`, `scrub`, `grid`, `snap`, `lscroll`, `hist`, `fut`, `ops`.
- Garden draws every text run above every quad, so an open context menu would
  otherwise have the panel's own labels showing through it. The app estimates
  the menu's rect (`menu_est`) and suppresses the text bands it covers.
- Text objects have no rotation handle: the draw API cannot rotate a text run,
  so offering one would lie.

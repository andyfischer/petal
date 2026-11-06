# 24 — Kanban board

A sprint board drawn entirely by a Petal script inside a Garden panel: four
lanes, twelve cards, and drag and drop built with
[`ui/drag`](../../../core-libs/petal-ui/docs/drag.md): the board is a function
of its columns, each card says where it is (`place`) and which boards a drag
can reach (`draggable` with `closest(moves_nested(...)) |> floating()`), and
the engine does the rest. While a card is held the other cards slide apart to
show where it would land; on release it eases into its slot.

## Run it

The quickest way is `tools/run-example.ts kanban` from the repo root (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`tools/run-example.ts kanban --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/kanban
GARDEN_HEADLESS_SIZE=1280x850 \
  /Users/andy/petal/garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default). The panel pane is
1268×778 inside that; the layout is computed from `screen_width()` /
`screen_height()` every frame, so it reflows if the pane is resized — below
roughly 1000px wide the card titles start wrapping hard.

Cards animate only when they move (a drag, a keyboard move); a still board
holds its last frame until the next input wakes it.

## Controls

| Input | Effect |
|---|---|
| **Drag a card** | Lift it (shadow + accent border) and it follows the pointer; the source column closes up and the lane it would land in outlines and opens a gap where it would go, the other cards sliding out of the way. Release to drop it there; it eases into the slot. `esc` puts it back. Works within a column and across columns, including into an empty one. |
| **Click a card** | Select it — accent border, left stripe, and full detail in the footer bar. |
| **Click bare well space** | Deselect. |
| **Hover a card** | Lighter card fill. |
| **Wheel over a column** | Pixel-scroll that column; a thumb appears on the right edge and the clipped cards fade out at both ends of the well. |
| `←` `→` `↑` `↓` | Move the selection (across lanes / within a lane). With nothing selected, the first arrow selects the first card. The selection is always scrolled into view. |
| `shift`+`↑`/`↓` | Reorder the selected card within its lane. |
| `shift`+`←`/`→` | Move the selected card to the neighbouring lane, at the same row. |
| `1` … `4` | Send the selected card to the top of lane 1–4. |
| `p` | Cycle the selected card's priority (low → normal → urgent), shown as the dot in the card's top-right corner. |
| `esc` | Deselect. |

The `IN PROGRESS` and `REVIEW` lanes carry WIP limits; the count in the lane
header turns red when a lane is over its limit. The header's progress bar
tracks story points in `SHIPPED` against the board total. While a card is
dragged, the lane counts show the board it would drop.

## What it exercises

**Language**

- `class Card` to name the record shape, with typed fields.
- `state` for everything that persists across frames (`cols`, `sel_id`,
  `scrolls`, `moves`); the one `var` is the hovered card, which the view reads.
  Every board mutation is a rebind of an immutable list or record.
- Immutable list surgery: `list_remove` / `list_insert` built from collecting
  `for` loops, and nested index assignment (`cols[c] = …`,
  `cols[c][i] = card`).
- `fn view(cs, sc, sel)`: the board as a function of the columns, which the
  drag engine also runs, speculatively, on every board a drag can reach.
- Functions returning records, string interpolation, `elsif` chains for the
  colour lookups.

**Host / petal-ui**

- Draw surface: `clear`, `draw_rect`, `draw_rect_rounded` (with alpha),
  `draw_rect_outline` (with alpha and width), `draw_circle`, `clip` /
  `clip_none`.
- Text: `draw_text` with style records (`{size, color, spacing}`),
  `draw_text_center`, `draw_text_right`, `text_width` for exact measurement,
  `preview` for two-line title wrapping, `ellipsize` and `fit_parts` for the
  footer. Hierarchy is built from size, colour and letter-spacing only.
- Input: `mouse_pressed(0)`, `mouse_x`/`mouse_y`, `scroll_y`, `key_pressed`,
  `mod_shift`.
- Prelude widgets: `rect`, `point_in`, `ui_target_rect` (the hover test reads
  where each card was drawn), `ensure_visible_px`.
- `ui/drag`: `drag_session`, `place` (with `place_origin` so the wheel scroll
  is instant while reorders animate), `draggable`, `closest`, `floating`,
  `moves_nested`, `drag_key`, `is_dragging`. `scenarios/drag.json` drags a
  card from BACKLOG into REVIEW headlessly; `scenarios/browse.json` clicks,
  scrolls and uses the keyboard.

**Debug server** — every logical value is observable in
`GET /state → panes[0].panel.values`: `cols` (the full board as four id
lists), `sel_id`, `scrolls`, `moves`, and the drag session `d` (`d.active`,
`d.key`, `d.target`, `d.spec`). Drag is driven with
`/mouse {"op":"down"|"move"|"up"}`.

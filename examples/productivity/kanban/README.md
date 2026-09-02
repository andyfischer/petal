# 24 — Kanban board

A sprint board drawn entirely by a Petal script inside a Garden panel: four
lanes, twelve cards, and a real drag-and-drop that reflows the source column,
opens a gap under the pointer, and drops the card where the gap is.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

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

Nothing animates, so the 10-second panel sleep window is invisible here: the
board simply holds its last frame until the next input wakes it.

## Controls

| Input | Effect |
|---|---|
| **Drag a card** | Lift it (shadow + accent border), the source column closes up, the target column outlines and opens a "drop here" gap under the pointer; release to drop it there. Works within a column and across columns, including into an empty one. |
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
tracks story points in `SHIPPED` against the board total, and both update
live as cards move.

## What it exercises

**Language**

- `class Card` to name the record shape, with typed fields.
- `state` for everything that persists across frames (`cols`, `sel_id`,
  `scrolls`, `drag`, `moves`) — no `var`/`set` anywhere; every mutation is a
  rebind of an immutable list or record.
- Immutable list surgery: `list_remove` / `list_insert` built from collecting
  `for` loops, and nested index assignment (`cols[c] = …`,
  `cols[c][i] = card`).
- `for` in value position as a mapping expression — the per-column plan
  (`base`, `tops`, `slots`) is four one-line comprehensions.
- Functions returning records, string interpolation, `elsif` chains for the
  colour lookups.

**Host / petal-ui**

- Draw surface: `clear`, `draw_rect`, `draw_rect_rounded` (with alpha),
  `draw_rect_outline` (with alpha and width), `draw_circle`, `clip` /
  `clip_none`.
- Text: `draw_text` with style records (`{size, color, spacing}`),
  `draw_text_center`, `draw_text_right`, `text_width` for exact measurement,
  `preview` for two-line title wrapping, `ellipsize` and `fit_parts` for the
  footer. Hierarchy is built from size, colour and letter-spacing only —
  the embedded face has no bold.
- Input: `mouse_pressed(0)` / `mouse_down(0)` / `mouse_released(0)` for the
  drag gesture, `mouse_x`/`mouse_y`, `scroll_y`, `key_pressed`, `mod_shift`.
- Prelude widgets: `rect`, `point_in`, `hovered`, `ensure_visible_px`.

**Debug server** — every logical value is observable in
`GET /state → panes[0].panel.values`: `cols` (the full board as four id
lists), `sel_id`, `drag`, `scrolls`, `moves`, and the derived plan `P`
(`P.tcol`, `P.tidx`, `P.lifting`). Drag is driven with
`/mouse {"op":"down"|"move"|"up"}`.

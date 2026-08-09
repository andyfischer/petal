# 23 — Meridian CRM (contact manager)

A pure-Petal Garden panel app: a CRM contact table with segment filtering,
incremental search, click-to-sort columns, and an inspector that flips into a
multi-field edit form which writes back into the record store.

Three regions: a segment rail on the left, the contact table in the middle, a
record inspector on the right.

## Viewport

Designed for **1280×850** (Garden's headless default). The pane itself ends up
1268×778 after Garden's own chrome; every measurement in `app.ptl` is derived
from `screen_width()` / `screen_height()`, so it reflows, but the column
proportions and the 13-row table window were tuned at this size.

## Run it

```bash
cd examples/testbed/23-crm-contact-manager
GARDEN_HEADLESS_SIZE=1280x850 \
  /Users/andy/petal/garden/target/debug/garden \
  --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

Kill it by PID when you are done (`kill <pid>`), never with `pkill`.

## Controls

### Table
| Gesture | Effect |
|---|---|
| Click a column header (`CONTACT`, `COMPANY`, `STAGE`, `DEAL`, `LAST CONTACT`) | sort by that column |
| Click the same header again | flip the sort direction (the caret flips too) |
| Click a row | select it; the inspector follows |
| `↑` / `↓`, `j` / `k` | move the selection (auto-scrolls to keep it visible) |
| `PageUp` / `PageDown`, `Home` / `End` | page / jump |
| Mouse wheel over the table | scroll freely; a proportional scrollbar tracks it |

### Filtering
| Gesture | Effect |
|---|---|
| Click a segment in the left rail (`All`, `Lead`, `Qualified`, `Proposal`, `Customer`, `Churned`) | filter to that stage; the rail shows live counts |
| `/` | focus the search box |
| Type | incremental search over name, title, company, email and tags |
| `Escape` | first press drops keyboard focus, next clears the search |

Filtering never blanks the inspector: if the selected contact is filtered out,
the first visible row takes the selection.

### Inspector / form
| Gesture | Effect |
|---|---|
| `Edit record` | flip the inspector into the form, seeded from the record |
| `Tab` / `Shift-Tab` | move through the six fields (name, title, company, email, phone, deal value) |
| Click a field | focus it directly |
| Click a stage chip | set the stage |
| `Return` or `Save changes` | write the edits back into `contacts` |
| `Cancel` or `Escape` (twice) | discard |
| `Advance stage` | push the deal one step down the pipeline and reset "last contact" to today; dim and inert on `Customer` / `Churned` |

The deal-value field is digit-masked — non-numeric input is dropped as it is
typed.

## What it exercises

**Language:** `state` for the record store and every view flag, `let`-rebinding
for per-frame dataflow, `var` / `set` for the accumulators inside helper
functions, `for`-as-mapping-expression with `continue` as a filter (the whole
filter pipeline is one expression), record spread (`{...c, stage: …}`) to write
one record back into a list without mutation, a hand-written comparator +
insertion sort (there is no `sort_by` builtin), string builtins (`lower`,
`contains`, `split`, `join`, `slice`), `int("…")` string→int parsing, and hex
color literals as records.

**Host / prelude:** `rect`, `point_in`, `hovered`, `ensure_visible`,
`ellipsize`, `preview` (wrapping the notes), `draw_text_right`,
`draw_text_center`, styled `draw_text` with `spacing` for the small-caps
labels, `text_width` for exact right-alignment, `draw_rect_rounded`,
`draw_circle`, `fill_triangle` (the sort caret), `clip` / `clip_none` for the
scrolling row list, and `text_input()` / `key_pressed` for a hand-rolled text
field with a blinking caret and its own focus ring.

**Observability:** `seg`, `q`, `sort_key`, `sort_dir`, `sel_id`, `scroll`,
`visible_rows`, `selection`, `editing`, `focus_id`, `saves` and the whole
`contacts` list are all readable from
`GET /state → panes[0].panel.values`, which is how every interaction above was
verified.

## Notes

- The app is entirely event-driven — no animation loop — so the 10 s panel
  sleep is invisible here. The only thing that stops is the caret blink in a
  focused text field.
- The scrollbar is an indicator, not a drag target; scroll with the wheel or
  the keyboard.

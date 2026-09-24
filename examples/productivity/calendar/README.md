# 16 — Calendar (Almanac)

A week / day / month calendar drawn entirely by a Petal script inside a Garden
panel. A paper-toned sidebar holds a mini month, the calendar toggles and an
"up next" list; the main area is a time grid (week or day) or a month grid.
Events are dragged to move, dragged by their bottom edge to resize, and drawn
into existence by dragging on empty grid. Clicking one opens a popover to
rename it, recolour it, nudge its length or delete it. Everything is undoable
and persists through the panel store.

There is no date builtin in Petal, so the calendar arithmetic (day numbers,
weekdays, month stepping, ISO week numbers) is written in the script.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/calendar
GARDEN_HEADLESS_SIZE=1280x850 GARDEN_PANEL_STORE_DIR=/tmp/almanac-store \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane. The sidebar is fixed at 268 px and the day view's
agenda column at 330 px; the grid columns share the rest, so other sizes
reflow.

The events and the chosen view are saved to the panel store on every change,
so a second launch shows what you left rather than the demo. Launch tests with
`GARDEN_PANEL_STORE_DIR` pointing at a scratch directory. **Reset demo data**
at the bottom of the sidebar restores the seed calendar (and is undoable).

"Today" is pinned to **Wednesday 23 September 2026** and the clock starts at
10:42, advancing with the panel's `time()`, so the demo always has a past, a
present and a future. Like every headless panel, this one stops running
frames after 10 s without input; a frozen now-line is the panel asleep, not a
hang.

## Controls

**Time grid (week and day)**

| Input | Effect |
|---|---|
| click an event | select it and open its popover |
| drag an event | move it; snaps to 15 minutes, can change day; the grid reflows live and a tag shows the new time |
| drag an event's bottom edge | resize it (a grip appears on hover) |
| drag on empty grid | create an event over the dragged range, then type its title |
| double-click empty grid | create a one-hour event there |
| double-click an event | open its popover with the title field focused |
| drag an all-day bar | move it by whole days (multi-day bars keep their span) |
| wheel | scroll the hours; dragging near the top or bottom edge scrolls too |

**Month grid**

| Input | Effect |
|---|---|
| click a day | make it the focus day |
| double-click a day, or click "+N more" | open that day in the day view |
| click a chip | select the event and open its popover |
| drag a chip | move the event to another day, keeping its time |

**Popover**

| Input | Effect |
|---|---|
| click the title | edit it (left/right/home/end, backspace, typing); `return` or `esc` finishes |
| a colour dot | move the event to that calendar |
| `- 15m` / `+ 15m` | shorten / lengthen the event |
| Delete | delete the event |

**Keyboard** (while no title is being edited)

| Key | Effect |
|---|---|
| `left` / `right` | previous / next day, week or month (with nothing selected) |
| `t` | jump to today |
| `d` / `w` / `m` | day / week / month view |
| `n` | new event on the focus day (the next half hour today, 9 AM otherwise) |
| `up` / `down` | move the selected event 15 minutes |
| `shift` + `up` / `down` | shorten / lengthen it by 15 minutes |
| `left` / `right` with a selection | move the selected event a day |
| `backspace` / `delete` | delete the selected event |
| `return` | open the selected event's popover |
| `esc` | close the popover, then clear the selection |
| `Cmd+Z` / `Shift+Cmd+Z` | undo / redo (60 levels; a title edit is one step) |

**Sidebar and header**

| Input | Effect |
|---|---|
| New event | a new event on the focus day, title focused |
| mini month: a date | jump there; `<` `>` page the mini month on its own |
| a calendar row | show or hide that calendar everywhere |
| Day / Week / Month | choose the view |
| `<` Today `>` | step the period, or return to today |

## What it exercises

**Language**

- Civil-date arithmetic from first principles: `days_from_civil` / `civil`
  (Hinnant's algorithms), `weekday`, `add_months` with day clamping, and
  `iso_week`. It relies on `/` being integer division.
- Overlap packing for the time grid (`pack_day`): events sort by start, form
  clusters of transitively overlapping events, take the first free column,
  and share the cluster's width. All-day bars pack into lanes the same way
  (`pack_allday`).
- A single `plan` function that builds every rect a frame draws and
  hit-tests, from the events with the in-flight drag already applied, so the
  packing reflows under the pointer. It runs three times a frame: before
  input, on release to commit the preview, and for the draw.
- Immutable edits: `replace_ev` is a collecting `for`, `remove_ev` a
  `filter`, undo and redo are stacks of whole event lists (`last`,
  `drop_last`), capped by `slice`.
- Records with spread (`{...e, day: z}`), `??` / `?.` on optional lookups,
  lambdas passed to `sort`, `filter` and `reduce`, and `state` for everything
  that lives across frames.

**Host / petal-ui**

- Text in the `ui` face with real weights, `text_width` for alignment,
  `ellipsize`, `text_layout.draw_text_line` for cap-height centring in
  buttons, and a hand-written word wrap (see Known limits).
- `draw_shadow` / `draw_elevation` for the popover and the lifted event,
  `tint` / `mix` for opaque event washes, `clip_push` / `clip_pop` for the
  grid and per-block text, `fill_triangle` for the popover nub and all-day
  continuation arrows, `draw_polyline` for chevrons and checkmarks.
- Input: `text_field_update` (prelude) as the input half of the title field
  with a custom-drawn box, `click_count` for double clicks, `claim_key` for
  `Cmd+Z` and `Shift+Cmd+Z`, `scroll_y` scoped to the grid, `request_frame`
  during drags and while the caret blinks, `panel_store_get` /
  `panel_store_set` with `json_stringify` / `json_parse`.

**Debug server.** The assertable values in `panes[0].panel.values`, all
prefixed `obs_` (`/state?values_prefix=obs_`): `obs_view`, `obs_focus` (a day
number) and `obs_focus_date` (ISO), `obs_sel` / `obs_pop` (event ids),
`obs_sel_ev` (the selected event record), `obs_events`, `obs_undo` /
`obs_redo`, `obs_drag` (`none`, `move`, `resize`, `create`, `amove`,
`mmove`), `obs_scroll`, `obs_hidden`, `obs_editing`. The state vars
`created`, `moved_n`, `deleted` and `saves` count edges a test can check
later.

## Known limits

- **No date or wall-clock builtin.** Petal has `time()` (seconds since the
  panel started) and nothing else, so "today" and "now" are pinned demo
  values rather than the real date.
- **`text_wrap` breaks a word too wide for the box** (by design: it never
  overflows). In a narrow event block that turned "Usability" into
  "Usabilit / y", so the app wraps by words itself (`wrap_words`) and
  ellipsizes a word that cannot fit instead.
- **No selection in the text field.** `text_field_update` has a caret but no
  selection, so a new event's title starts empty with an "Add title"
  placeholder rather than as a selected "New event". An event whose editor
  closes with an empty title becomes "New event".
- **Multi-day events in the month grid** draw as one chip per day, not as a
  continuous bar across the week row.
- **The type checker narrows `let pv = nil` to `nil` for good.** Passing it to
  a `record`-annotated parameter after rebinding it to a record warned, so
  `replace_ev`'s second parameter is unannotated.
- `hidden` (calendar visibility) is not persisted; only events and the view
  are.
- No recurrence model: the standup and gym sessions are separate seeded
  events.

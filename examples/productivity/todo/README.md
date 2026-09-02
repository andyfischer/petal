# 14 — Thicket (todo app)

A task manager written as a single Garden panel script. Three columns:

- **Rail** — the project lists with open counts, a "done today" progress card,
  and the autosave/undo status line.
- **Feed** — an inline composer, the Active/Done/All status filter, and the
  tasks themselves grouped by when they are due (Overdue, Today, Next seven
  days, Later, Someday, Completed), sorted by priority inside each group.
- **Inspector** — the selected task: editable title, status/list/due chips, a
  priority picker, the schedule, notes, and a clickable subtask checklist.

Everything is CRUD over one `state tasks` list of records. Each mutation bumps
a revision counter, which the rail reports as `autosaved · rev N`.

## Viewport

Designed for Garden's default headless viewport, **1280x850** — which gives the
panel pane **1268x778** logical pixels. The layout is computed from
`screen_width()`/`screen_height()`, so it degrades gracefully, but the column
widths were tuned at that size.

## Run

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/todo
garden/target/debug/garden --headless --debug-port 0 --init layout.ptl
```

From the repo root, with the debug server on a fixed port:

```bash
cd examples/productivity/todo && \
  /Users/andy/petal/garden/target/debug/garden --headless --debug-port 8080 --init layout.ptl
```

Drop `--headless` for a window.

## Controls

**Mouse**

| Action | Effect |
|---|---|
| Click a row | select it |
| Click the circle at the left of a row | complete / reopen the task |
| Click a rail entry | filter to that list |
| Click Active / Done / All | filter by status |
| Click the composer | start typing a new task |
| Click the search box | filter by text over titles, lists and notes |
| Click the inspector title | rename in place |
| Click a priority chip | set None / Low / Medium / High |
| Click a checklist row | toggle that subtask |
| Complete / Delete | act on the selected task |
| Wheel over the feed | scroll |

**Keyboard** (when no text field has focus)

| Key | Effect |
|---|---|
| `j` / `k`, arrows, PageUp/PageDown, Home/End | move the selection |
| `space` or `x` | complete / reopen the selected task |
| `0` `1` `2` `3` | set priority |
| `,` / `.` | pull the due date a day earlier / push it later |
| `e` | rename the selected task |
| `d` or Delete | delete it |
| `u` | undo the last delete |
| `n` | focus the composer |
| `/` | focus search |
| Tab | swap focus between the composer and search |
| Esc | drop focus, then clear the search |

**Composer syntax** — `!` medium, `!!` high, `@today` `@tomorrow` `@week`
`@someday`, and `#product` `#home` `#errands` `#reading` to file it in a list.
A live chip row on the right of the composer shows exactly what will be
created. Return adds the task and leaves the composer focused for the next one.

Completing or deleting the selected task hands the selection to whichever row
took its place, so a burst of `space` walks down the list.

## What it exercises

- **Language:** records and record spread as the update mechanism
  (`{...t, done: !t.done}`), `for`-in-value-position as map/filter (including a
  nested one inside a record field, to rewrite a subtask list), `var`/`set`
  accumulators inside helpers, string interpolation, immutable list append and
  slice for the undo stack, an insertion sort over a comparator, keyed
  multi-way `if`/`elsif` classification.
- **Host / petal-ui:** `state` across frames and hot reload, `ellipsize`,
  `preview`/`wrap`, `fit_parts`, `ensure_visible_px`, styled `draw_text`
  records, rounded rects and translucent fills, `clip` for the scrolled feed,
  exact `text_width` for right-alignment and wrap widths, `text_input()` +
  `key_pressed` for three hand-rolled focusable text fields.
- **Debug server:** every piece of logical state is observable in
  `/state → panes[0].panel.values` — `selection`, `visible_count`, `focus_id`,
  `revision`, `trash_size`, plus `tasks` itself.

## Notes

- There is no file-IO native in a panel, so "persistence" here is the `state`
  store surviving hot reload plus the revision counter; restarting Garden
  restores the seed data.
- The panel sleeps ten seconds after the last input (Garden's wake heuristic).
  Nothing here animates except the text cursor blink, which resumes on the next
  keystroke.

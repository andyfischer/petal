# 22 — Command palette (Beacon notes)

A command palette drawn entirely by a Petal script inside a Garden panel,
over a small notes workspace it commands: five seeded documents, a document
view with line numbers and a current line, a sidebar of files and recent
commands, and a status bar. `Cmd K` opens the palette; typing fuzzy-filters
forty-two commands with the matched letters highlighted; arrows, Return and
Escape drive it; some commands turn the palette into a sub-mode (a line
number prompt, a file picker) instead of closing it; toasts confirm what ran.

The point of this entry is **keyboard handling, fuzzy search and overlays**:
claimed chords next to bare-letter navigation, `text_input` with three kinds
of backspace, a subsequence matcher with scoring, and a floating panel plus
toasts composited over live content with a scrim.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/command-palette
GARDEN_HEADLESS_SIZE=1100x760 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1100×760** viewport, which gives the panel a 1088×688 pane.
The sidebar is fixed at 232 px and the palette at 640 px wide; the document
takes the rest, so other sizes reflow.

Nothing is persisted; every launch starts from the five seed documents. The
panel sleeps 10 s after the last input, so a toast that seems stuck has
simply stopped fading until the next key.

## Controls

**Workspace (palette closed)**

| Input | Effect |
|---|---|
| `Cmd K`, `Ctrl K`, `Cmd Shift P`, or click the search box | open the command palette |
| `Cmd O` / `Ctrl O` | open the file picker |
| `Ctrl G` | open the go-to-line prompt |
| `Cmd B` | toggle the sidebar |
| `Cmd T` | cycle the theme (dark → light → midnight) |
| `Cmd S` | save (clears the edited flag) |
| `↓` / `j`, `↑` / `k`, `pagedown` / `pageup` | move the current line |
| click a line | make it current |
| click a file in the sidebar | open it |
| wheel over the document | scroll |

**Palette**

| Input | Effect |
|---|---|
| typing | filter; the selection returns to the top |
| `backspace` | delete one character; `alt` deletes back to the previous word; `cmd` clears the query |
| `↓` / `↑`, `Ctrl N` / `Ctrl P` | move the selection, skipping section headers and wrapping |
| `pagedown` / `pageup` | move eight rows |
| `home` / `end` | first / last row |
| `return` | run the selected command (or open the file, or jump to the line) |
| `escape` | close; in a sub-mode, back to the command list |
| `Cmd K` | close |
| hover a row | select it; click runs it |
| wheel | scroll the list |
| click outside | close |

With an empty query the list is grouped: **recently used** (the last five
commands run) first, then every command by category. With a query the rows
are ranked by match score, and a command in the recent list gets a small
boost. A query that matches nothing in a label is retried against
`Category label`, so `vz` finds the three zoom commands.

**Commands** (42): File (new, save, save all, close, rename, reveal), Go (to
file…, to line…, top, bottom, next / previous file), Edit (upper / lower /
title-case the line, duplicate, delete, move up / down, trim trailing
whitespace, sort, reverse, toggle task checkbox), Insert (date, rule, task,
heading), View (sidebar, line numbers, zoom in / out / reset, status bar,
focus mode), Theme (dark, light, midnight, cycle), Help (shortcuts, about),
Palette (clear recent, reset demo documents).

## What it exercises

**Language**

- A fuzzy matcher (`fuzzy_from`, `fuzzy`) over `chars(lower(...))` lists: a
  greedy subsequence walk with a `while` inner search, `return nil` on a
  miss, consecutive and word-start bonuses, a gap penalty, and a second pass
  anchored at the first word-start occurrence; the better score wins.
- Rows as tagged records (`{kind: "header" | "cmd" | "file" | "line", …}`)
  built by one function per mode, sorted with a comparator lambda, walked
  with `step_sel` (wrapping, header-skipping) for the keyboard.
- A `match` over the command id with `do … end` arms as the whole command
  dispatcher; edits on an immutable line list through `set_lines`, which
  rebuilds the file list with a collecting `for`; `replace_line` takes a
  lambda; `swap_lines`, `insert_after`, `trim_right`, `title_case` are small
  pure helpers over `chars` / `char_slice` / `split` / `join`.
- `state var` for everything that persists, read with `get` inside every
  helper; `theme_for` returns a palette record and `theme_set` projects it
  onto the prelude so `draw_scrollbar` and `draw_elevation` follow the theme.

**Host / petal-ui**

- `claim_key` for eleven chords (`k`, `o`, `g`, `b`, `t`, `s`, `backspace`
  under `cmd`, `ctrl`, `alt`, `cmd+shift`), `text_input()` for the query,
  `mod_cmd` / `mod_ctrl` / `mod_alt` / `mod_shift` to split the chords, and
  a `chord` guard so the `k` of `Cmd K` does not also run the bare-letter
  navigation.
- Overlays: a full-pane scrim, `draw_elevation` level 3 under the palette
  and level 2 under toasts, `clip_push`/`clip_pop` for the row list and the
  document, `ensure_visible` and `draw_scrollbar` from the prelude, keycaps
  drawn from a caption split on spaces.
- Match highlighting as runs: `draw_highlighted` walks the label's
  characters, flushes a run when the matched flag changes, and measures each
  run with `text_width` in the exact style it draws it (bold accent for a
  hit, plain for a miss).
- Three faces: `ui` for chrome and the palette, `mono` for the document at a
  zoomable size, with the line height derived from the size.
- Mouse: `point_in` hit-testing for rows, the search box, sidebar entries
  and document lines; hover-selects only when the pointer actually moves
  (a `state` of the last pointer position), so keyboard navigation is not
  overridden by a resting pointer; `hover_first` is not needed.

**Debug server** — the assertable values in `panes[0].panel.values`:
`obs_open`, `obs_mode`, `obs_query`, `obs_sel`, `obs_rows`, `obs_results`,
`obs_top` (the selected row's command id, file name or prompt text),
`obs_ranked` (the first five row ids), `obs_runs`, `obs_last`, `obs_opens`,
`obs_recent`, `obs_file`, `obs_files`, `obs_line`, `obs_line_text`,
`obs_lines`, `obs_theme`, `obs_zoom`, `obs_sidebar`, `obs_numbers`,
`obs_status`, `obs_dirty`, `obs_saves`, `obs_toasts`. A command is one
sequence: `POST /key {"key":"k","mods":["cmd"]}`, `POST /text {"text":
"zoo"}`, `POST /key {"key":"return"}`, then read `obs_last` and whatever the
command changed.

## Known limits

- **`Cmd P` / `Ctrl P` cannot be claimed.** Garden opens its own fuzzy file
  finder on that chord before the panel's claims are consulted
  (`garden-app/src/app/input.rs`, `key_outcome`), even though
  `petal-graphical-panels.md` says only `Cmd Q` is unclaimable. Typing into
  the finder then opens a real file into the pane, replacing the panel. The
  file picker is on `Cmd O` instead. `Ctrl W` is reserved the same way (the
  window-command prefix) and `Cmd W` closes the window, so no command is
  bound to either.
- The Garden binary the app was built against predates `text_advance` and
  the three-argument `range(a, b, step)`, although `petal check --host
  garden` accepts both. Highlight runs are measured with `text_width`
  (rounded to whole pixels, so a long label can drift by a pixel or two
  between runs), and the keycap loop counts down with a `while`.
- No text selection or caret movement inside the query: the caret is always
  at the end, and there is no clipboard.
- The document is read-only except through commands; there is no free
  typing into it, which keeps the palette the only editing surface.
- `insert date` inserts a fixed date, since there is no clock-to-calendar
  builtin.

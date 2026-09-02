# 15 — Vellum (notes app)

A notes app drawn entirely by a Petal panel script: a notebook rail, a
searchable note list, and a **real multi-line text editor** on the right — soft
word wrap, a placed caret, click-and-drag selection, shift-arrow selection,
double-click word select, and live search highlighting inside the body.

Nothing here is a host widget. The wrap engine, the caret, the selection model,
the edit primitives (insert / delete / split / join) and the undo history are
all written in Petal over `state`, immutable lists and the `draw_*` natives.

## Viewport

Designed for **1280x850** (Garden's default headless size), which gives the
panel a pane of 1268x778. The layout is written against `screen_width()` /
`screen_height()`, so it reflows, but the three-column proportions are tuned
for that size.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/notes
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

Kill it by PID when you are done. Note that injected mouse coordinates are
window-relative: the pane's origin is `(6, 38)`, so panel `(x, y)` is screen
`(x + 6, y + 38)`.

## Controls

**Rail**

| | |
|---|---|
| click a notebook | filter the list to it (`All` clears the filter) |
| `undo last edit` | pop the newest snapshot — restores an edited *or deleted* note |

**Note list**

| | |
|---|---|
| click the search field, type | filter by title and body, case-insensitive |
| `esc` in the field | clear the query and return to the list |
| `x` in the field | clear the query |
| `+` | new note (opens with the title focused) |
| click a card | open that note |
| right-click a card | open/close its inline action row: Pin, Duplicate, Delete |
| wheel over the list | scroll |
| `up` / `down` when the list has focus | move the selection |
| `return` when the list has focus | jump into the body |

**Editor**

| | |
|---|---|
| click the title | rename (typing appends, `backspace` deletes) |
| click the body | place the caret |
| drag | select |
| double-click | select the word |
| `shift` + arrows / `home` / `end` | extend the selection |
| arrows | move by display row / character, with a remembered goal column |
| `home` / `end` | start / end of the **display** row (wrap-aware) |
| `pageup` / `pagedown` | page, caret kept on screen |
| typing | insert, replacing any selection |
| `backspace` / `delete` | delete the selection, or one character, joining lines at the edges |
| `return` | split the line |
| `esc` | drop the selection, then leave the body |
| `ctrl+s` | save now |
| wheel | scroll the page |
| `tab` / `shift+tab` | cycle focus: search → list → title → body |

Edits autosave three seconds after the last keystroke; the rail shows `unsaved`
/ `saved` and counts the writes.

## What it exercises

- **Text editing** — a line/column document model with pure edit primitives
  (`delete_range`, `insert_text`) that return new documents, plus a greedy
  word-wrap pass (`wrap_starts` / `build_rows`) that turns logical lines into
  contiguous display rows so the caret, the selection and click hit-testing all
  agree at a wrap point.
- **Selection** — anchor/head positions, ordered with `pos_before`, rendered as
  per-row bands, driven by drag, shift-arrows and a timed double-click.
- **Persistence** — `saved` is a snapshot of the whole store taken by value
  (Petal values are immutable, so the snapshot really is one), `history` is a
  bounded stack of per-note snapshots, and dirty tracking drives idle autosave,
  `ctrl+s`, and an undo that can resurrect a deleted note at its old index.
- **Search** — case-insensitive matching over titles and bodies filters the
  list, and `find_all` highlights every occurrence in the visible body rows.
- **Host surface** — `clip`, per-run text styles, `text_width`-exact
  right/centre alignment, pixel scrolling with proportional scrollbars, hover
  states, right-click, `mod_shift`/`mod_ctrl`, and `panel.values` observability
  (`sel_id`, `cur`, `anchor`, `q`, `focus`, `dirty`, `saves`, `edits` are all
  readable from `GET /state`).

## Notes

- The panel sleeps ten seconds after the last input, which is invisible here —
  nothing animates, so a sleeping frame is the correct frame. The caret is
  drawn solid rather than blinking for the same reason.
- There is no file I/O available to a panel script, so "persistence" is an
  in-memory store with an explicit saved snapshot rather than a file on disk.

# 19 — File browser (Burrow)

A Finder-style file browser drawn entirely by a Petal script inside a Garden
panel. A folder tree in the sidebar, the open folder as a sortable list or an
icon grid, breadcrumbs with back and forward, an info pane and Quick Look for
the selection, and the verbs a file browser needs (open, rename in place,
duplicate, new folder, move by drag and drop, trash, put back, undo) reached
three ways each: keys, a right-click menu and the mouse.

It browses a **virtual** filesystem. Nothing here reads or writes a disk: the
tree is a list of about 160 node records seeded from `fsdata.ptl` (a home
directory with projects, tax folders, a camera roll, music, some code), and
every change is a new list. That is also why undo is one line: a step is a
snapshot of the list.

## Run it

The quickest way is `tools/run-example.ts file-browser` from the repo root (it
finds the `garden` binary and sets the viewport; extra arguments are passed
through, e.g. `tools/run-example.ts file-browser --headless --debug-port 0`).
By hand:

```bash
cd examples/productivity/file-browser
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s "127.0.0.1:$PORT/screenshot?pane=0" -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane: a 232 px sidebar, a 292 px info pane and 744 px of
folder between them (six tiles across in the icon view). The info pane can be
hidden; the name column and the grid take the room. A narrower pane gives up
the list's Kind column below 600 px of folder and Date Modified below 470,
and the breadcrumbs fold their front into `…`.

Nothing is persisted. The filesystem lives in `state`, so it survives a hot
reload and is back to the seed after `POST /panel/reset` or a restart. No
`GARDEN_PANEL_STORE_DIR` is needed.

The panel sleeps 10 s after the last input, like every Garden panel. Nothing
here moves on its own: the only timed things are the status-bar notice (fades
after 5 s), the type-ahead buffer (0.9 s) and Quick Look easing in (0.14 s).

## Files

| File | What is in it |
|---|---|
| `fsdata.ptl` | The seed tree as `path\|size\|days ago\|time` lines, the text behind the readable files, and the pure helpers: kinds by extension, size and date formatting, name splitting, a name hash |
| `icons.ptl` | File and folder icons (a coloured object per kind, drawn the same at 16 px and 96 px) and the one-colour chrome glyphs |
| `peek.ptl` | What a file looks like inside: procedural photographs, text and code cards, a CSV table, a page mock, a waveform, a movie frame |
| `app.ptl` | One frame: state, metrics, the model (index, items, tree rows), the actions, input, drawing, overlays |

## Controls

**Main view, mouse**

| Input | Effect |
|---|---|
| click | select one item |
| `⌘`-click | add or remove one item |
| `⇧`-click | select the range from the anchor |
| drag from the background | rubber band: adds everything it touches (in the icon view the gaps between tiles are background) |
| double click | open: a folder becomes the open folder, a file opens in Quick Look |
| drag an item (or the selection) | move it into the folder it is dropped on: a folder in the view, a sidebar row, a breadcrumb or the Trash. The target lights up and the status bar says what will happen |
| right click an item | Open, Quick Look, Rename, Duplicate, New Folder with Selection, Move to Trash. In the Trash: Put Back, Delete Immediately |
| right click the background | New Folder, as List / as Icons, Sort by …, Select All, Empty Trash |
| click a column header | sort by it; again to reverse. Folders always come first |
| wheel | scroll |

**Main view, keys**

| Key | Effect |
|---|---|
| `↑` `↓` (and `←` `→` in the icon view) | move the selection; with `⇧`, extend it |
| `home` `end` `pageup` `pagedown` | jump; with `⇧`, extend |
| letters | type-ahead: select the first name starting with what was typed in the last 0.9 s |
| `return` | rename the selected item in place |
| `space` | Quick Look |
| `⌘O` or `⌘↓` | open |
| `⌘↑` | the enclosing folder (the one you left stays selected) |
| `⌘←` / `⌘→` | back / forward |
| `⌘A` | select all |
| `⌘D` | duplicate (`name copy.ext`; a folder is copied with its contents) |
| `⇧⌘N` | new folder, straight into rename |
| `⌘⌫` or `delete` | move to the Trash; in the Trash, delete for good |
| `⌘Z` / `⇧⌘Z` | undo / redo any change to the filesystem |
| `⌘F` | focus the filter box |
| `⌘1` / `⌘2` | list / icons |
| `⌘I` | show or hide the info pane |
| `tab` | move keyboard focus between the folder and the tree |
| `escape` | clear type-ahead, then the filter, then the selection |

**Renaming** (the prelude's text field): type, arrows, `⇧`-arrows, `⌘A`,
`⌘C/X/V`, `⌘Z`. `return` commits, `escape` cancels, a click elsewhere commits.
An empty name cancels; a name with `/` or one already used in the folder is
refused, the box turns red and the status bar says why.

**Sidebar**

| Input | Effect |
|---|---|
| click a row | open that folder |
| click a chevron | fold or unfold that folder |
| right click a row | Open, Expand / Collapse, New Folder Inside, Rename, Move to Trash; on the Trash: Open, Empty Trash |
| with tree focus (`tab`): `↑` `↓` | walk the visible rows, opening each |
| `→` / `←` | unfold, then step in / fold, then step out |
| `return` | back to the folder view |

The tree marks the open folder. A favourite wears the mark only when the
tree cannot (the folder's row is folded away).

**Toolbar**: back, forward, enclosing folder; breadcrumbs (click one to go
there; the front of a long path collapses into `…`); list / icons; the filter
box (narrows the open folder by name, highlights the match, `escape` clears,
`return` or `↓` drops into the results); the info-pane toggle.

**Quick Look**: `←` `→` walk the folder, `space` or `escape` or a click
outside closes.

## What it exercises

**Trees.** The filesystem is a flat list of nodes with parent ids; an index
(children per folder, sorted folder children, recursive sizes and file
counts) is rebuilt only when the list changes. The sidebar is a recursive
walk over it that emits one row per visible folder with its depth; unfolding
is a list of ids. Navigation keeps the tree unfolded down to the open folder
and scrolls its row into view.

**Icons.** Eleven file kinds and the folder are drawn from fills, strokes
and gradients, each inside whatever rect it is given: 18 px in a row, 60 px
in a tile, 96 px in the info pane. Images and movies are their own icons in
the grid: a procedural landscape (or a screenshot, or a logo) chosen by a
hash of the name. Twenty chrome glyphs are strokes in the caller's ink.

**Selection.** Single, toggled and range selection by mouse and by keys with
a separate lead and anchor; rubber-band selection; type-ahead; selection that
survives a sort and follows a trash ("the next one is selected"); contiguous
selected rows drawn as one block.

**Context menus.** The prelude's `context_menu`, with three different menus
built from what was right-clicked and what state it is in (disabled entries,
counts in labels, a tick on the current view and sort).

Language: immutable values carry the design (`commit` swaps a whole list
and undo keeps the old ones), along with the built-in `Rect`, collecting `for`
loops, `sort_by` chains for a stable multi-key order, `match` on strings,
default and named arguments, `state var` cells shared by top-level functions,
recursion (`tree_walk`), raw strings for the seed documents, and three
imported modules.

Host and prelude: `text_field_update` for the rename editor and the filter
box (drawn by the app), `context_menu` / `menu_show` / `menu_blocking`,
`claim_key`, `click_count`, modifiers on mouse and keys, `clip_push` with a
rounded mask, `draw_shadow`, gradients, `fill_polygon`, `draw_polyline`,
`text_layout` for every label, `text_range_rects`, `request_frame`.

**Values a test can read** (`panes[0].panel.values`):

| Name | Meaning |
|---|---|
| `obs_cwd` | the open folder as a path, `Home/Documents` |
| `obs_items` | names in the main view, in display order |
| `obs_sel`, `obs_lead` | selected names (display order), and the keyboard's one |
| `obs_view`, `obs_sort`, `obs_filter`, `obs_focus` | `list`/`grid`, e.g. `date desc`, the filter text, `main`/`tree`/`filter`/`rename` |
| `obs_tree`, `obs_expanded` | names of the visible tree rows; how many folders are unfolded |
| `obs_history`, `obs_can_back`, `obs_can_forward` | the visit stack |
| `obs_renaming`, `obs_rename_buf` | the item being renamed and the editor's text |
| `obs_menu`, `obs_menu_items` | the open menu's kind (`item`/`bg`/`side`) and its labels |
| `obs_trash`, `obs_nodes`, `obs_home_bytes` | names in the Trash, live node count, total size under Home |
| `obs_undo`, `obs_redo`, `obs_ops`, `obs_notice` | stack depths, changes so far, the status-bar notice |
| `obs_quick_look`, `obs_typed`, `obs_info` | the previewed name, the type-ahead buffer, whether the info pane shows |
| `obs_dragging`, `obs_drop` | how many items a drag carries and the folder it is over |
| `obs_scroll`, `obs_max_scroll` | the main view's scroll |
| `obs_item_pts`, `obs_side_pts`, `obs_crumb_pts`, `obs_hit` | pane-local points to click: every item, sidebar row (with its chevron's x), breadcrumb and toolbar cell |

A tree row scrolled out of the sidebar still appears in `obs_side_pts` with a
`y` outside the tree's viewport; clicking there hits whatever is drawn at
that pixel (usually the Trash row). Scroll it into view first.

## Known limits

- **Rename cannot pre-select the name without its extension.** The prelude
  text field owns its caret and selection as internal state and offers no way
  to set them, so the editor opens with the caret at the end. `⌘A` then
  typing replaces everything.
- **No `⌘[` / `⌘]` and no `F2`.** `leftbracket`, `rightbracket` and `f2` are
  in petal-ui's key vocabulary, but Garden does not deliver them: `POST /key`
  answers "a canonical key name Garden cannot deliver", and a `[` arrives
  with no key name at all, only as text (and with `⌘` held, as nothing). Back
  and forward are `⌘←` / `⌘→` instead, and rename is `return` only.
- **The context menu is flat.** `context_menu` has no submenus, shortcut
  hints or check marks, so "Sort by" is four entries and the tick is a `✓`
  typed into the label (drawn from a fallback face, since Inter has none,
  with spaces padding the other labels to line up).
- **Quick Look eases in.** A capture taken on the very frame it opens shows
  the card 12 px low under a lighter scrim; `POST /tick` a few frames, or use
  `/screenshot?settle=idle`, for the settled picture.
- **No scrollbar dragging and no auto-scroll.** The scrollbars are
  indicators; a drag or a rubber band does not scroll the view when it
  reaches an edge, so a move across a long folder goes by way of the sidebar.
- **The filter is the open folder only**, not a search of everything beneath
  it.
- **A put-back whose original folder is gone lands in Home.**
- **Record literals take identifier keys only**, so the table of document
  texts is a list of `[name, text]` pairs searched in order rather than a
  record keyed by file name. A raw `"""` string also cannot end in a quote
  character, which the one TOML sample ending in `"assets"` ran into (it
  carries a trailing newline and is trimmed on read).
- **Two injected clicks on one pixel are a double click.** `POST /mouse`
  with the default `clicks: 1` still chains with a click sent a moment
  earlier at the same point, so a test that clicks the same row twice opens
  it. Once a test has called `POST /tick` the panel's clock is virtual, so
  waiting does not break the chain and neither does moving the pointer away
  and back: tick some time (`tick 10 0.1`) or click a different pixel of the
  row. (The app opens on the second click's *release*, so a quick second
  press that turns into a drag still drags.)
- **After a chord through `POST /key`, a following `POST /text` is dropped**
  until a plain key arrives. A test that sends `⌘A` and then types into the
  rename editor has to send the first character with `/key`.

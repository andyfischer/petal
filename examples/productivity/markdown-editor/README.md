# 39 — Markdown editor + preview (Inkwell)

A Markdown editor drawn entirely by a Petal script inside a Garden panel. A
dark source editor on the left, a rendered page on the right, a draggable
splitter between them, and a rail with the documents and a live outline. The
preview follows every keystroke; ticking a task in the preview edits the
source; clicking a heading in the preview or the outline puts the caret on
its line. Documents persist through the panel store.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/productivity/markdown-editor
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane. The rail is fixed at 210 px; the editor and preview
share the rest through the splitter (each at least 300 px), so other sizes
reflow.

Every document is saved to the panel store 1.5 s after the last edit (or on
`⌘S`), so a second launch shows what you left, not the demo. Launch tests
with `GARDEN_PANEL_STORE_DIR=/tmp/somewhere` for a scratch store; the rail's
**reset demo text** button restores the three seed documents and clears the
store.

## What renders

| Block | Source |
|---|---|
| Headings | `#` … `######`; an H1 gets a rule under it |
| Paragraphs | consecutive lines join; blank lines separate |
| Emphasis | `**bold**`, `*italic*` / `_italic_`, `***both***`, `~~strike~~`, `` `code` `` |
| Links | `[text](url)`; hovering shows the URL in the status bar |
| Bullet lists | `-`, `*` or `+`, nested by two-space indent |
| Numbered lists | `1.` … with the numbers taken from the source |
| Task lists | `- [ ]` / `- [x]`; click the box in the preview to toggle the source |
| Block quotes | `>` lines, merged into one quoted paragraph |
| Fenced code | ```` ```lang ```` … ```` ``` ````, with the language tagged |
| Tables | pipe rows with a `---` separator; the first row is the header |
| Rules | `---`, `***` or `___` |

Emphasis opens only at a word boundary, so `snake_case` stays plain.
Everything else is treated as text.

## Controls

**Editor**

| Input | Effect |
|---|---|
| click / drag | place the caret / select |
| double-click | select the word |
| click the gutter | select the whole line |
| `shift` + arrows, `home`, `end` | extend the selection |
| `home` / `end` | start / end of the display row; `⌘` for the logical line |
| `pageup` / `pagedown` | page, caret kept on screen |
| typing | insert, replacing any selection |
| `return` | split the line; on a list item, continue the list (`- `, `- [ ] `, `4. `); on an empty item, end the list |
| `tab` | insert two spaces |
| `backspace` / `delete` | delete the selection or one character, joining lines at the edges |
| `⌘B` / `⌘I` / `⌘K` | wrap the selection (or the word at the caret) as bold / italic / a link |
| `⌘Z` / `⇧⌘Z` | undo / redo (100 levels; typing bursts collapse into one step) |
| `⌘A` | select all |
| `⌘S` | save now |
| `esc` | drop the selection |
| wheel | scroll |

**Preview**

| Input | Effect |
|---|---|
| click a task box | toggle `[ ]` / `[x]` in the source (undoable) |
| click a heading | put the caret on its source line and scroll the editor to it |
| hover a link | show its URL in the status bar |
| wheel | scroll the preview on its own |

**Chrome**

| Input | Effect |
|---|---|
| Editor / Split / Preview, or `⌘E` | choose the view (`⌘E` cycles) |
| sync scroll | when on, the preview follows the editor's first visible line |
| drag the divider | resize the panes |
| rail: a document | open it |
| rail: an outline entry | jump to that heading |
| rail: reset demo text | restore the seed documents |

The status bar shows the caret position or selection size, word and line
counts, reading time, and the save state.

## What it exercises

**Language**

- A hand-written Markdown parser in two layers: `parse_blocks` walks the
  line list into block records (a `while` with lookahead for fences, quotes,
  tables and lazy paragraph continuation), and `parse_inline` recurses into
  matched marker pairs, carrying a style record through the spread operator
  (`{...st, bold: true}`).
- One `match_at` scanner shared by the editor's colouring (byte ranges, markers
  kept) and the preview's tokens (markers stripped), so the two sides agree
  on what is markup.
- A pure edit model over an immutable line list (`delete_range`,
  `insert_text`) with a greedy soft-wrap index (`build_rows`), from which the
  caret, the selection bands and click hit-testing all derive; undo and redo
  are lists of snapshots with no bookkeeping beyond `drop_last`.
- Collecting `for` as `map` (`put_doc`, the toggle rewrite), `filter` with a
  lambda for the outline, `??` for `parse_int` on list numbers, `\"` and the
  no-brace rule inside the seed text.
- `state` for what persists across frames (documents, caret, scrolls,
  splitter, undo stacks, mode); `let` for every per-frame derivation; the
  layout pass is recomputed every frame from the parse.

**Host / petal-ui**

- Text: `draw_text` with style records across three faces (`mono` for the
  editor, `ui` for prose, bare size for chrome), `weight`, `italic`,
  `spacing`; `text_width` per word for pixel-exact flow layout; `ellipsize`
  for cells and rail rows.
- `splitter` from the prelude (state record, `min_a`/`min_b`, custom
  colours); `clip`/`clip_none` per pane; `draw_rect_gradient` for the
  preview's top fade; `draw_rect_rounded_outline` (bare native form) for the
  task boxes; `hovered`/`point_in`/`Rect`.
- Input: `claim_key` for the eight chords, `text_input`, `click_count`,
  `mod_shift`/`mod_cmd`/`mod_ctrl`, `scroll_y` scoped by hover,
  `panel_store_get`/`panel_store_set` (including `nil` to delete).

**Debug server** — the assertable values in `panes[0].panel.values`: `lines`
(the open document), `cur`, `anchor`, `sel_doc`, `mode`, `sync`, `escroll`,
`pscroll`, `sp` (splitter), `undo_n`/`redo_n`, `dirty`, `saves`, `edits`,
`blocks_n`, `outline_n`, `words`, `hover_link`. Text goes in through
`POST /text`, chords through `POST /key` with `mods`.

## Known limits

- No clipboard: there is no copy/paste API for a panel, so `⌘C`/`⌘V` are not
  bound.
- The embedded `ui` face draws no glyph for `⌘`, `—`, `·` or `⇧` (the `mono`
  face has them, and so does the chrome font), so the seed text spells out
  `Cmd+B`. Anything outside Inter's Latin coverage will show as a gap in the
  preview.
- Italic is upright on the embedded faces; the preview marks emphasis by
  colour as well.
- Wrapping in the editor is by character count (monospace), so a line of
  wide glyphs wraps a little early.
- Nested block structures (a list inside a quote, a code fence inside a
  list) are flattened to their outer kind.

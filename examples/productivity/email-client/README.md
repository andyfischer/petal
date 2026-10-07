# 17 — Email client (Plume)

A three-pane mail client drawn entirely by a Petal panel script: a mailbox
rail, a searchable conversation list and a reading pane that all follow one
selection. It holds 34 seeded conversations for a designer at a small studio
(a client rebrand, a broken token export, invoices, a flight, her mother's
lake photos), and everything on screen works: search with operators and live
highlighting, a cursor plus tick-box multi-selection with bulk actions and
undo, threaded conversations that expand and collapse, an inline reply
editor, a compose dialog with recipient completion and drafts, and a
draggable divider between the list and the reading pane.

Nothing here is a host widget. The list, the text fields and their carets,
the wrapped editors, the dialog, the toast, the tooltips and the icons are
all Petal over `state`, immutable records and the `draw_*` natives.

| File | What it holds |
|---|---|
| `app.ptl` | input and paint: layout, the edit model, every row, pane and overlay |
| `mailbox.ptl` | the data half: people, the seeded store, the clock, the query language, the transforms on the store. Draws nothing, so it also runs under `petal run` |
| `icons.ptl` | the glyphs, as polylines in a unit box |

## Run it

```bash
./launch.sh                               # a window
./launch.sh --headless --debug-port 0     # for the debug server
```

By hand:

```bash
cd examples/productivity/email-client
(GARDEN_HEADLESS_SIZE=1280x850 nohup ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > /tmp/plume.log 2>&1 < /dev/null &)
PORT=$(grep -o '127.0.0.1:[0-9]*' /tmp/plume.log | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o /tmp/plume.png
```

Designed for a **1280x850** viewport, which gives the panel a pane of
1268x778 at origin (6, 38); `POST /mouse` takes window coordinates, so a
pane point `(x, y)` is clicked at `(x + 6, y + 38)`. The layout is written
against `screen_width()` / `screen_height()` and the list width is the
divider's, so it reflows, but the proportions are tuned for that size.

Nothing is persisted: the mailbox is `state`, so it survives a hot reload and
is rebuilt from the seed by `POST /panel/reset`. There is no
`GARDEN_PANEL_STORE_DIR` to set.

The panel sleeps ten seconds after the last input. Nothing here animates at
rest (carets are drawn solid rather than blinking), so a sleeping frame is the
correct frame. A toast or tooltip calls `request_frame()` while it is up.

The mailbox lives at one fixed instant, Tuesday 6 October, 10:24, so "9:46
AM", "Yesterday" and "Sep 16" are the same on every run.

## Controls

**Keyboard, with the list focused**

| | |
|---|---|
| `j` / `down`, `k` / `up` | next / previous conversation (opening one marks it read) |
| `home`, `end`, `pageup`, `pagedown` | jump through the list; the list scrolls to keep the cursor in view |
| `/` | focus the search field |
| `x` | tick or untick the open conversation |
| `cmd+a` | tick everything in the list |
| `e` | archive the ticked conversations, or the open one (from Archive or Trash: move back to Inbox) |
| `#`, `delete`, `backspace` | move to Trash (from Trash or Drafts: delete for good) |
| `s` | star / unstar |
| `u` | mark unread / read |
| `z` | undo the last archive, trash, delete or discard (up to 20 deep) |
| `c` | compose |
| `r` | reply to the open conversation |
| `return` | on a draft: continue writing it |
| `space`, `shift+space` | page the reading pane down / up |
| `1` … `7` | Inbox, Starred, Sent, Drafts, Archive, All mail, Trash |
| `escape` | leave a field, then clear the ticks, then clear the search |

**Search field**

| | |
|---|---|
| typing | filters the current mailbox as you type; every word must match somewhere in the subject, a sender or a body |
| `from:maya` `is:unread` `is:starred` `has:attachment` `label:design` | operators, combinable with words and each other |
| `return` or `down` | hand the keyboard back to the list, on the first result |
| click | focus and place the caret at the click |
| `left` `right` `home` `end` `backspace` `delete`, `alt+backspace` | edit; the last deletes a word |
| the `x` button | clear |

**Mouse**

| | |
|---|---|
| click a row | open it |
| click a row's avatar, or `cmd`-click the row | tick it; a tick box replaces every avatar while anything is ticked |
| `shift`-click a row | tick the range from the last row opened or ticked |
| hover a row | archive, trash and star buttons replace its timestamp |
| chips `Unread` `Starred` `Files` | add or remove `is:unread`, `is:starred`, `has:attachment` in the search field |
| bulk bar (while rows are ticked) | archive, delete, mark read/unread; the tick box clears the ticks |
| "N more in other mailboxes" | re-run the same search in All mail |
| wheel over the list or the reading pane | scroll that pane |
| rail | switch mailbox or label; Compose |
| the divider between list and reading pane | drag to resize (340 px to 560 px), double-click to reset |
| a collapsed message in a conversation | expand it; click an expanded earlier message's header to collapse it |
| toolbar | archive, trash, mark unread, star (each with a tooltip), and previous / next |
| the reply bar | open the inline reply editor |
| "Undo" in the toast | the same as `z` |

**Reply editor and compose dialog**

| | |
|---|---|
| typing, `return` | text; Return starts a new line in a body |
| arrows, `home`, `end` | move the caret, row-wise in a wrapped body |
| click | place the caret |
| `cmd+return` | send |
| `tab` / `shift+tab` | next / previous field in the dialog |
| `tab` or `return` in To | accept the ghosted contact completion |
| `escape`, the close button, a click outside, "Save draft" | close; anything written is saved to Drafts |
| "Discard" | close without saving (a continued draft is deleted, undoably) |

A reply joins the conversation, which then also appears in Sent. A new
message needs a recipient that is a contact or contains `@`.

## What it exercises

- **Master/detail.** One `sel` id drives the rail's counts, the list's
  highlight and the reading pane. The list is derived from scratch every frame
  (`in_view` → `matches` → `sort` with a comparator), so there is no index to
  keep in step with the store; `reselect` picks the neighbour to land on when
  the open conversation leaves the list.
- **Lists.** Pixel scrolling under `clip_push`, rows culled outside the
  viewport, a sticky header with `draw_scroll_shadow`, proportional scroll
  thumbs, `ensure_visible_px` for keyboard movement, and a second,
  variable-height list (the conversation) laid out and measured in the same
  pass that draws it.
- **Search.** A small query language (`parse_query`, `matches`,
  `toggle_token`) written as pure functions over records; highlighting that
  measures each hit with `text_width` in the face it is drawn in; snippets
  that move to the text around the first hit.
- **Selection.** A cursor, a ticked set, shift ranges from an anchor,
  select-all through `claim_key("a", "cmd")`, and bulk actions over whichever
  of the two is active.
- **Immutable data.** Every change to the mailbox is a function from the list
  of threads to a new list, so undo is a stack of earlier lists and costs
  nothing to take.
- **Text editing in Petal.** One `edit_text` function serves all five fields:
  a character-indexed caret, word delete, and for wrapped bodies a wrapper
  that keeps character offsets so the caret can move between display rows.
- **Language features.** Three modules with `import … as` and `export`;
  `state`; collecting `for` with `continue` as filter; `match` with block
  arms; overloads by arity (`icons.icon`); spreads and field assignment on
  records; `sort` with a comparator lambda; early `return`.
- **Host and prelude.** `text_layout` for line placement and caret hit
  testing, `draw_shadow` / `draw_focus_ring` / gradients, `draw_polyline` and
  `fill_polygon` for the icon set, rounded outlines, nested clips, styled text
  in the `ui` face at two weights, `tint` for flat washes, `approach` for the
  dialog's fade, `request_frame`, `claim_key`.

Values a test can assert on (`GET /state` → `panes[0].panel.values`, or
`?values_prefix=obs_`): `obs_view`, `obs_query`, `obs_rows`, `obs_row_ids`,
`obs_sel`, `obs_sel_ix`, `obs_subject`, `obs_msgs`, `obs_checked`,
`obs_unread`, `obs_total`, `obs_focus`, `obs_compose`, `obs_sent`,
`obs_undo`, `obs_toast`, `obs_split`, `obs_list_scroll`,
`obs_detail_scroll`. The raw `state` is there too (`threads`, `checked`, `q`,
`q_c`, `reply`, `reply_c`, `c_to`, `keep_unread`, …).

`mailbox.ptl` has no panel calls in it, so its logic can be exercised with no
Garden at all: `petal run -I examples/productivity/email-client test.ptl` with
`import mailbox as mb` in `test.ptl`.

## Known limits

- **Actions land one frame late, by design.** A function cannot rebind
  module `state`, and the buttons are hit-tested where they are drawn, so
  every key and click only names an action (`act`, `act_ids`, `act_arg`) and
  one block at the bottom of the frame applies it. The next frame paints the
  result. `/screenshot` and `/state` settle a frame first, so a test never
  sees the gap.
- **No text selection in the editors.** There is a caret, not a range, so no
  shift-arrows, no copy or paste and no double-click word select. Up and down
  in a wrapped body keep the pixel column but do not remember it across
  several rows.
- **`text_wrap` returns strings, not offsets**, so the editors carry their own
  greedy wrapper (`wrap_rows`) that measures one growing prefix per character.
  That is fine for a reply and would not be for a long document.
- **Baselines are placed by a constant.** Two runs of different sizes on one
  line (a 14 px name beside a 12 px timestamp) need a shared baseline. On the
  Garden binary this was built against, `text_metrics(style).baseline` did not
  agree with where the `ui` face actually draws, so `base_of` uses 1.05 x the
  size, found by eye. Likewise `text_layout.text_line_y` returns a different
  `y` for a bold style than for the regular one, which made the rail's current
  item jump 3 px; the rail asks for the regular style's `y` for both.
- **A `state` named like a builtin shadows it silently in the panel.** The
  list width was first called `split`, and `split(text, "\n")` then tried to
  call an int. `petal check --strict` caught it as a capture warning; the
  runtime message would not have pointed there.
- **Seeded text is ASCII.** Highlighting lowercases a string and then slices
  the original by the same character offsets, which holds as long as `lower`
  does not change a string's character count.
- **Sent mail is all stamped "now".** The mailbox clock is fixed, so messages
  written in a session get a negative age and sort by the order they were
  sent rather than by a real time.
- **Attachments are pictures of attachments.** They hover but do nothing, and
  compose cannot attach. Labels filter but cannot be assigned.
- **Written without named arguments or default parameters**, so that it runs
  on a Garden built before those landed as well as on a current one.

# 15 Notes app (Vellum)

**Status:** complete
**Viewport:** 1280x850 (pane 1268x778)
**What works:** Everything I set out to build. A three-column notes app on a
warm paper palette: notebook rail with a stacked distribution bar and a save
indicator, a searchable/pixel-scrolled note list with inline per-card actions,
and a hand-written multi-line text editor — greedy soft word wrap, caret
placement by click, drag selection, shift-arrow selection, timed double-click
word select, wrap-aware Home/End and up/down with a goal column, insert/delete/
split/join, page scrolling with the caret kept on screen, live search
highlighting of every match in the visible rows, idle autosave + `ctrl+s`, and
an undo stack that restores an edited *or deleted* note (at its original index).
No `status_error` at any point across the full interaction sweep.

**What I could not do:** Nothing was cut for lack of a workaround, but two host
limits changed the design rather than just costing time — see Blockers.

## Blockers

Neither of these stopped the app, but both are hard limits I had to design
around rather than work around cleanly.

**1. A panel cannot draw an opaque overlay: shapes never paint over text.**
Every rect/line/circle a panel emits in one frame is batched into a single
`mesh` primitive per clip group, and the renderer draws meshes *under* all text
runs regardless of call order. So a popup is impossible — the content behind it
shows straight through its background.

Minimal repro (any panel):
```petal
draw_text("BEHIND", {x: 40, y: 40}, {size: 20, color: #000000})
draw_rect(rect(20, 20, 200, 60), #ffffff)   // drawn after — still under the text
```
The white rect does not cover "BEHIND".

This is not hypothetical: `petal-ui`'s own `context_menu` is affected. I opened
the prelude menu over the note list and read the list's titles straight through
the menu body (shadow, panel fill and all). Any panel that uses `context_menu`
over text today is rendering a broken-looking menu.

My workaround was to abandon the floating menu and put the per-note actions in
an **inline row inside the card**, which expands the card and pushes the list
down. It reads well and I would probably keep it, but it was forced.

**2. Cmd/Ctrl chords are swallowed by the host — except `ctrl+s`.**
`classify_panel_key` returns `PanelKey::Ignore` for anything with `cmd` or
`ctrl` set, with a hard-coded exception for `ctrl+s`. An app that *is* a text
editor therefore cannot have Cmd+A, Cmd+C/V/X, Cmd+Z, Cmd+F, Ctrl+Z — the whole
editing vocabulary a user already knows. I ended up with a clicked "undo last
edit" button in the rail because there is no key I am allowed to bind to it.

## Issues

**`ctrl+s` arrives with its character attached, so saving types an "s".**
The one chord that *is* forwarded is forwarded with `panel_key_text(key)`, so
`text_input()` returns `"s"` on the same frame as `key_pressed("s") &&
mod_ctrl()`. My first save inserted an `s` into the document. Every panel that
handles `ctrl+s` and also reads `text_input()` has this bug latent. Guard:
```petal
if len(text_input()) > 0 && !mod_ctrl() && !mod_cmd() then ... end
```
`text_input()` should be empty for a modified chord.

**`click_count()` is always 1 for a panel.** `POST /mouse {"op":"click",
"clicks":2}` is documented as a double-click and the `panel.input.click_count`
field is documented as part of the standard input contract, but the value the
script reads never leaves 1 (verified by binding it to a `state` var and reading
it back through `panel.values`). I implemented the double-click gesture myself
with `time()` + last-press position, which also works for real input, but the
native is either not wired for panels or not wired for the debug server's
`clicks` field.

**`clamp()` returns a float.** `clamp(i, 0, len(xs) - 1)` used as an index dies
with `Cannot index list with float`, which is a confusing error for a builtin
whose arguments were all ints. `petal-ui`'s prelude has a private `_clamp` with
a comment explaining exactly this — the fact the prelude needs its own means the
builtin is the wrong shape. Either make `clamp` preserve int inputs (like `min`/
`max` do) or export the integral one.

**The prelude `theme` is a fixed dark palette with no override.** `button`,
`context_menu`, `text_field`, `draw_scrollbar` and `section_label` all read the
module-level `theme` record directly, so on a light-themed panel every prelude
widget is a dark slab. `button(r, label, style)` has a style parameter; nothing
else does. I reimplemented buttons, the text fields, the scrollbars and the menu
in my own palette — perhaps 120 lines of the app is prelude code re-typed in a
different colour. A `theme_set(record)` (or reading `panel_theme()` by default)
would have saved all of it.

**Byte indices vs character positions.** `len` and `slice` are byte-based while
`text_width` is character-based; an editor's column arithmetic sits exactly on
that seam. I kept the seeded content ASCII to dodge it, but a real notes app
would hit it immediately (the prelude's own `ellipsize` carries a five-line
comment about a non-terminating loop caused by this). A `char_len` / `char_slice`
pair, or an explicit "columns" concept, would make text editing in Petal safe
rather than careful.

**Panel `state` survives hot reload, so editing seeded data does nothing.**
Expected and documented, but worth flagging as a workflow cost: every time I
changed the seeded notes I had to kill and relaunch Garden, because the live
`state notes = [...]` kept the old value. A debug endpoint to reset a panel's
state (or a `--panel-reset` on reload) would have saved a dozen restarts.

**Small things**
- `draw_rect_outline` is square-cornered with no rounded counterpart, so a
  rounded outlined control needs two rounded fills stacked (the todo app's `rr`
  helper does the same thing). A `draw_rect_rounded_outline` would be welcome.
- Inline `if ... then a else b end` inside a call argument needs the `end`, and
  forgetting it reports the error at the closing paren several tokens later
  (`Unexpected token: ')'`). Easy to fix once you know; mildly puzzling first
  time.
- The pane origin is `(6, 38)`, not `(0, 0)`. Nothing wrong, but every
  `POST /mouse` in a test has to carry that offset by hand and it is not in
  AUTHORING.md; the `rect` in `/state` is where I found it.

## Praise

- **`panel.values` is genuinely excellent.** Being able to assert `cur`,
  `anchor`, `sel_id`, `q`, `dirty`, `saves` by name, with no instrumentation at
  all, turned "does the editor work" into a ten-line shell loop. I found the
  `ctrl+s` typing bug in one `curl` because `edits` incremented when it should
  not have. This is the single best thing about the panel testing story.
- **Settle-then-capture really does mean no sleeps.** Input, then `/screenshot`,
  and the pixels are right. Across ~60 injected interactions I never once had to
  guess at a delay.
- **Immutable values made the persistence model trivial.** `saved = notes` is a
  real snapshot and `history = append(history, {note: note})` is a real undo
  entry, with no copying ceremony and no aliasing bugs. The whole
  edit/save/undo story is about 40 lines because of it.
- **`text_width` being exact** is what makes a monospace text editor possible at
  all — caret x, click-to-column, and the wrap width are all one multiplication.
- The pure-function edit primitives (`delete_range`, `insert_text` returning
  `{lines, cur}`) read beautifully in Petal. Loop-carried `let` rebinding inside
  nested `for`s worked exactly as documented, including a `for` inside an `if`
  inside a `for`.

## Feature requests

Prioritized.

1. **Let a panel draw over its own text.** Either flush the shape mesh in call
   order relative to text runs, or give scripts an explicit layer (`layer(n)` /
   `overlay_begin()`). Without it, popups, menus, tooltips, dialogs and toasts
   are all off the table — and the prelude ships a context menu that cannot look
   right.
2. **Forward modifier chords to panels**, or give scripts a way to claim them
   (`claim_key("z", ["cmd"])`). A text-editing panel with no Cmd+Z/Cmd+A is a
   toy.
3. **Suppress `text_input()` for modified chords** (a one-line fix in
   `panel_key_text`), and wire `click_count()` through to panels.
4. **A themeable prelude**: `ui_theme(record)` or default the `theme` record
   from `panel_theme()`, so `button` / `text_field` / `draw_scrollbar` /
   `context_menu` are usable in an app that is not dark grey.
5. **`clamp` that preserves ints**, and a rounded-rect outline primitive.
6. **Character-indexed string helpers** (`char_len`, `char_slice`, or a
   `text_width`-compatible column API) so text editing is not byte arithmetic.
7. **A debug-server way to reset panel state** for the edit-seed-data loop.

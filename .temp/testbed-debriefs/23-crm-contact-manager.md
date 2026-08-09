# 23 CRM contact manager

**Status:** complete
**Viewport:** 1280x850 (`GARDEN_HEADLESS_SIZE`), pane 1268x778
**What works:** Everything I set out to build. A 24-row contact table with five
click-to-sort columns (direction toggles on re-click), a segment rail with live
per-stage counts, incremental search over five fields, wheel + keyboard
scrolling with a proportional scrollbar, mouse and keyboard selection that is
stable across re-sorts (selection is by record id, not row index), an inspector
with an activity timeline, and a six-field edit form with its own focus ring,
Tab traversal, a digit-masked numeric field, stage chips, and Save/Cancel that
writes back into the `state` record store. Every control was driven through
`POST /key` / `/text` / `/mouse` and verified against both
`panes[0].panel.values` and the rendered PNG. `status_error` and
`panel.error` stayed null throughout.
**What I could not do:** Nothing was cut. Two things I chose not to build
because the primitives are missing rather than because they were out of scope:
a draggable scrollbar thumb (needs no new primitive, just more code) and a
text field with a real caret position / selection (the prelude's `text_field`
and mine both only append and backspace — see Issues).

## Blockers

None.

## Issues

**1. `if … then … else … end` inside a call argument is easy to get wrong and
the error points at the wrong thing.** Every one of my first-pass parse errors
was a missing `end` on an inline `if` used as an argument:

```petal
rr(search_r, 9, if q_focus then #10141b else CARD, if q_focus then ACCENT else LINE)
// Error: Unexpected token: ',' [line 546, column 50]
```

The caret lands on the comma that *follows* the unterminated `if`, so on a line
with two inline ifs it points at the first one when the real problem is that
neither is closed. A hint like "an `if` expression started at column 17 is
unclosed; expected `end`" would have saved several round trips. The `else CARD)`
form gives the same shape of error at the `)`.

**2. `when` is a reserved word and cannot be a record field name.**

```petal
[{label: head, when: c.days}]
// Error: Expected an identifier, got `when` [line 179, column 19]
```

`match` arms are the only place `when` is meaningful, and a record key is never
ambiguous with one. Contextual keywords already exist in the language (`config`
is documented as contextual), so this looks like an oversight rather than a
decision. Renamed the field to `ago_days`.

**3. `slice(s, 0, 1)` returns `""` for a leading multi-byte character.** Byte
indices snapping down to a char boundary is documented for the prelude's
`ellipsize`, but it silently breaks the obvious "first letter of each word"
loop:

```petal
upper(slice("Óscar", 0, 1))   // "" — the avatar for Óscar Delgado read "D"
```

I worked around it by widening until the slice is non-empty:

```petal
var ch = slice(part, 0, 1)
var n = 1
while len(ch) == 0 && n < 5 do
  set n = n + 1
  set ch = slice(part, 0, n)
end
```

A `chars(s)` builtin, or a `char_at(s, i)`, would make this a one-liner. This
is the single most likely bug for anyone writing text-handling Petal.

**4. No `sort_by` / comparator sort.** `sort` takes one argument and sorts
scalars. Sorting records by a chosen key with a chosen direction is the entire
point of a table app, so I hand-wrote an insertion sort over a closure
comparator (~18 lines). It works and is fast enough for 24 rows, but
`sort_by(xs, fn(a, b) -> …)` (or `sort(xs, key_fn)`) is the missing builtin I
felt most.

**5. `draw_rect_outline` is square-cornered only, so a rounded bordered box
takes two draws.** Every field, chip and card in this app is

```petal
fn rr(r, radius, fill, border)
  draw_rect_rounded(r, radius, border)
  draw_rect_rounded(rect(r.x + 1, r.y + 1, r.w - 2, r.h - 2), radius - 1, fill)
end
```

which doubles the mesh cost and is subtly wrong at radius 1. A
`draw_rect_rounded_outline(r, radius, c, a, width)` would close the gap.

**6. Text is always composited over quads, regardless of draw order.** I drew
the top bar's "sorted by …" caption before the inspector's full-height
background quad, and the caption still rendered on top of it. That is
presumably `Quad`/`Text` batching in `garden-render`, but it means "draw a panel
over what came before" does not hide text, which is a real hazard for any
overlay (a context menu over a text-heavy list would have the same problem).
Worth documenting in `petal-graphical-panels.md` even if it is not going to
change. I worked around it by moving the caption inside the table region.

**7. `/scene` cannot assert rounded geometry.** Anything drawn with
`draw_rect_rounded` / `draw_circle` / `fill_triangle` collapses into a single
`{"type":"mesh","triangles":N}` entry with no rect and no color. AUTHORING.md
sells `/scene` as "best for asserting layout numerically", but in a design that
uses rounded rects for everything, only text runs and square quads remain
assertable — I could not verify my scrollbar existed from `/scene` at all and
had to crop the PNG. Emitting the mesh's bounding rect and dominant color would
restore most of the value.

**8. Hand-rolling a text field is the default, not the exception.** The
prelude's `text_field` is welcome but it hard-codes `theme.*` colors, a 14px
size, and a 6px inset, so any app with its own palette has to reimplement it —
which then means reimplementing focus, the caret, and backspace too. Splitting
it into a `text_field_update(fc, id, r, buf) -> {focus, text, submitted}` (input
only) plus a separate draw would let apps keep the logic and bring their own
pixels.

**9. Minor: `int(v / 1000)` vs float division.** `10 / 3 == 3` for ints is
documented and fine, but it bit me once building a "$1.2M" formatter, where
`round(k * 10) / 10` silently truncates back to an int. Not a bug — just the
kind of thing that produces a wrong-looking number rather than an error.

**10. Minor: the panel-local vs window coordinate offset is undocumented in
AUTHORING.md.** `POST /mouse` takes window logical pixels, the script sees
pane-local ones, and the pane rect (`x: 6, y: 38` here) is only discoverable via
`/state → panes[0].rect`. Every one of my click scripts needed a `+6 / +38`
offset. One sentence in the "Driving it" section would help.

## Praise

- **`panel.values` is as good as advertised.** Being able to write
  `{"seg":"Qualified","sort_key":"value","sel_id":15,"editing":true,"focus_id":"name"}`
  out of a live app with no instrumentation whatsoever made this the fastest
  UI-testing loop I have used. I never once had to guess whether a click landed.
- **Settle-then-capture really does mean no sleeps.** `POST /mouse` immediately
  followed by `GET /screenshot` was always consistent. That alone probably
  halved the iteration time.
- **`for` as a mapping expression with `continue` as a filter** makes the whole
  filter stage one readable expression, and record spread makes the write-back
  a three-line loop with no mutation anywhere. Both read exactly the way I
  wanted the code to read.
- **`text_width` measuring the real font** — right-aligned columns, centered
  button labels and the sort caret all landed on the first try.
- **`preview(text, chars, lines)`** was precisely the right shape for the notes
  block. Bounded cost, returns `truncated`, no thinking required.
- **Hot reload preserving `state`** meant I could edit the layout while a
  selection and an in-progress form were live.

## Feature requests

Prioritized:

1. `chars(s)` / `char_at(s, i)` — or make `slice` char-indexed. Issue 3 is a
   silent data bug waiting to happen in every app that touches text.
2. `sort_by(list, key_or_cmp)` (Issue 4). Tables are a huge fraction of what
   people build.
3. Allow keywords as record field names, or at least free `when` (Issue 2).
4. A better unclosed-`if` diagnostic that names where the `if` started
   (Issue 1).
5. `draw_rect_rounded_outline(...)` (Issue 5).
6. `/scene` should report a bounding rect + color for mesh primitives (Issue 7).
7. Split the prelude's `text_field` into update + draw halves (Issue 8).
8. Document the text-over-quad compositing rule and the pane coordinate offset
   (Issues 6, 10).

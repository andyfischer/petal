# 33 Drawing / paint app (Petal Paint)

**Status:** complete
**Viewport:** 1280x850 (`GARDEN_HEADLESS_SIZE=1280x850`; panel pane 1268x778)
**What works:** Everything I set out to build. Six tools (brush / line /
rectangle / ellipse / eraser / flood fill), a 12-ink palette, size and opacity
sliders driven by real drags, a seeded 19-stroke sketch, snapshot undo/redo
(60 deep) covering every edit including Clear, a scrolling-free History list
with hover + selection + an on-canvas selection marquee, a right-click context
menu on history rows (select / duplicate / bring to front / delete), a grid
toggle, hover tooltips on the tool rail, a live brush-size cursor, and a status
bar with per-mille artboard coordinates. Every control listed in the README was
exercised through `POST /key` and `POST /mouse` against a headless Garden and
verified in both `panel.values` and the pixels. `status_error` stayed `null`
throughout.
**What I could not do:** Nothing was cut. The document is vector rather than
raster (no offscreen canvas natives in Garden), which is a design choice the
app leans into rather than a limitation I hit.

## Blockers

None.

## Issues

### 1. `/screenshot` renders a *stale* text run on top of a changed one

The single most confusing thing in the whole session. When a text run's content
changes, the capture shows the **old string and the new string overlaid** at the
same position, for exactly one frame. `GET /scene` at the same moment reports
only one run, so the script is emitting the right thing — the doubling is
renderer-side.

Reproduce: any panel that draws a string derived from the pointer position.

```petal
draw_text("{mouse_x()}, {mouse_y()}", {x: 30, y: 700}, 11, #5b6577)
```

```bash
curl -sX POST $B/mouse -d '{"op":"move","x":400,"y":300}'
curl -s $B/screenshot -o a.png     # "473, 495" and "352, 333" drawn on top of each other
curl -s $B/scene | jq '[..|objects|select(has("text"))|.text]'   # one run, correct value
```

Re-requesting `/screenshot` with no input in between reproduces it identically
(so it is not a torn read of a half-drawn surface); one more real frame clears
it, and then the *next* changed run ghosts instead. It made several
intermediate screenshots unreadable and cost me a while convincing myself my
script wasn't drawing twice. In a live 60 fps window this presumably reads as
one frame of ghosting on any live-updating label.

### 2. Text is drawn above *all* quads, regardless of draw order

`context_menu` draws an opaque panel over the history list, but every `draw_text`
issued **earlier in the frame** still shows through it — the renderer evidently
batches all quads and then all text. So an immediate-mode overlay can cover
shapes but never covers text.

This is a real problem for the prelude's own `context_menu`, not just for me:
the menu is documented as "drawn last so it sits on top", and it doesn't.

My workaround is ugly and I'd rather not have written it — the prelude keeps
`_menu_rect` private, so I had to *re-derive an estimate* of the menu's box in
app code and skip drawing any list text that would fall inside it:

```petal
fn menu_est(m)
  let w = 170          // guessed from _menu_w's clamp + my longest label
  let h = 122          // 4 items * 24 + one sep * 9 + 2 * 6 pad
  let x = m.x
  let y = m.y
  if x + w > screen_width() then x = max(0, m.x - w) end
  if y + h > screen_height() then y = max(0, m.y - h) end
  rect(x, y, w, h)
end
```

Two asks fall out of it: fix the layering, and failing that **export the menu's
rect** (either `menu_rect(m, items)` or as a field on the record `context_menu`
returns) so nobody has to guess these constants.

### 3. `theme`'s alpha convention is wrong in the prelude itself

`ui.ptl`'s `context_menu` draws its drop shadow as

```petal
draw_rect({x: r.x + 2, y: r.y + 3, w: r.w, h: r.h}, #000000, 0.35)
```

but alpha is `u8` 0..255 (`opt_u8` in `petal-ui/src/draw.rs`), so `0.35`
truncates to 0 and the shadow is invisible. Every menu in Garden is therefore
missing its shadow. (I hit the same 0..1-vs-0..255 confusion myself before
checking the Rust; the language guide's draw table says "0–255" but the
prelude's own example says otherwise.)

### 4. Overload-arity errors are good, but the record overloads are easy to trip

`draw_rect_rounded(r, 8, TEXT.r, TEXT.g, TEXT.b)` gives

```
draw_rect_rounded() expects 8 or 9 or 3 or 4 arguments, got 5
```

which is clear, but the flat form and the record form differ *only* in whether
you spread the color, and I mixed them four times in one file. A record/flat
hybrid arity (`(rect, radius, r, g, b)`) or a `Color` class with a
`c.rgb()` spread would remove the whole class of mistake. The error message
listing arities as `8 or 9 or 3 or 4` (unsorted, and not saying which shape each
arity is) is also less helpful than it could be — `(x,y,w,h,radius,r,g,b[,a])`
or `(rect, radius, color[, alpha])` would have fixed it instantly.

### 5. `draw_rect_outline` has no rounded variant

There is `draw_rect_rounded` but no `draw_rect_rounded_outline`, so every
outlined pill in this app is two stacked rounded rects (`plate()` in the
source): a 1-px-larger one in the border color with a slightly larger radius,
then the fill. It works but it costs two primitives and the radii never quite
agree at the corners.

### 6. No ellipse / arc / polyline primitive

`draw_circle` only takes one radius, so every ellipse is 60 `draw_line` calls
(`ellipse_outline`). Likewise a brush stroke is one `draw_line` per segment plus
a `draw_circle` per joint. A `draw_polyline(points, color, alpha, width)` with
proper joins would replace ~40 lines of app code and, more importantly, would
render **translucent strokes correctly**: today, drawing a semi-transparent
stroke as N overlapping quads makes every joint darker than the shaft. I had to
special-case it — joint dots are only drawn when `alpha >= 250`, which leaves
thick translucent strokes visibly notched at the corners. There is no way to
fix this in Petal without an offscreen surface.

### 7. `clamp` returns a float

Documented, and the prelude's `_clamp` comment warns about it, but a
`clamp(x, 0, 10)` feeding pixel geometry silently makes everything float and
then `draw_*` truncates in a place far from the mistake. An int-preserving
`clamp` (or a warning when a float lands in an int-typed slot) would help.

### 8. Small doc gaps

- `AUTHORING.md`'s draw table lists `draw_circle(cx,cy,radius,...)` but not that
  colors in the *record* overloads must be a `{r,g,b}` record — obvious in
  hindsight, but the two families are adjacent in `ui.ptl` and easy to blur.
- Nothing anywhere says that `/mouse` coordinates are **window**-relative while
  everything a panel script reasons about is **pane**-relative. The pane offset
  here was `(6, 38)`; I found it in `/state`'s `panes[0].rect` after a couple of
  confused clicks. Worth one sentence in AUTHORING.md.
- `mod_cmd()` / `mod_shift()` are not in AUTHORING.md's "Reads" list (I found
  them by grepping `petal-ui/src/input.rs`).

## Praise

- **`panel.values` is as good as advertised.** Writing the whole test script
  against `{tool, ink, brush, opacity, sel, ops, n, hist, fut}` instead of
  pixels made every interaction assertion a one-liner, and `ops` (a counter I
  added purely so one-frame edges would be observable across a later `/state`)
  worked exactly as the doc suggests.
- **The panel error overlay is excellent.** Full message, source line, caret,
  and a `Caused by:` chain through the prelude into my own call site. My first
  launch failed on an arity mistake and the screenshot told me the line, the
  column, and the prelude frame in between. That is a better first-run
  experience than most compiled UI toolkits.
- **Hot reload with state preserved** turned iteration into "save, curl,
  look" with no restart. Editing the layout constants while a drawing was on the
  canvas and watching the panel re-lay-out around it is genuinely delightful.
- **`text_width` being exact** meant `draw_text_right` / `draw_text_center` /
  `fit_parts` all just worked; the status bar's hint line sheds segments
  gracefully with zero fiddling.
- Storing points normalized and letting `Rect` methods (`inset`) do the geometry
  kept the whole layout function declarative. `for` in value position and
  `continue`-as-filter made the "rebuild the stroke list without item N" logic
  four lines.
- `settle-then-capture` — input then `/screenshot` with no sleep — is a huge
  quality-of-life win for scripted verification.

## Feature requests

Prioritized:

1. **Fix text/quad layering** (issue 2). It breaks the prelude's own documented
   contract for `context_menu`, and there is no clean app-level workaround.
2. **Fix the one-frame stale text run in `/screenshot`** (issue 1). It makes
   visual verification unreliable, which is the whole premise of this exercise.
3. **`draw_polyline(points, color[, alpha[, width]])`** with round joins
   (issue 6) — the single highest-value addition for any drawing app, and the
   only way to get correct translucent strokes.
4. **Export the context menu's rect** (`menu_rect(m, items)`, or a `rect` field
   on the `context_menu` result) so overlays can be reasoned about (issue 2).
5. **`draw_ellipse` / `draw_ellipse_outline`** with `rx`/`ry`.
6. **`draw_rect_rounded_outline(r, radius, color[, alpha[, width]])`** (issue 5).
7. Fix the `0.35` alpha in `ui.ptl`'s menu shadow (issue 3) — one character.
8. Arity errors that print each candidate **signature**, not just each candidate
   count (issue 4).
9. A `Color` built-in class alongside `Rect`, with `mix`/`lighten`/`luma`
   methods. I hand-rolled `mix`, `luma`, `on_color` and a hex formatter
   (`hex2` via a digit-lookup `slice`, because there is no `hex()` builtin) in
   every one of these apps, I suspect.

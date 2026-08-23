# 41 Analytics dashboard

**Status:** complete
**Viewport:** 1440x900 (pane 1428x828)
**What works:** Four KPI cards with sparklines and delta chips; an animated main
chart (area or bars) with y-grid, nice-stepped axis, x date labels, a hover
crosshair + tooltip, and a tweened morph between metrics/ranges; a hoverable
donut with a live centre label; a stacked column chart; a ranked horizontal bar
list; a table with mini deltas; a responsive card grid that folds 4-up → 2-up
and moves the donut into the bottom grid on narrow panes; keyboard *and* mouse
for every control. No `status_error` and no panel `error` at any point in the
interaction sweep (12 key steps + 7 mouse steps, all asserted).
**What I could not do:** Nothing I set out to do. Two things I deliberately did
not attempt: page scrolling when the solved layout exceeds a short pane (the
bottom row clips instead), and a bold weight anywhere (only Regular is embedded).

## Blockers

None that stopped me outright, but one host bug came very close — see
"Glyph atlas" below. It made the dashboard look broken and cost the largest
single chunk of time; I only got around it by rewriting the type scale.

## Issues

### 1. Glyph atlas overflow silently swaps the font (Garden renderer) — worst issue by far

Once a panel asks for enough distinct `(character, size)` pairs, whole text runs
come back rendered in a **proportional fallback face at roughly 0.7× the
requested size**, permanently, while `/scene` still reports the correct
`size`. Because the atlas fills up over time, it bites exactly the text a
dashboard changes: a headline that read `$372.0k` was perfect, and the instant a
range switch made it `$961.9k` the `9 6 1` arrived tiny while the `$` and `k`
(already rasterized at 27) stayed correct — inside one run.

Minimal repro (`--headless`, 900x300):

```petal
state n = 0
n = n + 1
clear(10, 12, 16)
let C = #e9eef6
let SIZES = [10, 11, 12, 15, 21, 24, 27, 30]
let y = 4
for s in SIZES do
  draw_text("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ", {x: 4, y: y}, {size: s, color: C})
  y = y + s + 2
end
draw_text("changing " ++ str(n * 7), {x: 4, y: 200}, {size: 27, color: C})
```

The 21/24/27/30 rows render in a proportional serif-ish face, not JetBrains
Mono; the short line stays mono. There is no error, no log line, nothing in
`/state`.

Things I tried that did **not** help:
- pre-warming every glyph at every size at alpha 0 on frame 1 — this made it
  strictly *worse*: every warmed glyph then rendered at the wrong size, while
  the warm run itself (drawn visibly) was correct.
- removing every `spacing:` style (not the trigger).
- flat `draw_text(s,x,y,size,r,g,b)` vs the styled record form (no difference).

What fixed it: **collapsing the type scale from nine sizes (10/11/12/13/14/15/
21/24/27/30) to five (10/12/15/21/28)**. After that the dashboard survived
3 full passes over every control plus every hover region with no degradation.
That is a real constraint on how rich a panel's typography can be, and it is
documented nowhere. It also appears to be aggravated by hot reload: the same
process after ~8 script reloads degraded much sooner than a fresh one.

### 2. `clamp` returns a float, and that silently breaks integer layout

`clamp(fit, 1, count)` for a column count yields e.g. `4.0`, after which
`i % cols` and `i / cols` become float ops, `i / cols` is `0.25` instead of `0`,
and a four-card row staircases down and to the right by a quarter of a row each
card. Nothing errors; the page just looks subtly drunk. The `ui` prelude already
knows about this and keeps a private `_clamp` for exactly this reason
(`_menu_w`'s comment says so) — but `_clamp` is not exported, so every app
re-declares it. **Please export an integer-preserving `clamp`**, or make `clamp`
type-preserving like `min`/`max` already are.

The same bite happened a second time with `hover_i = clamp(...)`, which produced
`Cannot index list with float`. That error at least is loud.

### 3. A `for` loop as the last expression of a function body is not captured

```petal
fn make_series(seed, base)
  for i in range(0, 90) do
    base * float(i)
  end
end
// -> nil
```

The guide says a `for` in "value position — assigned to a name, `return`ed,
passed as an argument, or placed as a list element" collects. The implicit-return
position of a function body is *not* one of those, so the function returns `nil`
and you find out several frames later as `Cannot get length of nil` pointing at
a helper three calls away. I hit this three separate times (`make_series`,
`channel_matrix`, `grid_cells`) and every time the workaround is the same noise:

```petal
let out = for ... end
out
```

Either make the function-body tail a capturing position (it is the most natural
place to write a mapping loop), or make the compiler warn at the `end` of a
function whose body's last statement is an uncaptured `for`. It already has the
machinery — it emitted `result of lerp is discarded, so this call has no effect`
for the *inside* of one of these loops, which is the same information arriving
at the wrong altitude.

### 4. `draw_rect_rounded` has no `(x, y, w, h, radius, color_record)` overload

The arities are `8 | 9 | 3 | 4`: flat-with-separate-rgb, or record-rect +
record-colour. So `draw_rect_rounded(x, y, w, h, 6, MY_COLOR)` is an arity error
and you have to write `draw_rect_rounded(rect(x, y, w, h), 6, MY_COLOR)`. Mixing
computed coordinates with a named palette colour is the single most common thing
a dashboard does. Same gap on `draw_rect`. A `(x,y,w,h[,radius], c[, a])` family
would remove a lot of `rect(...)` wrapping.

Also worth noting: the error message is good (`expects 8 or 9 or 3 or 4
arguments, got 6`) but the arity list is unordered, which makes it read like a
riddle.

### 5. A `.ptl` that fails to compile gives you an empty editor pane, not an error

My first launch produced a pane with `"kind": "editor"`, `"panel": null`, and
`status_error: null` — no indication anywhere in `/state` or the log that
`app.ptl` had a lex error on line 935. I found it by running
`petal check app.ptl` on a hunch. A panel whose script does not compile should
surface the compile error the way a *runtime* error does (which it does very
well — `panel.error` with source, caret and a stack trace is excellent).

### 6. `\"` is not usable inside an interpolation hole

```petal
let s = "mode: {if m == \"area\" then \"bars\" else \"area\" end}"
// Error: Unexpected character '\'
```

Understandable given the lexer, but the error points at the backslash with no
hint that the fix is to hoist the expression into a `let`. Worth a note in the
guide's string-interpolation section.

### 7. Overlapping `clip` rects double-composite

Not a bug, but a trap worth documenting: building a vertical gradient as N
horizontal clip bands, each `plot.h / N + 1` tall, draws a bright rule at every
seam because the +1 overlaps the next band and the translucent fill composites
twice. Pixel-snapping shared boundaries (`int(y0)`, `int(y1)`, height
`y1 - y0`) fixes it. `clip` taking ints while everything else takes floats is
the thing that invites the sloppy `+1`.

### 8. Minor

- `Rect`'s constructor keeps floats, which is right, but `grid_cells` returning
  float-valued rects then flows into `clip(int(...), ...)` — a `Rect.round()` or
  `Rect.snap()` method would be handy for the draw boundary.
- `panel.values` reports colour records with keys sorted alphabetically
  (`{b, g, r}`), which reads as BGR at a glance. Cosmetic.
- No `%`-style formatting or `round(x, places)`; every app that shows a number
  re-implements `commas()` and a fixed-point `dec()`. Two very small builtins
  would save every dashboard-shaped app the same 30 lines.

## Praise

- **`panel.values` is superb.** Being able to `curl /state` and read
  `kpi_cols`, `row3_h`, `hover_i`, `plan_hover`, `l1`, `l2` by their source names
  turned every "is the layout wrong or is the drawing wrong?" question into a
  one-line check. It found the float-`clamp` bug for me in about ten seconds
  (`kpi_cells` had fractional `y`s).
- **Runtime errors are the best I have seen in a small language**: message,
  source line, caret, and a `Caused by:` chain of the bindings that fed the
  failing term. `Cannot index nil with int` → `kpi_cells` → `grid_cells` →
  `content_x` is a complete diagnosis in four lines.
- **settle-then-capture really does mean no sleeps.** `POST /mouse` then
  `GET /screenshot` always showed the hover state. That contract saved an
  enormous amount of flakiness.
- **`text_width` with real advances** makes centring and right-alignment exact,
  and measuring the same style record you draw with is the right API shape.
  Every right-aligned column in this app is pixel-perfect for free.
- Hot reload preserving `state` while I reshaped the layout meant I could keep
  the selected metric and range across dozens of edits.
- The `ui` prelude's record overloads (`draw_rect(r, c)`) read beautifully, and
  `ellipsize` is exactly the primitive a table needs.

## Feature requests

1. **Fix the glyph atlas fallback** (or at least log it). It is invisible,
   permanent, and it makes a panel look broken rather than degraded. If a hard
   cap is unavoidable, expose the pressure somewhere in `/state` so an app can
   be told it is over budget.
2. **An integer-preserving `clamp`** (or export the prelude's `_clamp`). This is
   a two-line change that removes a whole class of silent layout corruption.
3. **Capture a `for` in function-tail position**, or warn about the uncaptured
   one. Highest-frequency papercut in this app.
4. **`draw_rect` / `draw_rect_rounded` overloads taking flat coordinates plus a
   colour record** (`x, y, w, h, radius, c[, a]`).
5. **Report a panel's compile error in `/state`** instead of silently falling
   back to an empty editor pane.
6. **`round(x, places)` and a thousands-separator formatter** as builtins.
7. Longer term: a `path`/polyline draw primitive with a fill rule that is not
   convex-only. Every area chart in every future dashboard will otherwise
   hand-tessellate trapezoids exactly the way this one does.

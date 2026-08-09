# 43 Personal finance dashboard (Ledger)

**Status:** complete
**Viewport:** 1280x850 (the headless default; the panel pane is 1268x778)
**What works:** Everything I set out to build. Seeded 672-transaction ledger;
four KPI tiles with micro-histograms; an interactive donut (hover lift, polar
hit testing, centre readout) with a clickable legend; a twelve-month column
chart with hover readout and click-to-filter; a cumulative/balance line chart
with a gradient area band and a dashed reference; a scrollable, re-sortable
transaction table. Three drill levels (all spending → category → merchant)
reachable by donut click, legend click, table-row click, keyboard, and
breadcrumb; `esc` climbs out. A month filter cuts across the drill path as an
independent dimension. No `status_error` and no panel error at any point in the
interaction script I exercised (drill in x2, esc x2, keyboard drill, breadcrumb
jump to level 0, column click, `n`/`p`/`a`, `t`, wheel scroll, empty-ish
scopes).
**What I could not do:** Nothing I wanted was blocked. I skipped a hover
crosshair on the line chart purely for time.

## Blockers

None.

## Issues

### 1. `for` in value position is captured *much* more narrowly than documented

This cost me three debug cycles. The language guide says a `for` collects "when
its value is actually used — assigned to a name, `return`ed, passed as an
argument, or placed as a list element". Two very natural cases are **not**
captured, and both fail silently at the point of the loop and loudly somewhere
far away:

```petal
fn zeros12()
  for i in range(0, 12) do 0.0 end     // returns nil, not a list
end
```

```petal
let bucket = if ci < 0 then
    for c in range(0, len(CATS)) do 0.0 end     // nil
  else
    for k in range(0, len(ms)) do 0.0 end       // nil
  end
```

Both are "the value of the enclosing expression is the loop", which reads
exactly like the documented cases. The error surfaces later as
`Cannot index nil with int` / `Cannot iterate over nil` at the first *use*, so
the trail points at the consumer, not the loop. The workaround is mechanical —
bind it (`let out = for … end` then `out`) — but it has to be applied at every
site, and I hit it three separate times in one file (function tail, `if` branch,
and a second `if` branch).

Either capturing these or making an *uncaptured* `for` whose value is demanded
a compile error would have saved all of it.

### 2. `state` survives hot reload, which makes a data-generation bug un-fixable in place

`state _income = build_income()` captured `nil` from the buggy version above.
After I fixed `build_income`, the panel kept the stale `nil` — because state is
preserved across reload, exactly as designed — and kept throwing the same error
against corrected source. I lost time re-reading correct code before realising I
had to kill and relaunch Garden. A `/state`-visible note, or a debug-server
"reset panel state" verb, would make this obvious. (Documented behavior, just a
sharp edge when the state initializer itself is what you're fixing.)

### 3. Draw overloads dispatch on arity, so a plausible call silently means something else

`draw_circle(cx, cy, r, COLOR)` is four arguments, and the prelude's four-arg
overload is `draw_circle(center, radius, c, a)` — so it binds `center = cx`,
`radius = cy`, `c = r`, `a = COLOR` and either errors oddly or draws nonsense.
Same shape for `draw_rect_rounded(x, y, w, h, radius, COLOR)` (six args, no such
overload). I caught these by reading `ui.ptl` before running, but the flat form
takes an unpacked colour and the record form takes a packed one, and mixing them
is the obvious mistake. A named-arg or a `Color` type check would help; at
minimum the prelude could add the mixed arities (`draw_circle(x, y, r, c)` and
`draw_rect_rounded(x, y, w, h, radius, c)`), which are what people will type.

### 4. `sort` has no comparator / key form

Sorting 672 records by date or amount needed the pack-rank-into-an-int trick
(`rank * 4096 + i`, sort, unpack). It works and is fast, but it is a puzzle, it
imposes an index ceiling, and it forces amounts through a fixed-point encoding.
`sort_by(list, fn)` or `sort(list, cmp)` would delete twenty lines and a comment
explaining why the twenty lines exist.

### 5. `ellipsize` measures with a bare size, but `draw_text` takes a style

`ellipsize(s, avail_px, size)` calls `text_width(s, size)`. My small-caps labels
carry `spacing: 1.4`, which `text_width(s, style)` accounts for and
`text_width(s, size)` does not — so a label that "fit" overflowed by 20% and ran
under the adjacent sparkline. I had to write a five-line `fit_style` that
measures the exact style record. `ellipsize(s, avail, style)` as an overload
would be a one-line prelude addition.

### 6. `rand01` over an arithmetic seed walk is an arithmetic sequence

Not a language bug, but worth recording because the failure was invisible: a
single LCG pass (`hashi`) over seeds that step by a constant produces outputs
that step by a constant mod 1. My "does this merchant bill this month?" coin
flip therefore came out the same way for all twelve months — one merchant billed
every month, another never did, and the column chart was twelve identical bars.
Hashing twice (`hashi(hashi(n) % 65521)`) fixed it. If the docs' `random`/`noise`
family is meant to be the answer here, a seeded-deterministic variant of
`random` would be better than everyone hand-rolling `hashi`.

### 7. Minor

- The panel's script error is reported in `/state` under
  `panes[0].panel.error`, not in `status_error` or `script_error`. Both of
  those stayed `null` the whole time my panel was crashing. AUTHORING.md points
  at `status_error`, which reads as "check this for script failures" — worth a
  sentence.
- A `.ptl` that fails to *parse* makes the pane come up as an `editor`, with
  `panel: null` and no error text anywhere in `/state`. `petal check app.ptl`
  found it in a second, but there is nothing in the debug output that says
  "your panel did not load".
- No printf/format builtin: every money string goes through hand-rolled
  `commas` and `dec` (which every dashboard app in this testbed apparently
  re-implements — mine is a near-copy of 41's).

## Praise

- **`panel.values` is the feature.** Writing assertions against `level`,
  `sel_cat`, `hot_slice`, `month`, `sort_mode` instead of pixels made the
  interaction pass trivial to verify, and the `_`-prefix filter meant I could
  keep a 672-row ledger in `state` without drowning the dump. Adding a `drills`
  counter to make a one-frame edge observable took one line and worked exactly
  as documented.
- **`/screenshot` and `/scene` settling the frame before answering.** POST a
  click, GET a screenshot, done — no sleeps, no flake, across dozens of cycles.
- **`text_width` with real font advances.** Right-aligned money columns,
  centred axis labels and the donut's centred readout all landed pixel-exact
  first try. That is rare.
- **The record overloads of `draw_*` plus `Rect`.** `rect(...)` +
  `draw_rect_rounded(r, 14, SURFACE)` + `hovered(r)` is a genuinely pleasant
  immediate-mode vocabulary; the card/inset arithmetic mostly disappeared.
- **`if` as an expression with `elsif`** makes palette and label selection read
  as tables rather than statement soup.
- **`state` + hot reload** made iteration fast: everything except the
  generator was editable live, with the drill path preserved across the edit.
- `petal check <file>` catching parse errors with a caret, without needing the
  host, was the fastest loop in the whole exercise.

## Feature requests

Prioritized.

1. **`sort_by(list, key_fn)` / `sort(list, cmp)`.** Highest value per line of
   implementation; every data-shaped app needs it.
2. **Capture a `for` in the remaining value positions** (function tail, `if`/
   `match` branch tail), or make an uncaptured-but-demanded `for` a compile
   error. Silent `nil` is the worst outcome.
3. **A format builtin** — `format("{:,.2f}", v)` or even
   `money(v)` / `fixed(v, n)` / `commas(n)` as builtins. Four apps in this
   testbed have four copies of the same 30 lines.
4. **`ellipsize(s, avail, style)` overload**, and generally: anywhere the
   prelude takes a bare `size`, take a style record too.
5. **Mixed-arity draw overloads** — `draw_circle(x, y, r, color)`,
   `draw_rect_rounded(x, y, w, h, radius, color)`.
6. **An arc primitive** — `fill_arc(cx, cy, r_in, r_out, a0, a1, color)`. Every
   donut/pie in Petal will otherwise hand-roll the same quad fan, and mine emits
   ~200 `fill_poly` calls a frame to do it.
7. **A seeded RNG** — `seeded_random(stream, i)` with decent mixing, so example
   apps stop shipping their own LCG.
8. **A debug-server "reset panel state" verb**, for the case where the bug is
   in a `state` initializer.

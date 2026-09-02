# Typography — plan and status

Status: **partially shipped.** Font selection, per-font measurement and font
objects are in `petal-ui` and honored by every host. Flow layout (styled
spans wrapping into paragraphs) is not started. The user-facing contract is
[text-and-fonts.md](../text-and-fonts.md); this document keeps the design
rationale and tracks what is left.

The goal is best-in-class text for Petal apps: multiple fonts (faces,
weights, styles), correct proportional measurement, and a lightweight
HTML-like *flow layout*. Hosts implement only rasterization; layout runs
script-side and is identical everywhere.

---

## 1. Status

| Phase | Status | What it is |
|---|---|---|
| 0 — metrics groundwork | done | Per-font advance tables keyed by face; `text_width(s, size, font)`; web-canvas and Garden measure real metrics; Garden honors `size`. |
| 1a — protocol + hosts | done | Optional `font` / `weight` / `italic` / `spacing` on `DrawCommand::Text`; style records on `draw_text` / `text_width`; SDL, web-canvas and Garden honor them. |
| 1b — font discovery | done, in a different shape | The plan called for a separate `petal-typography` crate with `FontBook`. Instead `petal-ui` gained a `FontSource` trait a host attaches, `font(name)` returning a font *object* (a style record), the `font_*` decorators, and `fonts()`. Garden draws panels in any installed family. No separate crate exists. |
| Pixel-budget helpers | done, early | `ellipsize` / `ellipsize_tail` (pixel budget, measured) and `draw_text_center` in `ui.ptl`. They needed no new protocol, so they landed ahead of the `typo` module. |
| 2 — the `typo` module | not started | Spans, rich single lines, pixel-measured word-wrap, flow layout with a measure/draw split, layout cache. |
| 3 — raster + migration | not started | A software glyph cache for SDL-class hosts; porting the apps that hand-roll flow layout. |

Where each host stands today (faces, weights, spacing, measurement) is the
"Per-host status" table in [text-and-fonts.md](../text-and-fonts.md#per-host-status).

---

## 2. The problem this solves

Before this work `DrawCommand::Text` carried no font, weight or style: the
host picked one font for everything. Measurement was `chars × size × 0.6`
unless a host bound a real advance table. Exactly one host (petal-sdl)
rendered proportional text with correct metrics; web-canvas rendered
proportional but measured monospace, so every centered label was off; Garden
projected panels down to one monospace face at a fixed 14 px.

On the script side, `ui.ptl` had `wrap` / `preview` (greedy word-wrap in
*character* budgets) and `truncate_head` / `truncate_tail`. Apps hand-rolled
the rest, over and over:

1. Pixel-to-character conversion: `cw = text_width("0000000000", FS) / 10`
   then `int(avail / cw)`. **Fixed** by `ellipsize` / `ellipsize_tail`.
2. Centering: `x + (w - text_width(s, size)) / 2` at every call site.
   **Fixed** by `draw_text_center`.
3. Multi-color lines: draw a run, `x += text_width(run)`, draw the next.
   No rich-text primitive exists. **Open.**
4. Variable-height wrapped rows: parallel `row_lines` / `row_y` / `row_h`
   arrays built from `wrap()` results — a hand-rolled flow layout. **Open.**
5. Color finer than one line inside a `text_view` widget. **Open.**

Items 3–5 are what the `typo` module should own. Items 1 and 2 turned out to
be cheap prelude helpers, which is worth remembering: not everything on the
list needs a new library.

Measuring instead of estimating also surfaced three latent bugs in Garden
when it migrated: an error string sized to the whole window, a focus
underline measuring a differently truncated copy of its heading, and a caret
placed at `len(s) * cw` (a byte count times an average width).

---

## 3. Design decisions

### Roles first, families second

Hosts vary wildly (system TTFs, CSS stacks, embedded faces). Scripts select
by **role** — `ui` (proportional sans), `mono`, `serif` — with CSS-style
fallback: `{font: "Inter, ui"}`. A host maps roles to faces (its policy);
the role vocabulary is the standard's. `fonts()` lets a script discover
host-specific families. An unknown name degrades to the default face for
both measuring and drawing, so scripts never break on a host with fewer
fonts.

### Layout lives script-side; hosts provide metrics only

The alternative — a `text_block` draw command the host wraps and lays out —
was rejected: it puts layout policy into six rasterizers, makes results
host-dependent, and breaks the measure-then-draw pattern apps need for
variable-height rows. Instead the host binds *data* (advance tables) and the
layout algorithm runs once, in Petal, identically everywhere. This is the
`text_width` model, generalized. The cost is layout in interpreted Petal,
mitigated by caching and by measurement being native.

### Measurement and drawing must agree

Every measurement bug found so far came from measuring one thing and drawing
another. Two rules follow: the same style record goes to `text_width` and to
`draw_text`, and a font object carries its size and decorations with it so
the two cannot drift. `bind_default_font_name` exists because a style with no
`font` used to measure regular metrics for bold text the host drew bold.

### Protocol compatibility

The new `Text` fields are `skip_serializing_if`-defaulted, so a plain text
command serializes to the exact pre-typography JSON. Hosts that ignore the
fields fall back to their one font — degradation, not breakage.

### Baseline and vertical metrics (for flow layout)

Today `y` means "top of the glyph box" and line spacing is by convention
(`row_h = FS + padding`). Flow layout will position runs on a shared
**baseline** per line, which is what makes mixed-size lines look right.
`ascent` / `descent` / `line_height` ratios will be added to each
`text_fonts` entry without changing the binding's shape. The plain
`draw_text` path keeps top-anchored semantics.

### Layout caching

Laying out a long paragraph every frame at 60 fps is wasted work.
`typo.layout` will return a plain record, so an app can hold it in `state`
and re-layout only when text or width changes; `typo.layout_cached(key,
blocks, width)` does exactly that.

---

## 4. The `typo` module — API sketch (not built)

Styles are records, composed with spread: `{...BODY, weight: 700}`.

```petal
let BODY  = {font: "ui", size: 15, color: #d8d8d8}
let EM    = {...BODY, italic: true}
let CODE  = {font: "mono", size: 13, color: #a8d8a8}

// Rich single line (replaces manual x-advancing)
typo.draw_line([
  typo.span("+12 ", {...BODY, color: #7ad87a}),
  typo.span("-4 ",  {...BODY, color: #d87a7a}),
  typo.span("in 3 files", BODY),
], x, y)
typo.line_width(spans)               // for centering / right-align
typo.draw_line_right(spans, right_x, y)

// Flow layout (the HTML-lite part)
let doc = [
  typo.p([typo.span("Merge pull request ", BODY),
          typo.span("#482", {...BODY, weight: 700})]),
  typo.p([typo.span(commit.body, BODY)], {spacing_before: 8}),
  typo.p([typo.span(diffstat, CODE)], {align: "right"}),
]
let layout = typo.layout(doc, avail_w)     // pure; no drawing
layout.height                              // size rows/cards by content
typo.draw(layout, x, y)                    // emit draw commands
// blocks: p (paragraph), h(level, spans), gap(px); paragraph opts:
// {align: left|center|right, line_height, spacing_before, max_lines, ellipsis}
```

`typo.layout` + `layout.height` + `typo.draw` replaces the
`row_lines`/`row_y`/`row_h` bookkeeping; `max_lines + ellipsis` replaces
`preview()`. A pixel-measured word-wrap is the natural first piece: `wrap`
and `preview` still take character counts, so any app wrapping a paragraph
still needs a `cw`.

Scope for this phase: blocks stack vertically, inlines wrap left-to-right.
No floats, tables, bidi, or justification. No text editing (that stays with
the host `text_view` widget). Shaping is best-effort advance-sum measurement;
hosts with real shapers may rasterize a few pixels differently, which is
accepted and bounded.

Tests: layout is pure, so line breaks and heights can be asserted headlessly
through the petal-ui harness without a renderer.

---

## 5. Open questions

- **Non-ASCII coverage.** Advance tables are dense codepoint-indexed lists,
  fine for Latin, wrong shape for CJK and emoji. Plan: cover 0–0x2FF and fall
  back to a per-font uniform ratio above that.
- **Perf of script-side flow layout** on very long documents. If
  `layout_cached` is not enough, the inner loop can move into Rust behind the
  same API.
- **Style interning.** If per-run optional fields bloat the command stream,
  add a `text_style(id, {...})` command and reference it by id. Deferred
  until measured.
- **Weight is effectively two-valued today**: SDL synthesizes at `>= 600`,
  the browser has whatever the family ships, Garden's embedded `mono` has one
  cut.
- **Raster feature.** SDL-class hosts still keep a per-size SDL_ttf ladder. A
  swash/fontdue glyph cache in petal-ui would replace it and give real
  weights; canvas hosts and Garden do not need it.

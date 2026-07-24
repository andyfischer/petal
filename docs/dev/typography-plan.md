# petal-typography — Tech Plan

Status: **proposed** · Author: investigation + plan, 2026-07-24

A new library for best-in-class text rendering in Petal apps: multiple fonts
(faces, weights, styles), correct proportional measurement, and a lightweight
HTML-like *flow layout* (styled spans wrapping into paragraphs, blocks stacking
into a column). It follows the established ecosystem split: a Petal-source
module for layout semantics, a small draw-protocol extension, and a host-side
engine crate that supplies metrics and (optionally) rasterization.

---

## 1. Where text rendering stands today

### The contract (petal-ui, Layer 2)

`DrawCommand::Text { text, x, y, size, r, g, b, a }` (`petal-ui/src/draw.rs:119`)
carries **no font, weight, or style** — the host picks one font for everything.
Measurement (`text_width`) has two models:

- **Monospace fallback** (default): `chars × size × 0.6`
  (`bind_text_metrics`, `SYM_TEXT_ADVANCE`).
- **Proportional**: per-codepoint advance-ratio table
  (`bind_text_advance_table`, `SYM_TEXT_ADVANCES`) — sums real glyph advances.

### Per-host reality

| Host | Font | Honors `size` | Proportional `text_width` |
|---|---|---|---|
| petal-sdl | System sans (Helvetica), SDL_ttf size ladder | yes | **yes** — measured ASCII advance table |
| petal-web-canvas (+ diagram-canvas) | `sans-serif` via canvas `fillText` | yes | **no — mismatch**: renders proportional, measures monospace 0.6 |
| Garden panels | JetBrains Mono via glyphon (cosmic-text + swash GPU atlas) | **no — fixed 14 px** (`panel_view.rs` drops `size`) | no (0.6 mono) |
| experiment-todo-app | System mono, SDL_ttf ladder | yes | no (0.6 mono, self-consistent by design) |
| experiment-cube-browser | `ui-monospace` canvas | yes | no (0.6 mono, self-consistent by design) |
| petal-fps | 5×7 bitmap (own `Text2d` command) | scale | n/a |

So: **monospace-first in practice**. Exactly one host (petal-sdl) renders
proportional text with correct metrics; web-canvas renders proportional but
measures monospace (a latent centering/alignment bug); Garden — ironically the
most capable text stack in the ecosystem — projects Petal panels down to one
monospace face at one fixed size. No host supports bold, italic, multiple
faces, or wrapping through the draw protocol.

### Script-side layout today

`ui.ptl` organically grew: `wrap` / `preview` (greedy word-wrap in *character*
budgets), `truncate_head` / `truncate_tail` (char-budget ellipsis),
`draw_text_right`, `fit_parts` (segment shedding). On top of that, apps
hand-roll, over and over:

1. px↔char conversion — `cw = text_width("0000000000", FS) / 10` then
   `int(avail / cw)` (git_panel, db_view, retro, garden-diff).
2. Centering — `x + (w - text_width(s, size)) / 2` at every call site.
3. Multi-color lines — draw a run, `x += text_width(run)`, draw the next run
   (retro.ptl:230, git_panel.ptl:437). No rich-text primitive exists.
4. Variable-height wrapped rows — parallel `row_lines`/`row_y`/`row_h` arrays
   built from `wrap()` results (retro.ptl:623-680) — a hand-rolled flow layout.
5. Line-granular color via the host `text_view` widget + parallel style array
   (git_panel.ptl:631) — anything finer than per-line is impossible today.

These are exactly the operations petal-typography should own.

---

## 2. Goals and non-goals

### Goals

- **Font selection** in the draw protocol: family/role, weight, italic — with
  graceful degradation on hosts that ignore it.
- **Correct proportional metrics everywhere**: per-font advance tables so
  `measure`/layout agree with what the host rasterizes.
- **Flow layout, HTML-lite**: styled spans → wrapped lines → paragraphs →
  stacked blocks with a width constraint; measure-then-draw so containers
  (rows, cards, scrollers) can size themselves from content.
- **Rich text**: one line mixing color/weight/size/face without manual
  x-advancing.
- Pixel-based truncation/fitting (ellipsis by *width*, not char count).
- Keep the ecosystem contract: **hosts implement only rasterization**; layout
  runs script/engine-side and is identical on every host.

### Non-goals (this phase)

- Full CSS: no floats, no inline-block, no tables, no bidi/RTL, no
  justification. Blocks stack vertically; inlines wrap left-to-right.
- No text editing/selection (that stays with the host `text_view` widget).
- No script-supplied font *files* — hosts own which fonts exist; scripts select
  from what's offered.
- Complex shaping correctness (ligatures, kerning pairs, CJK line-breaking
  rules) is best-effort: advance-sum measurement, per-codepoint. Hosts with
  real shapers (Garden/glyphon) may rasterize better than the measurement
  predicts; that error is accepted and bounded (same class of error HTML has
  with `ch`-based layouts).

---

## 3. Architecture

Mirrors the petal-query precedent — a script-side module paired with host-side
support, meeting at a small native/protocol boundary:

```
┌─ script ──────────────────────────────────────────────┐
│ typography.ptl  (module `typo`)                        │
│   spans, paragraphs, flow layout, fit/ellipsis, rich   │
│   lines — pure Petal, uses measure natives             │
└──────────────┬────────────────────────────────────────┘
               │ natives: font_list, font_metrics,
               │ measure(text, style) — and the existing
               │ draw buffer for output
┌──────────────┴────────────────────────────────────────┐
│ petal-typography crate (Rust)                          │
│   FontBook: enumerate/load fonts, per-font metrics     │
│   tables, registers natives + extended draw natives    │
│   optional feature: software rasterizer (swash) hosts  │
│   can embed                                            │
└──────────────┬────────────────────────────────────────┘
               │ draw_commands buffer (extended `text`)
┌──────────────┴────────────────────────────────────────┐
│ host rasterizers (SDL / canvas / Garden glyphon)       │
└───────────────────────────────────────────────────────┘
```

Three deliverables:

**(a) Protocol extension** — extend `DrawCommand::Text` with optional fields,
using the same `skip_serializing_if` trick that kept alpha/radius
backward-compatible (`draw.rs:47`): `font` (string), `weight` (u16, default
400), `italic` (bool), `spacing` (letter-spacing, f32 px, default 0). Absent
fields serialize to the exact pre-typography JSON, so every existing consumer
is untouched; hosts that don't understand the new fields fall back to their
one font — degradation, not breakage. No new command tag needed.

**(b) Host engine crate `petal-typography`** (new top-level dir, like
`petal-query`) — a `FontBook` the host constructs at startup:

- resolves **roles → concrete faces** (see §4.1), from system fonts and/or
  host-bundled TTFs;
- measures per-font/per-weight advance tables and binds them into the Env
  keyed by font id (generalizing today's single `SYM_TEXT_ADVANCES`);
- registers the script natives: `font_list()`, `font_metrics(style)`
  (ascent/descent/line-height ratios), `measure(text, style) -> width`;
- registers the extended `draw_text` native (record-style options bag);
- optional cargo feature `raster`: a swash/fontdue-based software rasterizer +
  glyph cache that SDL-class hosts can use instead of hand-rolling per-face ×
  per-weight ladders. Canvas hosts skip it (the browser rasterizes); Garden
  skips it (glyphon already does this better).

**(c) Script module `typography.ptl`** (module name `typo`), shipped inside the
crate via `include_str!` like `ui.ptl` — the flow-layout API (§5). Registered
as an implicit import by hosts that opt in, exactly like `ui` and `query`.

petal-ui itself stays put: its `Text` command grows the optional fields and
`text_width` learns to consult per-font tables, but its Layer-1/2 role is
unchanged. Typography is an opt-in layer above, so minimal hosts (petal-fps,
snippets) never pay for it.

---

## 4. Key design decisions

### 4.1 Font naming: roles first, families second

Hosts vary wildly (system TTFs on desktop, CSS stacks in browsers, embedded
faces in Garden). Scripts therefore select by **role**, with CSS-style
fallback:

- Built-in roles every host maps: `ui` (proportional sans), `mono`, `serif`.
- A style's `font` is a role name or a family name; `font_list()` lets a
  script discover host-specific families and pick with fallback:
  `{font: "Inter, ui"}`.

This keeps scripts portable while allowing a Garden theme or an SDL app to
offer real families. The host's role→face mapping is policy (its call), the
role vocabulary is semantics (the standard's call) — same split petal-ui
already uses for input.

### 4.2 Layout lives script-side; hosts provide metrics only

The alternative — a `text_block` draw command the host wraps and lays out —
was rejected: it moves layout policy into six rasterizers, makes results
host-dependent, and breaks the measure-then-draw pattern apps need for
variable-height rows. Instead the engine binds *data* (advance tables, line
metrics) and the layout algorithm runs once, in the `typo` module, identically
everywhere. This is the proven `text_width` model, generalized. Cost: layout
in interpreted Petal — mitigated by caching (§4.4) and by the measurement
natives being Rust.

### 4.3 Baseline and vertical metrics

Today `y` means "top of the glyph box" and line spacing is by convention
(`row_h = FS + padding`). Typography styles expose real
`ascent`/`descent`/`line_height` (as ratios of size, from the font). Flow
layout positions runs on a shared **baseline** per line — this is what makes
mixed-size/mixed-face lines look right, and is invisible to non-users: the
plain `draw_text` path keeps top-anchored semantics.

### 4.4 Layout caching

Flow layout of a long paragraph every frame at 60 fps is wasted work.
`typo.layout(...)` returns a plain record (lines → positioned runs), so apps
can hold it in a `state` slot and re-layout only when text/width changes. The
module provides `typo.layout_cached(key, blocks, width)` doing exactly that —
same "cache keyed by input" shape petal-query normalized.

---

## 5. Script API sketch (module `typo`)

Styles are records (the ecosystem's options-bag idiom); everything composes
with spread: `{...BODY, weight: 700}`.

```petal
// A style: any subset of {font, size, weight, italic, color, spacing, underline}
let BODY  = {font: "ui", size: 15, color: #d8d8d8}
let EM    = {...BODY, italic: true}
let CODE  = {font: "mono", size: 13, color: #a8d8a8}

// ── Measurement ───────────────────────────────────────────────
typo.measure("hello", BODY)          // -> width in px (native, per-font table)
typo.line_height(BODY)               // -> px
typo.fit("some/long/path.rs", BODY, 240)        // pixel-budget tail-ellipsis
typo.fit_head("a long subject line", BODY, 240) // head-ellipsis

// ── Rich single line (replaces manual x-advancing) ────────────
typo.draw_line([
  typo.span("+12 ", {...BODY, color: #7ad87a}),
  typo.span("-4 ",  {...BODY, color: #d87a7a}),
  typo.span("in 3 files", BODY),
], x, y)
typo.line_width(spans)               // for centering / right-align
typo.draw_line_right(spans, right_x, y)

// ── Flow layout (the HTML-lite part) ──────────────────────────
let doc = [
  typo.p([typo.span("Merge pull request ", BODY),
          typo.span("#482", {...BODY, weight: 700})]),
  typo.p([typo.span(commit.body, BODY)], {spacing_before: 8}),
  typo.p([typo.span(diffstat, CODE)], {align: "right"}),
]
let layout = typo.layout(doc, avail_w)     // pure; no drawing
layout.height                              // -> px: size rows/cards by content
typo.draw(layout, x, y)                    // emit draw commands
// blocks: p (paragraph), h(level, spans), gap(px); paragraph opts:
// {align: left|center|right, line_height, spacing_before, max_lines, ellipsis}
```

`typo.layout` + `layout.height` + `typo.draw` directly replaces the
`row_lines`/`row_y`/`row_h` bookkeeping in retro.ptl; `max_lines + ellipsis`
replaces `preview()`; `typo.fit` replaces every `cw`-budget truncation.

---

## 6. Host rollout

| Host | Work |
|---|---|
| **petal-sdl** | Adopt `FontBook` + the `raster` feature (swash glyph cache replaces the per-size SDL_ttf ladder; gains weights/italics/faces). Reference desktop host. |
| **petal-web-canvas** | Map style → `ctx.font` string (trivial — canvas already does faces/weights/italics). Bind advance tables from `ctx.measureText` at startup, **which also fixes the existing proportional-vs-monospace mismatch**. TS mirror of the role→family mapping. |
| **diagram-canvas / cube-browser** | Inherit the web-canvas renderer work; cube can stay mono-only (roles all map to mono) with zero script changes. |
| **Garden panels** | Biggest win, least new tech: glyphon/cosmic-text already shapes and falls back. Stop dropping `size` in `panel_view.rs`, carry font/weight/italic through `PanelCmd::Text` into `Primitive::Text`, map roles onto Garden's font config. Bind cosmic-text-measured advance tables instead of the 0.6 ratio. |
| **todo-app** | Optional; keeps working unchanged (mono roles). Good second SDL adopter to validate the `raster` feature. |
| **petal-fps** | No change (own command set; bitmap font is part of its aesthetic). |

---

## 7. Phasing

1. **Phase 0 — metrics groundwork (bug-fix value on its own).**
   Per-font advance-table plumbing in petal-ui (`text_width` consults the
   table for the *default* font as today; tables become keyed). Bind real
   tables in web-canvas (fixes the centering mismatch) and Garden (replaces
   0.6). Garden honors `size`. No new API surface for scripts.
2. **Phase 1 — protocol + engine.** Optional `font/weight/italic/spacing`
   fields on `Text`; `petal-typography` crate with `FontBook`, roles,
   `font_list` / `font_metrics` / `measure` natives; SDL + web-canvas +
   Garden honor the new fields. Docs: a `docs/text-and-fonts.md` describing
   the text protocol (today it's documented only in `draw.rs` comments).
3. **Phase 2 — the `typo` module.** Spans, rich lines, `fit`, flow layout
   with measure/draw split, layout cache. Headless tests via the petal-ui
   harness (layout is pure — assert line breaks/heights without a renderer).
4. **Phase 3 — `raster` feature + migration.** Swash-based glyph cache for
   SDL-class hosts; port retro.ptl's wrapped rows and git_panel's rich lines
   as the proving apps; petal-lang.org snippet showing flow layout.
   `ui.ptl`'s `wrap`/`preview`/`truncate_*` stay (char-budget tools are still
   right for mono grids) but docs point layout work at `typo`.

---

## 8. Risks / open questions

- **Measurement fidelity**: advance-sum ignores kerning/ligatures; with
  shaping hosts (Garden) rendered width can differ by a few px from measured.
  Bounded and acceptable for UI text; revisit only if justified text lands.
- **Non-ASCII coverage**: advance tables are dense codepoint-indexed lists —
  fine for ASCII/Latin-1, wrong shape for CJK/emoji. Plan: table covers
  0–0x2FF, everything above falls back to a per-font uniform ratio (emoji ≈
  1.0, CJK ≈ 1.0); hosts with shapers may register a native `measure`
  override, which the module always calls through.
- **Perf of script-side flow layout** on very long documents — mitigated by
  `layout_cached` and by `measure` being native; if still hot, the layout
  inner loop can move into the crate behind the same API.
- **Style interning**: if per-run optional fields bloat the command stream,
  add a `text_style(id, {...})` command + id reference later — deferred until
  measured.

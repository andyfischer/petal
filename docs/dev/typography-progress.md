# petal-typography — Progress & Handoff

Living status tracker for the typography work.
**Design rationale lives in [`typography-plan.md`](typography-plan.md)** — read
it first. This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-08-02 (prelude pixel-budget helpers + Garden migration) ·
Branch: `main`

---

## Status board

| Phase | Status | Summary |
|-------|--------|---------|
| 0 — metrics groundwork | ✅ done | keyed per-font tables + `text_width(s, size, font)`; web-canvas and Garden both measure real metrics; Garden honors `size` |
| 1a — protocol + hosts | ✅ done | optional `font`/`weight`/`italic`/`spacing` on `Text`; style records on `draw_text`/`text_width`; SDL, web-canvas and Garden all honor them |
| 1b — `petal-typography` crate | ⬜ not started | `FontBook`, system-font enumeration, `font_list` / `font_metrics` / `measure` natives |
| 2 — the `typo` module | ⬜ not started | spans, rich lines, `fit`, flow layout, layout cache — but see "pixel budgets in the prelude" below, which took the two cheapest items early |
| 3 — raster + migration | ⬜ not started | swash glyph cache; port retro.ptl / git_panel.ptl (their *single-line* clipping is already ported) |

## Phase 0 — what's done

- **petal-ui** (`petal-ui/src/draw.rs`)
  - `FontMetrics { advance, advances }` — advance ratios of the font size,
    with `::monospace` / `::proportional` constructors.
  - `bind_font_metrics(env, name, &metrics)` — registers a *named* face into
    the `text_fonts` binding (a record keyed by name). Accumulates: binding a
    second face keeps the first.
  - `text_width(s, size, [font])` — the optional third argument selects a
    registered face by role name or CSS-style fallback list (`"Inter, ui"`).
    Unregistered names fall back to the default font, so scripts degrade
    rather than break. The default font is still the pre-existing
    `text_advance` / `text_advances` binding pair — no host had to change.
- **petal-web-canvas** — `src/text-metrics.ts` measures per-codepoint advance
  ratios (0x20–0x2FF) with `ctx.measureText` for roles `ui` / `mono` / `serif`
  and binds them through two new WASM exports (`set_default_font_metrics`,
  `set_font_metrics`). This **fixes the proportional-vs-monospace mismatch**:
  the host rendered `sans-serif` but measured 0.6 × size, so every centered or
  right-aligned label was off. The renderer now draws with the same stack the
  default table was measured from (`FONT_STACKS[DEFAULT_ROLE]`).
- **petal-sdl** — registers its loaded system face under the `ui` role in
  addition to the default-font binding.
- **docs** — [`docs/text-and-fonts.md`](../text-and-fonts.md) now documents the
  text protocol and metrics contract (previously only in `draw.rs` comments).
- **Garden** (separate repo, `~/garden`, commit `2b0f1a7`):
  `Primitive::Text` now carries a per-run `size` that the glyphon stack
  applies (it was dropped, pinning every panel run to the editor's 14 px), and
  `garden-render::ascii_advance_ratios()` measures the embedded JetBrains Mono
  through cosmic-text. `garden_script::set_font_advance_ratios()` publishes
  that table into the panel runtime — `garden-app` wires the two, since
  sibling crates can't depend on each other. Roles `mono` and `ui` both
  resolve to Garden's one face. Verified headless: a 10→40 px size ladder
  renders at the requested sizes with `text_width`-wide rules ending flush
  with their text.

Verified in Chrome against the live canvas (throwaway page, `vite dev`), at
size 20 — `text_width` vs `ctx.measureText`:

| Sample | `text_width` | canvas | old 0.6 estimate |
|---|---|---|---|
| `"Hello, world"` | 104 | 104.48 | 144 |
| `"iiiiiiiiii"` | 44 | 44.43 | 120 |
| `"WWWWWWWWWW"` | 189 | 188.77 | 120 |

Residual error is sub-pixel (integer rounding). Roles resolve as designed:
`text_width("iiiiiiiiii", 20, "mono")` → 120, an unregistered name → 44 (the
default font).

### Phase 0 remainder

- **Rebuild the web-canvas WASM** after pulling: `pkg/` is gitignored and
  generated, and `bindFontMetrics` (called at runtime init) needs the two new
  exports, so run `integrations/petal-web-canvas/build-wasm.sh` on any
  environment with a stale build.
- Optionally: sample-apps/diagram-canvas inherits the web-canvas renderer work
  (it currently keeps its self-consistent monospace estimate).

## Phase 1a — what's done

- **petal-ui** (`petal-ui/src/draw.rs`)
  - `DrawCommand::Text` grew `font` / `weight` / `italic` / `spacing`, each
    `skip_serializing_if`-defaulted, so a plain text command serializes to the
    exact pre-typography JSON and existing consumers see no change. Decoding
    reads them from emitted args 8–11, which a pre-typography emitter simply
    doesn't send.
  - `TextStyle` — the script-facing style record (`{size, color, font, weight,
    italic, spacing}`), any subset. `draw_text(text, x, y, style)` (natively)
    and `draw_text(text, pos, style)` (prelude overload) emit it; the
    typography args are appended *only* when non-default.
  - `text_width(s, style)` measures the same record — face, weight/italic
    variant, and letter-spacing — so measurement and rasterization agree.
  - Variant registry: `bind_font_variant_metrics(env, name, weight, italic, …)`
    stores under canonical keys (`ui@700`, `ui@i`, `ui@700i`); lookup walks a
    family's variants most-specific-first before moving to the next family in a
    fallback list (CSS's family-then-variant order).
  - `bind_default_font_name(env, role)` — without it, a style with no `font`
    would measure regular metrics for bold text while the host *drew* it bold.
    Found by comparing `text_width` against `ctx.measureText` in Chrome.
  - `DrawCommand::plain_text(…)` for hosts/tests building a command by hand.
- **petal-sdl** — `FontBook` (roles `ui` + `mono`, each a size ladder) replaces
  the single ladder; synthetic bold/italic via `TTF_SetFontStyle`; letter-
  spacing by per-glyph placement; metrics bound per face × variant. Verified by
  screenshot: seven styles, each underlined by a `text_width`-wide rule.
- **petal-web-canvas** — style → CSS font shorthand (`italic 700 20px stack`),
  `ctx.letterSpacing` where the browser has it, role→stack resolution with
  fallback lists; tables measured for all four variants of every role. Verified
  in Chrome: `text_width` vs `ctx.measureText` agrees within a pixel for all
  seven styles (before the default-face fix, bold sans was off by 8%).
- **Garden** (separate repo) — `PanelCmd::Text` and `Primitive::Text` carry the
  axes; glyphon attrs get weight/slant; letter-spacing is applied host-side by
  splitting a spaced run into per-glyph runs whose pen matches `text_width`
  exactly. `/scene` reports the axes when a run uses one. Garden has one
  embedded face, so **bold degrades to regular** (italic happens to resolve
  through a system fallback); embedding JetBrains Mono Bold would light it up
  with no protocol change.
- **example** — `examples/typography.ptl` (shipped in both petal-sdl and
  petal-web-canvas): a seven-row style ladder, each row underlined by a rule
  drawn `text_width(label, style)` wide, so a measurement/rasterization
  disagreement is visible as an overshooting or short rule.

### Known gaps

- `~/biz/experiment-todo-app` does not compile against current petal-ui — but
  it already didn't before this phase (its `DrawCommand` patterns predate the
  alpha/radius/width fields). Fixing it is 7 patterns plus a `..`.
- Weight is only really two-valued in practice today: SDL synthesizes at
  `>= 600`, the browser has whatever the family ships, Garden has one weight.

## Pixel budgets in the prelude (out of phase order)

Plan §1 item 1 — every app converting a pixel width into a character count via
`cw = text_width("0000000000", FS) / 10` — was slated for the `typo` module's
`fit`. It turned out to need no crate and no new protocol, only helpers that
take a **pixel** budget and measure, so it landed in `ui.ptl` ahead of Phase 1b.
Item 2 (centering) came along with it.

- **`ellipsize(s, avail_px, size)`** — the measured counterpart to
  `truncate_head`: trims until the run plus its "…" fits.
- **`ellipsize_tail(s, avail_px, size)`** — the mirror, keeping the *tail* under
  a leading "…". This is what a file path wants; head-keeping spends the box on
  the directories every row shares. Both append/prepend the ellipsis **once, at
  the end**, never measuring it back into the string being trimmed — that is a
  fixed point that never returns, because `len`/`slice` are byte-indexed while
  `text_width` counts characters. Trimming from the front is nonetheless safe:
  `slice` snaps a *start* index up to a character boundary (an end index down),
  so each pass drops a whole character and the loop strictly shrinks.
- **`draw_text_center`** — the counterpart to `draw_text_right`.
- **Record draw overloads carrying alpha** on outlines, circles and text, so a
  translucent UI stops re-wrapping the flat arg-list forms.

**Garden migrated** (separate repo, commit `63fb391`): `cw` is gone from
`git_panel`, `db_view` and `garden_diff` entirely, and survives in `retro` only
where the budget genuinely is in characters — `wrap` / `preview`, and the prompt
text bound for a monospace `text_view` grid. Measuring instead of estimating
surfaced three latent bugs in the process: a pane-local error string sized to
the whole window, a focus underline measuring a *separately* truncated copy of
the heading drawn above it, and a text caret placed at `len(s) * cw` — a byte
count times an average width, which drifted on proportional text and jumped
outright on non-ASCII.

### What this does *not* cover

Multi-line work is still character-budgeted: `wrap` and `preview` take character
counts, so any app wrapping a paragraph still needs a `cw`. A pixel-measured
word-wrap is the natural next helper, and is the part that genuinely wants the
`typo` module's line breaker rather than another prelude loop. Plan §1 items 3–5
(rich runs, flow layout, sub-line color) are untouched.

## Notes for the next phase

- Phase 1's `font`/`weight`/`italic`/`spacing` fields on `DrawCommand::Text`
  should use the same `skip_serializing_if` trick the alpha/radius fields use
  (`draw.rs`), so pre-typography JSON stays byte-identical.
- The `text_fonts` record is deliberately shaped as
  `{name: {advance, advances}}` — Phase 1 adds `ascent` / `descent` /
  `line_height` to each entry for baseline-aware flow layout, without changing
  the binding's shape or the `text_width` path.

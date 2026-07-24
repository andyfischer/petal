# petal-typography — Progress & Handoff

Living status tracker for the typography work.
**Design rationale lives in [`typography-plan.md`](typography-plan.md)** — read
it first. This doc tracks *what is done, what remains, and how to continue*.

Last updated: 2026-07-24 (Phase 0 landed in-repo) · Branch: `main`

---

## Status board

| Phase | Status | Summary |
|-------|--------|---------|
| 0 — metrics groundwork | ✅ done | keyed per-font tables + `text_width(s, size, font)`; web-canvas and Garden both measure real metrics; Garden honors `size` |
| 1 — protocol + engine | ⬜ not started | optional `font`/`weight`/`italic`/`spacing` on `Text`; `petal-typography` crate |
| 2 — the `typo` module | ⬜ not started | spans, rich lines, `fit`, flow layout, layout cache |
| 3 — raster + migration | ⬜ not started | swash glyph cache; port retro.ptl / git_panel.ptl |

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

## Notes for the next phase

- Phase 1's `font`/`weight`/`italic`/`spacing` fields on `DrawCommand::Text`
  should use the same `skip_serializing_if` trick the alpha/radius fields use
  (`draw.rs`), so pre-typography JSON stays byte-identical.
- The `text_fonts` record is deliberately shaped as
  `{name: {advance, advances}}` — Phase 1 adds `ascent` / `descent` /
  `line_height` to each entry for baseline-aware flow layout, without changing
  the binding's shape or the `text_width` path.

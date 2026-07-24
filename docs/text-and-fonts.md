# Text and fonts

How text reaches the screen in a Petal app, and how a script measures it.
Companion to the [typography plan](dev/typography-plan.md), which describes
where this is heading.

## The contract

Drawing text is a two-party arrangement:

- The **script** decides *what* to draw and *where* — including anything that
  needs a width (centering, right-alignment, wrapping, ellipsis).
- The **host** owns *how* it rasterizes: which font file, which shaper, which
  glyph cache. Petal never ships fonts.

That only works if both sides agree on how wide a string is. So a host binds
its measurements into the environment, and the script reads them back through
`text_width`.

## Script side

```petal
draw_text("hello", x, y, size, r, g, b, [a])   // emits a `text` draw command
text_width("hello", size)                       // px width, default font
text_width("hello", size, "mono")               // px width, a named face
```

`text_width` is exact for the host's font when the host bound real metrics,
and an estimate otherwise (monospace, 0.6 × size per character).

The optional third argument names a face. Use the portable **roles** rather
than family names where you can:

| Role | Meaning |
|---|---|
| `ui` | The host's proportional UI face — normally also its default font |
| `mono` | A fixed-pitch face |
| `serif` | A serif face |

A CSS-style fallback list works too: `text_width(s, size, "Inter, ui")` takes
the first face the host registered. A face the host doesn't offer measures
with the default font — so a script asking for something exotic degrades
instead of breaking, and the same script is portable across embedders.

> Selecting a face for *drawing* (as opposed to measuring) is not in the draw
> protocol yet — see the plan's Phase 1. Today every `text` command renders in
> the host's default font.

## Host side

Bind metrics once the fonts are loaded (petal-sdl does it in
`on_program_loaded`; petal-web-canvas measures with `ctx.measureText` at
startup):

```rust
use petal_ui::draw::{bind_text_metrics, bind_text_advance_table, bind_font_metrics, FontMetrics};

// The default font — what `text_width(s, size)` measures.
bind_text_metrics(env, 0.6);                    // uniform fallback ratio
bind_text_advance_table(env, &ratios);          // ratios[codepoint] = advance ÷ size

// Additional named faces — what `text_width(s, size, "mono")` measures.
bind_font_metrics(env, "mono", &FontMetrics::monospace(0.6));
bind_font_metrics(env, "ui", &FontMetrics::proportional(ratios, 0.5));
```

Advance tables are **ratios of the font size**, not pixel widths, so one table
serves every size (glyph advance scales linearly with size). Measure at a
large probe size and divide. Control codes should measure 0.

A table is codepoint-indexed and dense, so it's sized for ASCII/Latin; anything
past its end uses the uniform fallback ratio. That's a known approximation for
CJK and emoji (see the plan's §8).

A host with a real shaper may register its own `text_width` native instead —
the binding path is the default, not a requirement.

## Per-host status

| Host | Font | Honors `size` | Measurement |
|---|---|---|---|
| petal-sdl | System sans, SDL_ttf size ladder | yes | measured ASCII advance table; also registered as role `ui` |
| petal-web-canvas | `sans-serif` via `fillText` | yes | measured with `ctx.measureText` (roles `ui` / `mono` / `serif`) |
| diagram-canvas / cube-browser | canvas, mono stack | yes | monospace estimate (self-consistent) |
| Garden panels | JetBrains Mono via glyphon | no — fixed 14 px | monospace estimate |
| petal-fps | 5×7 bitmap font (own command set) | scale | n/a |

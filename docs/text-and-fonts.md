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

### Styles

Anything beyond size and color is a **style record**, which both `draw_text`
and `text_width` accept — so what you measure is what you draw:

```petal
let BODY = {size: 15, color: #d8d8d8}
draw_text("Merge pull request ", {x: 10, y: 20}, BODY)
draw_text("#482", {x: 170, y: 20}, {...BODY, weight: 700})
draw_text("(draft)", {x: 210, y: 20}, {...BODY, italic: true, font: "mono"})
text_width("#482", {...BODY, weight: 700})      // measures the bold face
```

| Field | Meaning | Default |
|---|---|---|
| `size` | Font size in px | 14 |
| `color` | `{r, g, b, [a]}` | white, opaque |
| `font` | Role or family, CSS-style fallback list allowed | the host's default face |
| `weight` | CSS numeric weight, 100–900 (700 = bold) | 400 |
| `italic` | Slant | `false` |
| `spacing` | Letter-spacing in px, added after every glyph | 0 |

Every field is optional, and an omitted one means *plain text*: a style is a
diff against what `draw_text` always did. A style that names none of the
typographic fields emits exactly the pre-typography command, byte for byte —
which is also why adding styles to an app costs nothing until it uses one.

Degradation is per axis, not all-or-nothing: a host with one weight draws (and
measures) bold as regular; a host with one face resolves every role to it. The
script stays the same and nothing breaks.

A face is named by `font` (or by `text_width`'s optional third argument). Use
the portable **roles** rather than family names where you can:

| Role | Meaning |
|---|---|
| `ui` | The host's proportional UI face — normally also its default font |
| `mono` | A fixed-pitch face |
| `serif` | A serif face |

A CSS-style fallback list works too: `{font: "Inter, ui"}` takes the first face
the host registered. A face the host doesn't offer measures — and draws — with
the default font, so a script asking for something exotic degrades instead of
breaking, and the same script is portable across embedders.

Matching is family-first, then variant within it, like CSS: asking for bold in
a family the host has only a regular of gets *that family's* regular, not some
other family's bold.

## Host side

Bind metrics once the fonts are loaded (petal-sdl does it in
`on_program_loaded`; petal-web-canvas measures with `ctx.measureText` at
startup):

```rust
use petal_ui::draw::{
    bind_default_font_name, bind_font_metrics, bind_font_variant_metrics,
    bind_text_advance_table, bind_text_metrics, FontMetrics,
};

// The default font — what `text_width(s, size)` measures.
bind_text_metrics(env, 0.6);                    // uniform fallback ratio
bind_text_advance_table(env, &ratios);          // ratios[codepoint] = advance ÷ size

// Additional named faces — what `text_width(s, size, "mono")` measures.
bind_font_metrics(env, "mono", &FontMetrics::monospace(0.6));
bind_font_metrics(env, "ui", &FontMetrics::proportional(ratios, 0.5));

// One entry per variant you can actually draw — bold is wider, so a bold
// style measured with the regular table would come out short.
bind_font_variant_metrics(env, "ui", 700, false, &FontMetrics::proportional(bold, 0.5));

// Which role your default font *is*, so a style that names no face still
// finds that face's variants (`{weight: 700}` → `ui@700`).
bind_default_font_name(env, "ui");
```

Variants are stored under a canonical key — `ui`, `ui@700`, `ui@i`, `ui@700i`
— so the registry stays one flat record. Bind only the variants you have: an
unbound one falls back within the family, which is exactly how it will render.

Advance tables are **ratios of the font size**, not pixel widths, so one table
serves every size (glyph advance scales linearly with size). Measure at a
large probe size and divide. Control codes should measure 0.

A table is codepoint-indexed and dense, so it's sized for ASCII/Latin; anything
past its end uses the uniform fallback ratio. That's a known approximation for
CJK and emoji (see the plan's §8).

A host with a real shaper may register its own `text_width` native instead —
the binding path is the default, not a requirement.

## Per-host status

| Host | Faces | `weight` / `italic` | `spacing` | Measurement |
|---|---|---|---|---|
| petal-sdl | System sans (`ui`) + system mono (`mono`), SDL_ttf size ladder | yes — SDL_ttf synthetic bold/oblique | yes — per-glyph placement | measured per face × variant |
| petal-web-canvas | `ui` / `mono` / `serif` CSS stacks | yes — the browser's own faces | yes — `ctx.letterSpacing` where supported | measured per face × variant with `ctx.measureText` |
| diagram-canvas / cube-browser | inherits web-canvas | yes | yes | inherits web-canvas |
| Garden panels | one embedded JetBrains Mono; every role resolves to it | italic yes; **bold is synthetic** (no bold face embedded — `weight >= 600` is emboldened by a second offset draw, and measures regular) | yes — the host places each glyph | measured through cosmic-text |
| petal-fps | 5×7 bitmap font (own command set) | no | no | n/a |

`size` is honored everywhere (per run, including Garden panels).

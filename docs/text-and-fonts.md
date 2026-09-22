# Text and fonts

How text reaches the screen in a Petal app, and how a script measures it.
Companion to the [typography plan](dev/typography-plan.md), which describes
where this is heading.

## The contract

Drawing text is a two-party arrangement:

- The **script** decides *what* to draw and *where* — including anything that
  needs a width (centering, right-alignment, wrapping, ellipsis) *or a height*
  (vertical centering, setting two lines apart).
- The **host** owns *how* it rasterizes: which font file, which shaper, which
  glyph cache. Petal never ships fonts.

That only works if both sides agree on the shape of a string. So a host binds
its measurements into the environment, and the script reads them back through
`text_width` and `text_metrics`.

## Script side

```petal
draw_text("hello", x, y, size, r, g, b, [a])   // emits a `text` draw command
text_width("hello", size)                       // px width, default font
text_width("hello", size, "mono")               // px width, a named face
text_advance("hello", style)                    // the same width as a float
text_metrics(size)                              // px: where the ink sits (below)
text_wrap(s, style, 200.0)                      // -> [line], greedy word wrap
text_ellipsize(s, style, 200.0, "tail")         // -> "a long lab…"
text_index_at(s, style, dx)                     // the character a click lands on
font("Helvetica Neue")                          // a font object (see below)
fonts()                                         // families this host can draw
```

`text_width` is exact for the host's font when the host bound real metrics,
and an estimate otherwise (monospace, 0.6 × size per character). It rounds to
a whole pixel; `text_advance` takes the same arguments and returns the
unrounded float, which is what column math over a monospace face needs. The last three
above measure with the same table in one walk, rather than a `text_width` call
per character — which is what a script otherwise writes, on every frame.

### The other axis

`text_width` answers "how wide". `text_metrics(style)` answers where the ink
lands, in **pixels** at that style's size:

```petal
let m = text_metrics(style)
// m.baseline    the y draw_text was given -> the baseline
// m.descent     the baseline -> the bottom of the line box
// m.line_height one line's y -> the next line's y
// m.cap_height  the baseline -> the top of a flat capital
// m.x_height    the baseline -> the top of a lowercase "x"
```

Without this a script has to guess, and the guess every script made was

```petal ignore
draw_text(label, {x: x, y: r.y + (r.h - size) / 2}, style)   // wrong
```

— *the run is `size` px tall and starts at `y`*. Neither half is true: `y` is
the top of the line box, the baseline sits some way down inside it, and a
capital's ink reaches neither edge. The label lands a pixel or two high at
14 px and visibly high at 32.

Centre a UI label on its **cap height** — a label with no descender looks high
when its line box is centred:

```petal ignore
let m = text_metrics(style)
let baseline = r.y + r.h / 2 + m.cap_height / 2
draw_text(label, {x: x, y: baseline - m.baseline}, style)
```

In practice, don't write that either: use
[`petal-libs/text-layout`](../petal-libs/text-layout/), which is this
arithmetic plus alignment, wrapping and caret hit-testing.

A host that binds no vertical metrics still answers, with typical UI-sans
proportions (`baseline` 0.8, `descent` 0.2, `line_height` 1.2, `cap_height`
0.7, `x_height` 0.52 × size) — much closer than the assumption they replace,
but still a guess about someone else's font.

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

Every field is optional, and an omitted one means *plain text*. A style that
names none of the typographic fields emits the same plain `text` command as a
bare `draw_text` call, so adding styles to an app costs nothing until it uses
one.

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

### Font objects

`font(name)` returns the face as a **value** rather than a string:

```petal
let body = font("Helvetica Neue", 15)
let title = font_size(font_bold(body), 28)

draw_text("Chapter One", {x: 20, y: 40}, title)
let w = text_width("Chapter One", title)         // measures that exact cut
```

A font object *is* a style record — the same one `draw_text`, `text_width` and
every widget's `style` argument already take — so it drops into all of them and
merges the usual way (`{...title, color: ACCENT}`). Naming a face as an object
is what lets the size and the decorations travel with it: one value describes a
run's whole appearance, so measuring it cannot drift from drawing it.

| Helper | Returns |
|---|---|
| `font(name)` / `font(name, size)` | a font object naming that face |
| `font_size(f, size)` | `f` at a different size |
| `font_weight(f, w)` / `font_bold(f)` | `f` at a CSS weight / at 700 |
| `font_italic(f)` | `f` slanted |
| `font_spacing(f, px)` | `f` with letter-spacing |
| `font_color(f, c)` | `f` in a color |

Each decoration returns a **new** object, so they compose and never mutate the
font they were given.

`name` is a family name, a role (`ui`, `mono`, `serif`), or a CSS-style
fallback list — the same vocabulary the `font` field takes. A host that
recognizes the family answers with its own canonical spelling, so
`font("helvetica")` and `font("Helvetica")` produce the same object; one that
cannot draw it hands the name back unchanged and it degrades to the default
face for both measuring and drawing, exactly like a bare `{font: …}` string.

`fonts()` lists the families this host can draw, for a font picker. It is empty
on a host that offers only its own faces — treat that as "no choice here", not
as an error.

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

// …and the vertical half, for the default face. Every field is a ratio of the
// font size, and `baseline` is measured against **your own** convention: how
// far below the `y` of a `text` command your renderer puts the baseline.
bind_text_vertical_metrics(env, &VerticalMetrics {
    baseline: 0.78, descent: 0.22, line_height: 1.2,
    cap_height: 0.72, x_height: 0.52,
});
// A named face carries its own, attached to the same record:
bind_font_metrics(env, "ui", &FontMetrics::proportional(ratios, 0.5)
    .with_vertical(VerticalMetrics::from_ascent_descent(0.78, 0.22, 0.2)
        .with_heights(0.72, 0.52)));
```

`baseline` is the field only the host can answer, and the one worth getting
exactly right: SDL blits a run's surface with its top at `y` (so the baseline
is the face's ascent), a canvas with `textBaseline = "top"` uses its font
bounding box, and Garden lays a run out in a line box with leading above it. A
script never has to know which — it asks for `baseline` and gets the truth
about this host. Measure it rather than deriving it: the SDL host reads
`Font::ascent()`, the canvas host `fontBoundingBoxAscent`, and Garden reads
`line_y` back off a shaped run.

Publishing nothing is not an error, only an approximation — see
[The other axis](#the-other-axis).

Variants are stored under a canonical key — `ui`, `ui@700`, `ui@i`, `ui@700i`
— so the registry stays one flat record. Bind only the variants you have: an
unbound one falls back within the family, which is exactly how it will render.

Advance tables are **ratios of the font size**, not pixel widths, so one table
serves every size (glyph advance scales linearly with size). Measure at a
large probe size and divide. Control codes should measure 0.

A table is codepoint-indexed and dense, so it's sized for ASCII/Latin; anything
past its end uses the uniform fallback ratio. That's a known approximation for
CJK and emoji (see the [typography plan](dev/typography-plan.md)).

A host with a real shaper may register its own `text_width` native instead —
the binding path is the default, not a requirement.

### Fonts the host discovers rather than publishes

Binding works for the handful of faces a host knows up front. It is the wrong
shape for "any family installed on this machine": there can be hundreds,
measuring one means shaping every glyph in it, and a script typically wants
two. A host that can reach a real font database attaches a `FontSource`
instead, and is asked — lazily — only about the faces a script actually names:

```rust
use petal_ui::draw::{FontMetrics, FontSource};

struct SystemFonts;

impl FontSource for SystemFonts {
    /// The canonical spelling of `name`, or None if we can't draw it.
    fn resolve(&mut self, name: &str) -> Option<String> { … }
    /// ASCII advance ratios for one cut of a resolved family.
    fn metrics(&mut self, family: &str, weight: u16, italic: bool) -> Option<FontMetrics> { … }
    /// Everything a script could name here — the answer to `fonts()`.
    fn families(&mut self) -> Vec<String> { … }
}

// Swapped into the thread-local channel around `env.run`, like the host-data
// provider (`Headless::set_font_source` does this for tests).
let saved = petal_ui::draw::swap_font_provider(Some(Box::new(SystemFonts)));
```

The published registry still wins wherever it has an answer, so a host can keep
binding its own roles eagerly and let the source cover the rest — the two can
never disagree about the same name. Answers are memoized for the process (a
`text_width` in a 60fps draw loop must not re-measure a font every frame); a
test attaching a second source calls `clear_font_cache` first.

A host with no source is unchanged: `font(name)` hands back the name it was
given, `fonts()` is empty, and measuring an unknown face falls through to the
default font.

## Per-host status

| Host | Faces | `weight` / `italic` | `spacing` | Measurement |
|---|---|---|---|---|
| petal-sdl | System sans (`ui`) + system mono (`mono`), SDL_ttf size ladder | yes — SDL_ttf synthetic bold/oblique | yes — per-glyph placement | measured per face × variant, both axes |
| petal-web-canvas | `ui` / `mono` / `serif` CSS stacks | yes — the browser's own faces | yes — `ctx.letterSpacing` where supported | measured per face × variant with `ctx.measureText`, both axes |
| diagram-canvas / cube-browser | inherits web-canvas | yes | yes | inherits web-canvas |
| Garden panels | **any family installed on the machine**, via `font(name)` / a `font:` string; plus two embedded faces reachable as `mono` (JetBrains Mono) and `ui` (Inter) | real cuts on a system family and on `ui` (Inter Bold is embedded); **synthetic on `mono`**, which has one cut — `weight >= 600` is emboldened by a second offset draw and measures regular | yes — the host places each glyph | measured through cosmic-text, on demand per family × cut; vertical metrics for the two embedded roles |
| petal-fps | 5×7 bitmap font (own command set) | no | no | n/a |
| `petal-ui-run` (headless) | none — no renderer | n/a | n/a | the built-in defaults, on both axes, so a trace is stable across machines |

`size` is honored everywhere (per run, including Garden panels).

Every host in the table above publishes vertical metrics except `petal-fps`,
which has its own command set, and `petal-ui-run`, which has no renderer to
measure and so stays on the defaults deliberately: a headless trace must not
depend on the fonts installed on the machine running it.

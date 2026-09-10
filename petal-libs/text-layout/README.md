# text-layout

Placing text, written entirely in Petal. Cap-height centring, alignment on both
axes, wrapping, elision and caret hit-testing — about 400 lines of `.ptl` over
five host natives, and not one line of Rust.

```petal
import text_layout

// One line, centred the way a designer means centred.
text_layout.draw_text_line("Save", r, style, "center")

// A paragraph: wrapped, two lines at most, the second one elided.
text_layout.draw_text_block(summary, card, style, {lines: 2, lead: 1.15})

// A path that has to fit a column.
text_layout.elide(path, style, col.w, "tail")     // "…/src/place.ptl"
```

## The bug it exists to fix

Every `draw_text` in every Petal UI has had to answer the same question, and
almost all of them answered it the same wrong way:

```petal ignore
draw_text(label, {x: x, y: r.y + (r.h - size) / 2}, style)
```

That line says *the run is `size` px tall and starts at `y`*. Neither half is
true. `y` is the top of the run's line box; the baseline sits some way down
inside it; and the ink of a capital reaches neither edge. So the label lands a
pixel or two high at 14 px and visibly high at 32 — consistently enough, across
a whole library, that it reads as a style rather than as a bug.

The host has always known the real numbers. Now it says so: `text_metrics(style)`
reports, in pixels, where a run's baseline falls below the `y` `draw_text` is
given, how far its descenders go, how tall a capital and an "x" are, and how far
apart to set two lines. This library turns those into the answer a caller
actually wants — a `y` to draw at.

```
  y ──────────────────────────── the point draw_text was given
    │  ▲ baseline
    │  │            ┌───┐   ▲ cap_height
    │  │      ┌──┐  │   │   │        ▲ x_height
    │  ▼      │  │  │   │   │        │
  baseline ───┴──┴──┴───┴───▼────────▼
    │  ▲ descent      │
    │  ▼              g
  ───────────────────────────── y + line_height (the next line's y)
```

"Centred" here means the **cap height** is centred, not the line box. A label
like "Save" has no descender, so centring its line box leaves it looking high;
centring the cap height is what a designer means by "centred in the button".

## Requirements

A host that embeds [`petal-ui`](../../petal-ui/) — Garden panels,
`petal-desktop-sdl`, `petal-web-canvas`, the `petal-ui-run` driver, or your own
embedder calling `petal_ui::register_all`. It asks for no natives of its own,
no fonts and no files at runtime.

A host that publishes no vertical metrics is not an error: `text_metrics`
answers with typical UI-sans proportions, which are much closer than the
assumption they replace. A host that measures its own face publishes it with
`bind_text_vertical_metrics`, and then the placement is exact.

## Installing it in a project

It is a *package*: `petal.toml` names it, so its modules answer to
`text_layout/…` wherever this directory lands. The directory is `text-layout`
and the package is `text_layout` — a package name is the first segment of an
import path, so it has to be spellable as an identifier.

```bash
# on the module path — point -I at the library root, or at a directory of them
petal-ui-run myapp/app.ptl -I petal-libs
petal packages -I petal-libs            # check what that made importable
```

```rust
// or registered by the host, which also covers scripts pushed as source
env.add_package("petal-libs/text-layout")?;
env.register_package("text_layout", MODULES)?;   // include_str! each .ptl
```

Garden ships the second form in
[`garden/garden-script/src/text_layout.rs`](../../garden/garden-script/src/text_layout.rs),
so `import text_layout` works in every Garden panel with no setup — and has to,
because [bloom](../bloom/) draws every one of its labels through it.

## The modules

| Module | Contents |
|--------|----------|
| `text_layout` (`src/text_layout.ptl`) | The facade. Named like the package, so a bare `import text_layout` finds it |
| `text_layout/place` | One line: `text_baseline`, `text_top`, `text_line_y`, `text_align_x`, `text_place`, `draw_text_line`, `draw_text_baseline` |
| `text_layout/fit` | Making it fit: `wrap_text`, `elide`, `clamp_lines`, `line_step`, `lines_width`, `lines_size`, `caret_index`, `caret_x`, `caret_rect` |
| `text_layout/block` | A paragraph in a box: `text_block`, `draw_text_block`, `text_size` |

Import the facade, or one module when you want a slice of it
(`import text_layout/fit: elide` in a list row that only truncates).

## The API

### One line

```petal ignore
draw_text_line(s, r, style)                    // left, centred vertically
draw_text_line(s, r, style, "center")          // "left" | "center" | "right"
draw_text_line(s, r, style, "right", "top")    // valign: "top" | "middle" | "bottom" | "baseline"
```

Returns the run's width, for a caller flowing something after it.

`text_place(s, r, style, align)` is the same placement without drawing —
`{x, y, w, baseline}`, with `y` ready for `draw_text`. Use it when the label is
one part of a composite run (an icon, a label, a chevron) and the caller lays
the row out itself.

`draw_text_baseline(s, x, baseline, style, align)` draws on a baseline you
already know: a chart axis, or a row of runs at different sizes that must sit on
one line.

### A paragraph

```petal ignore
let b = draw_text_block(body, r, style, {
  align: "left",      // "left" | "center" | "right"
  valign: "top",      // "top" | "middle" | "bottom"
  wrap: true,         // false keeps one line whatever its width
  lines: 0,           // a maximum line count; 0 for no limit
  elide: true,        // mark a cut with "…"; "tail"/"both" pick which end survives
  lead: 1.0,          // ×the face's own line spacing, or px when 4 or more
})
```

Both it and `text_block` (which measures and places without drawing) return

```petal ignore
{lines: [{text, x, y, baseline, w}], w, h, overflow}
```

so a caller can size a panel to its text, decide whether it overflowed, or hand
one layout to a hit-test and to the paint. Each line's `y` is ready for
`draw_text`.

`text_size(s, style, max_w)` is the measurement alone — `{w, h}` — for a
tooltip or a table row that has to size itself before it has anywhere to go.

The block's `h` is ink to ink: the first line's cap height down to the last
line's descenders, not `count × line_step`. Counting a whole line box per line
adds a leading above the first line that nothing is drawn in, and a box fitted
to *that* looks bottom-heavy.

### Fitting

```petal ignore
wrap_text(s, style, max_w)               // -> [line]; honours newlines, breaks long words
elide(s, style, max_w)                   // -> "a long lab…"
elide(s, style, max_w, "tail")           // -> "…/src/place.ptl"
elide(s, style, max_w, "both")           // -> "commit 9f…identifier"
clamp_lines(s, style, max_w, 2)          // wrap, stop at 2, elide the second
```

### Carets

```petal ignore
caret_index(s, style, dx)   // the character index a click dx px in falls on
caret_x(s, style, i)        // the x offset of the caret before character i
caret_rect(s, r, style, i)  // a 1px bar spanning the line's real ink
```

`caret_index` returns a **character** index, which is what `char_slice` takes —
not a byte offset.

## What is native, and why

Five natives in [`petal-ui/src/text.rs`](../../petal-ui/src/text.rs) do the work
this library composes:

| Native | What it answers |
|---|---|
| `text_width(s, style)` | how wide (this one always existed) |
| `text_metrics(style)` | where the ink sits relative to `y`, in px |
| `text_wrap(s, style, w)` | the lines it breaks into |
| `text_ellipsize(s, style, w, end)` | it, cut, with "…" |
| `text_index_at(s, style, dx)` | the character a click lands on |

The last three could be written in Petal, and were: the `ui` prelude's
`ellipsize` was a `text_width` call per character — a native call and a fresh
string per step, over a string the user is typing, on every frame. In Rust each
is one walk of the host's advance table, which is what makes wrapping a
paragraph cost about what drawing a label costs. The prelude's `ellipsize` and
`ellipsize_tail` now delegate to `text_ellipsize` and keep their signatures.

## Versioning

`text_layout.VERSION` counts additions to the export surface. Levels are
additive: every existing export keeps its signature, so a script written against
an older one keeps working.

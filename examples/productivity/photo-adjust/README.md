# 35 — Aperture: photo adjustment UI

A photo-develop desk: a library rail of five photographs, ten adjustment
sliders, six saved looks, a draggable before/after split, and a live luminance
histogram.

The twist is that nothing is loaded from disk. Each "photograph" is a **pure
Petal function of `(u, v)`** — layered gradients, value noise, fbm, silhouette
functions — sampled on an 80 × 54 grid, and each adjustment is a **real
per-sample colour transform** (exposure, tone, contrast, white balance,
saturation, fade, grain, vignette) run in Petal over that grid. The result is
painted as one quad per sample, so the pixels on screen really are the output
of the pipeline, not a picture of one.

## Run it

```bash
cd examples/productivity/photo-adjust
GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden --headless --debug-port 0 --init layout.ptl
```

Drop `--headless --debug-port 0` for a window. Designed for a **1280 × 850**
viewport (a 1268 × 778 pane); the layout is computed from
`screen_width()`/`screen_height()` and degrades gracefully, but the sample grid
is a fixed 640 × 432 block of pixels.

## Controls

| | |
|---|---|
| `1`–`5` / click a library card | choose a photograph |
| `↑` `↓` | move between sliders |
| `←` `→` | nudge the selected slider (±2), with `⇧` ±10 |
| drag a slider | set it directly — develops a **1:2 draft** while the pointer is down, full resolution on release |
| wheel over a slider | ±2 per notch |
| drag inside the image | move the before/after split |
| `[` `]` | nudge the split ±5% |
| `b` | toggle After / Split |
| `v` | toggle Before / Split |
| `space` | peek at the untouched negative without changing the mode |
| click a look, or `p` | apply / cycle the six presets |
| `0`, or the Reset button | clear every adjustment |

The histogram readout shows mean level `before → after`, the signed delta, and
a clipping percentage at whichever end is pinned.

## What it exercises

**Language** — `state` for the whole document model and for every cache;
for-loops in value position as the image builder (`let img = for j … end`);
`var`/`set` inside the scene functions where a pixel is composited in layers;
records as float colours and as rect/style values; `iclamp`/`smoothstep`/`fbm`
helpers over `sin`/`exp`/`pow`/`floor`; string interpolation for the cache
signatures; list index assignment (`negatives[photo] = …`, `set pending[i] = …`).

**Host** — the full draw surface (`draw_rect`, `draw_rect_rounded`,
`draw_rect_outline`, `draw_circle`, `fill_triangle`, alpha, styled `draw_text`
with letter spacing), `text_width`-exact right/centre alignment, and the
`petal-ui` prelude (`rect`, `hovered`, `clicked`, `draw_text_right`,
`draw_text_center`, `fit_parts`, `theme`-independent palette). Input covers
press/move/release drag tracking (`mouse_down`, `mouse_pressed`), the wheel
(`scroll_y`), and `key_pressed` / `mod_shift`.

**Performance shape** — developing 4 320 samples costs ~0.4 s in the VM, so the
app is built around three caches: per-photo negatives (revisiting a photo in the
library is instant), a develop cache keyed by a signature string, and a
quarter-resolution draft that is what a live drag actually recomputes. The
`DRAFT 1:2` badge appears whenever you are looking at the cheap pass.

## Notes

* The panel sleeps 10 s after the last input, as every Garden panel does.
  Nothing here animates, so a sleeping panel looks identical to an awake one.
* Editing the scene functions and hot-reloading will **not** change a photo you
  have already viewed: its negative is cached in `state`. Restart Garden (or
  switch `GRID_W`/`GRID_H`) to re-scan.

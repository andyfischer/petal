# 48 — Fractal explorer

An escape-time renderer you can fly around in: the Mandelbrot set, the Julia
set of any point in it, the Burning Ship and the Tricorn, with smooth
colouring, five palettes, wheel zoom about the cursor, drag and keyboard pan,
and a locator map. It is one Petal script (`app.ptl`), drawn by Garden's panel
runtime. There is no shader and no image: every pixel on the canvas is a
`draw_rect` or a `draw_rect_gradient` the script issued this frame.

A script cannot compute a full-resolution fractal inside one frame, and a
panel has no canvas that survives the frame, so the app is mostly the
strategy for both:

- **Sample tiles.** The plane is cut into 64 px tiles on a lattice anchored
  at the origin. A tile holds 32 x 32 smooth iteration counts and is refined
  in four interlaced passes (16, 8, 4 and 2 px cells), each reusing the
  samples of the pass before. Each frame spends a budget of iterations on the
  coarsest unfinished tile nearest the pointer, so the picture arrives as a
  sketch and sharpens from where you are looking. A pan only moves the
  lattice under the view: the tiles already there are kept, and only the ones
  that scroll in are new work.
- **Fitted draw lists.** One draw call per sample would be about 170,000
  calls a frame. Each finished tile is instead fitted, row by row, with
  piecewise-linear segments of the palette phase: a run of the set's interior
  is one rect, a smooth stretch of the exterior is one gradient rect, and a
  row that repeats the row above makes that row taller. The home view of the
  Mandelbrot set is about 10,600 calls for 196,608 samples.
- **Phase, not colour.** The segments store a palette phase. Changing the
  palette, the band density or cycling the colours touches no sample and
  refits nothing.
- **Zoom preview.** A zoom keeps the old image on screen, scaled about the
  cursor, and each new tile replaces its part of it only once it is at least
  as sharp.
- **A draw-call budget.** Where the picture is all filaments nothing merges.
  Before the 2 px pass the app estimates the cost and holds the busiest tiles
  at 4 px (the status pill says how many), which also skips the most
  expensive iterations in the view.

## Run it

```bash
tools/run-example.ts fractal-explorer                              # windowed
tools/run-example.ts fractal-explorer --headless --debug-port 0    # headless
```

By hand, from this directory:

```bash
GARDEN_HEADLESS_SIZE=1320x900 ../../../garden/target/debug/garden \
  --init layout.ptl --headless --debug-port 0
```

Designed for a 1320x900 viewport (`layout.ptl`'s `headless-size`), which gives
a 1308x828 pane and a 980x696 canvas of 16 x 11 tiles. Other sizes work: the
tile store is sized from the pane and rebuilt when it changes.

The app keeps nothing between runs, so no `GARDEN_PANEL_STORE_DIR` is needed.

A panel sleeps 10 s after its last input. The app asks for frames only while
it is refining, cycling colours or gliding on a held arrow key, so a finished
picture is a still panel, not a hang.

## Controls

### Mouse, on the canvas

| Input | Does |
|---|---|
| Wheel | Zoom about the pointer, 1.22x per line |
| Drag | Pan |
| Double click | Zoom in 2x at the pointer (with Shift: out) |
| Right click | Mandelbrot: open the Julia set of the point under the pointer. Julia: go back |
| Hover | The pointer's coordinates and the cached sample under it |

### Keys

| Key | Does |
|---|---|
| `1` `2` `3` `4` | Mandelbrot, Julia, Burning Ship, Tricorn (each remembers its view) |
| Arrows | Pan: a tap nudges 48 px, a held key glides. Shift: three to four times as far |
| `=` / `+`, `-` | Zoom in / out 1.6x about the centre |
| `R` or `Home` | Reset the view of the current fractal |
| `[` `]` | Fewer / more iterations (turns Auto off) |
| `A` | Auto iterations: follow the zoom depth |
| `P` (Shift+`P`) | Next (previous) palette |
| `,` `.` | Fewer / more colour bands |
| `C` | Cycle the palette |
| `D` | Detail: 2 px, 4 px, 8 px |
| `T` | Tile overlay: every tile outlined and tinted by its level, the one being computed boxed in white |
| `J` | Same as right click, at the pointer (or the centre) |
| `Tab` (Shift+`Tab`) | Next (previous) place |

### Sidebar

| Control | Does |
|---|---|
| Fractal chips | Switch fractal |
| Locator map | The whole fractal with the viewport marked. Click or drag to move the view |
| Seed map (Julia) | The Mandelbrot set with the seed marked. Drag to morph the Julia set live |
| Iterations `-` `+`, Auto | As `[` `]` and `A` |
| Palette swatches | Pick a palette |
| Bands `-` `+`, Cycle | As `,` `.` and `C` |
| Detail chips, Tiles | As `D` and `T` |
| Places / Seeds | Jump to a named view, or load a named Julia seed |

## What it exercises

**Custom rendering.** The image is built only from `draw_rect` and
`draw_rect_gradient` in their flat forms, clipped to a rounded canvas with
`clip_push`. Gradient endpoints are looked up in a 5 x 256 colour table held
as three int lists. The locator maps are a second, smaller renderer (solid
runs of a 14-step ramp).

**Zoom and pan.** The view is a scale and an integer origin in world pixels,
so a pan is exact and a tile's screen position never rounds differently from
its neighbour's. Zoom keeps the point under the pointer fixed. Magnification
runs from 0.2x to about 1.8e12x, where the app stops and says that 64-bit
floats end there.

**Computation.** Three escape-time kernels that run four iterations per
bailout test (the smooth count `n + 1 - log2(ln|z|)` reads the same at any
step past the bailout). The main cardioid and the period-2 bulb are tested in
closed form. A tile that is interior at every 4 px sample skips its 2 px
pass. The work budget is `iter_rate * dt()`, tripled on frames with no input.

**Language.** `f64_array` storage written in place through `state var` cells
from top-level functions (`set vals[i] = v`), about 1.6 million floats in 13
arrays; `config let` tuning knobs; a single-pass slope-cone line fit; integer
world coordinates up to 1e15 with checked overflow; records for small data
(places, palettes, work queue) and flat arrays for the hot data.

**Memoized scopes.** Each tile is drawn by one call of `draw_tile`, and the
preview by one call of `draw_preview`. On a frame where a tile's draw list
and position did not change, Petal replays the call instead of running it,
so moving the pointer over a finished 10,000-call image is a 4 ms frame, not
a 20 ms one. This was measured, not planned: with the same loop inline at
module scope every frame re-issued every call.

**Debug-server values.** Everything a test needs is mirrored into `obs_*`:
`obs_kind`, `obs_scale`, `obs_zoom`, `obs_origin`, `obs_center`,
`obs_cursor`, `obs_cursor_mu`, `obs_maxit`, `obs_auto_iter`, `obs_seed`,
`obs_palette`, `obs_bands`, `obs_cycling`, `obs_cycle_off`, `obs_detail`,
`obs_epoch`, `obs_tiles`, `obs_tiles_blank`, `obs_tiles_done`,
`obs_tiles_capped`, `obs_tiles_shown`, `obs_lod_hist` (tiles at level 0..4),
`obs_complete`, `obs_progress`, `obs_samples`, `obs_iters_frame`,
`obs_iters_total`, `obs_budget`, `obs_segs` (draw calls for the image),
`obs_preview`, `obs_pass`, `obs_place`, `obs_status`, `obs_zooms`,
`obs_pans`, `obs_dragging`, `obs_show_tiles`, `obs_canvas`, `obs_map`.

### Testing it

Refinement is driven by `dt()`, so under the debug server's virtual clock only
`POST /tick` computes anything: an injected click or key runs a frame with
`dt() == 0`, which changes the view and spends no iterations. The refinement
order starts at the pointer, so put the pointer somewhere fixed first.

```bash
cd examples/games/fractal-explorer
P=../../../tools/panel-test.sh
$P panel_start fractal-explorer
$P move 10 10; $P tick; $P panel_reset 42
$P tick 20 0.016; $P tick 20 0.016; $P tick 20 0.016
$P obs obs_complete,obs_segs,obs_lod_hist   # true, 10631, [0,0,0,0,192]
$P scroll 320 476 -3                        # zoom in 1.816x about (-1.3286, -0.1857)
$P obs obs_zoom,obs_preview,obs_tiles_shown # 1.816, 10631, 0: the old image, scaled
$P tick 20 0.016
$P obs obs_lod_hist                         # [0,0,0,95,109]: sharp tiles replacing it
$P panel_stop
```

Keep tick batches to about 20 frames. A refining frame costs 40 to 100 ms on
a debug build, and `POST /tick` gives up waiting after 5 s (the frames still
run).

## Known limits

- **It is slow by design of the host, and the numbers are for a debug
  Garden.** One escape iteration costs about 0.2 microseconds in the VM, so a
  frame affords 60,000 of them while you interact and 180,000 when you do
  not. Driven headless, the home view reaches 2 px in 35 ticked frames (about
  4 s of wall time) and Seahorse Valley (464 iterations) in 55 (about 6 s); a
  deep, busy view takes longer. I could not measure a release build or a
  windowed one.
- **No retained canvas.** `create_canvas` ids restart every frame, so the
  image cannot be rendered once and kept. Everything here (draw lists, the
  fit, the draw budget) exists to work around that. A canvas that survived
  the frame, or a `draw_image` from pixels a script computed, would replace
  most of it.
- **One call per primitive.** A draw call costs about 1.5 microseconds when it
  is not replayed from the memo table. A view with 25,000 calls is a 40 ms
  frame while it pans, refines or cycles colours.
- **Busy views are not uniformly 2 px.** Where neighbouring samples are
  unrelated (the Burning Ship's rigging, the dust around a spiral) the
  draw-call budget holds tiles at 4 px, and those tiles blend sample pairs
  into 8 x 4 px gradients. The status pill reports it ("DONE - 31 BUSY TILES
  AT 4 PX"); `draw_budget` is a `config let`.
- **Gradients are horizontal only.** A fitted segment is smooth along x and
  stepped along y, so a slow vertical ramp shows faint 2 px rows, and the
  16 px sketch pass looks streaky rather than blocky.
- **No anti-aliasing.** One sample per 2 px cell. The boundary of the set
  aliases, as it does in any single-sample renderer.
- **Zooming out shows a dark rim** for a few frames: the scaled preview only
  covers the middle, and the rim has to be computed.
- **A dragged Julia seed morphs in the 16 px sketch.** Each new seed waits
  for the previous one to finish a coarse pass, which is 2 to 3 frames.
- **Precision ends at 2e-15 per pixel.** There is no perturbation or
  arbitrary-precision arithmetic.
- **The interior shortcut is a heuristic.** A tile whose 4 px samples are all
  interior is filled without its 2 px pass. An exterior filament thinner than
  4 px crossing such a tile would be missed. I have not seen one.
- **`_native_rect_gradient` is not callable from a script** (`Unknown
  builtin`), so the flat draw forms always pay for the prelude wrapper.
- **A script has no clock other than `dt()`**, so the work budget cannot be
  set from what the last frame actually cost. It is a fixed rate, tuned by
  hand for a debug build.
- **Memoization is all or nothing per array.** `draw_tile` reads the shared
  segment arrays, so finishing any one tile invalidates the replay of every
  tile for that frame. While the image refines, every frame re-issues every
  call.
- **Nothing is saved.** The view, palette and seed reset when the process
  restarts.

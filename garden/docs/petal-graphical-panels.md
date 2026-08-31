# Petal graphical panels

A **panel** is a pane whose pixels are drawn by a Petal script, rather than by
the editor. Where an `editor(...)` pane shows a text buffer and a `process(...)`
pane mirrors a subprocess, a `panel("clock.ptl")` pane runs a Petal script every
frame and paints whatever it draws. This is the same imperative draw model that
`../integrations/petal-desktop-sdl` pioneered — the script calls
`draw_rect`/`draw_text`/… and the host turns those calls into pixels — embedded
**in-process** inside Garden.

This doc records the design, the chosen trade-offs, and what is and isn't built
yet. Read `docs/architecture.md` first for the crate contracts this builds on.

The features built on panels are the **`:Diff`** and **`:Git`** viewers. `:Diff`
draws an interactive master-detail view (file list + line diff) plus a `:Diff
--stat` per-file changed-lines diagram; `:Git` is a three-region history browser
(commit list, per-commit files, line diff) built on the petal-ui focus registry.
Both load their data on demand rather than baking it in — the drawer asks for the
diff/log through the async `query` native and inspects the pending value while it
loads (see "Interactivity" below).

These began as in-process built-in panels but are now delivered as **GPP
apps** (`gpp-apps/git-viewers`, bin `git-log`; the diff views live in
`gpp-apps/garden-diff`): the app pushes the Petal drawer — colocated in that crate
as `git-viewers/src/git_panel.ptl` / `garden-diff/src/garden_diff.ptl` — which the host still runs
in-process using the exact panel draw/input vocabulary this doc describes, and
answers the drawer's data `query`s over the pipe. So everything below about the
panel runtime applies unchanged; only *who supplies the data* moved from a local
Rust provider to a subprocess. See the `:Git`/`:Diff` sections of
`docs/architecture.md` and the how-to in `docs/writing-gpp-apps.md`.

## The model

```
panel("sketch.ptl")            LayoutNode::Panel { script }
  │  layout solve                       │
  ▼                                     ▼
PaneContent::Panel { script }  ──►  Pane { panel: Some(PanelView) }
                                         │  each awake frame
                                         ▼
              PanelHost::frame(dt, frame_count, input) -> Vec<PanelCmd>
                                         │  translate (offset to pane rect, u8→sRGB)
                                         ▼
                          garden_render::Primitive::{Quad, Text}
```

- **`garden-script` owns the runtime.** A `PanelHost` holds its own Petal `Env`,
  program, and stack — one VM per panel. The input/draw natives and the widget
  prelude come from the upstream **`petal-ui`** crate (the standard every Petal
  embedder shares): `PanelHost` registers `petal_ui::input::register_input` +
  `draw::register_draw` + `register_prelude` (the `ui` module as an implicit
  import), binds per-frame input/timing/dimensions with `petal-ui`'s
  `bind_*` helpers, runs the script, and drains
  `petal_ui::draw::take_draw_commands`, projecting each `DrawCommand` onto a
  `Vec<PanelCmd>`. Garden adds the
  `emit(event, arg)` push native (GPP script→client signals; see
  below), the `text_view` selectable-region pair (plus `text_view_scroll_to` to
  scroll one programmatically and `text_view_wrap` to soft-wrap it), the
  `panel_theme()` host-theme
  read (injected per frame via `PanelHost::set_theme`, like `bind_input`), and
  its monospace metric (`bind_text_metrics`, ratio 0.6), and does not register
  the optional offscreen-canvas natives. `PanelCmd`, `PanelInput`, and
  `PanelTheme` are plain data types (no `garden-render` dependency — the same
  cross-crate rule the `Theme` capture follows): the host speaks `u8` RGB and
  `i32`/`u32` panel-local pixels. See `../docs/embedding-guide.md`.
- **`garden-app` owns presentation and scheduling.** A `PanelView` wraps a
  `PanelHost` with the per-pane animation bookkeeping (frame counter, last-frame
  instant for `dt`, last-activity instant for sleep/wake, the last frame's cached
  commands, and any script error). `App::build_scene` translates the cached
  `PanelCmd`s into `Primitive`s offset into the pane's rect and clipped to it,
  converting `u8` RGB → sRGB `Color` at the boundary.

### Coordinates and color

A panel draws in **panel-local logical pixels**: `(0,0)` is the pane's top-left,
and `screen_width()`/`screen_height()` report the pane's current size (rebound
every frame, so a resize just changes the numbers). Colors are `0–255` integer
RGB (petal-sdl convention), converted to Garden's sRGB `Color` when translated
to primitives — and they stay sRGB all the way to the pixel (see "Alpha
composites in sRGB space" below).

The pane is **not** the window: it is inset by the tab strip, the status bar and
a small gutter (a `1440x900` window gives a single pane `x:6 y:38 w:1428
h:828.4`). A script never needs those numbers — `screen_width()` and
`screen_height()` are already the pane's — but a *harness* does, and the debug
server's `panes[0].rect` is where to read them rather than assuming the window
size. In particular `POST /mouse` takes **window** coordinates while
`mouse_x()`/`mouse_y()` report **pane-local** ones; the difference is that
origin.

### Alpha composites in sRGB space, the way CSS does

`a` means what a design tool says it means. The scene renders into a target
that holds sRGB-*encoded* bytes with no transfer function (`Rgba8Unorm`, not
`Rgba8UnormSrgb`), so `BlendState::ALPHA_BLENDING` mixes the gamma-encoded
values — the same arithmetic CSS, Core Graphics and Figma do. Black over white:

| `a` | nominal | rendered |
|---|---|---|
| 26 | 10% | `#e6e6e6` |
| 64 | 25% | `#bfbfbf` |
| 128 | 50% | `#808080` |

i.e. `255 - a` exactly, and a color picked in a design tool at 20% opacity
lands on the pixel the tool showed. Garden's own tests pin those three numbers
(`garden-render/tests/srgb_compositing.rs`).

Text is included: glyphon runs in its `ColorMode::Web`, so a translucent label
lands on the same gray as a translucent rect of the same color. It would not
if it were left on glyphon's default, and the mismatch is nearly invisible
per-pixel while being completely wrong in aggregate.

**This changed in 2026-08.** The renderer used to linearize every color on the
CPU and let an sRGB target re-encode on store, blending in linear light: 50%
black over white came out `#bcbdbd`, and `draw_rect(cell, #ffffff, 10)` was a
plainly visible grey block rather than the whisper it names. Panel authors read
`a: 10` as a subtle hover tint and went looking for a bug elsewhere. **Any
translucent color that was hand-tuned against the old behavior is now weaker
than it was** — roughly, an old `a` of *n* is a new `a` of somewhere near
`255 * ((n/255)^(1/2.2))`, but the honest advice is to re-pick the value
against the design it came from rather than convert it.

Overlapping alpha still is not idempotent — two 50% fills over one pixel read
75% — so for a tint that must survive being drawn twice, compute an opaque
color with the prelude's `mix` / `lerp_color`.

## Animation: sleep/wake heuristics

Garden renders only on dirty frames — there is no continuous loop. A panel that
animates needs ticking, but ticking forever would burn CPU on a window the user
isn't touching. The heuristic:

- **Any user input wakes panels.** A key, click, mouse move, or scroll stamps
  every panel's `last_activity = now`. Spawning, hot-reloading, and resizing a
  panel also stamp it (so a freshly appeared animation plays immediately).
- **A panel stays awake for `PANEL_WAKE` (10s) after the last activity.** While
  awake it ticks at ~60fps (`PANEL_FRAME` ≈ 16ms): each tick runs the script,
  caches the new commands, and requests a redraw. This is the "let the animation
  settle" window.
- **After 10s idle the panel sleeps**: no run, no redraw, until the next input
  wakes it. Its last frame stays on screen.
- **`--panel-wake` overrides the window** process-wide: a bare `--panel-wake`
  never sleeps, `--panel-wake 60` sets the window in seconds. A running game is
  exactly the case the sleep model gets wrong, and a headless harness driving
  one has no user input to keep re-stamping activity with. *Landed in 216ec76,
  2026-08-12; feature flag `cli.panel-wake` — an older binary answers `garden:
  unknown option --panel-wake`, so check `garden --version` first.*

For a test, prefer `POST /tick {"n": 60, "dt": 0.016}` (see
[debug-server.md](debug-server.md#stepping-frames-and-resetting-panels)): it runs
frames on demand with a `dt` you choose, ignoring the wake window entirely, so an
animation test is neither a sleep nor a stream of fake keypresses.

A trap for any script that wants to **poll on a timer**: a panel reads `time()`
only on a frame, and it only runs frames while awake, so a `time() >= next` check
does not fire on its own once the panel sleeps. A poll survives only if something
re-stamps activity within each `PANEL_WAKE` — a query's answer landing does, which
is why `garden-diff`'s staleness probe works at all. The consequence is that the
interval must stay meaningfully **under 10s**: at exactly 10s the poll and the
sleep race and the poll dies (measured — two probes, then nothing until a
keypress). `garden-diff` uses 9s. A genuinely slow poll needs a host-side timer,
not a bigger constant.

`App::tick_panels(now)` runs every awake panel and returns whether *any* panel is
still awake. The windowed frontend uses that to pick its control flow:
`WaitUntil(now + PANEL_FRAME)` while animating, falling back to the normal
`RELOAD_POLL` (~200ms) cadence when everything is asleep. The terminal and
headless frontends tick panels on their existing poll loop — panels there animate
at the slow poll rate (acceptable degradation; these targets aren't the point).

### The headless contract

Headless is where panels get driven by a harness, and it is **not** a 60fps loop.
What a script actually gets there:

- **Roughly one frame per injected event** (measured: about two panel frames per
  `POST /key`), one per ~200ms idle poll while awake, and one settle before every
  `/screenshot` or `/scene`. Nothing else makes a frame happen.
- **`dt()` is wall-clock, so it is large and spiky**: ~0.1–0.2s on an idle poll,
  and the frame after a pause carries the whole pause. It is not 0.016.
- **The 10s sleep still applies**, so a simulation with no input simply stops.

A panel that integrates anything physical must therefore **clamp and sub-step its
own `dt`** — `let step = min(dt(), 0.05)`, then integrate in fixed slices —
rather than trusting the delta it is handed. A 0.2s step tunnels a ball through
a paddle.

Drive frames explicitly instead of faking input: `POST /tick {"n": 60, "dt":
0.016}` gives 60 frames of exactly 16ms each, and `garden --panel-wake` keeps a
long-running panel from sleeping at all.

### `state` survives a hot reload

Editing a panel's script reloads it in place, and Petal `state` is **carried
across the reload** — that is deliberate (a reload should not lose your selection
or scroll position), and it has one consequence worth stating outright, because
it is the single most common "my edit did nothing" report:

**A `state` value is not recomputed when you change the code that computes it.**
Edit a seed-data generator, or a function whose result you cached in `state`, and
the panel keeps showing the old value. Nothing is broken; the initializer simply
does not run again.

`POST /panel/reset` rebuilds every file-backed panel from source and drops
`state`, which is the fix — restarting the process is not necessary.

### Debug visibility

Because "is it sleeping?" is otherwise invisible, every panel draws a tiny dot in
its top-right corner — filled when awake, hollow/dim when asleep — and the debug
server's `/state` reports each panel pane's `awake` flag and `frame` count
alongside its script path.

## Supported draw surface

The full `petal-ui` draw vocabulary is wired, **including the optional
per-primitive `a` (alpha), corner `radius`, and stroke `width` fields** — a
Garden panel renders the same commands as any other petal-ui host (translucent
fills, rounded rects, thick strokes), not a truncated set. Rectangles and text
map onto Garden's `Quad`/`Text` primitives; the rest tessellate (on the CPU)
into the `Mesh` triangle-list primitive added to `garden-render` for exactly
this. Colors carry alpha through `Color::rgba`, and `garden-render`'s pipelines
alpha-blend, so overlapping translucent tints composite:

| Petal fn | becomes |
|---|---|
| `clear(r,g,b)` | a full-pane filled rect (in the panel mesh) |
| `draw_rect(x,y,w,h,r,g,b[,a])` | one filled rect (2 triangles), optionally translucent |
| `draw_rect_rounded(x,y,w,h,radius,r,g,b[,a])` | a filled rect with quarter-circle-fan corners |
| `draw_rect_outline(x,y,w,h,r,g,b[,a[,width]])` | four `width`-px filled edges |
| `draw_rect_rounded_outline(x,y,w,h,radius,r,g,b[,a[,width]])` | a genuinely hollow rounded frame: four bands + a quarter *ring* per corner |
| `draw_line(x1,y1,x2,y2,r,g,b[,a[,width]])` | a `width`-px-wide quad along the segment |
| `draw_polyline(points,r,g,b[,a[,width]])` | one stroked path with round joins and caps — **non-overlapping**, see below |
| `draw_circle(cx,cy,radius,r,g,b[,a])` | a triangle fan (segments scale with radius) |
| `draw_circle_outline(cx,cy,radius,r,g,b[,a[,width]])` | a `width`-px ring (the `rx == ry` ellipse outline) |
| `draw_ellipse(cx,cy,rx,ry,r,g,b[,a])` | a triangle fan on both semi-axes |
| `draw_ellipse_outline(cx,cy,rx,ry,r,g,b[,a[,width]])` | a `width`-px elliptical ring |
| `fill_arc(cx,cy,r_in,r_out,a0,a1,r,g,b[,a])` | one annular sector — the donut/pie wedge (`r_in = 0` is a solid slice) |
| `fill_triangle(x1,y1,x2,y2,x3,y3,r,g,b[,a])` | one triangle |
| `fill_poly(points,r,g,b[,a])` | a triangle fan from the first point (convex) |
| `fill_polygon(points,r,g,b[,a])` | a **concave-correct** fill (ear clipping) |
| `fill_fan(cx,cy,points,r,g,b[,a])` | a triangle fan from an explicit center |
| `draw_text(s,x,y,size,r,g,b[,a])` | one `Text` run at `size` logical px (glyphon) |
| `draw_image(source,x,y,w,h[,a[,radius]])` | a cached PNG texture scaled into the destination rect |
| `draw_rect_gradient(x,y,w,h,r0,g0,b0,a0,r1,g1,b1,a1,angle)` | a rect filled with a linear gradient along `angle` |
| `draw_rect_gradient_rounded(x,y,w,h,radius,…,angle)` | the same, with rounded corners |
| `draw_circle_gradient(cx,cy,radius,r0,g0,b0,a0,r1,g1,b1,a1)` | a disc shading center → rim (glow, vignette) |
| `draw_shadow(x,y,w,h,radius,blur,spread,dx,dy,r,g,b[,a])` | a CSS box-shadow — **non-overlapping**, see below |
| `clip_push(x,y,w,h[,radius])` / `clip_pop()` | a clip that nests inside the enclosing one, and its restore |

`examples/panels/shapes.ptl` draws every one of them on one screen.

The prelude adds record-and-color-record overloads over all of these — plus
`linear_gradient(rect, stops, angle[, radius])`, which subdivides three or more
stops into one two-stop band per pair, and `draw_shadow(rect, {radius, blur,
spread, dx, dy, color, a})`. A gradient stop may carry its own `a` field; it is
the one primitive where alpha rides with the color, because a two-stop fade
needs two of them.

Four of these exist because the naive composition of the older calls is
**wrong**, not merely slower:

- **`draw_polyline` and translucency.** Alpha blending is not idempotent: two
  50%-alpha shapes over one pixel read 75%. A stroke drawn as N `draw_line`s
  double-blends its whole join area, and one drawn as a circle per mouse sample
  (what a paint app does without this) double-blends nearly everything — the
  reason a translucent brush comes out mottled. `draw_polyline` tessellates a
  stroke whose pieces do not overlap: the segment quads are trimmed to their
  crossing point on the inside of each turn, and only the *wedge* the turn
  opens up is added on the outside. A path that crosses its own stroke (a loop)
  still overlaps itself — nothing short of a stencil pass fixes that.
- **`fill_polygon` vs `fill_poly`.** `fill_poly` fans from the first vertex,
  which fills a convex outline and spills across the reflex corners of anything
  else. `fill_polygon` ear-clips, so a star is one call instead of ten
  `fill_triangle`s.
- **`draw_rect_rounded_outline`.** A rounded border drawn as a rounded fill
  with a smaller rounded fill on top is opaque (nothing behind shows through),
  costs two meshes, and degenerates at radius 1. This is one hollow frame.
- **`draw_shadow` and translucency, again.** A soft shadow hand-rolled as a
  stack of concentric translucent rounded rects double-composites every ring
  over the ones inside it, so the falloff is wrong and every seam shows — the
  same non-idempotence that makes `draw_polyline` necessary. `draw_shadow` is
  tessellated as **one** mesh: a solid core (the rect displaced by `dx`/`dy`
  and grown by `spread`) plus a falloff whose per-vertex alpha runs from 1 at
  the core boundary to 0 at `blur` px out. Every ring is the region *between*
  two expansions of the same rounded silhouette about fixed corner centers, so
  corresponding vertices pair up one-to-one and the rings tile the falloff with
  no gap and no overlap. It is not a blur pass, and it needs no render target.

  Garden's renderer samples that falloff at twelve rings with a **smoothstep**
  alpha ramp (`3u^2 - 2u^3`) rather than a straight line: a real box-shadow is a
  Gaussian blur of the silhouette and its profile flattens at both ends, where
  a linear ramp leaves a visible crease against the solid core and another
  where it reaches zero. `petal_ui::tess::shadow_mesh` — what petal-sdl and the
  other hosts rasterize — still uses the single-ring linear ramp, so a shadow
  is slightly harder there than in Garden. Same geometry, same extent; only the
  curve differs.

**Gradients** go through the same mesh pipeline as everything else, with
per-vertex colour. That is not an optimization detail but the reason they are
exact: the rasterizer interpolates vertex colour affinely, and a two-stop
linear gradient *is* an affine function of position, so sampling it at the
corners of one rounded rect's worth of triangles reproduces it everywhere.
There are no bands to see and no stacked translucent layers to double-blend.
`linear_gradient`'s three-or-more-stop form subdivides into adjacent two-stop
bands, each one exact within itself.

plus the input/timing reads `dt`, `time`, `frame_count`, `screen_width`,
`screen_height`, `mouse_x`, `mouse_y`, `mouse_down`, `mouse_pressed`,
`mouse_released`, `click_count`, `drag_active`, `key_down`, `key_pressed`,
`key_released`, **`mod_shift`, `mod_ctrl`, `mod_alt`, `mod_cmd`**, `scroll_x`,
`scroll_y`, `text_input`, `text_width`, and the `clip`/`clip_none`
calls documented under "Interactivity" below. All of these —
and the `ui` prelude widgets (`rect`/`point_in`/`hovered`, the record `draw_*`
overloads, `button`, `list_update`, `scroll_update`, `truncate_tail`, `wrap`, `preview`,
`fit_parts`, `ensure_visible_px`,
`draw_text_right`, the `context_menu` family — `menu_state`, `menu_item`,
`menu_sep`, `menu_open_on_right_click`, `menu_blocking`, `menu_show`,
`menu_close`, `menu_rect` — and the level-3 component set: RectCut layout
(`cut_left/right/top/bottom`, `split_h/v`, `pad`, `hstack/vstack`, `row/col`),
`checkbox`, `toggle`, `radio_group`, `slider`, `tab_bar`, `splitter`, `table`,
`modal`, `tooltip`, `spinner`, `progress_bar`, `badge`/`pill`, `card`,
`empty_state`, `hint_bar`, `wrap_px` and the `load_state` family) — come from
`petal-ui` as an implicit import, so scripts call them bare. The component
reference is [`petal-ui/docs/components.md`](../../petal-ui/docs/components.md);
the showcase panel is `examples/panels/gallery.ptl`.

**The whole Petal builtin table is available too.** A panel is an ordinary Petal
program with extra natives registered, not a sandboxed subset: everything in
[`../../docs/Builtins.md`](../../docs/Builtins.md) is callable. Worth naming,
because panel authors have gone hunting for them or hand-rolled them —
`random(min, max)`, `random_int(lo, hi)` and `choose(list)` for seeded data;
`clamp` / `min` / `max` / `round(x, places)`, all int-preserving, so a computed
index or pixel offset stays an int; `parse_int` / `parse_float`, which answer
`nil` on bad input instead of aborting the frame; `chars` / `char_len` /
`char_at` / `char_slice` / `index_of` for text that may not be ASCII (`len` and
`slice` are byte-indexed); and `json_parse` / `json_stringify`, which is what
pairs with `panel_store_*` below. The only things a panel does *not* get are the
host-specific natives another embedder registers.

Three things a panel used to have to reimplement are also in the prelude now:

- **Theming.** `ui_theme()` is the live palette every widget paints with. In
  Garden it **defaults to the host palette automatically**: the panel host
  binds the resolved `palette()` each frame (petal-ui's `bind_host_palette`),
  so prelude widgets paint in Garden's colors with no script-side setup —
  `theme_set(theme_from_palette(palette()))` is no longer needed (it still
  works). `theme_set({panel: …, text: …})` merges explicit colors over that
  (leave a key out and it keeps its value) and outranks the host palette until
  `theme_reset()`. The theme also carries spacing/radius/type scales
  (`space`, `radius`, `font_md`, …). Every widget takes an optional trailing
  `style` record that overrides the theme for that one call, and a style may
  name only the keys it cares about. A light-themed panel is a `theme_set`
  call, not a reimplemented widget set.
- **Drag and drop.** `drag_state()` plus one `drag_update(ds, id, rect)` per
  draggable item per frame yields `{dragging, id, dx, dy, dropped}`;
  `insertion_index(rects, y)` (and `insertion_index_x`) turns the drop position
  into the list index an insert wants.
- **Text-field internals.** `text_field_update(fc, id, r, buf)` is the input
  half — click-to-focus, typing, backspace, Return — with no pixels of its own,
  and `draw_text_field(r, text, has_focus[, style])` is the stock look. Keep
  the logic, bring your own paint.

Plus the small helpers apps kept hand-rolling: `mix`/`lerp_color(a, b, t)`,
`luma(c)` and `contrast_text(bg)`.

> **Landed in 017c328, 2026-08-12** (petal-ui prelude level 2):
> `text_field_update/4`, the 4-argument `draw_text_field(r, text, has, style)`,
> `luma/1` and `contrast_text/1`. On an older binary these fail at *runtime*
> with `Unknown builtin: contrast_text`, which says nothing about why — so
> check before you write against them: `garden --version` prints the prelude
> level and `garden --version --json` lists every export as `name/arity`
> (`prelude.exports`), derived from the prelude actually compiled into that
> binary. `GET /version` reports the same to a running app.

> **Landed 2026-08-26** (petal-ui prelude level 3): the component-library
> expansion — host-palette theme resolution, semantic tokens + scales, RectCut
> layout, motion helpers, caret editing in `text_field`, and the widget set
> (`checkbox`, `toggle`, `radio_group`, `slider`, `tab_bar`, `splitter`,
> `table`, `modal`, `tooltip`, `spinner`, `progress_bar`, `badge`/`pill`,
> `card`, `empty_state`, `hint_bar`, `wrap_px`, `load_state` family). All
> additive; same runtime-failure caveat on older binaries. Reference:
> `petal-ui/docs/components.md`.

A **context menu** is two calls, because an immediate-mode pass has to reconcile
z-order with input order: the menu must be *drawn* last to sit on top, but its
click must be claimed *before* the widgets underneath react to the same press.
So the rest of the panel stands down for the frame while one is open, and the
menu resolves the click at the end.

The two halves are not "top-ish" and "bottom-ish" — they are different kinds of
call and each has a hard position:

- **`menu_blocking(m)` is an input guard.** It goes above the panel's own click
  handling, so nothing underneath reacts to the press the open menu owns.
- **`context_menu(m, items)` is a DRAW call.** It must come after the panel's
  *last* drawing call — the bottom of the frame, not the bottom of the input
  section. Since [draw order is call order](#draw-order), calling it early paints
  the menu and then paints the panel's own background straight over it: the menu
  vanishes entirely, with no error to explain why. (This has already cost an
  author a session.)

```petal
state menu = menu_state()

// input, near the top — before the panel's own click handling:
if !menu_blocking(menu) && mouse_pressed(0) && point_in(mouse_x(), mouse_y(), row) then … end
menu = menu_open_on_right_click(menu, row, i)   // right-click arms it, tagged with `i`

…all of the panel's drawing…

// the LAST draw call in the frame:
let picked = context_menu(menu, [menu_item("Only this"), menu_sep(), menu_item("All", enabled)])
menu = picked.menu
if picked.index >= 0 then … picked.label, picked.tag … end
```

`context_menu` also returns the menu's landing rect (what `menu_rect(m, items)`
computes), so a panel that needs the box does not re-derive it from the metric
constants.

`picked.index` counts **every** entry, separators included, so a `menu_sep()`
shifts the indices after it. Escape and a click outside dismiss without
choosing; Up/Down move the highlight past separators and disabled rows and
Return takes it; a menu that would run off the screen flips to the other side of
the pointer. `garden_diff.ptl`'s commit rows are the worked example. The `:Git`/`:Diff` viewers' drawers are written on this prelude.

Relative image sources resolve from Garden's working directory. RGB and RGBA
PNG files are supported; a missing or invalid bitmap is logged and skipped
without aborting the panel frame. Image commands honor the active panel clip.

### Draw order

**Calls composite in the order you make them** — a `draw_rect` after a
`draw_text` covers that text, and a `draw_text` after a `draw_rect` sits on top
of it. Order holds between every kind of call: shapes, text, and images alike.

This is what makes overlays work. A context menu, a modal, a tooltip, or a
dropdown is just "draw the surface, then draw its contents, after everything
it should hide" — no need to know what is underneath or to suppress it by hand.

Garden batches consecutive shape calls into one mesh to keep the draw-call
count down, and flushes that batch whenever a text or image call interrupts it,
so the batching is invisible to the script.

(Before this was fixed, the renderer drew all shapes and then all text, so text
composited above every shape regardless of call order — a menu could paint its
background over a list but the list's text showed straight through it. Panels
written against that behavior may still carry manual text-suppression
workarounds; they are no longer needed.)

### Clipping

`clip(x, y, w, h)` narrows every following call to that rect until `clip_none()`
restores the pane; the clip is itself intersected with the pane, so a script can
never paint outside its own pane.

`clip` **replaces** whatever clip is active — which is fine at the top level and
wrong inside anything reusable, since a widget that clips its own contents would
throw away the clip its caller set. `clip_push(x, y, w, h)` instead *intersects*
with the clip in force, and `clip_pop()` restores it, so clips nest:

```
clip_push(list_rect)          -- the viewport
for row in rows do
  clip_push(row.badge_rect)   -- intersected with the viewport
  ...
  clip_pop()                  -- back to the viewport
end
clip_pop()                    -- back to the pane
```

The pane rect is the base of the stack and is never popped, so an unmatched
`clip_pop()` means "back to the pane" and still cannot paint outside it; a clip
left pushed when the frame ends simply ends with it.

Both forms take an optional trailing `radius` to round the clip's corners.
**It applies to everything drawn under it** — fills, lines, images,
`draw_text`, and the text inside a `text_view` / `edit_view` region declared
while it is active (a region carries its own interior clip; the two are
intersected).

The rounding, though, reaches **shapes only**. A GPU scissor is four integer
edges and cannot express a corner, so a rounded clip is carried into the mesh
fragment shader as a rounded-rect SDF and feathered across one physical pixel —
which is what makes a circular crop come out with a clean antialiased edge
rather than a staircase. Text (glyphon, whose `TextBounds` is a rectangle) and
images (`Primitive::Image`, which has no mask field yet) are still cut to the
clip's **bounding rect**. That is a degradation, never a dropped clip: nothing
escapes the rect either way. Only one rounded mask is in force at a time — the
innermost — and nesting a *square* `clip_push` inside a rounded one keeps the
rounding, since un-rounding the card would be the surprising reading.

Text is cut, not dropped: a run that straddles the clip's bottom edge renders
its top half, which is exactly what a scrolling list wants at its viewport
boundary. A drawer therefore does **not** need to cull the half-visible row
itself — skipping rows that are entirely outside the viewport is still worth it
as cheap work avoidance, nothing more. (The terminal frontend cannot draw half a
character cell, so there a row is kept when its cell center is inside the clip.)

Whether a run survives its clip is observable: each `/scene` primitive carries a
`visible` flag (see `docs/debug-server.md`), so a headless test can tell a drawn
row from a clipped-away one.

*(Text clipping landed 2026-08-15; feature flag `panel.text-clip`. Before it, a
`clip(...)` narrowed fills but text drew straight through, which is why older
drawers cull their own straddling rows — that workaround is no longer needed.)*

### Text size and measurement

`draw_text`'s `size` is honored per run — a panel is not locked to the editor's
14 px, so a script can build a real typographic hierarchy (a 28 px heading over
a 10 px caption). Line height follows the size at the renderer's usual 1.4×
ratio.

`text_width(s, size)` measures with the **real advances of the font Garden
rasterizes with** (JetBrains Mono, measured through cosmic-text on the CPU), not
the generic 0.6-of-size guess. So a rule drawn `text_width(s, size)` wide ends
flush with its text at every size — centering and right-alignment are exact.

The measurement is published **per host**: `PanelHost::set_font_advance_ratios`
binds the advance table into that host's env, and `PanelView` calls it on every
host it installs (construction, both rebuild paths, restart) — a rebuilt host
starts over with garden-script's 0.6 fallback, so each one must be told. The
measuring is memoized in `panel_view.rs` (the embedded font is fixed at compile
time, so shaping the ASCII range once is enough), but nothing about the
*setting* is process-wide: an embedder with a different face, or a test with a
made-up table, tells its own hosts and disturbs no one else's.

A face can be named — `text_width(s, size, "mono")` — and Garden resolves the
name against the fonts actually installed (see [Any font on the
machine](#any-font-on-the-machine)). A name it can't resolve degrades to the
monospace face, on both the measuring and the drawing side.

### Any font on the machine

`font(name)` returns the face as a value, so its size and decorations travel
with it:

```petal
let body = font("Helvetica Neue", 15)
let title = font_size(font_bold(body), 28)

draw_text("Chapter One", {x: 20, y: 40}, title)
let w = text_width("Chapter One", title)      // measures that exact cut
```

`name` is a family name, one of the two role names (`mono`, `ui`), or a
CSS-style fallback list (`"Menlo, mono"`). Garden resolves it against every
family the machine has installed, case-insensitively, and records the system's
own spelling — so `font("helvetica")` and `font("Helvetica")` are the same
object, and the name in the draw command is one the shaper will match. A family
this machine lacks keeps the name as written and falls back to the monospace
face for both measuring and drawing, so a panel written on one machine still
lays out sensibly on another.

A font object *is* a style record (next section), so it goes anywhere one does
and merges the same way: `{...title, color: palette().accent}`. The decorations
— `font_size`, `font_weight`, `font_bold`, `font_italic`, `font_spacing`,
`font_color` — each return a new object rather than mutating the one they were
given.

`fonts()` lists the families available, for a picker.

Discovery and measurement are **lazy and process-wide**: the font directories
are not scanned until a panel first calls `font()` or `fonts()`, and a family's
advances are measured once and then reused by every panel and every frame.
Naming a face costs one measurement, not one per frame — but it is a real
measurement, so a panel that cycles through hundreds of families will feel it.

The two embedded roles are pinned to the cuts Garden ships (`mono` → JetBrains
Mono Regular, `ui` → Inter Regular/Bold) even when the machine has a family of
the same name installed. The editor's entire column arithmetic is derived from
the embedded advances, so `mono` has to mean the file in the binary and nothing
else.

### Styled text

`draw_text` also takes a **style record**, and `text_width` measures the same
record, so what you measure is what you draw:

```petal
let BODY = {size: 15, color: palette().text}
draw_text("Merge pull request ", {x: 10, y: 20}, BODY)
draw_text("#482", {x: 170, y: 20}, {...BODY, weight: 700})
draw_text("s p a c e d", {x: 10, y: 40}, {...BODY, spacing: 2})
```

Fields are `size`, `color`, `font`, `weight`, `italic`, `spacing`; any subset,
with omitted ones meaning plain text. What Garden does with each:

| Axis | In a Garden pane |
|---|---|
| `size` | honored per run |
| `font` | any family installed on the machine, plus the two embedded faces as `mono` (JetBrains Mono) and `ui` (proportional Inter); CSS-style fallback lists work, and an unresolvable name degrades to JetBrains Mono. The measurement side resolves names the same way, so what you measure is what you get. `font(name)` is the same thing as a value — see [Any font on the machine](#any-font-on-the-machine) |
| `italic` | passed to cosmic-text; resolves when a matching face is available (no italic cut is embedded, so this is currently upright) |
| `weight` | **real** on a system family (its own cuts, its own advances) and on `font: "ui"` (Inter Bold is embedded, so `weight >= 600` shapes the Bold cut). **Synthetic** on the monospace face, where only Regular is embedded: a heavy run is drawn twice at a size-proportional sub-pixel offset, which thickens without changing advances, so it measures regular and layout stays correct |
| `spacing` | honored — the host places each glyph, with the pen matching `text_width` exactly |

Embedding a mono Bold face would light `weight` up there too, with no protocol
change — or a panel can just name a system monospace family that has one.

### Measuring a named face

`text_width(s, size, "ui")` sums a **separate advance table**, because Inter is
proportional — `i` and `W` are nowhere near the same width, so the single ratio
that fully describes a monospace face does not describe this one. The same goes
for every system family. Garden publishes the two embedded tables up front
(`PanelHost::set_font_advance_ratios_with_ui`) and measures any other family on
demand (`PanelHost::set_font_source`), so centering and right-alignment are
exact in every face.

Measure in the face you draw in. Measuring a `font: "ui"` run without the third
argument sums monospace advances, and the result lands visibly wrong while
nothing about the drawing looks broken. Passing a font object to both
`draw_text` and `text_width` is the way not to have this problem.

```petal
let TITLE = {size: 22, weight: 700, font: "ui", color: palette().text}
let w = text_width("Pick your handle", 22, "ui")   // not text_width(s, 22)
draw_text("Pick your handle", {x: (screen_width() - w) / 2, y: 40}, TITLE)
```
The full cross-host contract lives in `../docs/text-and-fonts.md`.

### Rotated text is not supported

There is no `rotate(angle)` on a text draw, and adding one is not a small
change. Every text run becomes a glyphon `TextArea`, whose entire placement
API is `left` / `top` / `scale` — glyphon emits axis-aligned quads from its own
atlas through its own pipeline, and nothing in that path takes a transform.
Rotated labels would mean one of: forking glyphon's renderer, rasterizing each
run to an offscreen texture and blitting it rotated (needs per-run render
targets), or rasterizing glyphs with swash into a Garden-owned atlas and
drawing them through a new pipeline. All three are a new text path, not a
parameter.

So a 2-D editor in a panel can rotate its shapes but not its labels. The
workarounds that do exist: draw the label upright beside the rotated shape, or
lay out per-character positions along a path (each glyph is still upright).

## Persistence: `panel_store_get` / `panel_store_set`

A panel's `state` lives and dies with the process. There is deliberately no
file API in the panel vocabulary — a sketch that can open any path is a
different security story — so a panel that must remember something across a
restart uses the store:

```petal
state todos = json_parse(panel_store_get("todos") ?? "[]")
# …after an edit…
panel_store_set("todos", json_stringify(todos))
```

- **String → string, scoped to the script's own path.** Two panels running
  different scripts cannot see each other's keys, and no script names a file.
  `panel_store_set(key, nil)` deletes a key.
- **One JSON file per script**, under `~/.garden/panel-store/`, named after the
  script's absolute path (slugified, plus a hash so two same-named scripts in
  different directories don't collide). `GARDEN_PANEL_STORE_DIR` overrides the
  directory — how a test gets a scratch store.
- **Written after any frame that changed it**, atomically (temp file +
  rename), so a crash mid-write leaves the previous contents intact. A frame
  that changes nothing writes nothing.
- **Capped** at 256 KiB per value and 1024 keys: over either, the `set` call
  errors instead of growing a file without bound. It is a place to keep the
  kilobyte of state a panel owns, not a database.
- A write failure (read-only home, full disk) reaches the script's `print`
  output rather than failing the frame — the panel keeps drawing.

## Asking the host to act: `mutate`

`mutate(name, arg)` is the panel vocabulary's one effectful call. In a
GPP pane it is a request to the pane's **subprocess**
(`on_mutation`, see `gpp.md`); the reply becomes the status note, an error the
status error.

An **in-process** `panel(...)` pane has no subprocess — and `emit(...)`, which
only writes to a client's pipe, is silently dropped for it — so `mutate` is
also how such a panel asks *Garden itself* to do something. A short list of
names is therefore answered by the host before any forwarding happens:

| `mutate(…)` | effect |
|---|---|
| `mutate("open_path", { path: "…" })` | open that file in the focused pane, as `:e` does |
| `mutate("open_project", { path: "…" })` | record the directory as a project and browse it, as File ▸ Open Folder does |
| `mutate("open_pr", { number: 42 })` | open the PR review (`:PR 42`) |
| `mutate("open_file_dialog", { mode: "file" })` | pop the native picker, then open what was chosen (`mode: "folder"` picks a directory and opens it as a project) |

These exist for host screens — a start screen listing recent files, say — that
must drive the editor with no client behind them. Every other name still goes
to the subprocess, so a GPP app's own mutations (`"apply"`, `"save"`) are
unaffected; sending one of these names from a *client-backed* panel reaches the
host, not the client.

A malformed argument (no `path`, a non-numeric `number`) is a status error, and
`open_file_dialog` is refused with one under `--term` / `--headless`, where a
native modal has no window to be answered from.

### Reading the outcome: the handle `mutate` returns

`mutate(...)` returns an integer **handle**, and `mutate_result(handle)` reads
back what that request resolved to — nil while it is still in flight, otherwise
a record:

```petal
{ ok: true,  value: "wrote 2 files", error: nil }
{ ok: false, value: nil,             error: "panel has no subprocess to handle 'apply'" }
```

The round trip must not block a redraw, so the frame that *makes* the request
can never see its own answer. Keep the handle in `state` and read it later:

```petal
state saving = 0
if key_down("ctrl") && key_pressed("s") then
  saving = mutate("save", { text: edit_view_text(1) })
end
let saved = mutate_result(saving)
let save_failed = saved?.ok == false
```

Both `saving` and `saved` are ordinary `let`/`state` bindings, so they show up in
`panes[].panel.values` — which is how a headless test asserts that a save
actually happened rather than that a key was pressed. Every outcome is reported,
including the host-answered names above (`ok: true`, no `value`) and requests
that could not be delivered at all. Ignoring the return value keeps the old
fire-and-forget behavior exactly.

*(landed 2026-08-15; feature flag `panel.mutate-handle`.)*

## Navigating: `navigate(screen)` / `navigate(screen, arg)`

`navigate(screen)` pushes a new entry onto the pane's browser-style history and
`navigate_back()` / `navigate_forward()` walk it (as do the host's `Ctrl+[` /
`Ctrl+]`). `navigate_replace(screen)` swaps the current entry in place.

The two-argument form carries the **subject** the target screen is for — the row
that was clicked, the commit to show — and the target reads it back with
`nav_arg()`:

```petal
// list.ptl
if clicked then
  navigate("detail.ptl", { id: rows[sel].id })
end

// detail.ptl
let id = nav_arg()?.id ?? 0
```

`arg` is any JSON-representable value (a navigation may cross a subprocess
boundary, so it travels as JSON, like a `mutate` argument). `nav_arg()` is nil
for a screen reached without one — including the panel's origin screen, which
nothing navigated to — so the idiomatic read pairs it with a `??` fallback.

The argument is stored **on the history entry**, not on the pane, so it comes
back with the screen: *back* onto a detail screen shows the subject that visit
was opened with, not the most recent one, and *forward* walks the arguments
again in order. Without that, returning to a detail screen would redraw it with
no subject at all.

*(landed 2026-08-15; feature flag `panel.nav-arg`.)*

For a **subprocess** panel there is a second half to this. The entry carries the
argument, but the app on the other end of the pipe holds the data the screen
draws, and it would otherwise never learn that the user came back — the screen
would return drawn from whatever the provider happens to hold now. So back and
forward **re-issue the restored entry's `navigate` mutation**, with that entry's
own screen and argument, before the screen is redrawn. An app's `navigate`
handler is therefore called once per *visit*, not once per screen, and should be
idempotent; see
[writing-gpp-apps.md](writing-gpp-apps.md#multi-screen-navigation-optional).

It is best effort: the cursor moves first, so a provider that is gone, slow, or
refuses the screen leaves the entry on its cached source with the reason in the
status note — going back never fails because the app did.

*(landed 2026-08-15; feature flag `panel.nav-replay`.)*


## Input: what a focused panel receives

A focused panel pane reads input through the standard `petal-ui` contract —
`mouse_x`/`mouse_y`, `mouse_down`/`mouse_pressed`/`mouse_released`,
`key_down`/`key_pressed`/`key_released`, `mod_shift`/`mod_ctrl`/`mod_alt`/`mod_cmd`,
`drag_active`, `click_count`, `text_input`, `scroll_x`/`scroll_y`. Garden feeds
that contract from the same paths every frontend and the debug server use, so
what a real window delivers and what `POST /key` / `POST /mouse` deliver are the
same thing.

Four rules are worth stating outright, because each was once false:

- **A chord carries no text.** `text_input()` is empty on any frame whose key was
  held with `Cmd`, `Ctrl`, or `Alt`. (Shift is not a command modifier — it is
  already in the character, so `Shift+a` still types `"A"`.) A panel can handle a
  chord *and* read `text_input()` without the chord typing itself into the
  document.
- **Every modifier arrives, on keys and on the mouse alike.** `mod_alt()` is real:
  an alt-drag ("scale about the center") or a cmd-click sees its modifier.
- **Modifiers are also held keys.** `key_down("shift")` / `"ctrl"` / `"alt"` /
  `"cmd"` answer truthfully, as well as `mod_shift()` and friends.
- **`click_count()` is the real chain**: 2 on a double click, 3 on a triple.

### `request_frame` — staying awake while animating

A panel sleeps ten seconds after its last activity. That is right for a still
drawer and wrong for ambient motion: a skeleton shimmer, a spinner, a pulsing
live dot or a marquee simply **freezes** ten seconds after the last input, and
the stale frame stays on screen reading as a hang.

```petal
request_frame()   // this frame is part of an animation; keep ticking
animating()       // the same native, under the name that reads better in a loop
```

Like `claim_key`, the call is **declarative and per frame**: it covers only the
frame that makes it, so a script can ask while its motion is running and stop
asking when it settles, and the panel then sleeps again on the usual schedule.
Costs one push into an output buffer, so calling it every frame is free.

```petal
let busy = loading()
if busy then
  request_frame()
end
draw_spinner(rect(20, 20, 24, 24), time())
```

The idle heuristic is "nothing has happened for a while, so nothing is
happening". A frame that is mid-animation is exactly the case it gets wrong,
and the script is the only thing that knows. The process-wide alternatives
remain for a harness driving someone else's script: `garden --panel-wake`
(never sleep), `--panel-wake 60` (a longer window), and `POST /tick`, which
runs frames regardless.

### The panel's default face

A text run that names no face is drawn — and measured by `text_width(s, size)`
— in the panel's default face, which the host publishes from its theme. Garden's
is the monospace role unless the embedder names another (`PanelTheme::set_font`
/ `PanelHost::set_default_font`); the two always move together, so a bare
`text_width` can never be answered from a different table than the one the run
is drawn with. A run that names its own face (`{font: "ui"}`, `font("Inter")`)
outranks the default, and petal-ui's widgets always pass an explicit style built
from `theme.font`.

### `claim_key` — a panel's own command keyspace

Garden owns the Cmd/Ctrl chords: they are the editor's shortcuts, and a panel
never sees them. For a panel whose *bare letters are content* — a spreadsheet, a
console, a text editor — that leaves nothing to bind a command to. `claim_key`
is how a script asks for one back:

```petal
claim_key("z", "cmd")          // Cmd+Z reaches this panel, not Garden's Undo
claim_key("s", "cmd+shift")    // one exact chord
claim_key("escape")            // this key under *any* modifier combination
```

A claim is **declarative and per frame**: state it unconditionally near the top
of the script, not inside a branch, and it applies to the keys that arrive
before the next frame. The chord is then delivered like any other key —
`key_pressed("z")` with `mod_cmd()` true, and no `text_input()`.

The modifier argument is a spelling (`"shift"`, `"ctrl"`/`"control"`,
`"alt"`/`"option"`, `"cmd"`/`"super"`/`"meta"`, combined with `+`) or the raw
bitmask `1=shift 2=ctrl 4=alt 8=cmd` — the same encoding `/state` reports as
`panel.input.modifiers`. An unknown spelling is an error rather than a claim
that silently never fires.

**`Cmd`/`Ctrl`+`Q` cannot be claimed.** Quitting is never something a script can
capture. Everything else — including the bare `:` command bar and the `Ctrl+[` /
`Ctrl+]` history chords — is claimable, so a panel that claims them takes on the
job of offering its own way out.

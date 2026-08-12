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

These began as in-process built-in panels but are now delivered as **panel-mode
GPP apps** (`gpp-apps/git-viewers`, bin `git-log`; the diff views live in
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
  `emit(event, arg)` push native (panel-mode GPP script→client signals; see
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
RGB (petal-sdl convention), converted to Garden's sRGB `Color` when translated to
primitives; the renderer then linearizes as usual (`Color::to_linear`).

The pane is **not** the window: it is inset by the tab strip, the status bar and
a small gutter (a `1440x900` window gives a single pane `x:6 y:38 w:1428
h:828.4`). A script never needs those numbers — `screen_width()` and
`screen_height()` are already the pane's — but a *harness* does, and the debug
server's `panes[0].rect` is where to read them rather than assuming the window
size. In particular `POST /mouse` takes **window** coordinates while
`mouse_x()`/`mouse_y()` report **pane-local** ones; the difference is that
origin.

### Alpha composites in linear space

Colors are converted sRGB → linear before the GPU blends them
(`BlendState::ALPHA_BLENDING` against an sRGB target), so a low `a` reads far
brighter than its nominal percentage. Measured on a `#12161e` ground with white:

| `a` | nominal | rendered |
|---|---|---|
| 5 | 2% | `#2c2e32` |
| 10 | 4% | `#3c3d40` |
| 20 | 8% | `#525355` |
| 51 | 20% | `#7d7d7e` |
| 128 | 50% | `#bcbdbd` |

`draw_rect(cell, #ffffff, 10)` is therefore a plainly visible grey block, not a
whisper — panel authors have repeatedly read `a: 10` as a subtle hover tint and
gone looking for a bug elsewhere. For a genuinely subtle tint, either use a
single-digit `a` or compute an opaque color with the prelude's `mix` /
`lerp_color`, which is also idempotent where shapes overlap (alpha is not: two
50% fills over one pixel read 75%).

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
  one has no user input to keep re-stamping activity with.

For a test, prefer `POST /tick {"n": 60, "dt": 0.016}` (see
[debug-server.md](debug-server.md#stepping-frames-and-resetting-panels)): it runs
frames on demand with a `dt` you choose, ignoring the wake window entirely, so an
animation test is neither a sleep nor a stream of fake keypresses.

A trap for any script that wants to **poll on a timer**: a panel reads `time()`
only on a frame, and it only runs frames while awake, so a `time() >= next` check
does not fire on its own once the panel sleeps. A poll survives only if something
re-stamps activity within each `PANEL_WAKE` — a query's `queryResult` does, which
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
| `draw_image(source,x,y,w,h[,a])` | a cached PNG texture scaled into the destination rect |

`examples/panels/shapes.ptl` draws every one of them on one screen.

Three of these exist because the naive composition of the older calls is
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

plus the input/timing reads `dt`, `time`, `frame_count`, `screen_width`,
`screen_height`, `mouse_x`, `mouse_y`, `mouse_down`, `mouse_pressed`,
`mouse_released`, `click_count`, `drag_active`, `key_down`, `key_pressed`,
`key_released`, **`mod_shift`, `mod_ctrl`, `mod_alt`, `mod_cmd`**, `scroll_x`,
`scroll_y`, `text_input`, `text_width`, and the `clip`/`clip_none`
calls documented under "Interactivity" below. All of these —
and the `ui` prelude widgets (`rect`/`point_in`/`hovered`, the record `draw_*`
overloads, `button`, `list_update`, `scroll_update`, `truncate_tail`, `wrap`, `preview`,
`fit_parts`, `ensure_visible_px`,
`draw_text_right`, and the `context_menu` family — `menu_state`, `menu_item`,
`menu_sep`, `menu_open_on_right_click`, `menu_blocking`, `menu_show`,
`menu_close`, `menu_rect`) — come from `petal-ui` as an implicit import, so
scripts call them bare.

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

- **Theming.** `ui_theme()` is the live palette every widget paints with;
  `theme_set({panel: …, text: …})` merges new colors into it (leave a key out
  and it keeps its current value) and `theme_reset()` restores the dark
  default. `theme_set(theme_from_palette(palette()))` adopts Garden's own
  scheme. Every widget — `button`, `context_menu`, `text_field`,
  `draw_scrollbar`, `section_label` — also takes an optional trailing `style`
  record that overrides the theme for that one call, and a style may name only
  the keys it cares about. A light-themed panel is a `theme_set` call, not a
  reimplemented widget set.
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

A face can be named — `text_width(s, size, "mono")` — for portability with
other petal-ui hosts; Garden has one embedded face, so both `mono` and `ui`
resolve to it and an unknown name degrades to it too.

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
| `font` | one embedded face — every role and family resolves to it, and the measurement side agrees |
| `italic` | passed to cosmic-text; resolves when a matching face is available |
| `weight` | **synthetic** — only JetBrains Mono Regular is embedded, so a run at `weight >= 600` is emboldened by drawing it twice at a size-proportional sub-pixel offset. Visibly heavier, but a thickening rather than a true Bold cut; advances are untouched, so it still *measures* regular and layout stays correct |
| `spacing` | honored — the host places each glyph, with the pen matching `text_width` exactly |

Embedding the Bold face would light `weight` up with no protocol change.
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

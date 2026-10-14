# The `ui` host contract

What a host has to do so that scripts written against `import ui` draw and
respond correctly. It covers the frame loop, the draw commands a host is
handed, the hit-testing registry, and the three things a host can choose to
do itself for speed: batches, retained layers and images.

The script side is in [ui-reference.md](ui-reference.md). The Rust API is the
`petal-ui` crate (`core-libs/petal-ui`); the C API is `petal_bridge.h`
(`integrations/petal-c-bridge`, guide in [embedding-c.md](usage/embedding-c.md)).

## Summary

A host built on the `petal-ui` crate, directly or through the C bridge, gets
every feature on this page **with no host code**. The crate lowers what a
host did not ask for:

| Script feature | What reaches a host by default | What a host can ask for instead |
|---|---|---|
| `ui_layer` / `with_layer` / `ui_overlay` | The frame's commands, already sorted by layer. No new command. | Nothing: it is always resolved by the drain. |
| `interact`, `ui_modal`, `pointer_captured`, `ui_target_rect` | Nothing: resolved between the natives and `bind_input`. | Reading the registry (to click a widget by id). |
| `draw_rects` | One `rect` per entry. | `rects`, one command for the batch. |
| `cached_layer` | The layer's content every frame, moved and clipped. | `cache_begin` / `cache_end` / `cache_draw`, and keeping a texture. |
| `draw_image` with `tint`, `src`, `slice` | An `image` command with three more optional fields. | Drawing them. A host that ignores them draws the whole image untinted. |
| `image_load`, `image_size` | Requests in an output buffer; sizes read from a binding. | Registering its own natives under those names. |
| Text `shadow` / `outline` | Extra `text` commands under the run. | Nothing. |
| `ui_measure` | Nothing: the measured span is dropped by the drain. | Nothing. |

## The frame

Unchanged, and repeated here because two steps now do more:

```
input events → InputState::event
InputState::begin_frame(dt)
bind_frame_info / bind_time / bind_dimensions
bind_input                      ← also rolls the hit-test registry
[ gate: skip the run if nothing it read has changed ]
clear_draw_commands, reset_canvas_ids
Env::reset_stack, Env::run
take_draw_commands              ← also sorts layers and lowers what the host did not ask for
rasterize
```

In C: `pb_vm_input_*`, `pb_vm_begin_frame`, `pb_vm_run`, `pb_vm_drain_draw`.

Two rules follow from the additions:

1. **Call `bind_input` exactly once per frame, before the gate.** It is where
   the rects the last run registered become the registry the next run is
   tested against. A host that binds input by hand instead calls
   `petal_ui::hit::roll(env, &input)` at the same point.
2. **Drain with `take_draw_commands`** (or `_traced`, `_for`). Decoding the
   `draw_commands` buffer yourself with `DrawCommand::from_value` skips the
   layer sort and the lowering, and you will meet `layer`, `mute`, `rects`
   and the `cache_*` commands raw.

## Draw commands

Coordinates are logical pixels, (0, 0) at the top-left. Colours are sRGB,
0–255. The Rust type is `petal_ui::draw::DrawCommand`; its JSON form
(`serde`, tagged by `op`) is what `petal-ui-run` prints; the C form is
`pb_draw_cmd`.

Only the commands that are new or extended are listed. The rest are
documented on the enum.

### `image` (extended)

| Field | Type | Meaning |
|---|---|---|
| `source` | string | A host-resolved asset path or name. |
| `x, y, w, h` | int | The destination rect. |
| `a` | 0–255 | Opacity. Absent from JSON when 255. |
| `radius` | int | Corner radius. Absent when 0. |
| `tint` | `[r, g, b]`, optional | Multiply the image's pixels by this colour. Absent means none. |
| `src` | `[x, y, w, h]`, optional | The part of the image to draw, in the image's own pixels. Absent means all of it. |
| `slice` | `[left, top, right, bottom]`, optional | Nine-slice insets in the image's own pixels (of `src`, when there is one). |

Nine-slice: the four corners are drawn at their own size, the four edges
stretch along one axis, and the middle stretches along both. If the
destination is smaller than the corners need, scale the corners down
uniformly.

In `pb_draw_cmd` (`PB_DRAW_IMAGE`): `text` = source; `x y w h`; `color.a` =
opacity; `color.rgb` = tint, with 255, 255, 255 meaning none; `x1 y1 x2 y2` =
the source rect's x, y, w, h, with `x2 == 0` meaning the whole image; `x3 y3
cx cy` = slice left, top, right, bottom, all 0 meaning none.

A plain `draw_image` call emits exactly the command it always did.

### `rects` (only with the batch capability)

`rects: [{x, y, w, h, r, g, b, a}, …]`. Fill each in order; later entries
draw over earlier ones.

In `pb_draw_cmd` (`PB_DRAW_RECTS`): `points` holds `point_count` rects, five
`int32` each: `x, y, w, h`, then the colour as
`(r << 24) | (g << 16) | (b << 8) | a`.

### `cache_begin`, `cache_end`, `cache_draw` (only with the retained-layer capability)

| Command | Fields | Meaning |
|---|---|---|
| `cache_begin` | `id, w, h` | Start recording retained layer `id`, `w`×`h` pixels. |
| `cache_end` | `id` | End of the recording. It replaces whatever `id` held. |
| `cache_draw` | `id, x, y` | Draw layer `id` with its top-left at (`x`, `y`). |

In `pb_draw_cmd`: `PB_DRAW_CACHE_BEGIN` (`id`, `w`, `h`), `PB_DRAW_CACHE_END`
(`id`), `PB_DRAW_CACHE_DRAW` (`id`, `x`, `y`).

The rules a host keeping layers follows:

- The commands between `cache_begin` and `cache_end` are the layer's
  **content**. They are in the layer's own coordinates, (0, 0) at its
  top-left, and are **not drawn where they stand**. Render them into a
  transparent `w`×`h` target and keep it under `id`.
- `cache_draw` composites that target at (`x`, `y`). Nothing outside the
  layer's `w`×`h` is drawn. The clip in force at the `cache_draw` applies.
- A `cache_draw` refers to the most recent recording of its `id`, which may
  be many frames old. Most frames carry only the `cache_draw`.
- Recordings **nest**: a `cache_begin` may appear inside another's content,
  always properly closed before the outer `cache_end`. A `cache_draw` inside
  content is drawn into the layer being recorded, at that layer's
  coordinates.
- Scripts are told not to put `clear`, the replace form of `clip`,
  `clip_none` or the offscreen-canvas commands in content, and a host need
  not support them there. Content may contain `clip_push` / `clip_pop`
  (balanced), which are relative to the layer.
- Ids are small positive integers, stable for a call site for as long as the
  script keeps drawing that layer.
- **Freeing.** A host may free a layer that a frame's command list neither
  recorded nor drew. The script lets go of it under the same condition and
  will record it again if it comes back. *Drew* includes drawn by the
  content of a layer the frame drew: a nested layer's `cache_draw` is inside
  its parent's recording, which the frame replays without re-running the
  parent's body, so nothing would record the nested one again.
- **Losing a layer.** If the host cannot draw a layer it is asked for (a
  device reset, a `cache_draw` for an id it does not hold), it draws nothing
  for that command and calls `petal_ui::compose::invalidate_caches(env)`
  (`pb_vm_invalidate_layers`). Every `cached_layer` records again on the next
  run.
- A frame the gate skipped has no new command list. Redraw the last one; the
  layers it refers to are still the ones the host holds.

### Commands a host never receives

`layer` and `mute` exist in the enum and in the raw buffer. The drain
consumes both.

- **`layer n`** marks "from here on, draw layer `n`" (a frame starts on 0).
  The drain sorts the frame by layer, stably, lower layers first, and removes
  the markers. A run of commands that moves takes its clip with it: where the
  sort puts a run after one that left a different clip in force, the drain
  inserts `clip_pop`s, then `clip` or `clip_none`, then `clip_push`es to
  re-establish the clip the run was drawn under. Markers inside an offscreen
  canvas or a retained layer's content are ignored.
- **`mute on`** … **`mute off`** bracket what a script drew only to measure.
  The drain drops the span.

### Text decorations

A text style's `shadow` and `outline` are lowered by the `draw_text` native
into extra `text` commands emitted before the run itself: one for the shadow,
eight for the outline (the neighbours at `width` pixels). A host draws them
as ordinary text. `tracking` is `spacing` under another name.

## Capabilities

```rust
use petal_ui::compose::{HostCaps, set_host_caps};
set_host_caps(&mut env, HostCaps { batch_rects: true, retained_layers: true });
```

```c
pb_vm_set_draw_caps(vm, PB_DRAW_CAP_BATCH_RECTS | PB_DRAW_CAP_RETAINED_LAYERS);
```

Call it once after creating the env. It is stored as the binding
`ui_host_caps` (an int of bits: 1 = batches, 2 = retained layers). With
neither bit set a host sees only the classic vocabulary.

How a retained layer is lowered when the host does not keep it: the drain
stores the content on the env (bindings named `ui_cache:<id>`), and replaces
each `cache_draw` with `clip_push(x, y, w, h)`, the content translated by
(`x`, `y`), and `clip_pop`. The script's saving is the same either way: the
body does not run. The host's saving needs the capability.

## Hit testing

Scripts ask `interact(rect)` and get an answer that respects layers, modal
dialogs and clips. The host's part is rule 1 above. This section is the
mechanism, for a host that implements the natives itself or wants to read the
registry.

**Registration.** Each `interact` (and each built-in widget) calls the native
`ui_register(rect, layer, clip, id?, flags?)`, which pushes one row into the
output buffer `ui_targets`:

```
[x, y, w, h, layer, flags, id]
```

`x, y, w, h` is the rect intersected with the clip that was in force, as
floats. `flags` is bits: 1 = blocks the pointer for lower layers, 2 = not a
rect but a modal declaration for `layer` (`w = h = 0`). `id` is a string, an
int, or `nil`.

Two other values appear in that buffer. `nil` is a marker the `ui` module
pushes once per run, from module scope, so that a run which registered
nothing is not mistaken for a frame that did not run. `true` … `false`
bracket rows registered while measuring, which are ignored.

**The roll** (`petal_ui::hit::roll`, called by `bind_input`):

1. Drain `ui_targets`. If it is empty, no run happened since the last roll:
   keep the registry. Otherwise the rows (minus markers, muted spans and
   exact repeats) become the registry, bound as `ui_registry`.
2. Compute the **floor** for the frame about to run: the highest `layer`
   among rows that are modal, or that block and contain the pointer's current
   position. Bind it as `ui_floor` (an int, or `nil`).
3. Bind `press_x`, `press_y`: where the left button last went down. Unlike
   `drag_start_x/y` these survive the release.

**The probe.** `ui_hot(rect, layer, clip, run_floor)` and `ui_probe(...)`
answer from bindings only:

```
r       = rect ∩ clip
open    = layer >= max(ui_floor, run_floor)        (a nil floor is -∞)
hover   = open && pointer in r
pressed = hover && left button went down this frame
down    = open && (press_x, press_y) in r && left button held
released= hover && left button came up this frame
click   = released && (press_x, press_y) in r
```

`run_floor` is kept by the prelude: the highest layer on which a rect has
already turned out hot during this run, passed only to rects on a layer below
one already drawn on this run. That is what makes `pointer_captured()` exact
on the frame a panel opens.

Every one of these natives is either an emit or a pure function of its
arguments and bindings, so the frame gate and memoized scopes stay exact with
no special handling. When the floor moves, a run that read it runs again.

**`pointer_captured()`** is `run_floor != nil || ui_floor != nil`.

### The target registry

What a test, an agent or an editor uses to act on a widget by name.

```rust
let targets = petal_ui::hit::targets(&mut env);          // Vec<Target>
let save    = petal_ui::hit::target(&mut env, "Save");   // Option<Target>
let (x, y)  = save.unwrap().center();
```

```c
pb_ui_target t;
if (pb_vm_ui_target(vm, "Save", &t) == 1) click(t.x + t.w / 2, t.y + t.h / 2);

const pb_ui_target* all; size_t n;
pb_vm_ui_targets(vm, &all, &n);
```

`Target` is `{id, x, y, w, h, layer, blocks, modal}`. Straight after a run it
is that run's registrations; after the next `bind_input` it is the rolled
registry. The rows are the same either way. The rect is already clipped to
what was visible, so its centre is a point a click lands on. When an id was
registered more than once, lookup by id returns the last.

`petal_ui::harness::Headless` wraps these as `targets()`, `target(id)` and
`click_target(id)`.

## Images

`draw_image` emits the `image` command above. Loading and measuring go
through two natives the crate registers:

- `image_size(source)` returns the `{w, h}` record under `source` in the
  binding `image_sizes`, or `nil`.
- `image_load(source)` returns whether `image_sizes` has `source`, and if it
  does not, pushes `source` into the output buffer `image_requests`.

So a Rust host loads what `petal_ui::draw::take_image_requests(env)` returns
and publishes what it has with `bind_image_sizes(env, &[(source, w, h)])`,
which replaces the set. Publishing makes a script that asked run again.

A host may instead register its own natives named `draw_image`, `image_load`
and `image_size` after `petal_ui::register_all`; a later registration under
the same name replaces the crate's. The `ui` package declares them as:

```
native fn draw_image(source, x, y, w, h, a?, radius?, tint?,
                     sx?, sy?, sw?, sh?, slice_l?, slice_t?, slice_r?, slice_b?)
native fn image_load(source: string)
native fn image_size(source: string)
```

`tint` is `nil`, a `{r, g, b}` record, or a packed int `r * 65536 + g * 256 +
b`. `sw == 0` means the whole image. All four slice insets 0 means no
nine-slice. `image_size` must return a `{w, h}` record or `nil`, and
`image_load` a bool.

## Implementing the natives in another language

A host that does not embed the `petal-ui` crate implements these natives, in
addition to the input and draw sets, to load the `ui` package:

| Native | Arguments | Does |
|---|---|---|
| `ui_register` | `rect, layer, clip, id?, flags?` | Push a row into `ui_targets`. `rect` may be `nil` when `flags` has bit 2. |
| `ui_mark` | | Push `nil` into `ui_targets`. Declare it an effect, so the `ui` module's scope is never replayed from a memo record. |
| `ui_mute` | `on` | Push `on` into `ui_targets` and emit `mute` into `draw_commands`. |
| `ui_probe` | `rect, layer, clip, floor` | Return `{hover, down, pressed, released, click}`. |
| `ui_hot` | `rect, layer, clip, floor` | Return `hover`. |
| `ui_floor` | | Return the `ui_floor` binding. |
| `ui_target_rect` | `id` | Return `{x, y, w, h, layer}` from `ui_registry`, or `nil`. |
| `draw_layer` | `n` | Emit `layer [n]`. |
| `draw_rects` | `rects, palette?, dx?, dy?` | Emit `rects` with one packed array: five numbers per rect, the fifth `0xAARRGGBB`. |
| `cache_begin`, `cache_end`, `cache_draw` | `id, w, h` / `id` / `id, x, y` | Emit the command of the same name. |
| `cache_valid` | `id` | Return the epoch `e` if layer `id` is held, `-e - 1` if not. |
| `clip_push` | `x, y, w, h, radius?, within?` | With `within`, return `{x, y, w, h, prev, ox, oy, ow, oh, radius, base: false}`: the intersection with `within`, which becomes `prev`. |
| `draw_rect` | `(x, y, w, h, r, g, b, a?)`, `(x, y, w, h, c, a?)`, `(rect, c, a?)` | Told apart by the argument count. `c` is a record or a packed int. |
| `draw_polyline` | `(points, r, g, b, a?, width?)`, `(points, c, a?, width?)` | Four arguments with a record second is the second form. |
| `draw_image`, `image_load`, `image_size` | as above | |

And it performs the roll once per frame, before running the script.

`clip` in those signatures is `nil` or a record with `x, y, w, h`. A rect is
a record with `x, y, w, h`. The script side of every one of these is in
`core-libs/petal-ui/prelude/ui.ptl`.

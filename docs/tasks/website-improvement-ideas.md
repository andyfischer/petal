# Website improvement ideas (from the Seedling demo)

Status: 2026-11-06, re-checked 2026-11-11 (item 1 shipped; 2–9 open). These came out of building `seedling.ptl`, the new home-page
hero on petal-lang.org (`~/biz/petal-lang.org/frontend/public/petal/examples/seedling.ptl`).
Seedling is a growing plant drawn in software 3D. Each stem segment is one
recursive call with its own `state`. It uses a layered depth-of-field blur and
animated dew drops. Everything worked with the current WASM build; nothing
below blocks it. Each item is something the demo had to work around, or a
cost it pays every frame.

Nothing in `~/petal` was changed while writing the demo.

## 1. Sub-pixel coordinates: shipped, leftovers only

Shipped 2026-11-08 as an opt-in (petal-ui `DrawOptions { subpixel: true }`,
`new PetalCanvas({ subpixel: true })`, petal-sdl `--subpixel`; see
`core-libs/petal-ui/docs/host-contract.md` § "Sub-pixel coordinates").
Still open:

- petal-lang.org has not switched its loader to the sub-pixel drain, so
  Seedling still needs its 2× offscreen workaround.
- Prelude helpers that `int()` positions (`layer`, `snapshot`, `canvas`,
  `cached_layer`) stay whole.
- Images drawn under `pixel_snap(true)` can't yet be told apart from
  default-mode ones for smoothing.

## 2. Reuse offscreen canvases between frames

**Today.** `renderCommands` in
`integrations/petal-web-canvas/src/canvas-renderer.ts` starts each frame with a
new `Map` of offscreen canvases. Every `create_canvas` calls `createOffscreen`,
which is `document.createElement("canvas")`. `blur_canvas` creates one more
canvas per call, to hold the copy it blurs from.

**How it hurts.** Seedling uses 7 depth layers, each blurred, so it allocates
about 14 canvases per frame, some full-screen. That is GPU-texture churn and
garbage-collection pressure. It held 60 fps on a desktop, but it is the first
thing that will drop frames on a phone.

**What to do.**
- Keep a pool keyed by `(w, h)` across `renderCommands` calls.
- On `create_canvas`, take a canvas from the pool and clear it, instead of
  allocating one.
- Keep one scratch canvas per size for `blur_canvas`.
- At the end of a frame, return unused canvases to the pool and trim it.
- The script-facing semantics ("fresh each frame") stay the same.

## 3. Retained layers (`cached_layer`) on the web host

**Today.** In `core-libs/petal-ui/docs/host-contract.md`, a host that does not
ask for cache commands gets each `cached_layer`'s content inline, every frame.
`draw-wire.ts` already decodes `cache_begin` / `cache_end` / `cache_draw`, but
`canvas-renderer.ts` has no case for them, so the web runtime does not opt in.

**How it hurts.** Seedling's backdrop is a sky gradient, a mist band, god rays
and bokeh discs, blurred heavily. It changes slowly or not at all, but it is
recorded, painted and blurred again every frame. The heavy-blur layers are the
most expensive part of the frame.

**What to do.**
- Have the web runtime request cache commands.
- Implement the three cases in `canvas-renderer.ts`. `cache_begin` records
  into a kept canvas, `cache_end` stops recording, and `cache_draw`
  composites it.
- This depends on item 2: cached canvases must survive between frames, so
  they live alongside the pool.
- Handle invalidation (`pb_vm_invalidate_layers`, resize).
- Then Seedling can wrap `backdrop()` in `cached_layer`.

## 4. Additive / screen blending

**Today.** Draw commands only blend source-over. Glow can only be faked with
low-alpha gradients.

**How it hurts.** The light effects lose most of their punch: sun glow,
specular glints on drops, bokeh highlights and god rays. Overlapping lights
darken or muddy each other instead of adding up. A "light" made with alpha
can never be brighter than what it covers.

**What to do.**
- Add a blend mode, either as a per-command field or a scoped
  `with_blend("add", fn)` like `with_layer`.
- On the web it maps to `globalCompositeOperation` (`"lighter"`, `"screen"`,
  `"multiply"`). The renderer already uses `multiply` and `destination-in`
  internally for shadows.
- The SDL rasterizer needs the matching per-pixel blend functions.

## 5. Clamp colour channels instead of wrapping

**Today.** `color_of` in `core-libs/petal-ui/src/draw.rs` (and the
`r, g, b` arguments of the draw natives) convert with `as u8`. A Petal int
of 260 becomes 4. A negative value wraps to near 255.

**How it hurts.** Lighting maths often overshoots: lambert plus rim plus
translucency. A highlight that goes slightly over 255 turns nearly black, and
a shadow that goes negative turns bright. This shows up as speckled pixels
that are hard to trace. Seedling wraps every channel in its own `ch()` helper
(`clamp(v, 0.0, 255.0)`) to avoid it.

**What to do.**
- Convert with `.clamp(0, 255) as u8` everywhere a colour channel or alpha
  comes from script, in both `color_of` and the positional natives.
- Do the same for alpha.
- Add a test for 300 and −20.
- Note it in the CHANGELOG. No sensible script relies on wrapping.

## 6. Make `int()` of NaN or infinity an error, not a trap

**Today.** `int()` on NaN traps the WASM module. Per the site's CLAUDE.md, a
trap poisons the module for the whole page, and only a reload recovers.

**How it hurts.** A 3D sketch can produce NaN easily. Normalizing a
zero-length vector or projecting a point behind the camera are two ways. One
NaN reaching `int()` kills every sketch on the page. Seedling guards its
projection by clamping depth to at least 30. A playground user won't think to.

**What to do.**
- Note (2026-11-11): the native CLI does not trap; `int(sqrt(-1.0))` prints
  `0`, silently. Reproduce the trap in the wasm host before fixing.
- Make `int(NaN)` and `int(±inf)` raise an ordinary Petal runtime error,
  with the source span, instead of panicking.
- Audit other float-to-int paths in the natives for the same pattern.
- Draw natives already truncate safely with Rust `as`.

## 7. Fill in the recursive-call example in the inline-state docs

**Today.** `docs/features/inline-state.md` has three `TODO` markers:
- the two "In the debugger the state might look like" examples
- "Recursive calls — TODO: Example here"

**How it hurts.** Recursion is the most striking thing `state` does, because
the call tree becomes a state tree. It is also the least documented. The
website's hero demo is now built on it, and the docs the site imports
(`pnpm import:docs`) say "TODO".

**What to do.**
- Write a small recursive `branch(depth)` example where each call keeps its
  own `state age`, and show the resulting slot paths. Seedling's dump looks
  like `[key=0]/segment#2/[0]/leaf/age`.
- Mention that a `for g in [generation] key g` wrapper resets the whole tree
  when the key changes. Seedling uses this for "click to plant again".
- Fill the two debugger examples from a real `petal-ui-run` state dump.
- Re-import docs into the site.

## 8. A camera and projection helper for 3D sketches

**Today.** `import vec2` already gives `vec3`, `normalize`, `dot` and `cross`.
`lerp`, `distance` and `mag` take `vec3` too. What is missing is the camera.
Seedling hand-writes:
- a yaw and pitch rotation into camera space
- a perspective divide that returns screen `x, y`, depth and scale
  (`project()`, about 10 lines)
- the inverse rotation to put a camera-fixed light into world space

`rotate` is 2D only.

**How it hurts.** Every 3D sketch for the site will copy and re-tune the same
camera block. Getting pitch signs and the light's inverse rotation right is
where most of the early bugs in Seedling were. The depth clamp is easy to
forget, and without it a point behind the camera gives an infinite scale.

**What to do.**
- Add `rotate_x`, `rotate_y` and `rotate_z` for `vec3`. `rotate` could take
  an axis argument instead.
- Add a small `import camera`: `orbit_camera(yaw, pitch, dist, target, focal)`
  returning a record, plus `project(cam, p)` → `{x, y, depth, scale}` with a
  near-plane clamp, and `to_world(cam, v)` for camera-relative directions.
- Cheesecake's camera is the reference for the shape. Its focus distance and
  circle-of-confusion model could live here too, as `coc(cam, depth)`.
- Port Seedling to it as the first user, and check frame time does not regress.

## 9. Vendor the web input code with the SDK, as the renderer now is

**Today.** The site had its own `frontend/src/lib/petal/input.ts`, ported from
`petal-web-canvas` long ago, and it had drifted:
- It sent `e.button + 1` to `set_mouse_button`, so a left click arrived as
  middle.
- It re-sent held buttons and keys every frame. Each down event is a new
  press edge in petal-ui's `InputState`, so `mouse_pressed` and `key_pressed`
  fired on every held frame.
- It never released keys.

All three are fixed on the site. The site still lacks upstream's wheel
scrolling, context-menu suppression (right-click) and `type_text`.

The renderer had drifted the same way: it had no `blur_canvas`, gradients or
`draw_canvas` scaling. It is now copied verbatim by `scripts/build-petal-sdk.mjs`.

**How it hurts.** Every interactive sketch on the site was subtly broken. This
kind of bug is invisible in `~/petal`'s own tests, because they use upstream's
`input.ts`.

**What to do.**
- Make `input.ts` and `keys.ts` in `petal-web-canvas` host-agnostic: take the
  runtime as an interface rather than the wasm-bindgen class. They nearly are
  already.
- Then add them to `SITE_TS_FILES` in the site's `build-petal-sdk.mjs`, so the
  site's copy is generated, not hand-maintained.
- In `docs/usage/embedding-web.md`, add one line next to the
  `set_mouse_button` row: "feed events as they arrive; never re-send held
  state each frame". Upstream's `input.ts` header comment already says this.

## Suggested order

1. **Small and safe:** 5 (colour clamp), 6 (NaN trap) and 7 (docs).
2. **Same file, biggest performance win for the hero:** 2 (canvas pool), then
   3 (cached layers).
3. **Removes drift on the site:** 9.
4. **Larger features with the most visual payoff:** 4 (blend modes), and
   the site's switch to the sub-pixel drain (item 1).
5. **Wait for a second 3D demo:** 8, to confirm the API shape.

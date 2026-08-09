# 35 Photo adjustment UI

**Status:** complete
**Viewport:** 1280x850 (pane 1268x778)
**What works:** Five procedurally-synthesised photographs (scene functions over
`(u,v)`, 80x54 samples), ten real per-sample adjustments (exposure, contrast,
highlights, shadows, saturation, temperature, tint, fade, grain, vignette), six
presets, a draggable before/after split with a chevron handle, a peek-at-the-
negative toggle, a live 40-bin luminance histogram with mean-delta and clipping
readouts, a library rail with thumbnails, full mouse *and* keyboard control, and
a three-level cache (per-photo negatives, develop cache, quarter-res draft while
dragging). No `status_error` anywhere in a 27-step interaction sweep.
**What I could not do:** Nothing was cut, but the sample grid is coarser than I
wanted (see Issues → performance) and hold-to-compare had to become a toggle
because injected keys never latch `key_down`.

## Blockers

None.

## Issues

**1. A `for` loop as a function's implicit return value is not "captured".**
This cost me the first two debugging cycles and the error is far from the cause.

```petal
fn build(gw, gh)
  for j in range(0, gh) do
    let row = for i in range(0, gw) do i end
    row
  end
end
let img = build(3, 2)
for row in img do end     // Cannot iterate over nil
```

The guide says a for-loop collects when its value is "assigned, returned, passed
as an argument, or placed as a list element". The *implicit* return — a for-loop
as the last expression of a function body — is not in that set, and silently
yields `nil` instead. `let img = for … end` then `img` works. Either implicit
return should count as value position, or `fn f() for … end end` should be a
compile-time warning; right now the failure surfaces hundreds of lines away as
"Cannot iterate over nil".

**2. `clamp` always returns a float, which poisons integer geometry.**

```petal
let split_col = clamp(int(split * 80.0), 0, 80)
for i in range(0, split_col) do end   // numeric for-loop bounds must be integers
```

Every argument is an `int` and the result is still a float. I ended up writing
`fn iclamp(v, lo, hi) max(lo, min(v, hi)) end` — and so does the `ui` prelude,
which has its own private `_clamp` with a comment explaining exactly this trap.
When the prelude has to work around a builtin, the builtin is wrong: `clamp`
should preserve int-ness the way `min`/`max` do.

**3. No line continuation.** An expression cannot be wrapped with the operator
leading the next line:

```petal
fn fbm(x, y)
  vnoise(x, y) * 0.6
    + vnoise(x * 2.3, y * 2.3) * 0.3    // Unexpected token: '+'
end
```

Trailing-operator style works, so this is only a style constraint — but it is
the opposite of the convention most people write maths in, and the error
("Unexpected token: '+'") does not hint that moving the operator up fixes it.

**4. Functions are not hoisted, and the error is a runtime `Cannot call nil`.**
`scene_neon` called `scene_neon_upper` declared 40 lines below it; `petal check`
passed, and the failure only appeared at runtime as `Cannot call nil` pointing
at the call site. Classes *are* hoisted and the guide says so; plain `fn`s not
being hoisted is stated only in a parenthetical about method pinning. A check
pass that already walks the file could report "call to `scene_neon_upper` before
its declaration".

**5. Performance: ~90 µs per sample for straight-line float arithmetic.**
Developing 4 320 samples (unpack → ~8 record-producing colour ops → pack) takes
~400 ms, and *synthesising* a scene (fbm ≈ 12 `sin` calls per sample) takes
~1.8 s for the same grid. That put a hard ceiling on image resolution: I wanted
a 128 x 86 grid and had to ship 80 x 54, which is visibly blocky. Suspects worth
measuring: every `fc(r,g,b)` allocates a record, and Garden's `panel.values`
observation buffer records *every* named binding on *every* iteration of these
hot loops — I would like a way to opt a function out of observation (a `#[quiet]`
attribute, or just excluding bindings inside loops).

I worked around it with three caches and a quarter-resolution draft pass that a
live drag recomputes instead of the full frame. That turned into a feature (the
`DRAFT 1:2` badge), but it was forced, not chosen.

**6. `key_down("shift")` is not how you read a modifier.** It silently returns
false — no error, the coarse-step keybinding just did nothing. `mod_shift()` is
the answer, and it is discoverable only by reading `ui.ptl`; the panel doc's
input list (`key_down`, `key_pressed`, …) does not mention `mod_*` at all.

**7. Injected keys cannot express "held".** `POST /key` delivers press and
release in one frame, so `key_down(k)` is never observable from a later
`GET /state`. Hold-to-compare (space) is the natural gesture for a before/after
control and I could not test it, so I shipped a toggle instead. A
`{"op":"down"}` / `{"op":"up"}` form of `/key` (which `/mouse` already has, and
which is what made slider dragging testable) would fix this.

**8. `state` caches survive hot reload, which makes iterating on generated
content confusing.** Editing a scene function and saving changed nothing,
because the negative for that photo was already in `state`. Correct behaviour,
but I lost several minutes to it; a hot-reload note in the panel doc about
`state` outliving the code that produced it would help. (The language guide does
say state is preserved — the panel doc, which is what you read while writing a
panel, does not.)

## Praise

* **`panel.values` is superb.** Being able to assert `adj`, `dragging`,
  `split_col`, `live`, `draft_sig` by name, with no instrumentation in the
  script, made the whole drag/preview state machine verifiable in one `curl`.
  It found two real bugs (the split column mapping for the half-res draft, and
  shift not registering) faster than pixels would have.
* **`/mouse` with `down` / `move` / `up`** is exactly the right shape. Slider
  drags, the draft-while-dragging path, and the split handle were all
  exercisable end to end.
* **Styled `draw_text` + exact `text_width`.** `draw_text_right`,
  `draw_text_center` and `fit_parts` land on the pixel, at every size. Building
  a typographic hierarchy out of size/colour/letter-spacing with no bold
  available worked better than I expected.
* **Alpha and rounded rects on every primitive** carried most of the visual
  design: scrims, the selected-row plate, the draft badge, dimming the histogram
  during a draft pass.
* **Records as colours** (`fc(r,g,b)` floats internally, hex literals for
  chrome, both interchangeable with the `{r,g,b}` the draw overloads take) made
  the whole colour pipeline pleasant to write.
* Error messages, when they fire, are genuinely good — source line, caret,
  `Caused by:` provenance chain naming the bindings involved.

## Feature requests

Prioritised.

1. **Make `clamp` int-preserving** (or add `iclamp`). It is a five-minute fix
   that removes a trap the standard prelude already documents.
2. **A `{"op":"down"|"up"}` form for `POST /key`**, matching `/mouse`. Without
   it, no held-key interaction in any panel app is testable headless.
3. **Treat a for-loop in implicit-return position as captured**, or warn.
4. **Speed on numeric inner loops** — this is the ceiling on what a Petal panel
   can draw. Two concrete asks: avoid heap-allocating small records that never
   escape, and let a function opt out of the observation buffer.
5. **Warn at check time on a call to a function declared later in the file.**
6. **Document `mod_shift()`/`mod_ctrl()`/… in `petal-graphical-panels.md`**
   alongside `key_down`.
7. A note in the panel doc that `state` survives hot reload, with the "caches of
   generated content go stale" consequence spelled out.
8. Nice-to-have: an offscreen pixel buffer native (`image_from_samples(list,
   w, h)` → drawable) would let a panel show a real photograph instead of a grid
   of quads, and would make this whole category of app an order of magnitude
   cheaper.

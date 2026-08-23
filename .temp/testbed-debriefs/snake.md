# 03 Snake

**Status:** complete
**Viewport:** 1280x850 (panel pane 1268x778)
**What works:** The full game. 32x23 grid, fixed-step `dt()` accumulator (155ms
down to 56ms across 10 speed levels), snake drawn as a continuous ribbon with a
head→tail colour ramp and direction-aware eyes, apples, a timed golden apple
worth 5x, self-collision, wall collision with an optional wrap mode, four
phases (ready/playing/paused/over) each with its own centred overlay card, a
death shake, an eat flash, and a sidebar with score/best, a speed meter, four
stat tiles, three hover-lit buttons and a bar chart of the last five runs.
Keyboard (arrows/WASD/space/R/T) and mouse (all three buttons) both drive it,
and every piece of logical state is readable by name from
`/state → panes[0].panel.values` — I wrote a bash bot that reads `body[0]` and
`food`, steers, and plays the game headlessly, which is how I regression-tested
scoring, growth, the golden apple, and both death paths.
**What I could not do:** Nothing I set out to do. One host bug (below) forced a
design constraint on the type scale.

## Blockers

None.

## Issues

### 1. Garden's glyph atlas corrupts once a frame uses ~10+ distinct text sizes (host bug, cost me the most time)

This is the big one, and it is **not** Petal — it is Garden's text renderer, and
it corrupts Garden's own chrome too.

Minimal repro (`probe.ptl`, run on a *fresh* headless Garden):

```petal
clear(10, 12, 17)
let sizes = [11, 12, 13, 14, 16, 20, 22, 24, 30, 36, 40, 44, 46, 54]
let y = 10
for i in range(0, len(sizes)) do
  let s = sizes[i]
  draw_text("Ag7N 0123 size {s}", 20, y, s, 220, 230, 240)
  y = y + s + 10
end
```

Every run at size ≤ 24 renders as a jumble of glyphs from the *other* sizes —
letters at the wrong scale, wrong shapes, overlapping. Sizes 30+ are fine. The
tab-bar title and the `NORMAL probe.ptl 1:1` status line — drawn by Garden, not
by my script — are garbled in the same screenshot, so the whole atlas is
trashed, not just the panel's runs.

`GET /scene` reports the runs correctly (`{"text":"7 steps/s","size":11.0}`),
so the bug is strictly at rasterisation/atlas time — the scene dump and the
pixels disagree, which also means `/scene` cannot be used to catch it. Only
looking at the PNG finds it.

Bisecting the trigger:

| sizes in one frame | result |
|---|---|
| `[11, 13, 20, 44]` | clean |
| `[10, 11, 12, 13, 16, 20, 24, 30]` | clean |
| `[10, 11, 12, 13, 16, 20, 24, 30, 44]` | clean |
| `[11, 12, 13, 14, 16, 20, 22, 24, 30, 36, 40, 44, 46, 54]` | corrupt |

So it is a capacity/eviction problem rather than a specific size pair. It is
also *sticky*: once a session corrupts, hot-reloading the script down to four
sizes does not fix it — you have to restart Garden.

In my app it did not show up on the first screenshots and then appeared several
minutes in, once enough distinct (glyph, size) pairs had accumulated across the
session — so the failure mode is "your app looks perfect, then silently rots".
My first version had ten sizes (10/11/12/13/16/20/22/24/30/44/46) and by the
end of a play session the sidebar was rendering `SPEED LEVEL 2` with a 46px `2`
in it.

**Workaround:** I collapsed the type scale to six sizes — 11 / 13 / 16 / 22 /
30 / 44 — and it has stayed clean through long sessions. That is a fine
discipline for a design system, but it is a real constraint imposed by a bug,
and an app author has no way to know about it except by staring at PNGs.

Note the docs actively encourage the thing that breaks: *"a script can build a
real typographic hierarchy (a 28 px heading over a 10 px caption)"*.

### 2. `weight` degrading to regular really does cost you hierarchy

Documented, so not a surprise, but worth restating with a concrete cost: with
no bold and (per issue 1) a budget of ~6 sizes, the only levers left are colour,
spacing and size. I ended up using `spacing: 2` on 11px all-caps micro-labels as
the "different kind of text" signal. Embedding JetBrains Mono Bold would buy
back a whole axis.

### 3. `panel.values` for a `state` written inside a top-level `while` is not
reliably the post-loop value

My step loop is a top-level `while` that rebinds `state body` each iteration.
Most reads of `panes[0].panel.values.body` matched the drawn frame, but a
handful of consecutive reads returned a value one step stale (and once, one
step *behind* a previous read) while the pixels were correct and monotonic. I
could not reduce it to a clean repro, and the panel never actually
mis-simulated — but it means "last write wins" is not quite true for a `state`
written from inside a `while`, and a test asserting on that key can flake.
Bindings written once per frame (`score`, `phase`, `lvl`, `pause_label`) were
rock solid.

### 4. No docs on where the sidebar/pane origin sits for `/mouse`

`/mouse` takes *window* coordinates while the script draws in *pane-local*
coordinates, and the pane starts at `(6, 38)` because of the tab bar. Nothing
in `debug-server.md` or `AUTHORING.md` says so; I found it by reading
`panes[0].rect` and adding the offset by hand. A sentence in AUTHORING.md would
have saved a confused round trip.

### 5. `if/elsif` chain in expression position needs its `end` on the same
logical construct, and the error does not say so

This parsed but did something I did not expect:

```petal
let radius = if i == 0 then 9 else 7
end
```

I had written the `end` on the next line while editing; it compiled, so I never
saw an error — but it reads as a mistake and I only noticed on a re-read. Not a
bug, just a place where the layout rules are looser than they look.

### 6. `random_int` is registered but undocumented for panels

`random_int(min, max)` (max-exclusive) is exactly what a grid game needs and it
works in a Garden panel, but it appears in neither
`petal-graphical-panels.md`'s list of available natives nor the language
guide's builtin sections. I found it by grepping
`rust/src/builtins/creative_coding.rs`. The panel doc's "plus the input/timing
reads …" list reads as exhaustive and isn't.

## Praise

- **`panel.values` is genuinely excellent.** Being able to write a 12-line bash
  script that reads `body[0]` and `food` out of `/state`, computes a direction,
  and `POST /key`s it — and then assert on `score`/`apples`/`length` — turned
  "test a game" from a screenshot-squinting exercise into a real integration
  test. Nothing in the script participates; a plain `let` is already visible.
  This is the best part of the whole stack.
- **`/screenshot` and `/scene` settling frames before answering** means no
  sleeps anywhere in the test loop. That is a small decision that removes an
  entire category of flake.
- **Petal's `let`-rebinding-with-dataflow** is a good fit for a per-frame draw
  script: I wrote the whole simulation with `let`/`state` and only needed
  `var`/`set` twice (a rejection-sampling loop and a step counter), and both
  times the compiler's "use `set`" / "`=` inside a function would shadow"
  errors were clear.
- **The `ui` prelude's record overloads** (`draw_rect(r, c)`,
  `draw_rect_rounded(r, radius, c)`, `draw_text(s, pos, style)`) plus `Rect`
  make the drawing code read like layout rather than like arithmetic. `rect()`
  keeping sub-pixel coordinates matters for the death shake.
- **Colour literals desugaring to `{r,g,b}` records** so a `mix(a, b, t)`
  helper is four lines and works with every draw call. The head→tail gradient
  and the "tint a card toward its accent" pattern both fell out of that.
- `text_width` measuring the real font meant `draw_text_center` /
  `draw_text_right` landed exactly, first try, at every size.
- Hot reload preserving `state` let me retune the palette and the layout with a
  game in progress.

## Feature requests

1. **Fix the glyph atlas** (issue 1). Highest priority by a wide margin — it
   silently destroys any panel with a real type scale, and it breaks the
   editor's own chrome, so it is not only a testbed concern.
2. **Embed JetBrains Mono Bold.** One extra face, no protocol change, and it
   restores the weight axis the docs already describe.
3. **A `/scene`-level or `/state`-level render-health signal**, or failing
   that, a documented statement that `/scene` cannot catch rasterisation bugs.
   Right now an automated check can pass on a completely garbled frame.
4. **Document `random_int` / `random` / `choose` for panels**, and say in
   `petal-graphical-panels.md` whether the full Petal builtin table is
   available or only a subset.
5. **Say in AUTHORING.md that `/mouse` coordinates are window-space** and that
   the pane origin is in `panes[0].rect`.
6. A `panel_size()`-style note, or an example, showing that the panel pane is
   ~1268x778 inside a 1280x850 headless window — every layout decision starts
   from that number and I had to discover it by screenshotting past the bottom
   edge.
7. Optional: a way to keep a panel awake deliberately (`request_animation()` or
   a `--panel-wake` flag) for genuinely continuous simulations. The 10s sleep
   is the right default, but a game is exactly the case where it is wrong, and
   "inject a fake mouse move every second" is a strange thing to have to do.

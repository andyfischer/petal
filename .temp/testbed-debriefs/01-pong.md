# 01 Pong

**Status:** complete
**Viewport:** 1040x720
**What works:** The whole game. Title / serve / play / paused / match-over
phases; keyboard (`W`/`S`/`↑`/`↓`) and mouse paddle control; dt-driven,
sub-stepped physics with angle-off-paddle returns and per-hit acceleration; a
predicting CPU opponent with a reaction delay and a per-rally aim error; a
particle system; wall/goal flashes; rally + best-rally + ball-speed readouts and
match pips. `status_error` stayed `null` across the entire interaction script
(serve, both key axes, both arrow keys, mouse tracking, pause, resume, restart,
scoring, match over).
**What I could not do:** Nothing I set out to do. Two host-side rendering
issues cost time (below); neither required a design compromise.

## Blockers

None.

## Issues

### 1. Glyph atlas corruption after repeated hot reloads (host, Garden)

The most expensive thing in this session by far. After roughly a dozen
hot reloads of the panel script — several of which changed `size` values in my
text styles — text runs started rendering with **the wrong glyph size**, per
glyph, while `/scene` reported the correct size for every one of them.

`GET /scene` for the overlay banner:

```json
{"pos":[448,341],"size":36.0,"text":"P"}   … A, U, S, E, D all size 36.0
```

Pixels: `P` at ~12 px, `A` at ~12 px, `U` at 36 px, `S`/`E`/`D` at ~12 px —
correctly *positioned* on the 36 px advances, but rasterised at a stale small
size. Which glyphs survived correlated exactly with which `(glyph, size)` pairs
had already been rasterised earlier in the process's life: after I changed the
banner from 36 px to 30 px, the only glyph that rendered right was `P`, which
was the one character the 30 px title `PONG` had already put in the atlas.

Killing the process and relaunching against the *identical* script rendered
everything perfectly. So it is accumulated process state (atlas full, or evicted
entries not re-rasterised), not the script.

This is nasty for the agent workflow specifically, because the loop the
authoring guide prescribes is "edit, hot reload, screenshot, judge with your own
eyes" — and after enough iterations the screenshot silently stops being a
faithful render. I spent a long time convinced I had a Petal bug (`spacing`,
`{...S_BIG, color: …}` spread, draw ordering) before I thought to restart.
Suggested: bound the atlas or re-rasterise on miss; failing that, note in
`petal-graphical-panels.md` that a long-lived hot-reloaded panel should be
restarted before trusting a screenshot.

### 2. `/state` and `/scene` disagreeing means "restart Garden", and there is no signal

Corollary of the above: there is no way from the debug protocol to notice that
the renderer has degraded. `/scene` is generated from the panel commands and was
always right. A `X-Garden-Atlas-Full` header or a line in `/state` would have
saved the hour.

### 3. Headless panels advance ~one frame per injected event

Documented (the 10 s wake window, the ~200 ms poll), but the practical
consequence is stronger than the doc implies: a game loop advances essentially
one frame per `POST /key`, so playing a rally out took 300+ curl round trips in
a shell loop. Two things would make interactive-panel testing much cheaper:

- `POST /tick {"frames": 60}` or `POST /tick {"dt": 1.0}` — advance the panel
  clock deliberately, without pretending to be input.
- `GET /screenshot?ticks=N` for capturing an animation mid-flight.

I worked around it by injecting a key the app ignores (`{"key":"x"}`) in a
`for` loop. It works but it is slow and it conflates "input" with "time".

### 4. `dt()` in headless is the wall-clock poll interval, so physics needs its own clamp+substep

Not a bug, but worth writing down as a pattern for the other testbed apps: with
`dt()` around 0.03–0.2 s a 400 px/s ball moves 12–80 px per frame and tunnels
straight through a 13 px paddle. Every headless-tested Petal game needs

```petal
let sdt = min(dt(), 0.06)
var left_t = sdt
while left_t > 0.0 do
  let h = min(left_t, 0.004)
  set left_t = left_t - h
  …
end
```

A prelude helper (`substep(dt, max_h, fn(h) … end)`) would be a reasonable
addition to `petal-ui`, though the closure would need `var` capture to be
useful.

### 5. `=` rebinding is not usable for game state, so almost everything became `state var`

Because the simulation mutates from inside a `while` loop and from inside `if`
blocks, and because `=` inside a function targeting an outer binding is an
error, the safe choice was to declare all 30-odd simulation variables as
`state var` and write them with `set`. That works, and the disjoint `=`/`set`
rule is genuinely clear — but it costs exactly the dataflow provenance the
language guide advertises, and it makes the top of the file a wall of
declarations. I do not have a better proposal; I mention it because "idiomatic
Petal uses `let` for dataflow" and "write a game loop" pull in opposite
directions, and the guide could say so.

### 6. `panel.values` does not obviously surface `state var` cells

I could not tell from the docs whether a `state var` shows up in
`panes[0].panel.values`, so I defensively added a block of `let obs_*` mirrors
at the bottom of the file. That turned out to be a good idea for other reasons
(stable, typed, few names) and I would do it again — but the doc should say
what happens to a `var` cell.

### 7. Minor: `point_in`/`hovered` use panel-local coordinates, `POST /mouse` uses window coordinates

Obvious in hindsight (the pane is offset by the tab bar), but my first mouse
test asserted against the coordinates I had posted and looked like a failure.
`panel.input.mouse` in `/state` is what resolved it; worth a sentence in
AUTHORING.md next to the `POST /mouse` examples.

### 8. Minor: the first mouse `move` after launch is swallowed

`mouse_x()` reads a sentinel until the pointer has been somewhere, so an
app that computes "did the mouse move this frame" needs an arming frame and the
first injected `move` does nothing. Not wrong, just surprising when scripting.

## Praise

- **`panel.values` is excellent.** Being able to assert `obs_phase == "paused"`
  and `obs_score == [0,1]` straight out of `/state`, with no instrumentation
  beyond naming a `let`, made this app testable in a way a canvas game normally
  is not. It is the single best thing in this toolchain.
- **The settle-then-capture contract really holds.** `POST /key` then
  `GET /screenshot` with no sleep, every time, hundreds of times. No flakes.
- **The draw surface is complete.** Alpha on every primitive, rounded rects,
  and `clip` meant the paddle glows, the translucent scrim and the particle
  containment all worked first try, with no compositing tricks.
- **`text_width(s, style)` measuring the same record you draw** is exactly
  right. Centring and right-alignment were exact at every size, including
  letter-spaced runs.
- **Collecting `for` with `continue` as a filter** made the whole particle
  update one expression:
  ```petal
  set bits = for p in bits do
    let nl = p.life - sdt
    if nl <= 0.0 then continue end
    {...p, x: p.x + p.vx * sdt, life: nl}
  end
  ```
  That is nicer than the equivalent in most languages.
- Record spread plus `if`-as-expression made the style system (`{...S_LABEL,
  color: C_YOU}`) pleasant.
- Error messages I did hit were precise and pointed at the right line.

## Feature requests

1. **Fix the glyph-atlas degradation, or make it detectable.** Highest
   priority: it silently invalidates the screenshot-driven workflow every agent
   in this batch is using.
2. **`POST /tick {"frames": n}`** on the debug server — advance panel time
   without faking input. Would make every animated testbed app 10× cheaper to
   verify, and would let a test assert "after 2 simulated seconds, the ball has
   crossed the court".
3. **A `key_repeat`/held-key story for injected input.** `POST /key
   {"key":"w","hold_ms":300}` would let a headless test exercise the
   `key_down` path at all; today only `key_pressed` is reachable, and every app
   has to invent a tap-impulse hack to stay drivable.
4. **Document `state var` in `panel.values`** (and whether `config let` is
   distinguished there — a live-editing host could show sliders for the six
   `config let`s in this file, which would be a lovely demo).
5. **A `substep` / fixed-timestep helper in `petal-ui`**, or at least a worked
   example in `petal-graphical-panels.md`, since every real-time panel needs it.
6. Lower priority: a `draw_circle_outline`, and a `lerp_color`/`mix` in the
   prelude — I hand-rolled the latter and then deleted it, but every panel that
   fades between two palette entries will want it.

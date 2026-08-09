# 02 Breakout

**Status:** complete
**Viewport:** 1100x780 (panel pane 1088x708)
**What works:** Everything I set out to build. A 99-cell brick grid with
per-brick armour and a fill-level damage read; grid-indexed continuous
collision with a 4 ms sub-step integrator; four generated wall layouts
(`level % 4`); four catchable power-ups including multiball up to nine
simultaneous balls; a combo multiplier; falling pills that are caught or lost;
a particle system and a per-ball comet trail; screen shake; keyboard *and*
mouse control plus click-to-launch; and the five phases (title / serve / play /
paused / cleared / over) with their overlay cards. Verified live against a
headless Garden: level clear advanced through levels 1→4, game over fired,
pause resumed, multiball reached six balls, `status_error` stayed `null`
across every interaction I drove.
**What I could not do:** Nothing was cut. Two things I worked around rather
than solved (below): multi-line boolean conditions, and the fact that
`petal check` is the only place a syntax error is visible.

## Blockers

None.

## Issues

### 1. A syntax error in a panel script is invisible — the pane silently opens empty

This cost me the first debugging cycle. `layout(panel("app.ptl"))` with an
app.ptl that does not parse produces:

- a normal launch line (`garden: headless, debug server on …`),
- **nothing** in `log.txt`,
- `GET /state` reporting `"status_error": null` and one pane of
  `"kind": "editor"`, `"title": "[untitled]"`, `"panel": null`.

So the failure mode is indistinguishable from "you wrote the layout wrong",
which is exactly what AUTHORING.md warns about for a bare `panel(...)` at top
level. I only found the real cause by running `petal check app.ptl` by hand.
Either the parse error should land in `status_error`, or the pane should stay a
panel pane and report the error in `panel.script_error` — a panel that fails to
*compile* should not degrade to a different pane kind.

### 2. `if` conditions cannot span lines

```petal
// error: Expected `then`, got '&&'  [line 495, column 8]
if ny + 9.0 >= float(pad_y) && ny - 9.0 <= float(pad_y + PADH)
   && d.x >= padx - 10.0 && d.x <= padx + float(pad_w()) + 10.0 then
```

A record literal, a list literal and a call argument list all wrap across lines
happily, so the newline sensitivity here is surprising. The workaround is fine
(`let in_y = …` / `let in_x = …` and then `if in_y && in_x then`) and arguably
reads better, but it is a rule you learn by hitting it. Allowing a continuation
when the line ends in a binary operator would remove the surprise.

### 3. `=` vs `set` inside `if`-inside-`for`-inside-`fn` is easy to get wrong

```petal
fn power_by_key(k)
  var found = POWERS[0]
  for p in POWERS do
    if p.key == k then found = p end     // error: `found` is a `var`
  end
  found
end
```

The error message is excellent and told me exactly what to type. The friction
is that a `var` accumulator is *the* idiom for "search a list", so a Petal
newcomer will write this exact wrong line every time. This is a docs nit more
than a language one: the `var`/`set` section of the language guide shows the
`for` accumulator with `set` but the "reach for `var` when a write must land"
framing makes `=` look like the natural inner-loop write.

### 4. Two names for the same thing in one top-level scope

I originally computed `let mult = 1 + combo / 6` inside the physics loops and
then, hundreds of lines later at draw time, wrote `let mult = 1 + combo / 6`
again for the HUD. Both are top-level (only *function* bodies qualify names),
so the second is a rebind of the first rather than a fresh local, and
`panel.values` shows one `mult`. Nothing broke, but it is a real hazard in a
long single-file panel: block scopes that are not scopes. I renamed to
`cmult` defensively. A lint for "top-level `let` shadows an earlier top-level
`let` of the same name inside a different block" would catch it.

### 5. `panel.values` is buried under the prelude and the constants

`GET /state | jq '.panes[0].panel.values'` on this app returns ~120 keys before
you find yours: every colour constant, every type-scale record, `LAYOUTS`,
`POWERS`, `theme` (from the prelude — the `::`-filter does not catch it because
it is re-exported bare), plus loop temporaries like `idx`, `hp`, `on`, `f`,
`row_color.i`. The `obs_*` mirror convention from 01-pong is what makes this
usable at all. A `?prefix=obs_` query parameter on `/state`, or filtering out
bindings whose value never changes between frames, would make the killer
feature actually pleasant.

### 6. Small things

- `random(lo, hi)` is float-only, so my first cut at picking a random list
  element was `POWERS[int(random(0.0, 3.999))]`. `random_int(lo, hi)` and
  `choose(list)` both exist and are exactly right (the shipped code uses
  `choose`), but nothing in `random`'s own Builtins.md entry points at them and
  the language guide's "List Builtins" section does not list either. A
  cross-reference would have saved the detour.
- `clamp` returns a float, which is right, but it means pixel geometry has to
  be re-`int()`-ed constantly. The prelude has a private `_clamp` for exactly
  this reason (its comment says so) and does not export it. Please export an
  int-preserving `clampi`, or make `clamp` type-preserving.
- Integer division truncates toward zero, so `(x - GX) / CW` for an `x` left of
  the grid gives `0`, not `-1`. That happens to be safe here after a `max(0, …)`,
  but a grid-index computation is the classic place where floor-vs-truncate
  matters and the language guide does not say which one `/` is.
- `weight` degrading to regular is documented and I designed around it, but the
  headline of every card is a 30 px letter-spaced run precisely because there is
  no bold. Embedding the Bold face would immediately improve every panel app.

## Praise

- **`panel.values` plus `POST /key`/`/mouse` is a genuinely great test loop.**
  I drove 200-step play sessions from bash, asserted `obs_left` decreasing and
  `obs_balls` splitting, and never once had to decode a pixel. Confirming
  multiball worked was a one-line poll on `obs_balls >= 3`.
- **Hot reload with `state` preserved is superb.** I changed `drop_chance` to
  0.9, watched pills rain, changed it back, and the game kept playing — no
  relaunch, no lost state. Testing the level-clear card by temporarily making
  `cell_hp` return one brick took ten seconds.
- **`set xs[i].field = v` on a `state var` list of records just works**, at any
  nesting depth. That is what made a 99-brick grid and a nine-ball list
  practical without any escape hatch.
- **Collecting `for` with `continue` as a filter** is the single nicest thing in
  the language for this genre. Four object pools, four one-expression reap
  passes, no index bookkeeping.
- **`Rect` methods** (`inset(-7)` to grow a glow, `offset(sx, sy)` for screen
  shake, `center_x()`) removed a lot of arithmetic noise from the draw code.
- **`text_width` measuring the real font** made every self-sizing pill, badge
  and card exact on the first try.
- The **error messages** are consistently excellent: the `var`/`set` one, the
  parse caret, and the arity warnings all told me the fix, not just the fault.

## Feature requests

Prioritized.

1. **Surface panel compile errors.** A `.ptl` that fails to parse must show up
   in `/state` (`status_error` or a `panel.error`) instead of silently
   degrading the pane to an empty editor. This is the single highest-value fix
   for anyone authoring a panel.
2. **`GET /state?values_prefix=obs_`** (or `?panel_values=obs_*`) to filter
   `panel.values`. Cheap, and it makes the observation buffer usable on a real
   app without the mirror-variable convention.
3. **Line continuation inside `if` / `while` conditions** when the line ends in
   a binary or logical operator.
4. **Embed the Bold face.** `weight` is in the protocol, measured correctly,
   and does nothing.
5. **Export an integer-preserving clamp** from the prelude (the private
   `_clamp` already exists), and cross-reference `random_int`/`choose` from
   `random` in Builtins.md.
6. **A frame-stepping endpoint** — `POST /tick {"dt": 0.016, "n": 60}` — so a
   headless game loop can be advanced deterministically instead of by injecting
   ignored keypresses at the poll rate. Every one of my test scripts is a loop
   that posts a no-op key purely to get a frame.

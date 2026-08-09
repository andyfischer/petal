# 42 Live server-monitoring dashboard (ORBITAL)

**Status:** complete
**Viewport:** 1440x900 (`GARDEN_HEADLESS_SIZE=1440x900`; the panel itself gets ~1428x828)
**What works:** Everything I set out to build. 18 seeded hosts in a scrollable,
selectable fleet column with per-row status dot and sparkline; four KPI tiles
with deltas and filled sparklines; a 96-sample streaming area chart with a
self-scaling axis, dashed warning/critical thresholds, a live head marker and a
hover crosshair readout; an 8x36 per-core utilization heatmap with a colour
ramp legend; a threshold-driven alert feed with hysteresis, severity chips,
relative timestamps, acknowledge-all and click-to-jump. Keyboard (arrows/j/k,
1-4, space, `[`/`]`, `a`) and mouse (row click, tile click, alert click, pill
clicks, wheel on two independent scroll regions, chart hover) both drive it, and
every one of those was exercised through `/key`, `/mouse` and `/state` with
`panes[0].panel.error` null throughout.

**What I could not do:** Nothing was cut. One design decision forced by the host:
the stream is a *pure function of the tick index* rather than a stored history
buffer, because there is no cheap growable ring buffer and rebuilding
18x4x96 nested lists per tick would have been silly. That turned out better than
the thing I originally planned, but it was the host's cost model that pushed me.

## Blockers

None that stopped me outright.

## Issues

**1. Panel script errors are invisible in `status_error` — and AUTHORING.md
sends you there.** The guide says to check `/state` for `status_error`. A hard
panel runtime error leaves `status_error` **null**; the error lives in
`panes[0].panel.error` (and is painted into the pane, which you only see if you
open the PNG). I burned ~15 minutes convinced the mouse was not being delivered
when in fact the frame was dying on the line right after the hover test.
Please fix the guide, or surface panel errors in `status_error` too.

**2. `panel.values` reports only the last *good* frame, which reads as "that
line never ran".** Compounding (1): I added debug bindings above the failing
line, and they came back **absent** from `panel.values` — not stale, absent —
because the whole frame errored before publishing. The natural read of an
absent key ("the branch didn't execute") is exactly wrong in that case. A
`values_frame` number, or keeping the erroring frame's partial values, would
have made this obvious.

**3. `print()` from a panel script never reached `/state`'s `script.output`.**
It stayed `[]` across many frames with an unconditional `print("DBG", …)` at top
level. AUTHORING.md advertises `script.output` as the way to see script prints,
so with (1) and (2) I had *no* working debug channel except screenshotting the
error banner.

**4. `curl localhost:$PORT` hit a different agent's Garden.** My process was
bound to `127.0.0.1:65113` (confirmed with `lsof`), but `curl -s
localhost:65113/screenshot` returned a 1100x800 PNG of somebody's Breakout
game — presumably an IPv6 `::1` bind of the same port number by another
concurrent instance. This is a *nasty* failure mode: it looks like your app
spontaneously turned into a different app. AUTHORING.md should say
`127.0.0.1`, not `localhost`.

**5. No scientific-notation float literals.** `let lo = 1.0e9` lexes as `1.0`
followed by the identifier `e9`:

```
panel error: Undefined variable: e9 [line 417, column 15]
417 |   let lo = 1.0e9
                     ^^
```

The error is accurate but the cause is not obvious. `1000000000.0` works.

**6. Int/float leakage into places that demand ints, caught only at runtime.**
Two separate instances, both from the same root cause — arithmetic and `clamp`
promote to float, and lists/`range` demand ints:

```petal
let a_vis = feed_r.h / AROW_H          // feed_r.h is float
for k in range(0, a_vis) do            // runtime: "numeric for-loop bounds must be integers"

hover_k = clamp(hover_k, 0, WINDOW - 1) // all three args ints, result float
let hv = series[hover_k]                // runtime: "Cannot index list with float"
```

`clamp(int, int, int) -> float` is the surprising one. `min`/`max` keep ints;
`clamp` does not. Fixes were `int(...)` wrappers, but `petal check` said nothing
about either, and a Rect field being float is invisible at the call site.

**7. No string formatting builtin.** Every dashboard needs fixed-decimal and
thousands separators, and `str(12.3)` gives `12.300000000000001`, so I hand-rolled
`dec(x, places)` and `commas(n)` again — the same two functions the 41 dashboard
next door also hand-rolled. This is the single highest-value missing builtin.

**8. `M_MAX[m]`-style dispatch tables are fine, but records can't be indexed by a
variable key.** I wanted `host.base[metric_name]`; I used parallel lists indexed
0-3 instead. Workable, but it means the metric identity is an untyped int
everywhere.

**9. Panel size is not the headless size.** `GARDEN_HEADLESS_SIZE=1440x900`
gives the panel 1428x828 (tab strip + status bar). Obvious in hindsight, but my
first layout was written against 900 and the fleet column ran off the bottom.
Worth one sentence in AUTHORING.md.

**10. A `state` list rebuilt every tick is awkward to write in place.** I wanted
`levels[si][m] = lvl` inside a nested loop over a `state` binding; I rebuilt the
whole 18x4 table with two collecting `for` loops instead and diffed old vs new in
a second pass. That reads fine — arguably better — but it was not my first
instinct and the "which write forms are legal on a `state` nested index" question
is not answered anywhere I found.

## Praise

- **`panel.values` is the feature.** Asserting `sel_host`, `sel_metric`,
  `paused`, `alerts_scroll`, `hover_k`, `a_vis`, `ROW_H` by name after each
  injected event — with zero instrumentation in the script — is the best
  UI-testing story I have used. Layout constants coming back too (`low_h`,
  `chart_h`) let me debug geometry numerically instead of by eye.
- **Hot reload with state preserved.** Editing the file and injecting one
  keypress reloads the panel and keeps `tick`, `alerts` and the selection. I
  iterated ~20 times on one long-lived process without restarting it once.
- **Error messages.** The caret plus the `Caused by:` chain naming the
  intermediate bindings (`series [line 685]`, `WINDOW [line 673]`) told me
  exactly which value was float. Genuinely better than most languages.
- **Collecting `for` loops.** `let row = for m in range(0, 4) do … end` for the
  per-metric threshold row is exactly the right amount of syntax.
- **`noise` / `smoothstep` / `color_lerp` / `clamp` / `map_range`.** The whole
  simulated telemetry signal and the heatmap ramp are four lines each because
  these exist.
- **`text_width(s, style)` honours `spacing`.** I assumed it wouldn't and wrote a
  manual tracking allowance; it does, exactly as the prelude comment promises,
  so letter-spaced labels right-align perfectly. Nice API.
- **The prelude's `list_update` / `scroll_update` / `draw_scrollbar` trio.** Two
  independent scroll regions with keyboard gating cost me about six lines.

## Feature requests

1. **Fix the debugging story for panels** (highest priority, and it is three
   things): surface `panel.error` in `status_error` *or* correct AUTHORING.md;
   make `print()` from a panel reach `script.output`; and mark `panel.values`
   with the frame it came from so an absent key can be told from a dead frame.
2. **A `format` / `fmt` builtin** — at minimum `str(x, places)` and a
   thousands-separator option. Two testbed apps in a row hand-rolled the same
   two functions.
3. **`clamp` should preserve int-ness** when all three arguments are ints, the
   way `min`/`max` do; and `petal check` should flag a float flowing into
   `range(...)` or a list index where it can see the types.
4. **Scientific-notation float literals** (`1e9`, `1.5e-3`).
5. **`localhost` → `127.0.0.1` in AUTHORING.md**, plus a note that the panel
   pane is smaller than `GARDEN_HEADLESS_SIZE`.
6. **Record indexing by a computed key** (`r[k]`), so dispatch tables can be
   keyed by name instead of by parallel-list position.

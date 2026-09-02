# 01 — Pong

A complete game of Pong as a Garden panel, written entirely in Petal: a real
game loop, sub-stepped continuous collision, keyboard *and* mouse control, an
opponent that predicts the ball, a particle system, and a five-phase UI
(title → serve → play → paused → match over).

## Viewport

Designed for **1040 × 720** logical pixels. The layout is computed from
`screen_width()`/`screen_height()`, so other sizes work, but the type scale and
the footer chip row were tuned at 1040 × 720.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/games/pong

# windowed
GARDEN_HEADLESS_SIZE=1040x720 ../../../garden/target/debug/garden --init layout.ptl

# headless + debug server (what the agent workflow uses)
GARDEN_HEADLESS_SIZE=1040x720 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

## Controls

| Input | Effect |
|---|---|
| `W` / `↑` | move your paddle up (hold for continuous motion, tap for a nudge) |
| `S` / `↓` | move your paddle down |
| mouse move over the court | your paddle tracks the pointer |
| `SPACE` | serve · pause · resume · dismiss the match-over card |
| `R` | restart the match from 0–0 |

You are the amber paddle on the left; the CPU is cyan on the right. First to
**7** points. The ball accelerates 4.5 % on every paddle hit up to 680 px/s, and
the return angle is set by where on the paddle it lands, so an edge hit is a
sharp angle and a centre hit is flat.

The header reads out the live rally length, the longest rally of the match, and
the current ball speed; the pip rows under the score show progress to 7.

## What it exercises

**Language.** `state var` cells written with `set` from inside a `while`
sub-step loop; a collecting `for` with `continue` as a filter (the particle
update is one expression); record spread (`{...p, x: …}`) for immutable particle
updates; `config let` for the tunables; `elsif` chains as value expressions;
string interpolation; closures-free pure helpers (`predict_y`, `burst`) that
return new values rather than mutating.

**Host / petal-ui.** `dt()`-driven animation with a clamped, sub-stepped
integrator (so the ball cannot tunnel through a paddle at the slow headless
frame rate); `key_down` + `key_pressed` together, so the app is playable both by
a held key in a real window and by a single injected `POST /key` headlessly;
`mouse_x`/`mouse_y` edge detection; `clip`/`clip_none` to keep particles inside
the court; the full draw vocabulary — `draw_rect_rounded` with alpha for the
paddle glows, `draw_circle` for the ball, trail and sparks, `fill_triangle` for
the serve arrow, `draw_rect_outline`; styled `draw_text` records plus
`text_width` for exact centring and right-alignment.

**Debug server.** Every piece of logical state is mirrored into plain `let`
bindings (`obs_phase`, `obs_ball`, `obs_score`, `obs_rally`, `obs_you_y`,
`obs_cpu_y`, `obs_speed`, `obs_bits`) so the whole game is assertable from
`GET /state → panes[0].panel.values` without decoding pixels.

## Note on animation while headless

Garden puts a panel to sleep 10 s after the last input, and headless panels tick
at the ~200 ms poll rate rather than 60 fps. So driving the game from `curl`
advances it roughly one frame per injected event, and the match freezes 10 s
after you stop poking it. That is the documented panel scheduling model, not a
bug in the app — inject any key (`{"key":"x"}` is ignored by the game) to step
the simulation forward. In a real window it runs at 60 fps.

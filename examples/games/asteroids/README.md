# 05 — Asteroids

A complete game of Asteroids as a Garden panel, written entirely in Petal: a
ship with rotation, thrust and inertia on a wrapping toroidal field, rocks that
are generated as irregular polygons and split twice when shot, a bullet pool
with a cooldown and a cap, hyperspace, a shield window after each respawn, a
particle system (impact sparks, exhaust, and the ship's own three edges flying
apart when it dies), screen shake, and a six-phase UI
(title → play → paused / dead → sector clear / game over).

The point of this entry is **vector movement and rotation**: nothing on the
field is axis-aligned. Every rock is a list of radius multipliers rotated
rigidly frame to frame, the ship is a polyline rebuilt from its heading every
frame, and collision is circle-against-circle on positions that wrap.

## Viewport

Designed for **1100 × 780** logical pixels (which gives the panel pane
1088 × 708 inside Garden's chrome). Everything is laid out from
`screen_width()` / `screen_height()`, so other sizes work, but the type scale
and the footer chip row were tuned at that size.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/games/asteroids

# windowed
GARDEN_HEADLESS_SIZE=1100x780 ../../../garden/target/debug/garden --init layout.ptl

# headless + debug server (what the agent workflow uses)
GARDEN_HEADLESS_SIZE=1100x780 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

## Controls

| Input | Effect |
|---|---|
| `←` / `→`, `A` / `D` | rotate (hold for continuous motion, tap for a nudge) |
| `↑` / `W` | thrust |
| `SPACE` | fire (up to five shots in flight, 160 ms between shots) |
| `SHIFT` / `H` | hyperspace: jump somewhere random, briefly shielded, 2.2 s cooldown |
| `P` / `ESC` | pause · resume |
| `SPACE`, `ENTER` **or** left click in the field | launch · resume · next sector · play again |
| `R` | restart the run from sector 1 |

You have three lives. The ship keeps its momentum when you stop thrusting and
bleeds it off slowly, so the game is about managing drift, not steering. After
a respawn (and after a hyperspace jump) a ring around the ship marks the
shield window, during which rocks pass through you.

### Rocks

Three tiers. Shooting a rock splits it into two of the next tier down, which
inherit some of the parent's velocity; the smallest tier just breaks.

| Tier | Radius | Score | Splits into |
|---|---|---|---|
| large | 54 px | 20 | 2 medium |
| medium | 30 px | 50 | 2 small |
| small | 16 px | 100 | — |

Each sector starts with `3 + sector` large rocks spawned on the rim of the
field, never near the ship. Rock speed rises 9 px/s per sector, and clearing a
sector scores 250 + 100 per remaining life.

## What it exercises

**Language.** Records for every entity, updated immutably with spread
(`{...rk, x: …}`) inside collecting `for` loops, with `continue` as the
destruction filter for expired shots and sparks. A rock's silhouette is a list
of floats generated once by a collecting `for` and rotated every frame, so the
shape is stable without a mesh. Nested collecting `for` inside a `while`
sub-step loop; `var` accumulators (`spawned`, `survivors`) written from inside
the collision pass; `state var` for everything that persists; `config let` for
the tunables; `match` on the phase string as a value expression; a `for` over a
literal list (`for i in [2, 1, 0]`) for the title legend.

**Host / petal-ui.** `dt()`-driven simulation with a 12 ms sub-step so a
560 px/s shot cannot tunnel through a 16 px rock at the headless frame rate.
`draw_polyline` for every outline (the glow is the same path stroked wide at
low alpha under a 1 px stroke), `fill_polygon` for the concave-correct rock
fills, `fill_triangle` for the flickering exhaust flame, `draw_circle_outline`
for the shield ring, `draw_line` for the wreckage. Rocks straddling an edge
are drawn a second time on the opposite side so the torus reads as one.
`key_down` for the held controls (turn, thrust) and `key_pressed` for the
one-shot verbs (fire, warp, pause); headlessly, hold a key with
`POST /key {"key": "left", "op": "down"}` and release it with `"op": "up"`;
`mouse_pressed(0)` as a second launch verb; `clip`/`clip_none` to keep sparks
inside the field; styled `draw_text` records plus `text_width` for centring,
right-alignment and self-sizing badges. Every draw call uses the prelude's
colour-record overloads, so the whole app talks in palette records.

**Debug server.** Every piece of logical state is mirrored into plain `let`
bindings (`obs_phase`, `obs_level`, `obs_score`, `obs_lives`, `obs_rocks`,
`obs_shots`, `obs_bits`, `obs_ship`, `obs_ang`, `obs_speed`, `obs_safe`) so
the whole game is assertable from `GET /state → panes[0].panel.values` without
decoding pixels.

## Note on animation while headless

Garden puts a panel to sleep 10 s after the last input, and headless panels
tick at the ~200 ms poll rate rather than 60 fps. So driving the game from
`curl` advances it roughly one frame per injected event, and the run freezes
10 s after you stop poking it. That is the documented panel scheduling model,
not a bug in the app. `POST /tick` with a fixed `dt` is the deterministic way
to advance it; `--panel-wake` keeps it awake. In a real window it runs at
60 fps.

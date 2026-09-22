# 06 — Flappy Bird clone (Flappy)

A one-button flying game as a Garden panel, written entirely in Petal: a bird
under gravity that gets one upward impulse per flap, an endless procession of
pipe columns whose gaps wander from column to column and tighten as the score
climbs, a three-layer parallax backdrop (clouds, two ranges of hills, a
scrolling ground strip) that moves with the run, feathers on impact, a score
pop, medals, and a four-phase UI (title → play → dead → over, plus pause).

The point of this entry is **physics, procedural obstacles and scoring**: the
bird is a velocity integrated in 4 ms sub-steps against a clamped `dt()`, every
column is generated on demand from the previous one, and the score is the
count of columns whose trailing edge the bird has cleared.

## Viewport

Designed for **1000 × 740** logical pixels (which gives the panel pane
988 × 668 inside Garden's chrome). Everything is laid out from
`screen_width()` / `screen_height()`, so other sizes work, but the type scale,
the pipe spacing and the footer chip row were tuned at that size.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/games/flappy

# windowed
GARDEN_HEADLESS_SIZE=1000x740 ../../../garden/target/debug/garden --init layout.ptl

# headless + debug server (what the agent workflow uses)
GARDEN_HEADLESS_SIZE=1000x740 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Nothing is persisted; every launch starts at the title with a zero best.

## Controls

| Input | Effect |
|---|---|
| `SPACE`, `↑`, `W` **or** left click in the field | flap (one impulse); on the title card, start the run; on game over, back to the title |
| `P` / `ESC` | pause · resume |
| `R` | restart at the title |

Touching the ceiling bumps the bird back down without ending the run. The
ground and either half of a pipe end it; the bird then falls the rest of the
way on its own and the game-over card appears about a second later.

### Pipes and scoring

Columns are `pipe_spacing` (250 px) apart and scroll at 185 px/s. Each new
column's gap centre is the previous one's plus a random step of up to
±150 px, clamped to the field, so consecutive gaps are always reachable. The
gap height starts at 190 px and loses 4 px per point down to a floor of
128 px, so the difficulty curve is in the level, not the physics. One point
per column cleared. Medals on the game-over card: bronze at 5, silver at 15,
gold at 30.

Every knob is a `config let` at the top of `app.ptl` (`gravity`,
`flap_impulse`, `scroll_speed`, `pipe_spacing`, `pipe_w`, `gap_start`,
`gap_min`, `gap_shrink`, `max_fall`).

## What it exercises

**Language.** Records for every pipe and feather, updated immutably with
spread inside collecting `for` loops (`continue` filters expired feathers); a
`while` sub-step loop that also sets `t_left = 0.0` to leave early on death;
`filter` + `reduce` with lambdas to recycle the column that scrolled off and
spawn its replacement at the far end; `state var` for everything that
persists, read with `get` from the helpers (`make_pipe`, `hills`); `config let`
for the tunables; `match` on the phase string as a value expression; a
function declared after the top-level `let`s it reads (`rot`, `hills`) so it
is not hoisted above them.

**Host / petal-ui.** `dt()`-driven simulation clamped to 60 ms and cut into
4 ms slices, so a 720 px/s terminal-velocity fall cannot tunnel through a
26 px pipe cap at the headless frame rate. `request_frame()` while flying and
while feathers are live, so the animation does not stop under the 10 s sleep.
`draw_rect_gradient` for the sky, `draw_ellipse` for clouds, `fill_polygon`
for each hill range (one polygon per range, sampled every 18 px from three
summed sines), `fill_triangle` for the ground stripes and the beak,
`draw_ellipse_outline` for the wing, `clip_push`/`clip_pop` with a radius so
the field's rounded corners cut the pipes, `approach` from the prelude for
the tilt easing and `ease_out` for the score pop. Every draw call uses the
colour-record overloads. `key_pressed` for the one-shot verbs, `mouse_pressed`
with `point_in` as the second flap verb.

**Debug server.** Every piece of logical state is mirrored into plain `let`
bindings so the whole game is assertable from
`GET /state → panes[0].panel.values` without decoding pixels: `obs_phase`,
`obs_score`, `obs_best`, `obs_runs`, `obs_flaps`, `obs_bird` (`[x, y]`),
`obs_vy`, `obs_pipes`, `obs_next_gap` (`[x, gap_y, gap_h]` of the first column
the bird has not yet passed), `obs_scroll`, `obs_cause` (`"pipe"` or
`"ground"`).

A whole run is a deterministic script: `POST /panel/reset`, `POST /seed
{"seed": 42}`, then `POST /key {"key":"space"}` and `POST /tick {"n": 4,
"dt": 0.016}` in a loop, flapping whenever `obs_bird[1]` is below
`obs_next_gap[1]`. That controller scores 31 with seed 42 before the run is
cut off, and the same script gives the same numbers every time.

## Note on animation while headless

Garden puts a panel to sleep 10 s after the last input, and headless panels
tick at the ~200 ms poll rate rather than 60 fps. A run that seems to have
stopped has gone to sleep, not hung; any key wakes it. Drive the game with
`POST /tick` rather than wall-clock time.

## Known limits

- The chip caption `SPACE / ↑ / W` relies on the arrow glyph being present in
  the chrome (monospace) face; it is. The embedded `ui` face lacks it, so the
  caption stays in the default face.
- `text_width` on a style record whose `size` changes every frame (the score
  pop) is re-measured each frame; fine at one label, not for a whole layout.
- A pipe column that is generated while the panel is asleep is not
  generated: columns spawn only on a played frame, which is what the
  `request_frame()` call guarantees while flying.

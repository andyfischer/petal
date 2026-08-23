# 02 — Breakout

A complete game of Breakout as a Garden panel, written entirely in Petal: a
99-cell brick grid with per-brick armour, grid-indexed continuous collision,
four generated wall layouts, four catchable power-ups (including multiball, so
up to nine balls run at once), a combo multiplier, a particle system, a
per-ball comet trail, screen shake, and a five-phase UI
(title → serve → play → paused → level clear / game over).

The point of this entry is **population**: every frame spawns and destroys
objects — bricks leave the grid, sparks are born and expire, pills fall and are
caught or lost, balls split and drain away — and all of it is plain Petal lists
and `state var` cells.

## Viewport

Designed for **1100 × 780** logical pixels (which gives the panel pane
1088 × 708 inside Garden's chrome). Everything is laid out from
`screen_width()` / `screen_height()`, so other sizes work, but the type scale,
the brick cell size and the footer chip row were tuned at that size.

## Run it

```bash
cd examples/games/breakout

# windowed
GARDEN_HEADLESS_SIZE=1100x780 ../../../garden/target/debug/garden --init layout.ptl

# headless + debug server (what the agent workflow uses)
GARDEN_HEADLESS_SIZE=1100x780 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s localhost:$PORT/screenshot -o shot.png
```

## Controls

| Input | Effect |
|---|---|
| `←` / `→`, `A` / `D` | move the paddle (hold for continuous motion, tap for a nudge) |
| mouse move over the court | the paddle tracks the pointer, eased |
| `SPACE` **or** left click in the court | launch · pause · resume · next level · play again |
| `R` | restart the run from level 1 |

You have three lives. Where the ball lands on the paddle sets the return
angle — an edge hit is a sharp angle, a centre hit is flat — and the ball
speeds up 1.8 % on every paddle hit, up to 690 px/s.

### Bricks

Rows are coloured from a five-stop ramp, hot at the top and cool at the
bottom, and hue tells you the value: the top three rows take **three** hits and
score 75, the middle three take two and score 50, the bottom three take one and
score 25. Armour reads as a fill level — a chipped brick is painted back only
part-way in its own hue, with a hairline where the armour ran out.

Breaking bricks without touching the paddle builds a **combo**; every six
bricks adds ×1 to the score multiplier, and the multiplier resets the moment
the paddle returns the ball.

### Power-ups

17 % of broken bricks drop a pill. Catch it with the paddle:

| Pill | Effect |
|---|---|
| `WIDE` | paddle grows from 116 px to 182 px for 11 s |
| `MULTI` | every live ball splits in two (capped at nine balls) |
| `SLOW` | the simulation runs at 62 % speed for 11 s |
| `+LIFE` | one extra life, up to six |

Active effects show as badges on the top rail of the court; losing a ball
clears them.

### Levels

Clearing a wall advances the level and scores 250 + 100 per remaining life.
The wall is generated, not stored — `cell_hp(level, row, col)` picks one of
four patterns by `level % 4`: **SOLID WALL**, **OPEN ARCH**, **WEAVE**,
**PILLARS**. Ball speed starts 22 px/s higher on each level.

## What it exercises

**Language.** A flat `ROWS * COLS` list of hit points mutated in place with
`set bricks[idx] = hp` — index-target `set` on a `state var`, from inside a
`for` nested in a `for` nested in a `while`. Collecting `for` loops with
`continue` as a filter do all four destruction passes in one expression each
(dead balls, expired particles, faded trail points, landed pills). Record
spread (`{...p, x: …}`) for immutable particle updates; `config let` for the
tunables; `elsif` chains as value expressions; string interpolation in every
banner; functions returning records (`mix`, `row_color`, `power_by_key`); a
`var` accumulator written from inside a collecting `for` (`caught`).

**Host / petal-ui.** `dt()`-driven simulation with a clamped 4 ms sub-step
integrator, so the ball cannot tunnel through a brick at the slow headless
frame rate. Collision is O(1) in the grid: the ball's bounding box is mapped
straight onto cell indices, so only the two-to-four bricks it could touch are
tested, whatever the population. `key_down` + `key_pressed` together, so the
app is playable both by a held key in a real window and by a single injected
`POST /key` headlessly; `mouse_pressed(0)` as a second launch verb;
`clip`/`clip_none` to keep sparks inside the court; `draw_rect_rounded` with
alpha for glows and pills, `draw_circle` for balls, trail and sparks,
`draw_rect_outline`; styled `draw_text` records plus `text_width` for exact
centring, right-alignment and self-sizing pills and cards.

**Debug server.** Every piece of logical state is mirrored into plain `let`
bindings (`obs_phase`, `obs_level`, `obs_score`, `obs_lives`, `obs_left`,
`obs_balls`, `obs_drops`, `obs_bits`, `obs_combo`, `obs_padx`, `obs_speed`,
`obs_ball0`, `obs_powers`) so the whole game is assertable from
`GET /state → panes[0].panel.values` without decoding pixels.

## Note on animation while headless

Garden puts a panel to sleep 10 s after the last input, and headless panels
tick at the ~200 ms poll rate rather than 60 fps. So driving the game from
`curl` advances it roughly one frame per injected event, and the run freezes
10 s after you stop poking it. That is the documented panel scheduling model,
not a bug in the app — inject any key (`{"key":"z"}` is ignored by the game) or
a mouse move to step the simulation forward. In a real window it runs at
60 fps.

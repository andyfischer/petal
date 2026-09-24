# 09 — Memory matching game (Pairs)

A memory game drawn entirely by a Petal script inside a Garden panel. Cards
are dealt face down onto a table. Flip two per move: a match stays up, and a
miss stays up for a beat before turning back. There are three board sizes.
Every face is one of sixteen glyphs drawn from vector primitives (sun, moon,
star, heart, gem, drop, bolt, bloom, leaf, crown, peak, eye, frost, clover,
tide, ringed planet). The flip is a real half-turn: the card lifts, squashes
to its edge and opens on the other side. A side rail shows the round's stats,
a collection of the pairs found so far and a personal best for each board,
which is kept in the panel store.

The entry is about **card components, animation and delayed state changes**.
Almost everything that happens here happens *later* than the input that caused
it. The match is decided on the click, but it pops only after the flip lands.
A miss waits `FLIP_T + MISS_VIEW` before turning back. The result screen opens
`WIN_DELAY` after the last pair lands. The deal staggers each card's flight,
and a peek holds the board open on its own timer.

## Run it

The quickest way is `./launch.sh` in this directory. It finds the `garden`
binary and sets the viewport, and passes extra arguments through, e.g.
`./launch.sh --headless --debug-port 0`. By hand:

```bash
cd examples/games/memory
GARDEN_PANEL_STORE_DIR=/tmp/pairs-store GARDEN_HEADLESS_SIZE=1280x850 \
  ../../../garden/target/debug/garden \
      --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Designed for a **1280×850** viewport (the headless default), which gives the
panel a 1268×778 pane. The rail is fixed at 296 px. The card size is solved
each frame from the space left and the board's columns and rows, so other
sizes reflow.

Personal bests and the last difficulty chosen are saved to the panel store.
Launch tests with `GARDEN_PANEL_STORE_DIR` pointing at a scratch directory, or
they start from whatever the last run saved. The **Reset** link on the
Personal bests card clears the saved records.

The panel keeps itself awake with `request_frame()` while anything is moving
or the round's clock is running. Once a board sits still with the clock
stopped (before the first flip, or on the result screen), Garden puts the
panel to sleep after 10 s. That is not a hang: the next input wakes it.

## How it plays

| Board | Grid | Pairs | 3 stars at | 2 stars at |
|---|---|---|---|---|
| Easy | 4 × 3 | 6 | ≤ 10 moves | ≤ 15 moves |
| Classic | 6 × 4 | 12 | ≤ 21 moves | ≤ 30 moves |
| Expert | 8 × 4 | 16 | ≤ 28 moves | ≤ 40 moves |

A move is two cards. Each board is dealt from a random subset of the sixteen
designs, so the smaller boards vary from round to round. The clock
starts on the first flip and stops the moment the last pair is found. A best
is the fewest moves on that board, with time breaking ties.

**Peek** (once per round) shows every card for 1.6 s and costs 2 moves. It
also breaks the streak.

A third pick while a missed pair is still showing turns that pair back at
once, so a fast player never waits on the animation.

## Controls

**Board**

| Input | Effect |
|---|---|
| click a card | flip it |
| arrow keys | move the selection ring (appears on first use; the mouse hides it) |
| `space` / `return` | flip the selected card; on the result screen, play again |
| `p` | peek |
| `n` | new game on the same board |
| `1` / `2` / `3` | new game on Easy / Classic / Expert |

**Chrome**

| Input | Effect |
|---|---|
| Easy / Classic / Expert | switch board (deals a new game) |
| Peek (+2) | the peek; disabled once used and outside play |
| New game | re-deal the current board |
| result card: Play again / Try … | replay, or move up a board |
| Personal bests: Reset | clear the saved records |

## What it exercises

**Language**

- A card as a plain record (`{d, up, flip, matched, t_match, t_miss, t_deal,
  hv, sparked}`), updated immutably with spread inside a collecting `for`
  every frame. A small `put(xs, i, v)` does single-card edits, and
  `turn_back` rebinds a copy of the list with index assignment.
- Delayed effects as *timestamps*, not timers. A match records
  `t_match = clock + FLIP_T`, and every visual that follows (the pop, the gold
  ring, the settle to the matched look, the sparks, the collection tile
  popping in) is a pure function of `clock - t_match`. The only real
  countdowns are the miss `hold` and the `peek_t`.
- Glyphs as unit geometry built once at the top level by collecting `for`
  (`U_STAR`, `U_HEART`, a parametric teardrop, `almond` for the leaf and eye,
  `ring_band` for the planet, filled ribbons for the waves), drawn through one
  transform record `T = {cx, cy, s, sx, bg}`. The flip's squash is just
  `T.sx`, so every glyph, the back's lattice and the emblem narrow together.
- `match` with `do … end` arms dispatching the sixteen glyphs. `for … do
  continue end` as a filter for particles, and `filter` with lambdas for
  floaters and the deck count.
- Fisher–Yates `shuffled` with `random_int`, a `state var` per piece of game
  state, `get`/`set` in the helper functions that change it (`new_game`,
  `flip_card`, `start_peek`, `finish_game`), and `let` for every per-frame
  derivation.

**Host / petal-ui / bloom**

- Draw surface: `fill_polygon` (concave: the star, bolt, crown and mountain),
  `draw_ellipse` (every disc, so it can squash), `draw_polyline` for the
  snowflake and leaf rib, `fill_poly` quads for rotated confetti,
  `draw_rect_gradient_rounded` for the card backs and progress bar,
  `draw_circle_gradient` for the table glow, `draw_shadow` growing with the
  card's lift, and nested `clip_push`/`clip_pop` (rounded) for the lattice and
  for keeping confetti on the table.
- `bloom.segmented` (springing thumb), `bloom.button` (primary, icons,
  `disabled` fading) and `bloom.link`, themed with `bloom.theme_set`.
  `text_layout.draw_text_line` centers the chip and banner labels. The prelude
  supplies `ease_in_out`/`ease_out`/`approach`, `mix`/`over` and `ellipsize`.
- `request_frame()` only while something moves, `panel_store_get`/`set`
  (including `nil` to delete) with `json_parse`/`json_stringify`.

**Debug server.** The assertable values in `panes[0].panel.values`:
`obs_phase` (`deal`/`play`/`won`), `obs_level`, `obs_deck` (the design id at
every position, which is enough to solve the board from a script),
`obs_matched`, `obs_open`, `obs_face_up`, `obs_moves`, `obs_found`,
`obs_pairs`, `obs_misses`, `obs_streak`, `obs_best_streak`, `obs_time`,
`obs_peeks`, `obs_peeking`, `obs_cursor`, `obs_hover`, `obs_stars`,
`obs_record`, `obs_records`, `obs_games`, `obs_note`. The geometry `gx0`,
`gy0`, `cw`, `ch`, `GAP` and `COLS` are there too, so a harness can compute
any card's centre.

A deterministic run: `POST /panel/reset`, `POST /seed {"seed":42}`, then press
`n` (see Known limits for why), `POST /tick {"n":120,"dt":0.016}` to finish
the deal, then click cards with `/tick` between them. It was verified that
way through a miss, a third-pick turn-back, a peek, a clear on Easy and on
Classic, the result screen's buttons, a process restart restoring the bests,
and Reset clearing them, with `status_error` null throughout.

## Known limits

- **`/seed` cannot pin the first deal.** `POST /panel/reset` runs the panel's
  first frame immediately, and this app deals on that frame, before a
  following `POST /seed` can take effect. Seeding before the reset does not
  help either. A test that needs a known board resets, seeds, and then presses
  `n` to re-deal from the seeded stream.
- The copy avoids `·`, `—` and `×` ("6 x 4", commas instead of dots).
  [AUTHORING.md](../../AUTHORING.md) says the embedded `ui` face has no glyph
  for the first two, while
  [petal-graphical-panels.md](../../../garden/docs/petal-graphical-panels.md)
  says missing glyphs fall back to a system face. It was not worth finding out
  which is right for a board label.
- Thick strokes fray. A `draw_polyline` several pixels wide with many short
  segments (the tide glyph) or two stroked halves meeting (the planet's ring)
  showed ragged edges at the joins. Both glyphs are filled ribbons
  (`fill_polygon` of an offset curve) instead.
- A flip is faked by scaling x, since there is no transform stack. Every
  primitive goes through the glyph transform by hand, and circles are
  `draw_ellipse` so they can narrow.
- Real input frames advance the flip by wall-clock `dt`, so a screenshot a
  few milliseconds after a click already shows most of the half-turn. To
  inspect the squash, raise `FLIP_T` (a `config let`) and step with `/tick`.

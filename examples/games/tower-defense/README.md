# 10 — Tower Defense mini-game (Thornwall)

A complete tower-defence game as a Garden panel, written entirely in Petal.
Pests walk two gated routes that merge on their way to a greenhouse; you plant
four kinds of tower on the plots beside the path, pick what each one aims at,
upgrade or sell them, and hold out through twelve scripted waves that end in a
pair of Hornet Queens and a sixty-aphid swarm.

The point of this entry is **paths, many entities, targeting and simulation**:

- **Paths.** A route is a polyline with a running-length table, and every pest
  is just a distance along it. `route_at(route, d)` turns that distance into a
  position and a heading, and the same function places the drifting flow marks
  on the path and gives the Puffball mortar its lead: it aims at where the
  target *will* be by advancing the pest's distance by speed × flight time.
- **Many entities.** Six independent pools (pests, darts, shells, lightning
  bolts, rings, sparks) plus rising gold labels, all immutable records
  rebuilt by collecting `for` loops. Wave 12 puts 74 pests through the gates;
  the scripted playthrough below peaks at 41 on the board at once, and an
  unopposed rush of nine waves was run at 244 with no trouble.
- **Targeting.** Each tower has one of five priorities (first, last, strong,
  weak, near), all expressed as "largest score wins" in one `pick_target`.
  Frostcap selects everything in reach, Puffball everything in a splash radius
  with distance falloff, and Stormvine builds a nearest-neighbour chain.
- **Simulation.** A fixed-step loop (slices of at most 20 ms) over a spawn
  queue, movement with slows that pests resist to different degrees, flat
  armour, a hit list applied once per slice so every tower aims at the same
  snapshot, an economy, a speed multiplier and a pause.

## Run it

```bash
cd examples/games/tower-defense
../../../tools/run-example.ts                             # windowed
../../../tools/run-example.ts --headless --debug-port 0   # headless + debug server
```

By hand, for the agent workflow:

```bash
(nohup ../../../tools/run-example.ts --headless --debug-port 0 > /tmp/td.log 2>&1 < /dev/null &)
PORT=$(grep -o '127.0.0.1:[0-9]*' /tmp/td.log | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o /tmp/td.png
```

Designed for a **1200 × 800** viewport (`GARDEN_HEADLESS_SIZE`, set by
`run-example.ts`), which gives the pane 1188 × 728: a 21 × 14 board of 40 px cells
on the left and a 280 px sidebar on the right. The board scales its cell size
to a smaller pane, but the sidebar needs about 560 px of height (see Known
limits). The app stores nothing, so no `GARDEN_PANEL_STORE_DIR` is needed.

Garden puts a panel to sleep 10 s after the last input, and a headless panel
ticks at about 5 fps rather than 60. The app calls `request_frame()` while a
wave is running, so a wave in progress keeps going; a build phase that stops
animating is the documented scheduling model, not a hang. A paused game asks
for no frames at all, so it sleeps too. Use `POST /tick` to advance it
deterministically.

To check the script without launching anything, name the Garden host so that
`request_frame` resolves (the default profile does not know it):

```bash
./ts/bin/run-petal.ts check --strict --host garden examples/games/tower-defense/app.ptl   # from the repo root
```

## Controls

### Keys

| Key | Effect |
|---|---|
| `1` `2` `3` `4` | arm Thorn / Frostcap / Puffball / Stormvine for planting (press again to disarm) |
| `ESC` | disarm, or deselect the selected tower |
| `U` | upgrade the selected tower (three levels) |
| `X` | sell the selected tower for 70% of everything spent on it |
| `T` / `SHIFT`+`T` | cycle the selected tower's targeting forwards / backwards |
| `SPACE` / `RETURN` | begin · send the next wave · call the next wave early · play again |
| `F` | speed: 1× → 2× → 3× |
| `P` | pause · resume |
| `R` | restart from the title card |

### Mouse

| Gesture | Effect |
|---|---|
| click a shop card | arm that tower (click again to disarm) |
| move over the board while armed | a ghost tower with its reach; red when the plot is path, scenery, taken or unaffordable |
| click a plot while armed | plant; the tower stays armed while you can afford another |
| click a tower | select it: reach ring, stats, targeting, upgrade and sell in the sidebar |
| hover a tower | a faint reach ring |
| click empty ground | deselect |
| right click | disarm, or deselect |
| click a `TARGETS` segment | set that priority directly |
| click `UPGRADE` / `SELL` | the same as `U` / `X` |
| click `SEND WAVE` / `CALL WAVE EARLY` | the same as `SPACE`; the button reads `BEGIN`, `PLAY AGAIN` or `TRY AGAIN` on the title and end cards |
| click anywhere on the title or end card | begin / play again |

When the game ends the selection is dropped and the lower sidebar card turns
into a debrief: pests stopped, pests through, towers planted, the most pests
on the path at once, and the score.

### The towers

| Tower | Cost | Upgrades | What it does |
|---|---|---|---|
| Thorn | 60 | 45, 85 | a homing dart at one pest, twice a second and faster with levels |
| Frostcap | 90 | 70, 115 | a pulse that nicks and slows every pest in reach for 1.8 s |
| Puffball | 120 | 95, 150 | a lobbed shell aimed ahead of its target, full damage at the centre and half at the rim |
| Stormvine | 150 | 110, 175 | an instant arc that leaps to the nearest pest within 1.7 cells, 3 / 4 / 6 pests, a quarter weaker each leap |

### The pests

| Pest | Health | Speed | Bounty | Notes |
|---|---|---|---|---|
| Aphid | 14 | 2.3 | 3 | comes in swarms |
| Beetle | 46 | 1.45 | 7 | the standard column |
| Wasp | 26 | 3.1 | 6 | fast |
| Snail | 150 | 0.85 | 16 | armour 4 off every hit; costs 2 lives; feels 80% of a slow |
| Hornet Queen | 1000 | 0.78 | 150 | armour 3; costs 8 lives; feels 40% of a slow |

Health is scaled per wave (×1.0 on wave 1 to ×3.0 on wave 12); speed is in
cells per second. You start with 230 gold and 20 lives. Clearing a wave pays
25 + 5 × wave; calling the next wave while one is still on the board pays
10 + 4 × wave at once, and stacks the two waves on the path.

## What it exercises

**Language.** Records for every entity, updated immutably with spread inside
collecting `for` loops, with `continue` as the removal filter for leaked pests,
spent darts, landed shells and dead effects. Accumulators that carry out of a
collecting loop (`leaked`, `hits`, `credit`). A `while` sub-step loop holding a
nested `while` for the spawn queue. `match` on the targeting mode as a value
expression inside a hot loop. `state var` for everything that persists, read
with `get` from the three functions that touch it (`say`, `new_game`,
`send_wave`). `config let` tunables. Module-level geometry (`ROUTES`, `grid`,
`CELL`) recomputed every frame from literals, with the helpers that read it
declared below it. `sort_by`, `flat`, `filter`, `reduce`, `contains`, `slice`,
`clamp`, `atan2`, `fract`, `round(x, places)`, `wrap_px`, nested index writes
(`g[r][c] = 1`) to build the buildability grid, and named colour literals
throughout.

**Host / petal-ui.** One wide `draw_polyline` per route for the path bed
(round joins for free), `draw_polyline` for bolts and flow marks,
`fill_polygon`, `fill_triangle`, `draw_ellipse`, `draw_circle_outline`,
`draw_polygon_outline`, `draw_rect_gradient` with per-stop alpha for the
damage wash, `draw_shadow`, and a rounded `clip_push`/`clip_pop` so pests
emerge from under the board's edge. One `draw_tower` routine draws the board,
the placement ghost and the shop icons at three scales. Styled `draw_text`
records in the embedded `ui` face with `text_width` for right-alignment,
centring and self-sizing chips. `hovered`, `point_in`, `mouse_pressed(0)` and
`(1)`, `key_pressed`, `mod_shift`, `request_frame`, `ease_out`, `lerp_color`.

**Debug server.** The game is assertable without pixels from
`GET /state → panes[0].panel.values`:

| Value | Meaning |
|---|---|
| `obs_phase` | `title` / `build` / `wave` / `won` / `lost` |
| `obs_paused`, `obs_speed` | pause flag, 1–3 |
| `obs_wave`, `obs_gold`, `obs_lives`, `obs_score` | the header numbers |
| `obs_creeps`, `obs_queue` | pests on the board, spawns still to come |
| `obs_towers`, `obs_darts`, `obs_shells`, `obs_bolts`, `obs_bits` | pool sizes |
| `obs_kills`, `obs_leaks`, `obs_spawned`, `obs_built`, `obs_peak` | running totals |
| `obs_build_sel` | armed tower type, or -1 |
| `obs_sel`, `obs_sel_lvl`, `obs_sel_mode` | selected tower id, its level and targeting name |
| `obs_hover` | board cell under the pointer `[col, row]`, or `[-1, -1]` |
| `obs_clock` | simulated seconds of wave time |
| `obs_cell`, `obs_board` | cell size and board rect, for turning a cell into a click |
| `toast` | the last message (a `state var`, so it is readable directly) |

A board cell `(c, r)` is clicked at pane-local
`(obs_board[0] + c*obs_cell + obs_cell/2, obs_board[1] + r*obs_cell + obs_cell/2)`,
plus the pane origin from `panes[0].rect`.

### A deterministic playthrough

The frame delta comes from `time()` rather than `dt()`, so once the panel is on
the debug server's virtual clock the idle frames Garden runs between requests
add nothing and a tick script replays exactly. Tick once after the reset to
enter virtual time before sending input:

```bash
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{"seed":42}'
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":1,"dt":0.016}'
# SPACE (begin), F F (3x), then before each wave spend down this list in order
# while gold allows, press SPACE, and tick {"n":300,"dt":0.016} until obs_phase != "wave".
```

Build order used (`b` = arm key then click the cell, `u` = select the cell and
press `U`): b1 (5,5) · b1 (5,7) · b1 (3,7) · b2 (3,5) · b3 (8,5) · b1 (10,3) ·
u (5,5) · b4 (10,5) · u (8,5) · b3 (13,4) · u (5,7) · b2 (13,3) · b4 (15,9) ·
u (10,5) · b3 (16,8) · u (5,5) · u (13,4) · u (15,9) · u (8,5) · b1 (17,9) ·
u (16,8) · u (10,5) · u (13,4) · u (15,9) · b1 (15,3) · u (15,3) · u (15,3) ·
b4 (13,6).

That run was repeated three times with identical per-wave results and ends
`obs_phase: "won"` with 9 lives, 690 gold, 14 towers, 431 pests stopped, 4
through and a score of 28240: one pest slips by in wave 2, two in wave 4, and
the wave 9 queen costs 8 lives. For the other ending, seed 7 with a single
Thorn at (5,5) and waves 1–3 sent back to back reaches `obs_phase: "lost"`
during wave 3.

## Known limits

- **`dt()` is not on the virtual clock.** After a `POST /tick`, the frames
  Garden runs by itself (idle polls, the frame behind each injected key or
  click) still hand the script a wall-clock `dt()`, so a simulation stepped by
  `dt()` drifts between requests and two runs of the same tick script differ.
  The workaround here is to derive the frame delta from `time()`, which is
  virtual. Before the first `/tick` the panel is still on the wall clock, hence
  the single warm-up tick in the recipe above.
- **Fixed layout height.** The sidebar is laid out for the designed pane. At
  1000 × 680 the board scales down cleanly (30 px cells) but the scouting
  report overlaps the send button, and the footer drops its status text when
  the key chips would run into it.
- **Kill credit goes to the last hit in a slice.** When two towers finish a
  pest in the same 20 ms slice, only one gets the kill on its card.
- **Calling waves early is unlimited.** Every `SPACE` during a wave pays its
  bounty and stacks another wave, so mashing it is a fast way to lose rich.
- **No rotated text**, so the gate labels sit above their paths rather than
  along them.
- **No audio**, and nothing is saved between runs: `R` or a restart of Garden
  begins again from the title card.

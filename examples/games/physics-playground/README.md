# 12 — Physics playground

A 2D physics sandbox as a Garden panel, written entirely in Petal. Discs fall,
collide, roll and stack; rigid rods and springs join them into pendulums,
chains, bridges, trusses and soft wheels; pins nail them to the stage. You can
grab anything and throw it, launch balls from a slingshot, draw new links
between bodies, cut links with a stroke of the mouse, and tip or switch off
gravity while it all runs. Six seeded stages show the solver off; the sixth is
empty and yours.

The point of this entry is **dragging, gravity, collisions and constraints**:

- **Dragging.** Four different gestures share one stage. *Grab* is a velocity
  servo on the body under the pointer, so a dragged pendulum bob still obeys
  its rod and a dragged ball still pushes the pile, and letting go throws it.
  *Ball* is a slingshot with a predicted flight path. *Rod* and *Spring* draw
  a link from one body to another (an end left on open stage gets a pinned
  anchor). *Cut* severs every link the pointer crosses between two frames.
  The sliders and the gravity compass are drags too.
- **Gravity.** Strength from 0 to 2 g and any direction, live: drag the
  compass, tap an arrow key to tip the stage, or press `0` for free fall.
- **Collisions.** Disc against disc, wall and static bar, with restitution,
  Coulomb friction and spin. Friction acts on the sliding speed of the rims,
  so a ball rolls down a ramp instead of skating, and every disc carries a
  spoke so you can see it turn. A sort-and-sweep broadphase runs once a frame.
- **Constraints.** Rigid rods solved by sequential impulses with a warm start
  (a loaded 19-plank bridge holds its length to under 0.1 px), springs that
  ring at a chosen frequency whatever they carry, and pins.

The solver is `physics.ptl`, a module with no drawing or input in it; the
stages are `scenes.ptl`; `app.ptl` is input, chrome and drawing. The world is
one immutable record, so undo is a list of earlier worlds.

## Run it

```bash
cd examples/games/physics-playground
./launch.sh                                  # windowed
./launch.sh --headless --debug-port 0        # headless + debug server
```

By hand, for the agent workflow:

```bash
(nohup ./launch.sh --headless --debug-port 0 > /tmp/pp.log 2>&1 < /dev/null &)
PORT=$(grep -o '127.0.0.1:[0-9]*' /tmp/pp.log | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o /tmp/pp.png
```

Designed for a **1240 × 820** viewport (`GARDEN_HEADLESS_SIZE`, set by
`launch.sh`), which gives the pane 1228 × 748 and the stage 828 × 604 at pane
position (104, 84). The stages are built from the stage size, so they follow a
resize; the sidebar wants about 600 px of height. The app stores nothing, so
no `GARDEN_PANEL_STORE_DIR` is needed.

Garden puts a panel to sleep 10 s after the last input, and a headless panel
ticks at about 5 fps rather than 60. The app calls `request_frame()` while
anything is moving, so a running stage keeps running. It stops asking once the
world has been still for 0.7 s (the stage shows an `AT REST` badge and the
solver is skipped entirely) or while paused, and then the panel sleeps: that is
the scheduling model, not a hang. Any click, key or slider wakes it. Use
`POST /tick` to advance it deterministically.

To check the scripts without launching anything:

```bash
./ts/bin/run-petal.ts check --strict --host garden examples/games/physics-playground/app.ptl   # from the repo root
```

The solver also runs with no host at all, which is how it was tuned:

```bash
cat > /tmp/pendulum.ptl <<'EOF'
import physics as ph
let env = {gx: 0.0, gy: 980.0, bounce: 0.5, fric: 0.4, drag: 0.0, w: 800.0, h: 600.0, grab: -1, tx: 0.0, ty: 0.0}
let wd = ph.add_ball(ph.add_pin(ph.empty_world(), 400, 100, 5, 6), 600, 100, 20, 0)
wd = ph.add_link(wd, 0, 1, ph.ROD, 0.0, -1.0)
for i in range(0, 120) do wd = ph.step(wd, 0.016, env).world end
print(wd.x[1], wd.y[1], ph.audit(wd))
EOF
./ts/bin/run-petal.ts run -I examples/games/physics-playground /tmp/pendulum.ptl
```

## Controls

### Mouse, on the stage

| Tool | Gesture | Effect |
|---|---|---|
| any | hover a body | a ring round it |
| any | right-click a body | pin it, or free it |
| any | right-click open stage | cancel the gesture in progress, or deselect |
| Grab | drag a body | it follows the pointer as far as its links and neighbours allow; release throws it |
| Grab | click a body / open stage | select it (the sidebar shows radius, mass, speed, links) / deselect |
| Ball | click | drop a ball of the chosen size |
| Ball | press, drag back, release | launch it the opposite way at 5 px/s per pixel of pull (measured to the stage edge if the pointer leaves it); dots show the first 0.7 s of flight |
| Ball | wheel | change the size |
| Rod, Spring | drag from a body to a body | join them at their current distance |
| Rod, Spring | drag between a body and open stage | the open end gets a small pinned anchor |
| Pin | click a body | pin it, or free it |
| Cut | click a body or a link | remove it (a body takes its links with it) |
| Cut | drag across links | cut every one the stroke crosses; the stroke leaves a short red trail |

### Mouse, on the chrome

| Control | Effect |
|---|---|
| scene tabs | load Cradle, Bridge, Pegboard, Wrecker, Jelly or Sandbox |
| tool rail | arm Grab, Ball, Rod, Spring, Pin or Cut |
| `SIZE` dots | ball radius: 8, 14, 22 or 32 px |
| gravity compass | drag the arrow round; it snaps to 15° |
| `GRAVITY` | 0 to 2.00 g |
| `BOUNCE` | restitution, 0 to 1.00 |
| `FRICTION` | Coulomb μ, 0 to 1.00 |
| `AIR DRAG` | fraction of velocity lost per second, 0 to 0.50 |
| `PAUSE` / `RUN`, `STEP` | stop and start time; advance one 1/60 s frame |
| `1×` `½` `¼` | slow motion |
| `VECTORS` `TRAILS` `CONTACTS` | velocity arrows; motion trails on the larger bodies; a dot at every contact |
| `PIN` / `FREE`, `DELETE` | act on the selected body |

### Keys

| Key | Effect |
|---|---|
| `1`–`6`, or `G` `B` `L` `S` `P` `X` | Grab, Ball, Rod (link), Spring, Pin, Cut |
| `TAB` / `SHIFT`+`TAB` | next / previous scene |
| `R` | reset the scene |
| `SPACE` | pause · run |
| `N` or `.` | pause and advance one frame |
| `F` | cycle 1× → ½ → ¼ |
| `←` `↑` `↓` `→` | point gravity that way |
| `0` | zero gravity, and back |
| `Z`, `U` or `CMD`+`Z` | undo the last edit (spawn, link, cut, pin, delete, clear), up to 24 deep. Undo restores the whole world as it was at that edit, positions and the Pegboard's dispenser included, so it also rewinds whatever has moved since |
| `C` | clear every body that is not pinned |
| `BACKSPACE` / `DELETE` | delete the selected body |
| `-` `=` (or `[` `]`) | smaller / larger ball |
| `V` `T` `K` | toggle vectors, trails, contacts |
| `ESC` | cancel the gesture in progress (a launch, a link or a cut is abandoned; a grabbed body is let go where it is, at rest, with no throw), or deselect |

### The scenes

| Scene | What is in it | What to try |
|---|---|---|
| Cradle | five bobs on rods, bounce 1.00, and a double pendulum | watch one bob in, one bob out; drag two bobs back; press `↑` |
| Bridge | 19 rod planks between two pins, two sprung weights, three loads | cut a plank; drag a load across the deck |
| Pegboard | 95 pegs, a dispenser that drops 56 balls, ten counted bins | press `←` halfway through; raise `BOUNCE` |
| Wrecker | a dense ball on a nine-link chain, and a chocked 5-4-3-2-1 pyramid that the first swing scatters, sending ten of the fifteen to the floor | grab the ball and swing it yourself; cut the chain mid-swing |
| Jelly | two spring-laced wheels, a six-rod truss, two ramps | squeeze a wheel against the floor; cut its spokes |
| Sandbox | nothing | build something |

## What it exercises

**Language.** A three-file program: `import physics as ph` and
`import scenes as sc`, with `export fn` / `export let` drawing the line
between the solver's surface and its internals. The world as a
structure-of-arrays record of float lists, advanced by one long function that
rebinds local lists by index (`vx[i] = …`) inside nested `for` and `while`
loops, with `continue` and `break` doing the culling. Flat interleaved lists
(`[i, j, nx, ny, …]`) for contacts, where a list of records would cost a
field read per use. Immutable values doing real work: undo is
`undo = append(undo, world)` and nothing else. Collecting `for` loops with
`continue` as the filter (`without`, `remove_body`), spread to rebuild records,
`config let` tunables, `state` for everything that persists, written only at
module scope so nothing needs `var`/`set`/`get`. An intent block: keys and
widgets set plain `let` flags (`want_scene`, `want_pin`, `want_remove`) and one
place carries them out. `reduce`, `flat`, `split`, `slice`, `clamp`, `atan2`,
`round(x, places)`, `degrees`, `index_of`, `last`, `drop_last`, and colour
literals in lists.

**Host and prelude.** `time()` as the frame clock, `request_frame()`,
`claim_key`, held and edge mouse reads on two buttons, the wheel, `mod_shift`
and `mod_cmd`. `draw_polyline` for springs and trails, `fill_polygon` for the
energy chart, `fill_triangle`, `draw_circle_outline` and `draw_line` with
alpha and width, rounded rects and outlines, `clip`, styled text in the `ui`
face measured with the same style record it is drawn with. The sliders, the
compass, the buttons and the tool glyphs are all drawn here from primitives.

**Debug-server values** (`/state` → `panes[0].panel.values`):

| Value | Meaning |
|---|---|
| `obs_scene`, `obs_tool` | `"Cradle"` …, `"GRAB"` … |
| `obs_bodies`, `obs_pins`, `obs_rods`, `obs_springs` | what the world holds |
| `obs_contacts` | contacts solved in the last sub-step |
| `obs_time`, `obs_steps`, `obs_subs` | simulated seconds, sub-steps so far, sub-steps last frame |
| `obs_paused`, `obs_asleep` | paused by the user; at rest and skipping the solver |
| `obs_gravity`, `obs_gdir` | in g; direction in degrees (90 is down) |
| `obs_bounce`, `obs_friction`, `obs_drag`, `obs_speed`, `obs_size` | the settings |
| `obs_ke`, `obs_pe` | kinetic energy; potential plus spring energy |
| `obs_moved` | fastest body last frame, px/s, by actual displacement |
| `obs_sel`, `obs_grab`, `obs_hover` | body ids, or -1 |
| `obs_undo` | depth of the undo stack |
| `obs_spawned`, `obs_linked`, `obs_cut`, `obs_thrown`, `obs_removed` | counters for each kind of edit |
| `obs_emit_left`, `obs_bins` | Pegboard: balls still to drop; the count in each bin |
| `obs_toast` | the last message shown |
| `obs_stage` | `[x, y, w, h]` of the stage in pane pixels |
| `obs_first`, `obs_last` | position of the first and last body, to one decimal |
| `obs_audit` | `{stretch, overlap}`: worst rod error and worst disc overlap in px. Computed only while paused or at rest (it is O(n²)), `nil` otherwise |

The `world` state record is readable too (`?values=world`): `x`, `y`, `vx`,
`vy`, `r`, `id`, `pin` are parallel lists, which is how a test finds a body to
click. Add `obs_stage[0]` and `[1]` to a world position to get pane pixels,
then the pane origin from `panes[0].rect` to get window pixels.

A deterministic run:

```bash
curl -sX POST 127.0.0.1:$PORT/panel/reset -d '{"seed":42}'
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":1}'              # onto the virtual clock
curl -sX POST 127.0.0.1:$PORT/key  -d '{"key":"r"}'          # restart the scene from t = 0
curl -sX POST 127.0.0.1:$PORT/tick -d '{"n":75,"dt":0.016}'
curl -sX POST 127.0.0.1:$PORT/key  -d '{"key":"space"}'      # pause, so obs_audit is filled in
curl -s "127.0.0.1:$PORT/state?values_prefix=obs_" | jq '.panes[0].panel.values'
```

That leaves the cradle at `obs_time` 1.2 with the struck bob stopped at
x 235.0 (its rest position is 234.2) and the far bob flung out to x 531.8
(`world.x[5]` and `world.x[9]`), `obs_last` `[558.3, 117.9]`, `obs_ke` 170977
and `obs_audit.stretch` under 0.001. The same ticks give the same numbers
every run, and the Pegboard, whose dispenser uses `random()`, replays exactly
under the seed.

## Known limits

- **Discs only.** There are no boxes or polygons, and a link attaches at a
  body's centre, so it cannot apply a torque. A rigid shape is a truss of
  discs and rods, as in Jelly.
- **The Pegboard is not a Galton board.** A disc keeps most of its sideways
  speed through a peg strike (a zero-bounce contact removes only the normal
  component, and friction turns at most a third of the slide into spin), so
  balls run diagonally through the lattice and the bins fill fairly evenly
  with heavier ends, not in a bell curve. Five lattice geometries were tried
  before the scene was renamed for what it is.
- **Rods lose a little energy.** Velocity projection followed by position
  projection costs a free pendulum about 0.6% of its energy per second. The
  Cradle at bounce 1.00 swings for minutes, not for ever, and the top edge of
  the energy chart sags accordingly.
- **Rods are Gauss–Seidel.** The warm start makes static loads exact, but a
  sudden yank on a long chain carrying something more than about twenty times
  heavier than its links stretches it for a few frames. The scenes keep their
  mass ratios under 8:1.
- **Contacts have no warm start.** A settled pile keeps about 8 px/s of
  solver velocity that never becomes motion, so `obs_ke` is small but not
  zero at rest, discs in a deep pile overlap by up to about 3 px (2.7 px
  measured in a full Pegboard bin), and rest is judged from displacement
  (`obs_moved`) instead.
- **Not every scene comes to rest.** Cradle and Wrecker swing on, as they
  should. Bridge and Jelly end in a pile that keeps trembling at 5 to 20 px/s
  (the sprung weights and the squashed wheels feed the contact solver), which
  is above the 3 px/s rest threshold, so they never show `AT REST` and the
  panel stays awake on them. The Pegboard does settle, 10 to 20 s after the
  last ball drops.
- **A chocked pile is a ramp.** A hex-packed pyramid held by two pins deflects
  a ball that meets its slope upward instead of breaking, so the Wrecker's
  shelf is placed to put the ball's lowest, fastest point at the second row.
- **Undo is a snapshot, not an inverse.** It puts back the world of the moment
  before the edit, so on a running stage it is also a rewind.
- **Speed.** The bytecode VM runs this solver at roughly 100 instructions per
  contact per pass. On a debug Garden the 13–28 body scenes cost about 8 ms a
  frame and the Pegboard about 17 ms once all 56 balls are down (151 bodies,
  ~115 contacts), until it comes to rest and costs nothing. The stage refuses
  new bodies past 200.
- **Thick lines are not antialiased along their length.** A near-vertical rod
  shows the sub-pixel steps of its tessellated quad.
- **`dt()` is not on the virtual clock.** Frames Garden runs by itself after a
  `POST /tick` still report a wall-clock `dt()`, so the frame delta is derived
  from `time()`. The first tick after a reset moves the panel onto the
  virtual clock and its delta is whatever the jump between the two clocks
  clamps to, so the recipe above sends one warm-up tick and then restarts the
  scene with `R`.
- **Punctuation keys arrive as characters.** The debug server refuses the
  canonical names `period`, `minus` and `equals` ("a canonical key name Garden
  cannot deliver"), so the script reads both spellings and a test posts `.`,
  `-` and `=`.
- **Written for the Garden binary in the tree.** `garden/target/debug/garden`
  was built from `82aee56` and warns that it is 112 source files behind the
  checkout. That build has no default parameter values and no `hypot`, so the
  modules pass every argument explicitly and carry their own `norm`. Both
  compile cleanly on the current `petal check --strict --host garden` too.
- **Fixed chrome.** The sidebar and rail are laid out for the designed pane.
  Nothing is saved between runs, and there is no sound.

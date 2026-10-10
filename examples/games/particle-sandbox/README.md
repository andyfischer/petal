# 11 — Particle sandbox

Up to twelve thousand point particles in a box, written entirely in Petal as a
Garden panel, and seven mouse tools for pushing them around: attract, repel,
swirl, grab and carry, spray, erase, and drop gravity wells that keep pulling
after you let go. Four scenes set the stage (a two-armed galaxy around one
well, a binary pair, a gas stirred by a noise flow field, a burst under
gravity), and the header reports how many particles are alive, how many sit
under the brush, their mean speed and the frame interval.

The point of this entry is **thousands of objects, mouse interaction and
per-frame cost**. Everything a particle needs in a frame (forces, integration,
walls, statistics, colour, its draw call) happens in one iteration of one
loop over five flat `f64_array`s.

## Run it

```bash
tools/run-example.ts particle-sandbox                              # windowed
tools/run-example.ts particle-sandbox --headless --debug-port 0    # for tests
```

By hand:

```bash
cd examples/games/particle-sandbox
GARDEN_HEADLESS_SIZE=1280x850 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s "127.0.0.1:$PORT/screenshot?pane=0" -o shot.png
```

Designed for **1280 × 850** logical pixels, which gives the pane 1268 × 778
and the stage 956 × 640. The sidebar is fixed at 248 px and the stage takes
the rest, so other sizes reflow; the simulation box is whatever the stage is.

Nothing is persisted. The panel asks for frames while it is not paused, so it
keeps running in a window; a headless run sleeps 10 s after the last input and
otherwise steps only on `POST /tick`, so a still picture there is not a hang.

## Controls

**Mouse, on the stage**

| Input | Effect |
|---|---|
| hold left button | use the selected tool at the pointer, for as long as it is held |
| `SHIFT` + hold | invert the tool: Attract pushes, Repel pulls, Vortex spins the other way |
| right click | blast: an outward kick to everything near the pointer, with a shockwave ring |
| wheel | brush radius (24 to 220 px) |

**Tools** (click the row, or press its number)

| Key | Tool | What holding the button does |
|---|---|---|
| `1` | Attract | pulls particles toward the pointer; peak pull at about 0.7 brush radii |
| `2` | Repel | the same force, outward |
| `3` | Vortex | a strong sideways force plus a weak pull, so particles orbit the pointer |
| `4` | Grab | particles inside the brush move with the pointer and take on its velocity; let go to throw them |
| `5` | Emit | 80 particles on the press, then 1,800 a second along the pointer's path, their hue cycling |
| `6` | Erase | deletes every particle inside the brush |
| `7` | Well | a click drops a gravity well (up to six); `SHIFT` + click drops a repulsor; a click on a well removes it |

**Keys**

| Key | Effect |
|---|---|
| `SPACE` | pause / resume |
| `G` `F` `D` `W` | toggle gravity, the flow field, drag, and wrap-around walls (off: walls bounce) |
| `T` | toggle streaks (a short line along each fast particle's velocity) or plain dots |
| `C` | colour ramp: Ice and Ember colour by speed, Prism by the hue a particle was born with |
| `B` | blast at the pointer (the keyboard form of right click) |
| `[` / `]` | shrink / grow the brush |
| `X` | delete every particle |
| `Z` | remove every well |
| `R` | reload the current scene |
| `TAB` / `SHIFT TAB` | next / previous scene |
| `-` / `=` | density: 1,000 / 2,000 / 4,000 / 6,000 / 8,000 / 12,000, reloading the scene |

Every toggle, scene and the density stepper is also a chip in the sidebar, and
the brush slider there drags.

## What it exercises

**Language.**

- Structure-of-arrays in `f64_array`: `px`, `py`, `vx`, `vy`, `hu`, each
  12,000 floats, held in `state var` cells so `spawn` and `load_scene` (plain
  top-level functions) and the module-scope loop all write the same storage
  in place with `set px[i] = …`. No record is built per particle, ever.
- One fused loop. A particle's row of forces, its integration step, the wall
  test, three running sums, the brush count, the colour lookup and the draw
  call are one iteration; the app never walks the pool twice in a frame
  (except on the frames that erase or blast).
- Forces as data. Each well, and the mouse while a force tool is held, is one
  row of five small lists (`x`, `y`, radial gain, tangential gain, softening).
  The inner loop is the same arithmetic for every row, so adding a tool or a
  well adds a row, not a branch. The index list `ks` is built once per frame
  rather than calling `range` twelve thousand times.
- A fixed pool. `spawn` appends while there is room and then overwrites the
  oldest slot through a ring cursor; Erase swap-removes in a `while` loop. The
  arrays are allocated once.
- `config let` for the twelve tunables, a `state` initialiser
  (`state lut = build_lut()`) that builds the three 64-entry colour ramps once
  with `lerp` on colours and `hsv`, collecting `for` with `continue` to drop a
  well, `ease_out` for the blast ring, named arguments throughout the chrome.

**Host / petal-ui.**

- `dt()` clamped to 1/30 s drives everything, so the run is exact under
  `POST /tick`. A frame an injected event runs sees `dt() == 0`; the cursor's
  travel is accumulated across those frames and consumed by the next
  simulated one, so Grab and Emit see the right pointer velocity either way.
- Mouse: `mouse_pressed` / `mouse_down` for a held tool that began on the
  stage, `mouse_pressed(1)` for the blast, `scroll_y` for the brush,
  `mod_shift` to invert, `point_in` for every sidebar chip, a draggable slider.
- Draw surface: the flat native forms `draw_line(x1, y1, x2, y2, r, g, b, a,
  width)` and `draw_rect(x, y, w, h, r, g, b, a)` with float coordinates for
  the particles; `draw_circle_gradient` halos for the wells;
  `draw_circle_outline`, `fill_arc`, `draw_polyline`, rounded rects and a
  rounded `clip_push` for the rest.
- `request_frame()` while running.

**What a frame costs.** `panel.frame_stats.last_ms` (the script's own time)
and the wall time of a `POST /tick` (script plus Garden's scene translation
and headless render), on a debug Garden build on an M-series Mac, Galaxy
scene, streaks on:

| Particles | Script, ms | Tick wall time, ms |
|---|---|---|
| 1,000 | 3.1 | |
| 2,000 | 5.6 | 13.6 |
| 4,000 | 11.4 | 24.0 |
| 6,000 (default) | 16.8 | 34.6 |
| 8,000 | 22.4 | |
| 12,000 | 34.0 | 66.0 |

At 12,000 the script's 33 ms splits roughly into 16 ms of draw calls, 5 ms of
physics and 11 ms of everything else in the loop body (reads, the speed
`sqrt`, sums, the colour lookup); the chrome is under 1 ms. The header's
FRAME figure and the sidebar sparkline show the interval between frames,
which is the only clock a script has; the figure turns warm past 34 ms.

**Debug server.** Values in `panes[0].panel.values`: `obs_n`, `obs_tool`,
`obs_radius`, `obs_acting`, `obs_invert`, `obs_in_brush`, `obs_wells`,
`obs_well_signs`, `obs_scene`, `obs_target`, `obs_gravity`, `obs_flow`,
`obs_drag`, `obs_wrap`, `obs_streaks`, `obs_cmode`, `obs_paused`,
`obs_mean_speed`, `obs_centroid` (stage-local), `obs_emitted`, `obs_erased`,
`obs_blasts`, `obs_sim_frames`, `obs_cursor` (stage-local), `obs_stage`.
Read them with `?values_prefix=obs_`: an unfiltered `/state` carries the five
pool arrays.

A held tool needs a held button, which is `down` … ticks … `up`:

```bash
cd examples/games/particle-sandbox && source ../../../tools/panel-test.sh
panel_start particle-sandbox
tick; panel_reset 42; tick 60 0.016      # obs_n 6000, obs_centroid [477.8, 320.2]
move 300 300; tick 1 0.016               # obs_in_brush 704
click 300 300 '{"op":"down"}'; tick 60 0.016
obs obs_in_brush                         # 1587: the brush filled up
click 300 300 '{"op":"up"}'
key 2; click 300 300 '{"op":"down"}'; tick 60 0.016
obs obs_in_brush                         # 31: Repel emptied it
click 300 300 '{"op":"up"}'
key 6; click 502 396; obs obs_n          # 4912: Erase is immediate
panel_stop
```

## Known limits

- **One draw call per particle.** There is no batched point or line
  primitive, so twelve thousand particles are twelve thousand calls through
  the prelude's `draw_line` / `draw_rect` overloads; that is about half the
  script's frame. A `draw_points(xs, ys, …)` taking the arrays would remove
  most of it.
- **No persistent canvas.** Canvas ids restart every frame, so true motion
  trails (draw onto last frame's picture and fade it) are not possible; the
  streaks are one line per particle along its velocity, capped at 13 px.
- **No additive blending**, so dense regions saturate toward the particle
  colour instead of glowing. The wells' halos are gradient discs drawn under
  the particles.
- **Particles do not interact.** They feel the wells, the mouse, gravity and
  the flow field, not each other, so there are no collisions and no pressure:
  under gravity with drag on they all settle onto the floor line.
- **A script cannot time itself.** There is no wall clock, so the HUD shows
  the frame interval (`dt()`), which under `POST /tick` is whatever the test
  asked for. The real cost is `panel.frame_stats.last_ms` in `/state`.
- **Long tick batches time out.** `POST /tick {"n":150}` at 6,000 particles
  takes about 5 s on a debug build and the debug server answers `timed out
  waiting for the event loop`; the frames still run. Keep batches to 60.
- **Held modifiers.** `POST /key {"key":"shift","op":"down"}` is rejected
  (`shift` is not a deliverable key), so a test inverts a tool with
  `"mods":["shift"]` on the mouse op instead.
- The integrator is semi-implicit Euler with one step per frame and a speed
  limit of 1,400 px/s. Orbits close to a well precess and slowly widen; a
  particle that passes within a few pixels of a well is flung out fast.
- Resizing the pane resizes the box; particles left outside are folded back
  in by the wall test on the next frame.

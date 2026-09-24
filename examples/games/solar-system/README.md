# 50 — Interactive solar system

An orrery as a Garden panel, written entirely in Petal: the Sun, eight
planets and thirteen moons in a body table with parent links, resolved every
frame into world positions by walking the hierarchy (a moon orbits its
planet, which orbits the Sun), then pushed through one view transform (pan,
zoom about the cursor, an optional three-quarter tilt) onto the screen.
Time runs at an adjustable rate on the panel clock; the camera can follow any
body; labels place themselves greedily so they never overlap; hovering shows
a fact card and an inspector lists the selected body, its facts and its
moons.

The point of this entry is **hierarchical transforms, animation, zoom and
labels**: every position is parent-relative, every screen coordinate goes
through the same `to_screen`, and zooming keeps the world point under the
pointer fixed.

## Viewport

Designed for **1200 × 800** logical pixels (which gives the panel pane
1188 × 728). The inspector is fixed at 268 px; the space view takes the rest,
so other sizes reflow.

## Run it

The quickest way is `./launch.sh` in this directory (it finds the `garden`
binary and sets the viewport; extra arguments are passed through, e.g.
`./launch.sh --headless --debug-port 0`). By hand:

```bash
cd examples/games/solar-system
GARDEN_HEADLESS_SIZE=1200x800 ../../../garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
curl -s 127.0.0.1:$PORT/screenshot -o shot.png
```

Nothing is persisted. The panel sleeps 10 s after the last input; the
simulation asks for frames while it is playing, so it keeps turning, but a
headless run steps only on `POST /tick` or input.

## Controls

| Input | Effect |
|---|---|
| wheel over the view | zoom about the pointer (0.35× to 40×) |
| `]` / `[` | zoom in / out about the centre |
| drag | pan (and stop following) |
| click a body | select it and follow it; click empty space to deselect |
| hover a body | fact card |
| `TAB` / `SHIFT TAB` | select the next / previous planet (the Sun is in the cycle) and follow it |
| `F` | toggle following the selected body |
| `SPACE` | pause / resume |
| `+` / `=` and `-` | double / halve the speed (0.125 to 512 simulated days per second) |
| `T` | toggle the three-quarter tilt / top-down view |
| `L` | toggle labels |
| `O` | toggle orbit paths |
| `0` | reset zoom and pan, stop following |
| `ESC` | deselect and stop following |
| inspector row | select and follow; a selected planet's row expands to list its moons |

Orbit radii are display units, not to scale (Neptune would be 30 times
farther than Earth); periods are real, so Mercury laps Earth four times a
year and Phobos circles Mars three times a day. The fact card and inspector
carry real semi-major axes, periods and radii. Triton's period is negative:
it orbits backwards.

Body radii grow with the square root of the zoom, so a planet never fills the
view but a moon still separates from its planet as you zoom in. Moons and
their orbits appear once their orbit is 6 px across; their labels once it is
22 px, or as soon as their planet is selected.

## What it exercises

**Language.** A `class Body` with eleven typed fields and a literal table of
21 instances; `resolve(t)` walks the table parents-first and reads each
parent's position out of the list it is building, which is the whole
hierarchy in eight lines; `vec2` arithmetic throughout (`pos[parent] +
vec2(cos(a) * r, sin(a) * r)`, `mag`, `normalize`, `distance`); collecting
`for` with `continue` for `children_of` and the sidebar rows; `var`
accumulators (`placed`, `labels_drawn`) written from inside `try_label`;
`state var` for everything that persists; `config let` for the tunables; a
`fmt_int` that inserts thousands separators by walking `chars`.

**Host / petal-ui.** Simulated time is read off `time()` rather than
accumulated from `dt()`, so under `POST /tick` (which virtualises the panel
clock) a scripted run is exact, and pausing or changing speed folds the
elapsed time into a base (`mark()`). `request_frame()` while playing and
while the follow camera is still easing (`approach` from the prelude).
`draw_ellipse_outline` for every orbit (the tilt is a y scale, so a circular
orbit is an ellipse on screen), `draw_circle_gradient` for the Sun's corona,
a dark offset disc for each body's night side, Saturn's ring as a wide
ellipse outline, `clip_push` with a radius for the rounded view. Mouse:
`scroll_y` for the zoom, `mouse_pressed` / `mouse_down` / `mouse_released`
with a movement threshold to tell a drag from a click, `point_in` for the
inspector rows. Labels are placed greedily: the selected and hovered bodies
first, then planets, then moons, each skipped if its rect would overlap one
already placed.

**Debug server.** Assertable values in `panes[0].panel.values`: `obs_days`,
`obs_speed`, `obs_playing`, `obs_zoom`, `obs_pan`, `obs_tilt`, `obs_labels`,
`obs_orbits`, `obs_sel`, `obs_sel_name`, `obs_follow`, `obs_hover`,
`obs_labels_drawn`, `obs_bodies`, `obs_zooms`, `obs_clicks`, `obs_earth` and
`obs_moon` (world positions), `obs_moon_rel` (the Moon's distance from
Earth, always 13.0), `obs_earth_screen` (where to click to select Earth),
`obs_rows`. `POST /panel/reset {"seed":42}` and `POST /tick {"n":120,"dt":0.016}`
give the same `obs_days` and `obs_earth` every run;
six presses of `=` (256 days per second) and 90 ticks of 15.86 ms carry
Earth through roughly a year, back near its starting position.

## Known limits

- Orbits are circular and coplanar; the tilt is a view-space y scale, not
  an inclination, so moons never pass behind their planet.
- Text cannot be rotated, so orbit labels sit beside the body rather than
  along the path.
- The night-side shading is a flat offset disc, not a terminator; it reads
  as a crescent at high zoom.
- Bodies are drawn in table order (parents first), so a moon always draws
  over its planet's glow, even when it is "behind" it in the tilted view.
- `days` accumulates on the wall clock while the panel is awake and playing,
  so the elapsed counter in an interactive session depends on how long the
  window has been open; headless tests use `/tick`.

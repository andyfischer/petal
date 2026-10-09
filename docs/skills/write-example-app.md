---
name: write-example-app
description: Build one app from the testbed challenge list as a pure-Petal Garden panel app (or a headless console program) under examples/, verify it against a live headless Garden, document it, and commit it.
---

# Write an example app

Use this when asked to build one of the apps in
[docs/dev/testbed-challenge-plan.md](../dev/testbed-challenge-plan.md), or any
new showcase app of the same kind. The recent entries built this way are
Asteroids (`ffa3c41`, testbed 05), the node-based editor (`5278d43`, testbed
38) and the Markdown editor (`a0449e4`, testbed 39). Each landed as one commit
touching only its own directory, the plan table and `examples/README.md`.

The full reference for panel apps is
[examples/AUTHORING.md](../../examples/AUTHORING.md). This skill is the
procedure; the guide is the detail. When they disagree, the guide wins.

## Two kinds of app

| Kind | Where | What it is |
|---|---|---|
| Panel app (the default) | `examples/<category>/<slug>/` | A Garden panel script: `app.ptl` + `layout.ptl` + `README.md`, launched by `tools/run-example.ts` |
| Console program | `examples/games/<slug>/<slug>.ptl` | Prints to stdout, deterministic, terminates on its own. Used when the entry stresses the language more than the host (Tetris, 2048, Minesweeper, boids, terrain) |

Categories: `games/` for the "Simple games" and "Creative / graphical
experiments" tracks, `productivity/` for "Everyday UI", "Business" and "Media &
creative tools", `dashboards/` for "Data visualization". Never put anything in
`examples/console/` — it is a golden-tested corpus.

## Procedure — panel app

### 1. Pick the entry and claim it

Take the row from the plan table. Its "What it tests" column is the brief: the
app must genuinely exercise those things, not merely resemble the named
product. Choose a `slug` (kebab-case, no number prefix; the testbed number goes
only in the README title and commit message).

### 2. Read before writing any code

Petal is not JavaScript; guessing the syntax wastes more time than reading.
In order:

1. `docs/writing-petal-guide.md` - The main doc on how to write Petal.
1. `examples/AUTHORING.md` — Guide to writing example apps, including
   the layout on disk, launching, inspecting, driving
   input, the headless frame contract, `state` and hot reload, draw and input
   vocabulary, text and fonts, alpha, the `ui` prelude, the quality bar.
2. `docs/language-guide.md` - a more detailed guide to the language.
3. `docs/Builtins.md` — list of builtin stdlib functions.
4. `core-libs/petal-ui/docs/components.md` (the `ui` prelude) and, if the app wants
   animated controls, `core-runtime/bloom/docs/components.md`.
5. `garden/docs/petal-graphical-panels.md` and `garden/docs/debug-server.md`.
6. Two or three finished apps closest to yours, for example
   `examples/games/pong/app.ptl` for a game or
   `examples/productivity/notes/app.ptl` for a text-heavy tool, plus
   `garden/examples/panels/sketch.ptl` for the draw surface.

### 3. Scaffold

```bash
mkdir -p examples/<category>/<slug> && cd examples/<category>/<slug>
printf '// headless-size: 1280x850\nlayout(panel("app.ptl"))\n' > layout.ptl   # layout(...) is required
```

`tools/run-example.ts <slug>` finds the example's directory and the Garden
binary at `garden/target/debug/garden` or `$GARDEN_BIN`, exports
`GARDEN_HEADLESS_SIZE` from the `headless-size` comment in `layout.ptl`, and
`exec`s `garden --init layout.ptl` with any further arguments. It
exits 3 with a `STALE GARDEN BINARY` banner, launching nothing, when that
binary is behind the checkout; if `log.txt` has no port in it, read it. Pick the
viewport that suits the app (1280x850 is the headless default; Asteroids uses
1100x780) and remember the pane is smaller than the viewport (roughly `W-12`
by `H-72`); use `screen_width()`/`screen_height()` inside the script.

Make sure Garden is built (`cd garden && cargo build`) before you start. Do
not rebuild it as part of the app work.

### 4. Write `app.ptl` in short loops against a live Garden

Do not write the whole app blind and hope. The loop is:

```bash
# from the repo root, after every edit
./tools/run-petal.ts check --strict --host garden examples/<category>/<slug>/app.ptl
./tools/run-petal.ts lint examples/<category>/<slug>/app.ptl          # `lint --fix` applies the fixes

source tools/panel-test.sh          # or call it per command: tools/panel-test.sh click 80 30
panel_start <slug>                  # headless Garden on a free port, written to ./log.txt

obs                                 # every obs_* value; fails loudly on a status_error
shot                                # shot.png of the pane. OPEN it and look at it
click 80 30                         # pane-local pixels, the ones the script draws with
key space; tick 60 0.016            # one keypress, then 60 frames of exactly 16 ms
panel_reset 42                      # restart on seed 42 after changing seed data
curl -s 127.0.0.1:$(panel_port)/scene?pane=0 | jq ...   # assert layout numerically
panel_stop
```

`--host garden` is not optional. Without it `check` knows only the `ui`
natives and reports `claim_key`, `request_frame`, `palette` and the rest of
Garden's own as unknown functions on a correct script. `petal lint` is a
separate pass (it takes no `--host`): it reports spellings that have a better
form, and the app is not done while it reports any.

[`tools/panel-test.sh`](../../tools/panel-test.sh) is the one copy of the
`click`/`key`/`tick`/`obs`/`shot` helpers; its header lists every command. Do
not write your own. Each agent tool call is a new shell, so either `source` it
at the top of each call or use the `tools/panel-test.sh <command>` form; both
find the port again from `./log.txt` (or `$PORT`).

Rules that cost the most time when broken:

- `--headless --debug-port 0` always (`panel_start` does this). A window
  steals focus; a fixed port collides with another Garden. By hand, launch
  inside `(nohup … < /dev/null &)` so the process outlives the tool call.
  Always `127.0.0.1`, never `localhost`.
- Editing `app.ptl` hot-reloads it but keeps `state`. After changing seed
  data or anything cached in `state`, `POST /panel/reset`; do not restart the
  process. A frame that raised leaves its error card up until a reset.
- If the app uses `panel_store_*`, launch with `GARDEN_PANEL_STORE_DIR=<scratch>`
  or every run starts from the last run's save.
- `POST /mouse` takes window coordinates unless the body names a pane; the
  script sees pane-local ones. Send `"pane": 0` with pane-local `x`/`y` (the
  helpers do) rather than adding `panes[0].rect` yourself.
- A headless panel is not a 60 fps loop: about one frame per injected event,
  one per ~200 ms idle poll, and sleep after 10 s of no input. Drive motion
  from `dt()`, clamp and sub-step physics, keep any polling interval well
  under 10 s, and use `request_frame()` while animating.
- Open every test with `tick; panel_reset 42`. From the first `POST /tick`
  the panel's `time()` and `dt()` are both virtual, and stay so across
  resets: only ticks advance them, and the frame an injected key or click
  runs sees `dt() == 0`.
- Garden owns Cmd/Ctrl chords; `claim_key("z", "cmd")` near the top of every
  frame to get one back.
- `context_menu(...)` must be the last draw call of the frame;
  `menu_blocking(...)` the first input check.
- `petal check` does not catch a misspelled global. `status_error` in
  `/state` does.
- Work around language and host limits in Petal and note them in the README.
  Do not patch `core/`, `garden/*/src/`, `core-libs/petal-ui/src/` or any `Cargo.toml`
  as a side effect of the app. Language fixes are a separate change.

Design the app to be testable. Mirror every piece of logical state into plain
`let` bindings the debug server can read (`panes[0].panel.values`): Asteroids
exposes `obs_phase`, `obs_score`, `obs_lives`, `obs_rocks` and so on; the
Markdown editor exposes `lines`, `cur`, `sel_doc`, `mode`, `undo_n`. One-frame
edges (`key_pressed`, `text_input`, `scroll`) are cleared by the next tick, so
anything a test must observe later gets counted into a `state` var.

### 5. Verify like a user, then like a machine

Before calling it done:

- **Look at the screenshot with your own eyes.** A PNG you never opened is
  not verification. Fix overlaps, cramped padding, muddy contrast,
  misalignment, text running out of its box, and glyphs the embedded face
  lacks (`⌘`, `⇧` and friends fall back to a system face, so they render,
  but in a different design).
- **Exercise every control the README documents** through `POST /key`,
  `/text` and `/mouse`, and confirm both the values and the pixels change.
  Held keys are `{"key":"left","op":"down"}` … `{"op":"up"}`.
- **Games and animations: prove the whole arc deterministically.**
  `tick`, then `panel_reset 42` (`POST /panel/reset {"seed":42}`: the seed in
  the reset body, so frame 1 is seeded too), then `tick 60 0.016` between
  inputs. The
  same script then ends on the same `obs` values every run, however long it
  paused between commands. Asteroids was driven this way through a sector
  clear and a game over before it was committed.
- `status_error` is `null` at every point of that script. Check `values_stale`
  is not set; a missing key because the frame raised is not the same as a
  branch that never ran.
- Reload the process once and confirm a persisted app restores what it
  saved, and its "reset demo data" path restores the seed.

### 6. Write the README

Follow the shape of `examples/productivity/markdown-editor/README.md`:

1. Title `# NN — <App name> (<optional codename>)` and a one-paragraph
   description of what it is.
2. **Run it**: `tools/run-example.ts <slug>`, the by-hand headless command, the designed
   viewport and the pane size it yields, any env such as
   `GARDEN_PANEL_STORE_DIR`, and a note that the panel sleeps after 10 s so a
   reviewer does not read a stopped simulation as a hang.
3. **Controls**: a table per surface (keys, mouse, chrome).
4. **What it exercises**: language features, host and prelude features, and
   the debug-server values a test can assert on.
5. **Known limits**: every workaround and every host gap you hit, with the
   exact symptom. This section is where language feedback lives now that the
   `.temp/testbed-debriefs/` files are no longer written.

### 7. Write a debrief

Write a file to `.temp/<todays-ISO-date>-<name>-debrief.md`

The debrief should contain:

 - Any bugs or blockers or challenges that were encountered while building the app.
 - Ways in which it was harder to build the app due to the design of Petal or Garden.
 - Ideas to improve or streamline the core platform to make this easier.
 - Ideas for tooling or skills to improve the development process.
 - Feedback on the documentation, any areas that were unclear or confusing.
 - Short list of action item ideas to improve the project.

### 8. Register it and commit

- Change the entry's Status cell in `docs/dev/testbed-challenge-plan.md` to
  ``built — `examples/<category>/<slug>/` `` and bump the counts in that
  file's intro paragraph.
- Add the app to the category row in `examples/README.md`.
- Stage explicit paths only, never `git add -A`. One commit:

```bash
git add ...
git commit -m "examples: add <App name> example app/game <with description>"
```
## Quality bar

The goal is to write very **high quality** apps in order to fully demonstrate
the project.

This is a showpiece, not a smoke test. A considered palette, a real
typographic hierarchy (size, color and spacing, not just bold), a consistent
spacing scale, generous padding, plausible seeded content rather than
placeholder junk, and interactions that feel deliberate. Take the time to
make it genuinely beautiful.

Once the initial app is running, check it in various ways, including
debugging and visual verification, then iterate on the result to 
hone and perfect it.


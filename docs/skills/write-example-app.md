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
| Panel app (the default) | `examples/<category>/<slug>/` | A Garden panel script: `app.ptl` + `layout.ptl` + `launch.sh` + `README.md` |
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
4. `petal-ui/docs/components.md` (the `ui` prelude) and, if the app wants
   animated controls, `petal-libs/bloom/docs/components.md`.
5. `garden/docs/petal-graphical-panels.md` and `garden/docs/debug-server.md`.
6. Two or three finished apps closest to yours, for example
   `examples/games/pong/app.ptl` for a game or
   `examples/productivity/notes/app.ptl` for a text-heavy tool, plus
   `garden/examples/panels/sketch.ptl` for the draw surface.

### 3. Scaffold

```bash
mkdir -p examples/<category>/<slug> && cd examples/<category>/<slug>
echo 'layout(panel("app.ptl"))' > layout.ptl        # layout(...) is required
cp ../../games/pong/launch.sh .                      # edit the header comment and GARDEN_HEADLESS_SIZE
```

`launch.sh` resolves its own directory, finds the Garden binary at
`garden/target/debug/garden` or `$GARDEN_BIN`, exports a default
`GARDEN_HEADLESS_SIZE`, and `exec`s `garden --init layout.ptl "$@"`. Pick the
viewport that suits the app (1280x850 is the headless default; Asteroids uses
1100x780) and remember the pane is smaller than the viewport (roughly `W-12`
by `H-72`); use `screen_width()`/`screen_height()` inside the script.

Make sure Garden is built (`cd garden && cargo build`) before you start. Do
not rebuild it as part of the app work.

### 4. Write `app.ptl` in short loops against a live Garden

Do not write the whole app blind and hope. The loop is:

```bash
./ts/bin/run-petal.ts check --strict examples/<category>/<slug>/app.ptl   # from the repo root

cd examples/<category>/<slug>
(nohup ./launch.sh --headless --debug-port 0 > log.txt 2>&1 < /dev/null &)
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
GPID=$(pgrep -f "garden --init $(pwd)/layout.ptl" | head -1)

curl -s 127.0.0.1:$PORT/state | jq '.status_error, .panes[0].panel.values'
curl -s 127.0.0.1:$PORT/screenshot -o shot.png     # then OPEN shot.png and look at it
curl -s 127.0.0.1:$PORT/scene | jq ...             # assert layout numerically
```

Rules that cost the most time when broken:

- `--headless --debug-port 0` always. A window steals focus; a fixed port
  collides with another Garden. Launch inside `(nohup … < /dev/null &)` so
  the process outlives the tool call. Always `127.0.0.1`, never `localhost`.
- Editing `app.ptl` hot-reloads it but keeps `state`. After changing seed
  data or anything cached in `state`, `POST /panel/reset`; do not restart the
  process. A frame that raised leaves its error card up until a reset.
- If the app uses `panel_store_*`, launch with `GARDEN_PANEL_STORE_DIR=<scratch>`
  or every run starts from the last run's save.
- `POST /mouse` takes window coordinates; the script sees pane-local ones.
  Read `panes[0].rect` from `/state` and add its origin.
- A headless panel is not a 60 fps loop: about one frame per injected event,
  one per ~200 ms idle poll, and sleep after 10 s of no input. Drive motion
  from `dt()`, clamp and sub-step physics, keep any polling interval well
  under 10 s, and use `request_frame()` while animating.
- Garden owns Cmd/Ctrl chords; `claim_key("z", "cmd")` near the top of every
  frame to get one back.
- `context_menu(...)` must be the last draw call of the frame;
  `menu_blocking(...)` the first input check.
- `petal check` does not catch a misspelled global. `status_error` in
  `/state` does.
- Work around language and host limits in Petal and note them in the README.
  Do not patch `rust/`, `garden/*/src/`, `petal-ui/src/` or any `Cargo.toml`
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
  misalignment, text running out of its box, missing glyphs (the embedded
  `ui` face has no `⌘`, `—`, `·`, `⇧`).
- **Exercise every control the README documents** through `POST /key`,
  `/text` and `/mouse`, and confirm both the values and the pixels change.
  Held keys are `{"key":"left","op":"down"}` … `{"op":"up"}`.
- **Games and animations: prove the whole arc deterministically.**
  `POST /panel/reset`, then `POST /seed {"seed":42}`, then
  `POST /tick {"n":60,"dt":0.016}` between inputs. Asteroids was driven this
  way through a sector clear and a game over before it was committed.
- `status_error` is `null` at every point of that script. Check `values_stale`
  is not set; a missing key because the frame raised is not the same as a
  branch that never ran.
- Reload the process once and confirm a persisted app restores what it
  saved, and its "reset demo data" path restores the seed.
- Stop Garden with `kill $GPID`. Never `pkill -f garden` or `killall`.

### 6. Write the README

Follow the shape of `examples/productivity/markdown-editor/README.md`:

1. Title `# NN — <App name> (<optional codename>)` and a one-paragraph
   description of what it is.
2. **Run it**: `./launch.sh`, the by-hand headless command, the designed
   viewport and the pane size it yields, any env such as
   `GARDEN_PANEL_STORE_DIR`, and a note that the panel sleeps after 10 s so a
   reviewer does not read a stopped simulation as a hang.
3. **Controls**: a table per surface (keys, mouse, chrome).
4. **What it exercises**: language features, host and prelude features, and
   the debug-server values a test can assert on.
5. **Known limits**: every workaround and every host gap you hit, with the
   exact symptom. This section is where language feedback lives now that the
   `.temp/testbed-debriefs/` files are no longer written.

### 7. Register it and commit

- Change the entry's Status cell in `docs/dev/testbed-challenge-plan.md` to
  ``built — `examples/<category>/<slug>/` `` and bump the counts in that
  file's intro paragraph.
- Add the app to the category row in `examples/README.md`.
- Stage explicit paths only, never `git add -A`. One commit:

```bash
git add examples/<category>/<slug> docs/dev/testbed-challenge-plan.md examples/README.md
git commit -m "examples: add <App name> as a Garden panel app (testbed NN)"
```

The body can carry two or three lines on what it does and how it was verified
(for example "Verified headlessly through sector clear and game over via
/tick"). Remove `log.txt` and `shot.png` from the app directory before
staging.

## Procedure — console program

For an entry that is better as a headless program (the Aug 30 batch: Tetris,
2048, Minesweeper, boids, terrain):

1. One file, `examples/games/<slug>/<slug>.ptl`, run with
   `./ts/bin/run-petal.ts run examples/games/<slug>/<slug>.ptl`.
2. Deterministic: a fixed seed or a scripted sequence of moves, so two runs
   print byte-identical output. Tuning knobs are `config let` bindings.
3. Prints enough to be checked without a screen: the board as ASCII after key
   events, plus a numeric summary (score, lines, a cohesion metric, a
   histogram) so correctness is verifiable rather than merely visible.
4. Terminates on its own in a second or two, exit 0, `check --strict` clean.
5. A short README in the directory, the plan-table entry set to
   ``console — `examples/games/<slug>/<slug>.ptl` ``, and the app listed in
   `examples/README.md`.

That batch was also a documentation stress test: each agent was allowed to
read only `docs/writing-petal-guide.md`, nothing else in `docs/`, no source,
no other `.ptl`, and had to discover every missing fact by running
`run-petal.ts run -e '<snippet>'`, logging each guess-and-check round with the
exact snippet and error text. Reuse that restriction when the goal is to
improve the guide rather than the app; the friction log then feeds edits to
the guide.

## Quality bar

From the original build prompt, still the standard:

> This is a showpiece, not a smoke test. A considered palette, a real
> typographic hierarchy (size, color and spacing, not just bold), a consistent
> spacing scale, generous padding, plausible seeded content rather than
> placeholder junk, and interactions that feel deliberate. Take the time to
> make it genuinely beautiful.

Plus idiomatic Petal: `state` for what persists across frames, `let` for
per-frame dataflow, `var`/`set` only where mutation is genuinely needed,
functions to factor drawing, classes to name record shapes. Run
`petal lint` at the end; recent work folds function-local `var`s into `let`s
and `x = x op e` into `x op= e`.

## Running several at once

The original 50-app run (Aug 8, 2026) drove five agents per batch, one from
each track, through a workflow. What it learned about a shared checkout:

- No worktrees: `garden/target` alone is ~15 GB. Share one checkout and one
  prebuilt binary; nobody runs `cargo build` during the run.
- Each agent touches only its own directory. Concurrent commits contend on
  `.git/index.lock`; sleep and retry, never checkout/reset/stash/rebase.
- `--debug-port 0` per agent, and kill by PID.
- Each agent returned a short status (complete/partial/blocked), one line on
  what it built, and its single biggest issue; those were collated into a
  ranked fix list. Fifteen of the fifty landed before the run stopped, and
  zero reported a hard blocker.

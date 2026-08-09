# Testbed apps — authoring guide

Every app in `examples/testbed/` is a **pure-Petal Garden panel app**: a `.ptl`
script drawn by Garden's panel runtime, driven and inspected through Garden's
headless debug server. No Rust, no TypeScript.

Read this whole file before writing any code.

## Layout on disk

```
examples/testbed/<NN>-<slug>/
  app.ptl        the app itself (a Garden panel script)
  layout.ptl     the launcher: layout(panel("<abs-or-cwd-relative>/app.ptl"))
  README.md      what it is, what it demonstrates, how to run it, controls
```

Multi-file apps may add modules next to `app.ptl` and import them.

## Running it

`layout(...)` is **required** — a bare `panel("...")` at top level silently
leaves you with an empty editor pane.

```bash
cd examples/testbed/<NN>-<slug>
echo 'layout(panel("app.ptl"))' > layout.ptl        # path relative to cwd
/Users/andy/petal/garden/target/debug/garden \
    --headless --debug-port 0 --init layout.ptl > log.txt 2>&1 &
GPID=$!
PORT=$(grep -o '127.0.0.1:[0-9]*' log.txt | cut -d: -f2)
```

Always `--headless --debug-port 0`. A windowed launch steals the user's focus;
a fixed port collides with the other agents running right now.

`GARDEN_HEADLESS_SIZE=WxH` sets the virtual viewport (default 1280x850). Pick a
size that suits the app and record it in the README.

### Shutting down

**Kill by PID only** — `kill $GPID`. Never `pkill -f garden` or `killall`:
other agents have their own Garden processes running and you will kill them.

## Inspecting it

| What | How |
|---|---|
| Pixels | `curl -s localhost:$PORT/screenshot -o shot.png` (PNG; frame number in the `X-Garden-Frame` header) |
| Draw calls | `curl -s localhost:$PORT/scene` — every quad/text run with rect + color. Best for asserting layout numerically. |
| Logical state | `curl -s localhost:$PORT/state \| jq '.panes[0].panel'` — `awake`, `frame`, `values` (every binding the last frame made), `input` |
| Script `print` | the `script.output` array in `/state` (**draining** — each read consumes it) |

`panel.values` is the killer feature: a plain `let sel = 2` in your script is
readable as `sel`, and a binding inside `fn list_row` keys as `list_row.y`.
Write your assertions against those names rather than against pixels.

`/screenshot` and `/scene` settle panel frames before answering, so
**input-then-capture needs no sleep**.

### Driving it

```bash
curl -sX POST localhost:$PORT/key   -d '{"key":"left"}'
curl -sX POST localhost:$PORT/key   -d '{"key":"s","mods":["cmd"]}'
curl -sX POST localhost:$PORT/text  -d '{"text":"hello"}'
curl -sX POST localhost:$PORT/mouse -d '{"op":"click","x":80,"y":30}'
curl -sX POST localhost:$PORT/mouse -d '{"op":"click","x":80,"y":30,"button":1}'   # right
curl -sX POST localhost:$PORT/mouse -d '{"op":"drag","x":80,"y":30,"to":{"x":300,"y":90}}'
curl -sX POST localhost:$PORT/mouse -d '{"op":"scroll","x":80,"y":30,"lines":3}'
```

Named keys: `enter`, `tab`, `space`, `backspace`, `delete`, `escape`,
`left`/`right`/`up`/`down`, `home`, `end`, `pageup`, `pagedown`. Mods:
`cmd`, `ctrl`, `shift`, `alt`.

**One-frame edges** (`keys_pressed`, `*_released`, `click_count`, `scroll`,
`text`) are cleared by the next idle tick (~200ms headless). A script that must
observe an edge across a later `GET /state` has to *count it into a `state`
var*, which is then observable under its own name in `panel.values`.

## The drawing API

Panel-local logical pixels, `(0,0)` at the pane's top-left;
`screen_width()`/`screen_height()` give the current pane size. Colors are
integer RGB `0..255`.

```
clear(r,g,b)
draw_rect(x,y,w,h, r,g,b[,a])
draw_rect_rounded(x,y,w,h,radius, r,g,b[,a])
draw_rect_outline(x,y,w,h, r,g,b[,a[,width]])
draw_line(x1,y1,x2,y2, r,g,b[,a[,width]])
draw_circle(cx,cy,radius, r,g,b[,a])
fill_triangle(x1,y1,x2,y2,x3,y3, r,g,b[,a])
fill_poly(points, r,g,b[,a])            // convex; points = [[x,y], ...]
draw_text(s, x, y, size, r,g,b[,a])
draw_image(source, x,y,w,h[,a])         // PNG path, relative to Garden's cwd
clip(x,y,w,h) / clip_none()
```

Reads: `dt()`, `frame_count()`, `time()`, `screen_width()`, `screen_height()`,
`mouse_x()`, `mouse_y()`, `mouse_down(btn)`, `mouse_pressed(btn)`,
`key_down(name)`, `key_pressed(name)`, `scroll_y()`, `scroll_x()`,
`text_input()`, `text_width(s, size)`, `panel_theme()`.

`text_width` measures with the real font advances (JetBrains Mono), so
centering and right-alignment are exact. **`weight` degrades to regular** — only
the Regular face is embedded. Do not rely on bold for hierarchy; use size,
color, and spacing.

### The `ui` prelude

`petal-ui/prelude/ui.ptl` is an implicit import — call these bare. Read that
file; it is the authoritative reference. Highlights:

- `rect(x,y,w,h)`, `point_in(px,py,r)`, `hovered(r)`, `clicked(r)`
- record overloads of every `draw_*` — `draw_rect(r, c)`, `draw_text(s, pos, style)`
  where `style` is `{size, color, weight, italic, spacing}`
- `draw_text_right`, `draw_text_center`, `ellipsize`, `truncate_tail`, `wrap`,
  `preview`, `fit_parts`
- `button(r, label[, style])`
- `list_state()`, `list_update(lst, count, visible_rows, r[, active])`,
  `list_row_rect`, `ensure_visible`, `ensure_visible_px`, `draw_scrollbar`
- `scroll_update(offset, total, visible, r[, active])`
- `focus_state()`, `focused`, `focus_set`, `focus_next/prev`, `focus_update`,
  `text_field(fc, id, r, buf)`, `section_label`
- context menus: `menu_state`, `menu_item`, `menu_sep`, `menu_open_on_right_click`,
  `menu_blocking`, `menu_show`, `menu_close`, `context_menu(m, items)`
- `theme` — a palette record

Worked examples to read before starting:
`garden/examples/panels/sketch.ptl` (draw surface),
`garden/gpp-apps/garden-diff/src/garden_diff.ptl` and
`garden/gpp-apps/git-viewers/src/git_panel.ptl` (real interactive panels:
focus registry, lists, scrolling, menus).

## Animation and the sleep trap

Garden renders only dirty frames. A panel stays awake for **10s** after the
last input, ticking at ~60fps, then sleeps until the next input. So:

- A continuously-running simulation (a game loop, boids, a clock) **stops** 10s
  after the last injected event. That is expected behavior, not a bug in your
  app — note it in the README, and inject a key/mouse event to keep it running
  while you test.
- A `time() >= next` poll does not fire on its own once asleep. Keep any poll
  interval meaningfully under 10s if you use one.
- Drive animation off `dt()`, never off a fixed per-frame delta — headless
  frames tick at the slow poll rate, not 60fps.

## Quality bar

These are showpieces, not smoke tests. Aim for:

- A real visual design: a considered palette, consistent spacing scale, a
  typographic hierarchy built from size/color/spacing, generous padding,
  alignment that holds at the declared viewport size.
- Actual interactivity — hover states, selection, keyboard *and* mouse where
  both make sense, transitions where they help legibility.
- Enough content to look alive (plausible seeded data, not `foo`/`bar`).
- Idiomatic Petal: `state` for what persists across frames, `let` for dataflow,
  `var`/`set` only where mutation is genuinely needed, functions to factor
  drawing, classes to name record shapes.
- No crashes and no script error in `/state`'s `status_error` at any point in
  the interaction script you exercise.

## Hard rules for this run

1. **Do not modify anything under `rust/`, `garden/*/src/`, `petal-ui/src/`, or
   any `Cargo.toml`.** No `cargo build`, no `cargo run`. The prebuilt binaries
   at `garden/target/debug/garden` and `rust/target/debug/petal` are what you
   use. A build would serialize every other agent behind the cargo lock and can
   swap the binary out from under a running test.
2. **Do not touch files outside your own `examples/testbed/<NN>-<slug>/`
   directory** (plus your one debrief file). Other agents are editing this
   checkout concurrently.
3. Never `pkill`/`killall`. Kill your Garden by PID.
4. Do not add `.ptl` files to `examples/` top level — that directory is a
   golden-tested corpus and a new file there breaks the suite. A subdirectory
   is fine.
5. If you hit a language or host limitation, **work around it in Petal and
   record it in your debrief** — do not fix it in Rust. Language fixes are a
   separate batch.

## Committing

When the app works, commit only your own files:

```bash
git add examples/testbed/<NN>-<slug> .temp/testbed-debriefs/<NN>-<slug>.md
git commit -m "examples(testbed): <NN> <app name>"
```

Other agents commit concurrently, so `git commit` can fail on
`.git/index.lock`. Sleep a couple of seconds and retry, up to a few times.
Never `git add -A`, never `git checkout`/`reset`/`stash`, never rebase.

## Debrief

Write `.temp/testbed-debriefs/<NN>-<slug>.md`:

```markdown
# <NN> <App name>

**Status:** complete | partial | blocked
**Viewport:** WxH
**What works:** ...
**What I could not do:** ...

## Blockers
Things that stopped you outright. Include the smallest reproducing snippet
and the exact error text.

## Issues
Friction that cost you time but that you worked around. Missing builtins,
confusing errors, surprising semantics, awkward API shapes, docs that were
wrong or missing. Be specific and include snippets — this list is the point
of the exercise.

## Praise
What was genuinely good to use.

## Feature requests
Concrete, prioritized.
```

Be honest and specific. A debrief that says "everything was fine" from an app
that took three workarounds to finish is worse than useless.

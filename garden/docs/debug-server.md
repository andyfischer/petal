# Debug server

A localhost HTTP server inside the running app for live inspection and input
injection. It exists so a script, an agent, or a person with `curl` can see
what the editor is doing and drive it deterministically, with no
screen-recording permission and no AppleScript. It works in every run mode
(window, `--term`, `--headless`); in headless mode it is the only interface
the app has.

Source: `garden-app/src/debug.rs` (server and routing) and
`garden-app/src/app/debug_server.rs` (the handlers).

## Enabling

```bash
cargo run -p garden-app -- --debug-port 8080              # explicit port
cargo run -p garden-app -- --debug-port 0                 # OS picks a free port
cargo run -p garden-app -- --headless --debug-port 8080   # headless requires a port
```

There is no default port: without `--debug-port <n>`, no server runs. The
bound port is printed on startup (except in `--term`, where stderr would
scribble on the TUI):

```
garden: debug server on http://127.0.0.1:8080
```

**Address it as `127.0.0.1:<port>`, never `localhost:<port>`.** On macOS
`localhost` resolves to `::1` first, and with several Gardens running that has
repeatedly landed a session on someone else's process. The server binds both
`127.0.0.1` and `[::1]` to make that less likely, but use the literal IPv4
address anyway, and check `identity` in `/state` (below) when more than one
Garden is running.

Headless mode has no window to resize and no resize endpoint. Its virtual
viewport defaults to 1280×850 logical pixels; set `GARDEN_HEADLESS_SIZE=WxH`
before launch to run and screenshot at another size.

### When a headless run stops by itself

A headless session has no window to close, so it also ends on its own in two
cases. Otherwise a run whose launcher died would sit holding its port until
reboot.

- **Orphaned.** If the process is reparented to pid 1 (its launcher exited),
  it shuts down within one poll. A run that was already parented to pid 1 at
  startup (a supervisor, `nohup`) is not watched.
  `GARDEN_HEADLESS_KEEP_ORPHAN=1` turns the check off.
- **Idle, opt-in.** `GARDEN_HEADLESS_IDLE_TIMEOUT=<seconds>` ends the session
  after that long with no debug request. Unset (or `0`) means no timeout.
  Turn it on for the case the orphan check cannot see: a launcher that exited
  before the pid was sampled (backgrounding from `sh -c` is enough). The test
  harness sets it on every launch.

Neither applies to the windowed or terminal frontends.

## How it works

A background thread accepts connections and parses each request into a
`DebugCmd`. The command travels to whichever event loop owns the `App` (a
winit proxy event for the windowed frontend, an mpsc channel for headless and
terminal), is handled against the live `App`, and the reply comes back the
same way. So every command sees a consistent snapshot, and injected input runs
through the same code paths as real input.

Screenshots never touch the OS screen-capture APIs: the current scene is
rendered into an offscreen wgpu texture, read back, and PNG-encoded. The
terminal frontend answers `/screenshot` with its character grid as plain text.

## Endpoints

All request bodies and responses are JSON. Errors are `{"ok": false, "error":
"..."}` with a 4xx/5xx status. One command per request; the connection closes
after the response.

### Inspection

| Endpoint | Returns |
|----------|---------|
| `GET /state` | Editor state (below) |
| `GET /state?values=…` | The same, with each panel's `values` map narrowed. See [Filtering `panel.values`](#filtering-panelvalues) |
| `GET /state?output=…` | The same, choosing how script `print` output is read. See [Reading script output](#reading-script-output) |
| `GET /version` | Which build is answering: version, git commit, build date, feature flags, and the petal-ui prelude exports. Answered without touching the event loop. See [Which build am I talking to?](#which-build-am-i-talking-to) |
| `GET /buffer/<n>` | Full text of pane *n*'s buffer (`text/plain`) |
| `GET /scene` | The primitives of the current frame: quads, text runs, meshes, images, and canvas ops. See [Asserting on the scene](#asserting-on-the-scene) |
| `GET /scene?pane=<n>` | The same, restricted to pane *n* and rebased onto that pane's origin, so it lines up with `GET /screenshot?pane=<n>` |
| `GET /screenshot` | PNG of a complete, settled frame at physical-pixel size. The captured frame number is in the `X-Garden-Frame` header |
| `GET /screenshot?pane=<n>` | The same, cropped to pane *n*: no tab strip, status bar, or gutter. This is the supported way to get chrome-free pixels |
| `GET /frame` | `{"ok": true, "frame": n}`, the global frame counter, answered instantly. `?min=N` adds `"reached": true/false` |
| `GET /windows` | The open OS windows by ordinal, which is focused, and each one's pane count. See [Multiple windows](#multiple-windows) |
| `GET /menu` | The catalog of menu actions `POST /menu` accepts |

`GET /state` carries:

- `identity`: pid, port, layout script path, cwd, the build stamp, and the
  panel scripts running. Check it first when more than one Garden is running.
- `frame`: the global frame counter (see [Frame consistency](#frame-consistency)).
- window size and scale, cell metrics, `focus` (the focused pane index).
- `status_error`, `status_note`, `script_error`, `panel_error`: what the status
  bar shows. Panel script errors (a compile failure or a frame that raised)
  land in `panel_error`, and `status_error` falls back to it, so one field
  answers "is anything broken?".
- `script.output`: `print(...)` lines from the layout script and every panel.
- `text_atlas` and `unresolved_fonts`: see [Text rendering health](#text-rendering-health).
- `command_line`: an open `:` / `/` / `?` prompt with its text, if any.
- `file_finder`: the open fuzzy finder's query, selection, and top matches.
- `trace`: the traced draw call under the pointer on a Petal IDE canvas, or
  `null`. See [petal-ide-mode.md](petal-ide-mode.md#automation--headless-1).
- `panes[]`: per pane, `kind` (`editor` / `panel`), file, title, `mode` (vim
  mode), `pending` (mid-command vim state, below), dirty flag, cursor,
  selection (text capped at 10k chars), `scroll_top` / `scroll_sub` /
  `scroll_frac` / `scroll_left`, `wrap`, `line_count`, `visible_lines`, `rect`
  (window-relative logical pixels), `trace_highlight`, and for a panel pane a
  `panel` object (next).

### Panel panes: `panel.values`

A panel pane (a Petal graphical panel, including every GPP app) reports a
`panel` object:

```jsonc
"panel": {
  "script": "…/diff.ptl",
  "client": "garden-diff",  // the spawn command, for a GPP pane
  "awake": true,            // within its animation wake window
  "frame": 234,             // frames run so far
  "error": null,            // the full multi-line error, or null
  "values": {               // every value the last good frame bound, by name
    "sel": 2, "scroll_right": 45, "mode": "unified",
    "list_row.y": 88, "files": ["a.rs", "b.rs"]
  },
  "values_frame": 233,      // the panel frame `values` came from
  "values_stale": true,     // true when that is NOT the frame that just ran
  "values_partial": {       // present only when the last frame raised:
    "frame": 234,           // how far it got before it did
    "values": {"sel": 3}
  },
  "input": {                // the exact input delivered to the last frame
    "mouse": [120, 88],
    "keys_down": [], "keys_pressed": ["down"], "keys_released": ["down"],
    "mouse_buttons_down": [], "mouse_buttons_pressed": [], "mouse_buttons_released": [],
    "scroll": [0, 0],
    "modifiers": 0,         // bitmask: 1=shift 2=ctrl 4=alt 8=cmd
    "drag_active": false, "drag_start": [0, 0],
    "click_count": 0, "text": ""
  }
}
```

`panel.values` is the hook for testing interactive panels: the last value
bound to every named term the frame evaluated, so a panel's logical state
(selection, scroll offset, hit rectangles) is assertable without decoding
pixels. The script does nothing to participate: a plain `let sel = 2` is
readable as `sel`, and a `state` var as itself. Rules:

- **Keys are function-qualified.** A top-level `let y` keys as `y`; the same
  binding inside `fn list_row` keys as `list_row.y`. A `let` inside a
  top-level `if` or `for` still keys plainly.
- **Last write wins, for the frame that ran.** A name bound in a loop reports
  its final value; a binding that never executed is absent, not null.
- **Values keep their types**: ints, floats, strings, bools, lists, records.
- **`values` is the last good frame.** A frame that raises leaves the previous
  frame's values in place (they are what is still on screen); `values_frame`
  and `values_stale` say so, and `values_partial` carries the failing frame's
  own bindings as far as it got.
- **Only the script's own bindings are reported.** Keys containing `::`
  (imports), keys starting with `_`, and function values are filtered out, so
  the petal-ui prelude does not bury your names.
- **One-frame edges** (`*_released`, `click_count`, `scroll`, `text`) are
  cleared by the next idle tick (~200 ms in headless). A script that must
  expose them to a later `GET /state` should count them into a `state` var.

### Filtering `panel.values`

Unfiltered, `values` is every binding the script made, which on a real app is
thousands of lines per request. Narrow it:

```bash
curl -s "127.0.0.1:$PORT/state?values=sel,scroll_right"   # exact names
curl -s "127.0.0.1:$PORT/state?values_prefix=obs_"         # by prefix
curl -s "127.0.0.1:$PORT/state?values=none"                # drop values entirely
```

Both selectors take a comma-separated list, may be combined, and match a
function-qualified key by its tail too (`values=y` finds `list_row.y`).
`values=all` is the explicit default. Filtering applies to `values_partial`
as well.

### Reading script output

`script.output` is every `print(...)` line from the layout script and every
panel, merged.

| Query | Returns | Cursor |
|-------|---------|--------|
| *(none)* / `?output=new` | Everything since the last draining read | moves it |
| `?output=all` | The whole retained buffer | untouched |
| `?output=<n>` | From absolute line `n` on | untouched |

Every reply carries `script.output_first` and `script.output_next` (absolute
line numbers), so a second client can resume with `?output=<output_next>`
instead of racing the drain. The session keeps 2000 lines.

```bash
curl -s '127.0.0.1:8080/state?values=none&output=all' | jq -r '.script.output[]'
```

### Text rendering health

```jsonc
"text_atlas": {"runs": 4, "distinct_sizes": 2, "dropped_batches": 0, "overflows": 0},
"unresolved_fonts": ["serif"]
```

- `text_atlas` is glyph-atlas pressure as of the last frame rendered (`null`
  before anything has been drawn). The atlas has a hard ceiling; once it is
  full a whole text batch is dropped and the frame is silently missing a line
  of text. A nonzero `overflows` or `dropped_batches` means the run is invalid
  and should not be scored.
- `unresolved_fonts` lists the font specs a panel asked for that this machine
  cannot draw. They fall back to the default monospace face and keep drawing.

### Editor `pending`: mid-command vim state

An editor pane's `pending` field is its buffered but unresolved vim state:
`null` at a clean command boundary, otherwise an object with only the buffered
parts (`count`, `operator`, `g_pending`, `z_pending`, `replace_pending`,
`find_pending`, `object_pending`).

This matters when driving the editor key by key: `mode` reads `NORMAL` even
while a command is half-typed, so a stray key left by an earlier step makes the
next command resolve against stale state. Before asserting a command's result,
check `pending` is `null`; a `POST /key {"key":"escape"}` always restores a
clean boundary.

### Stepping frames and resetting panels

| Endpoint | Body | Effect |
|----------|------|--------|
| `POST /tick` | `{"n": 60, "dt": 0.016}` | Advance every panel by `n` frames of exactly `dt` seconds each, ignoring the sleep/wake window. Both fields optional (`n: 1`, `dt: 1/60`); `n` is capped at 600. Replies with each panel's new `frame` and `clocks` |
| `POST /tick` | `{"n": 60, "advance_clock": false}` | The same, leaving the panel's `time()` clock alone |
| `POST /seed` | `{"seed": 42}` | Reseed every panel's `random()` stream, so a script that generates placeholder content draws the same content on two renders. Reset first, then seed |
| `POST /panel/reset` | — | Restart every file-backed panel from its source, discarding Petal `state`. A GPP-pushed panel has no file and is skipped |

`POST /tick` is how you drive an animation or a game: panel time advances
deterministically, with the `dt` you asked for, and no input is fabricated.

**Virtual time.** `POST /tick` switches each ticked panel onto a virtual
clock, and `time()` then advances by exactly the `dt` of each driven frame.
Frames Garden runs on its own schedule (idle repaints, settle passes before a
capture) advance it by nothing, so two identical tick sequences draw the
identical frame. That is what makes a golden image of a moving UI stable:

```bash
curl -s -XPOST 127.0.0.1:8080/panel/reset
curl -s -XPOST 127.0.0.1:8080/seed -d '{"seed":42}'
curl -s -XPOST 127.0.0.1:8080/tick -d '{"n":30,"dt":0.016}'
curl -s -o frame.png '127.0.0.1:8080/screenshot?pane=0'   # byte-identical each run
```

The switch is one-way per panel: an interactive run never calls `/tick` and
keeps the wall clock. `{"advance_clock": false}` opts a call out.

`POST /panel/reset` exists because `state` survives hot reload, which is
correct and is exactly what makes iterating on seeded data impossible in
place: you edit the generator, the old value is restored from state, and
nothing appears to change. Reset instead of restarting the process.

For a long-running panel, pair these with `--panel-wake`: a panel sleeps ten
seconds after its last activity, which is wrong for a running game. `garden
--panel-wake` never sleeps; `--panel-wake 60` sets the window in seconds. A
script can also keep itself awake with `request_frame()`; see
[petal-graphical-panels.md](petal-graphical-panels.md#request_frame-staying-awake-while-animating).

### Asserting on the scene

`/scene` is the numeric complement to `/screenshot`. Every primitive carries
`id` (its index in the draw-command stream, stable within a frame and the same
under `?pane=<n>`), `clip`, and `visible` (whether anything of it survives its
clip). A panel that clips a scrolling list still emits the rows above and
below it, so `visible` is how a test tells a drawn row from a clipped-away
one. `sceneTextCount` / `sceneVisibleTexts` in `tools/lib/debug-client.ts`
count only visible runs.

A **text** run:

```jsonc
{"type": "text", "id": 7,
 "pos": [20, 100], "text": "Hello title", "size": 24,
 "advance": 110.52,                 // measured width, in this run's own face
 "weight": 700, "italic": false, "spacing": 0,
 "font": {"family": "Inter", "weight": 700, "italic": false, "synthetic_bold": false},
 "clip": {…}, "visible": true, "color": [1, 1, 1, 1]}
```

`font` is what got rasterized, not what was asked for: for a spec this
machine cannot draw, `family` is the default monospace (and the spec is listed
in `/state`'s `unresolved_fonts`). `synthetic_bold` says the weight is faked
by over-drawing. A letter-spaced run appears as one entry per glyph.

A **mesh** (panel fills are tessellated: rounded rects, circles, lines):

```jsonc
{"type": "mesh", "triangles": 44,
 "rect": {"x": 0, "y": 0, "w": 800, "h": 600},   // bounds of the whole batch
 "color": [0.12, 0.12, 0.14, 1.0],               // the color covering the most area
 "shapes": [                                      // the fills that went into it
   {"rect": {"x": 0, "y": 0, "w": 800, "h": 600}, "color": [0.12, 0.12, 0.14, 1.0], "triangles": 2},
   {"rect": {"x": 780, "y": 40, "w": 8, "h": 220}, "color": [0.35, 0.37, 0.42, 1.0], "triangles": 42}
 ],
 "clip": {…}, "visible": true}
```

A panel batches consecutive fills into one mesh, so `rect` and `color`
describe the batch. `shapes` is the useful field: the batch split back into
runs of same-color triangles, one per `draw_*` call. "There is an 8px-wide bar
at x≈780 in the scrollbar color" is a search over `shapes`. Adjacent fills of
the identical color merge; the list is capped at 256 entries per mesh.

An **image** carries `radius`, the rounded-rect mask its corners were cut
against (`0` for a square bitmap), which is the assertion behind "the avatar
is a circle". Offscreen canvas ops appear as `canvas`, `target`, `snapshot`,
`blur`, and `canvas_draw` entries; primitives between a `target` naming a
canvas and the one switching back are in canvas coordinates.

**One pane, cropped and rebased.** `?pane=<n>` crops the capture to pane
*n*'s rect and drops every primitive outside it, translating what remains
onto the pane's own origin, so a coordinate from `/scene?pane=1` indexes
straight into `/screenshot?pane=1`. The reply carries `pane: {index, rect}`
(the rect in window coordinates) for anything that still needs to map back.
An unknown index is a 400.

```bash
curl -s -o pane.png '127.0.0.1:8080/screenshot?pane=1'
curl -s '127.0.0.1:8080/scene?pane=1' | jq '.pane, .primitives[0]'
```

### Multiple windows

The windowed frontend can host several OS windows in one process (File ▸ New
Window, `:windownew`). One debug server serves them all: every endpoint
targets one window, defaulting to the focused one. Add `?window=<ordinal>` to
any path to target a specific window (`GET /state?window=2`, `POST
/key?window=2`). The selector must be the sole or last query parameter.

Ordinals are 1-based, assigned in creation order, and never reused. `GET
/windows` lists the live ordinals. A `?window=<n>` for a window that does not
exist replies `{"ok": false, "error": "no window with ordinal n"}`.

```bash
curl -s 127.0.0.1:$PORT/windows | jq .
curl -s -X POST 127.0.0.1:$PORT/command -d '{"command":"windownew"}'   # spawn a second window
curl -s "127.0.0.1:$PORT/buffer/0?window=2"
curl -s -X POST "127.0.0.1:$PORT/key?window=1" -d '{"key":"w","mods":["cmd"]}'  # close just window 1
```

`tools/multi-window-integration-test.ts` drives this end to end (windowed
only).

## Frame consistency

`/screenshot` and `/scene` follow a settle-then-capture contract, the same in
every run mode: before the scene is built, panel frames are run in a bounded
loop until no panel's drawn output changes, or until 10 passes. Settle passes
run back to back with `dt ≈ 0`, so animations are not fast-forwarded, and
sleeping panels are untouched. The capture itself is atomic.

So **input then screenshot needs no sleep**: a `POST /key` reply means the
input was applied, and an immediately following `GET /screenshot` reflects it,
including panel state that takes an extra frame or two to propagate. Not
covered: data a GPP panel fetches asynchronously from its subprocess. Poll
`/state` until the panel's `values` show it.

Every scene build gets a global frame number, monotonically increasing for the
app's lifetime (unlike a panel's `frame`, which resets when its script
reloads). It is reported as the `X-Garden-Frame` header on `/screenshot`, the
top-level `frame` in `/state` and `/scene`, and by `GET /frame`. Clients that
want to order captures against presented frames (mainly windowed mode) poll
instead of sleeping:

```bash
before=$(curl -s 127.0.0.1:$PORT/frame | jq .frame)
curl -s -X POST 127.0.0.1:$PORT/key -d '{"key":"j"}' > /dev/null
until [ "$(curl -s "127.0.0.1:$PORT/frame?min=$((before + 1))" | jq .reached)" = true ]; do :; done
```

The server never blocks a request on a future frame, since requests are
answered on the event-loop thread. In headless mode you rarely need this:
`/screenshot` and `/scene` settle before answering.

## Input injection

| Endpoint | Body | Effect |
|----------|------|--------|
| `POST /key` | `{"key": "s", "mods": ["cmd"]}` | One key press through the normal dispatch. Named keys: `enter`/`return`, `tab`, `space`, `backspace`, `delete`, `escape`, `left`, `right`, `up`, `down`, `home`, `end`, `pageup`, `pagedown`. Mods: `cmd`/`super`/`meta`, `ctrl`/`control`, `shift`, `alt`/`option`. Single characters map to character keys. Add `"op": "down"` / `"op": "up"` to hold a key (below) |
| `POST /text` | `{"text": "hello\nworld"}` | Insert a string into the focused pane, replacing any selection. A newline is delivered as `enter`, a tab as `tab` |
| `POST /command` | `{"command": "Diff main"}` | Run an ex command as typed, without the leading `:`. Faster and less error-prone than typing it through `/key` |
| `POST /theme` | `{"scheme": "light"}` | Switch the color scheme (a key or label: `dark`, `light`, `brown`, `amiga`). Panels repaint from `palette()` next frame |
| `POST /mouse` | see below | Mouse press, move, release, or scroll |
| `POST /menu` | `{"action": "Save"}` | Fire a native menu item by name. See [Native menu](#native-menu) |

`POST /mouse` ops (`x`/`y` in logical pixels, window-relative, the same units
as the rects in `/state`):

```jsonc
{"op": "click", "x": 80, "y": 30}                      // press + release
{"op": "click", "x": 80, "y": 30, "mods": ["shift"]}   // shift-click extends the selection ("shift": true also works)
{"op": "click", "x": 80, "y": 30, "mods": ["cmd"]}     // cmd-click a shape on a Petal IDE canvas: jump to its code
{"op": "click", "x": 80, "y": 30, "clicks": 2}         // double-click: select word (3 = line)
{"op": "drag", "x": 80, "y": 30, "to": {"x": 300, "y": 90}}  // press, move, release
{"op": "down", "x": 80, "y": 30}                       // the three phases,
{"op": "move", "x": 200, "y": 60}                      // individually, for
{"op": "up"}                                           // multi-step drags
{"op": "scroll", "x": 80, "y": 30, "lines": 3}         // vertical scroll of the pane under (x,y); negative = up
{"op": "scroll", "x": 80, "y": 30, "lines": 0.25}      // fractional, as a trackpad sends
{"op": "scroll", "x": 80, "y": 30, "cols": 3}          // horizontal scroll; negative = left
{"op": "click", "x": 80, "y": 30, "button": 1}         // right click: the context-menu gesture
{"op": "down",  "x": 80, "y": 30, "button": 1}         // its phases, for menu-open-then-choose
{"op": "up", "button": 1}
```

`mods` takes the same names as `/key`, and every modifier named is delivered,
to the editor and to a panel script's `mod_alt()` / `mod_cmd()` alike.
`button` (0 = left, 1 = right) applies to `click`, `down`, and `up`. A right
press routes only to panel panes, where the script sees `mouse_pressed(1)`; it
places no cursor and starts no drag. `clicks` (default 1) applies to `click`,
`down`, and `drag`: 2 selects the word under the point, 3 the whole line; a
panel script reads it as `click_count()`. Real frontends count clicks by
timing; the debug server takes the count explicitly so tests do not depend on
it.

**Panels.** A focused panel receives `/key`, `/text`, and `/mouse` through
the normal dispatch: a key is forwarded by name (and as `text_input()`, unless
it is a `Cmd`/`Ctrl`/`Alt` chord), a press starts a drag captured by the
panel, and the wheel feeds `scroll_y()` / `scroll_x()`. Cmd/Ctrl chords are
the editor's shortcuts and are not forwarded unless the script claimed them
with `claim_key`; see
[petal-graphical-panels.md](petal-graphical-panels.md#claim_key-a-panels-own-command-keyspace).
The pane is ticked immediately, so `POST /key` then `GET /state` reflects the
change.

**`/text` into an editable panel region.** When the focused pane is a panel
and its focused region is an `edit_view` (the `garden diff` unified stream and
after column), each character is fed through the region's vim state machine,
so injected typing performs real edits. Text is therefore interpreted in
whatever vim mode the region is in. Enter insert mode first:

```bash
curl -s -X POST $BASE/mouse -d '{"op":"click","x":343,"y":178}'   # focus the region
curl -s -X POST $BASE/key  -d '{"key":"i"}'                       # insert mode
curl -s -X POST $BASE/text -d '{"text":"HELLO"}'                  # real typing
curl -s -X POST $BASE/key  -d '{"key":"escape"}'
curl -s -X POST $BASE/key  -d '{"key":"s","mods":["ctrl"]}'       # fold back to the files
```

### Holding a key

A plain `POST /key` is a tap: the press and its release land in the same
frame, so `key_down(k)` is never true by the time a later `GET /state` reads
it. `{"key": "w", "op": "down"}` presses and holds: the key stays in the
focused panel's `key_down(...)` and in `panel.input.keys_down` until `{"key":
"w", "op": "up"}` releases it.

```bash
curl -s -X POST $BASE/key -d '{"key":"w","op":"down"}'   # start moving
curl -s -X POST $BASE/tick -d '{"n": 30}'                # 30 frames of holding it
curl -s -X POST $BASE/key -d '{"key":"w","op":"up"}'     # stop
```

Holding is a panel capability: over an editor pane a `down` acts like a tap
and an `up` is dropped. `Cmd`/`Ctrl`+`Q` still quits rather than being held.

Input endpoints reply with a small acknowledgment of where things landed:

```json
{"ok": true, "focus": 0, "cursor": {"line": 9, "col": 7},
 "selection": {"anchor": {"line": 6, "col": 0}, "head": {"line": 9, "col": 7},
               "text": "...", "truncated": false}}
```

### Native menu

The macOS menu bar is the one input keystroke and mouse injection cannot
reach: its clicks are delivered by AppKit, outside the winit path. `/menu`
fires a menu item directly, so the menu wiring is verifiable headlessly.

`GET /menu` returns the catalog: `{"ok": true, "actions": [{"action":
"NewFile", "arg": null}, {"action": "SetTheme", "arg": "scheme"}, …]}`. `arg`
names the argument an action needs. `POST /menu {"action": "Save"}` fires
one by name (case-insensitive). Two kinds of item take an `"arg"`: the Open
items a filesystem path (standing in for the native picker), and `SetTheme`
a scheme key or label. The names match the `MenuAction` variants in
`garden-app/src/app/types.rs`, and each routes through the same
`App::dispatch_menu` a real click does.

```bash
curl -s 127.0.0.1:$PORT/menu | jq '.actions[].action'
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"SplitDown"}'
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"SetTheme","arg":"light"}'
curl -s -X POST 127.0.0.1:$PORT/menu -d '{"action":"OpenFile","arg":"README.md"}'
```

The reply echoes the resolved variant in an `action` field (e.g.
`"SetTheme(Light)"`). Unknown actions and a missing `arg` return a 400.

## Example session

```bash
PORT=8080
cargo run -p garden-app -- --debug-port $PORT &

curl -s 127.0.0.1:$PORT/state | jq '.panes[].title'     # what's open?
curl -s -X POST 127.0.0.1:$PORT/mouse \
     -d '{"op":"drag","x":80,"y":30,"to":{"x":300,"y":90}}'   # select by drag
curl -s 127.0.0.1:$PORT/state | jq '.panes[0].selection.text'  # what got selected?
curl -s -X POST 127.0.0.1:$PORT/text -d '{"text":"replacement"}'
curl -s -X POST 127.0.0.1:$PORT/key -d '{"key":"z","mods":["cmd"]}'  # undo
curl -s 127.0.0.1:$PORT/screenshot -o shot.png          # see the result
curl -s 127.0.0.1:$PORT/buffer/0 | diff - README.md     # buffer vs disk
```

## Which build am I talking to?

A binary that reports nothing about itself can only be probed by calling
something and reading the error, and `unknown option --panel-wake`, `no
endpoint GET /state?values=none`, and `Unknown builtin: contrast_text` all
look like "unsupported" when they mean "your `garden` is old". Ask up front:

```bash
garden --version            # version, commit, build date, features
garden --version --json     # the same as JSON
curl -s 127.0.0.1:$PORT/version | jq .features
```

```jsonc
{
  "ok": true,
  "version": "0.1.0",
  "build": {"version": "0.1.0", "commit": "216ec76", "commit_date": "2026-08-12",
            "build_date": "2026-08-12", "dirty": false, "prelude_level": 2},
  "features": ["cli.panel-wake", "state.values-filter", "debug.tick", …],
  "prelude": {"level": 2, "ui_version": 1,
              "exports": ["contrast_text/1", "text_field_update/4", "draw_text_field/4", …]}
}
```

- `features` is the list to test against: stable dotted names
  (`<area>.<feature>`) that are never renamed or removed once published. The
  full list is `HOST_FEATURES` in `garden-app/src/version.rs`.
- `prelude.exports` is derived from the petal-ui prelude compiled into this
  binary, one entry per overload, so it cannot drift from reality.
  `prelude.level` is incremented on every additive prelude change.
- A 404 from `/version` means the binary predates the endpoint.

Degrade deliberately:

```bash
if curl -sf $BASE/version | jq -e '.features | index("state.values-filter")' >/dev/null; then
  curl -s "$BASE/state?values=sel"
else
  curl -s "$BASE/state"    # older build: filter client-side
fi
```

The integration harness does this for you: `launchGarden({ requireFeatures:
["cli.panel-wake"] })` fails at launch with the build stamp in the message.

**Adding a feature flag**: append it to `HOST_FEATURES` in
`garden-app/src/version.rs` in the same commit that adds the endpoint or flag.
`cli.*` names are checked against the real argument parser by a unit test.

## Notes and limitations

- **Shared desktop** (windowed mode): the window is real and frontmost, so the
  human's mouse and keyboard can interleave with injected input. Read `/state`
  after acting instead of assuming earlier coordinates, or run `--headless`,
  which is the right choice for scripted sessions.
- Requests time out after about 5 s if the event loop is unresponsive.
- A panel whose script is broken keeps its pane and recovers by itself once
  the file is fixed.
- The server has no authentication; it binds loopback only. Don't forward the
  port.

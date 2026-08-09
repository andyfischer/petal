# Debug server

A localhost HTTP server inside the running app for **live inspection and
input injection** — the same idea as petal-sdl's `--agent` JSON protocol,
adapted to a long-running interactive app. It exists so an agent (or a
curious human with `curl`) can see what the editor is doing and drive it
deterministically, without screen-recording permissions or AppleScript
keystroke synthesis. It works in **every run mode** (window, `--term`,
`--headless`); in headless mode it is the only interface the app has.

Source: `garden-app/src/debug.rs` (server + protocol) and the
`App::handle_debug` / `state_json` handlers in
`garden-app/src/app/debug_server.rs`.

## Enabling

```bash
cargo run -p garden-app -- --debug-port 8080 # explicit port (any free port)
cargo run -p garden-app -- --debug-port 0    # OS picks a free port
cargo run -p garden-app -- --headless --debug-port 8080   # headless requires a port
```

The server binds `127.0.0.1` only. The bound port is printed on startup
(except in `--term`, where stderr would scribble on the TUI):

```
garden: debug server on http://127.0.0.1:8080
```

There is no default port: without `--debug-port <n>`, no server runs. Headless
mode is unreachable without it and exits with an error if it is omitted.

Headless mode has no window to resize and the server has no resize endpoint;
its virtual viewport defaults to 1280×850 logical pixels. Set
`GARDEN_HEADLESS_SIZE=WxH` (e.g. `700x850`) before launch to run — and
screenshot — at another size; integration tests use it to exercise
narrow/wide layouts (see `scripts/session-retro-integration-test.sh`).

## How it works

A background thread accepts connections and parses each HTTP request into a
`DebugCmd`. The command travels to whichever event loop owns the `App`
through the `debug::RequestSink` trait — a winit `EventLoopProxy` user event
for the windowed frontend, a plain mpsc channel for the headless and terminal
frontends — is handled against the live `App`, and the reply comes back over
an mpsc channel. So every command sees a consistent, current snapshot, and
injected input runs through **the same code paths as real input**
(`apply_key`, `mouse_down` / `mouse_moved` / `mouse_up`).

Screenshots never touch the OS screen-capture APIs: the current scene is
re-rendered into an offscreen wgpu texture, read back, and PNG-encoded. In
the windowed frontend this uses `Renderer::capture` (works while the window
is occluded, no permissions needed); in headless mode a surface-less
`HeadlessRenderer` is created lazily on the first request. The terminal
frontend answers `/screenshot` with its character grid as **plain text**
instead of a PNG.

## Endpoints

All JSON request bodies and responses; errors are
`{"ok": false, "error": "..."}` with a 4xx/5xx status.

### Inspection

| Endpoint | Returns |
|----------|---------|
| `GET /state` | Editor state: the global `frame` counter (see [Frame consistency](#frame-consistency)), window size/scale, cell metrics, focused pane, status-bar error, status note (file reloaded / changed-on-disk warning), drained script `print` output, and per pane: `kind` (`editor`/`process`/`panel`), file, title, `mode` (vim mode label), `pending` (mid-command vim state, see below), dirty flag, cursor, selection (anchor/head/text, text capped at 10k chars), scroll_top, scroll_sub (wrapped sub-row at the top), scroll_frac (sub-row offset in rows, `0.0..1.0` — the smooth part of the position, invisible in the two fields above), scroll_left (fractional display columns), wrap (soft-wrap on/off), line_count, visible_lines, rect, `trace_highlight` (the direct-manipulation source range under the pointer, or `null`), and — for a panel pane — a `panel` object (see below). Plus a top-level `trace`: the whole traced draw call the pointer is over — `callee`, `call` span, and per argument its `source` (`literal`/`binding`/`computed`), `value`, `is_int`, `span` (where it is written in the call) and `editable_span` (the range a rewrite must replace, at the *definition* for a binding). `null` when the pointer is over no shape. See [petal-ide-mode.md](petal-ide-mode.md#automation--headless-1) |
| `GET /buffer/<n>` | Full text of pane *n*'s buffer (`text/plain`) |
| `GET /scene` | The primitives of the current frame: quads (rect + sRGB color) and text runs (pos, text, color, clip, size — plus `weight`/`italic`/`spacing` on a run that uses them, and a letter-spaced run appears as one entry per glyph) — "what would be drawn", like petal-sdl's `capture_draw_commands`. Panels are settled first and the dump carries its `frame` number, per the same consistency contract as `/screenshot` |
| `GET /screenshot` | PNG of a **complete, settled** frame at physical-pixel size, rendered offscreen; the captured frame number comes back in an `X-Garden-Frame` response header (see [Frame consistency](#frame-consistency)) |
| `GET /frame` | `{"ok": true, "frame": n}` — the current global frame counter, answered instantly (never blocks). Optional `?min=N` adds `"reached": true/false` (`frame >= N`) for one-liner client polls |
| `GET /windows` | `{"ok": true, "windows": [{"window": 1, "focused": true, "panes": 2}, …]}` — the open OS windows by ordinal (see [Multiple windows](#multiple-windows)), which one is focused, and each one's pane count. Single-window frontends (headless, terminal) always report the one window as ordinal `1` |

A **panel** pane (a Petal graphical panel, e.g. the `:Diff` viewer) reports a
`panel` object — the hook for testing interactive panels:

```jsonc
"panel": {
  "script": "…/diff.ptl",
  "awake": true,           // within its animation wake window
  "frame": 234,            // frames run so far
  "values": {              // every value the last good frame bound, by name
    "sel": 2, "scroll_right": 45, "mode": "unified",
    "list_row.y": 88, "files": ["a.rs", "b.rs"]
  },
  "input": {               // the exact input delivered to the last frame (the
                           // uniforms petal-ui bound — read back, not editor state)
    "mouse": [120, 88],
    "keys_down": [], "keys_pressed": ["down"], "keys_released": ["down"],
    "mouse_buttons_down": [], "mouse_buttons_pressed": [], "mouse_buttons_released": [],
    "scroll": [0, 0],      // [scroll_x, scroll_y] this frame
    "modifiers": 0,        // bitmask: 1=shift 2=ctrl 4=alt 8=cmd
    "drag_active": false, "drag_start": [0, 0],
    "click_count": 0, "text": ""
  }
}
```

`panel.values` is Petal's **observation** buffer: the last value bound to every
named term the frame evaluated, so an interactive panel's logical state
(selection, scroll offset, hit rectangles) is assertable without decoding
pixels. The script does nothing to participate — a plain `let sel = 2` is
already readable as `sel`, and a `state` var is readable as itself. Three things
to know:

- **Keys are function-qualified.** A top-level `let y` keys as `y`; the same
  binding inside `fn list_row` keys as `list_row.y`. Only function bodies
  qualify, so a `let` inside a top-level `if` or `for` still keys plainly.
- **Last write wins, and only for the frame that ran.** A name bound in a loop
  reports its final value, not its history; a binding that never executed is
  *absent* rather than null.
- **Values keep their types** — ints, floats, strings, bools, lists, records.

Garden reports the script's own bindings only: keys containing `::` (imported),
keys starting with `_` (internal plumbing), and values that are functions are
filtered out, which is what keeps the `petal-ui` widget prelude from burying
your dozen names under a hundred of its own. `panel.input` is the full standard input contract the
script saw (Phase 4): pressed/released edges, held state, the drag gesture,
click count, modifiers, and typed text. A focused panel receives `/key`,
`/text`, `/mouse` (click/down/move/up/drag), and `/mouse scroll` through the
normal dispatch — a key is forwarded by name **and** as `text_input()`, a press
starts a drag captured by the panel so moves/release reach it, and the wheel
feeds `scroll_y()`/`scroll_x()`. The pane is ticked immediately, so `POST /key`
then `GET /state` reflects the change. **Note:** one-frame edges
(`*_released`, `click_count`, `scroll`, `text`) are cleared by the next idle
tick (~200ms in headless), so a script that must observe them across a later
`GET /state` has to *count* them into a `state` var rather than sample them —
that var is then observed under its own name in `panel.values`. See
`scripts/diff-review-integration-test.sh` for a full example.

### Editor `pending` — reading mid-command vim state

An editor pane's `pending` field is its buffered-but-unresolved vim state: a
partial command the state machine is still waiting to complete. It is `null` at
a clean command boundary, and otherwise an object with only the buffered parts —
`count` (a leftover numeric prefix), `operator` (`d`/`c`/`y` awaiting a motion),
`g_pending`/`z_pending`/`replace_pending`, `find_pending` (`{till, forward}` for
a pending `f`/`F`/`t`/`T`), and `object_pending` (`"inner"`/`"around"` for a
pending `i`/`a` text object).

This matters when driving the editor keystroke-by-keystroke: `mode` reads
`NORMAL` even while a command is half-typed, so on a **long-lived instance** a
stray key left by an earlier step (a lone `d`, a dangling count) makes the next
command resolve against that stale state and appear to "fail" when nothing is
broken. Before asserting a command's result, check `pending` is `null`; a single
`POST /key {"key":"escape"}` always restores a clean boundary.

### Multiple windows

The windowed frontend can host several OS windows in one process (File ▸ New
Window / `:windownew`). One debug server (one port) serves them all: every
endpoint targets **one** window, defaulting to the **focused** one so existing
single-window tooling is unchanged. Add `?window=<ordinal>` to any path to
target a specific window instead — `GET /state?window=2`, `GET /buffer/0?window=1`,
`POST /key?window=2`. The selector must be the sole query parameter or the last
one (`/frame?min=5&window=2`).

Window **ordinals** are 1-based, assigned in creation order, and never reused —
when a window closes, the survivors keep their ordinals (there is no ordinal 2
after the second window closes; the next new window is ordinal 3). `GET /windows`
lists the live ordinals and which is focused. A `?window=<n>` for a window that
does not exist replies `{"ok": false, "error": "no window with ordinal n"}`; a
non-numeric or `0` ordinal is a 400.

```bash
curl -s localhost:$PORT/windows | jq .                      # who's open, who's focused
# per-char command-line entry (see below) types :windownew
for c in : w i n d o w n e w; do curl -s -X POST localhost:$PORT/key -d "{\"key\":\"$c\"}" >/dev/null; done
curl -s -X POST localhost:$PORT/key -d '{"key":"enter"}'    # spawn a second window
curl -s "localhost:$PORT/buffer/0?window=1"                 # window 1's buffer
curl -s "localhost:$PORT/buffer/0?window=2"                 # window 2's, independently
curl -s -X POST "localhost:$PORT/key?window=1" -d '{"key":"w","mods":["cmd"]}'  # close just window 1
```

`scripts/multi-window-integration-test.sh` drives this end to end (windowed
only — it opens real OS windows).

## Frame consistency

`/screenshot` (and `/scene`) follow a **settle-then-capture contract**, the
same in every run mode (window, `--term`, `--headless`): before the scene is
built, panel frames are run in a bounded loop (`App::settle_panels`) until no
panel's drawn output changes — a fixed point, since a further frame would see
no new input — or until 10 passes, whichever comes first (a continuously
animating panel never reaches a fixed point; its capture is simply the latest
complete frame). Settle passes run back-to-back with `dt ≈ 0`, so animations
are not fast-forwarded, and sleeping panels are untouched. Capturing is itself
atomic: the settled scene is rendered into a private offscreen texture and
read back, never sampling a partially drawn surface.

The upshot: **input then screenshot needs no sleep**. A `POST /key` /
`POST /mouse` reply means the input was applied; an immediately following
`GET /screenshot` reflects it, including panel state that takes an extra frame
or two of script propagation. (Not covered: data a GPP panel client fetches
asynchronously from its subprocess — poll `/state` until the panel's
`values` show it.) `scripts/screenshot-consistency-test.sh` exercises
this contract end to end, down to decoding the PNG pixels.

Every scene build gets a **global frame number** — monotonically increasing
for the app's lifetime (unlike a panel's `frame`, which resets when its script
hot-reloads). It is reported as:

- the `X-Garden-Frame` response header on `GET /screenshot` (the body stays
  pure PNG) — the frame number of the captured scene;
- the top-level `frame` field in `GET /state` and `GET /scene`;
- `GET /frame`, an instant, never-blocking read.

Clients that want to order captures against presented frames (mainly windowed
mode, where the live render loop presents in the background) poll client-side
instead of sleeping — e.g. wait until a redraw after your input has been
built:

```bash
before=$(curl -s localhost:$PORT/frame | jq .frame)
curl -s -X POST localhost:$PORT/key -d '{"key":"j"}' > /dev/null
until [ "$(curl -s "localhost:$PORT/frame?min=$((before + 1))" | jq .reached)" = true ]; do :; done
```

The server never blocks a request on a future frame: requests are answered on
the event-loop thread, and parking it would stall the very ticking that
produces the next frame. Polling `/frame` is cheap (one JSON read per probe).
In headless mode you rarely need it — `/screenshot` and `/scene` settle before
answering, so their responses already include everything injected before them.

### Input injection

| Endpoint | Body | Effect |
|----------|------|--------|
| `POST /key` | `{"key": "s", "mods": ["cmd"]}` | One key press through the normal key dispatch. Named keys: `enter`/`return`, `tab`, `space`, `backspace`, `delete`, `escape`, `left`, `right`, `up`, `down`, `home`, `end`, `pageup`, `pagedown`. Mods: `cmd`/`super`/`meta`, `ctrl`, `shift`. Single characters map to character keys. |
| `POST /text` | `{"text": "hello\nworld"}` | Insert a string into the focused pane (replaces the selection if one is active). A focused **editable panel region** takes it one character at a time through the region's vim state machine — see below |
| `POST /mouse` | see below | Mouse press / move / release / scroll |

`POST /mouse` ops (`x`/`y` in **logical pixels**, window-relative — the same
units as the rects in `/state`):

```jsonc
{"op": "click", "x": 80, "y": 30}                      // press + release
{"op": "click", "x": 80, "y": 30, "shift": true}       // shift-click extends selection
{"op": "click", "x": 80, "y": 30, "mods": ["cmd"]}     // cmd-click a shape: jump to its code
{"op": "click", "x": 80, "y": 30, "clicks": 2}         // double-click: select word (3 = line)
{"op": "drag", "x": 80, "y": 30, "to": {"x": 300, "y": 90}}  // press, move, release
{"op": "down", "x": 80, "y": 30}                       // the three phases,
{"op": "move", "x": 200, "y": 60}                      // individually, for
{"op": "up"}                                           // multi-step drags
{"op": "scroll", "x": 80, "y": 30, "lines": 3}         // vertical scroll pane under (x,y); negative = up
{"op": "scroll", "x": 80, "y": 30, "lines": 0.25}      // fractional: a quarter of a row, as a trackpad sends
{"op": "scroll", "x": 80, "y": 30, "cols": 3}          // horizontal scroll (panel scroll_x); negative = left
{"op": "click", "x": 80, "y": 30, "button": 1}         // RIGHT click: the context gesture
{"op": "down",  "x": 80, "y": 30, "button": 1}         // its phases, for menu-open-then-choose
{"op": "up", "button": 1}
```

`mods` takes the same names `POST /key` does (`cmd`/`super`/`meta`, `ctrl`,
`shift`); `"shift": true` remains valid as the shorthand for `["shift"]`. On a
Petal-IDE canvas, `cmd`/`ctrl` turns a press on a traced shape into a jump to the
`draw_*` call that drew it (see
[petal-ide-mode.md](petal-ide-mode.md#direct-manipulation-point-at-a-shape-find-its-code)).

`button` (default 0 = left, 1 = right) applies to `click`, `down`, and `up`;
`drag` and `scroll` ignore it, since neither has a right-button form. A right
press routes only to **panel** panes, where the script sees it as
`mouse_pressed(1)` — the gesture `context_menu` opens on. It places no cursor,
starts no drag, and does not clear a focused region, so it is safe to send
mid-edit. `shift`/`clicks` are left-button concerns and are ignored for it.

**`/text` into an editable panel region.** When the focused pane is a panel and
its focused region is an `edit_view` (the `garden diff` unified stream and after
column), `POST /text` no longer goes to the script's `text_input()`: each
character is fed through the same path `POST /key` uses
(`PanelView::region_key` → `vim::handle`), so injected typing performs real
edits and records real projection splices. The consequence is that **text is
interpreted in whatever vim mode the region is in** — a `/text` sent in Normal
mode runs as normal-mode commands, exactly as the equivalent `/key` sequence
would. Enter insert mode first:

```bash
curl -s -X POST $BASE/mouse -d '{"op":"click","x":343,"y":178}'   # focus the region
curl -s -X POST $BASE/key  -d '{"key":"i"}'                       # insert mode
curl -s -X POST $BASE/text -d '{"text":"HELLO"}'                  # real typing
curl -s -X POST $BASE/key  -d '{"key":"escape"}'
curl -s -X POST $BASE/key  -d '{"key":"s","mods":["ctrl"]}'       # fold back to the files
```

A newline in the string is delivered as `enter` and a tab as `tab`. Without a
focused editable region, `/text` behaves as before — a panel gets it as
`text_input()`, an ordinary editor pane inserts it.

`clicks` (default 1) applies to `click`, `down`, and `drag`: 2 behaves like a
double-click (select the word under the point; a drag then extends word-wise),
3+ like a triple-click (select the whole line including its newline; a drag
extends line-wise). Real frontends count clicks by timing/proximity; the
debug server takes the count explicitly so tests don't depend on timing.

Input endpoints reply with a small acknowledgment of where things landed:

```json
{"ok": true, "focus": 0, "cursor": {"line": 9, "col": 7},
 "selection": {"anchor": {"line": 6, "col": 0}, "head": {"line": 9, "col": 7},
               "text": "...", "truncated": false}}
```

### Native menu

The macOS menu bar (File, Edit, View, Go, Git, Window) is the one input the app
takes that keystroke and mouse injection can't reach: muda's menu clicks and
accelerators are delivered by AppKit, outside the winit input path the debug
server drives. `/menu` fires a menu item directly, so the menu wiring is
verifiable headlessly instead of only by a live windowed launch.

| Endpoint | Body | Effect |
|----------|------|--------|
| `GET /menu` | — | The catalog of menu actions `POST /menu` accepts: `{"ok": true, "actions": [{"action": "NewFile", "arg": null}, {"action": "SetTheme", "arg": "scheme"}, …]}`. `arg` names the argument an action needs (`null` = none) |
| `POST /menu` | `{"action": "Save"}` | Fire a menu item by name (case-insensitive). Two items take an `"arg"`: the Open items a filesystem path (standing in for the native file picker), `SetTheme` a theme key or label (`dark`/`Midnight`) |

The action names match the [`MenuAction`](../garden-app/src/app/types.rs) variants
— e.g. `NewFile`, `Save`, `SaveAll`, `CloseWindow`, `Undo`/`Redo`, `Cut`/`Copy`/`Paste`,
`SelectAll`, `Find`/`FindNext`/`FindPrev`, `SetTheme`, `ToggleWrap`,
`ToggleLineNumbers`, `ToggleStateInspector`, `GoToFile`, `Back`/`Forward`,
`ExploreDirectory`, `GitLog`/`GitDiff`/`GitDiffStat`, `ReviewChanges`,
`SplitDown`/`SplitRight`, `CloseOtherPanes`/`ClosePane`, `NextPane` — but `GET /menu`
is the authoritative list. Each routes through the same `App::dispatch_menu` a
real click does, so behavior matches the menu exactly.

```bash
curl -s localhost:$PORT/menu | jq '.actions[].action'          # what can I fire?
curl -s -X POST localhost:$PORT/menu -d '{"action":"SplitDown"}'
curl -s -X POST localhost:$PORT/menu -d '{"action":"SetTheme","arg":"light"}'
curl -s -X POST localhost:$PORT/menu -d '{"action":"OpenFile","arg":"README.md"}'
```

`POST /menu` replies with the standard input acknowledgment plus an `action`
field echoing the resolved variant (e.g. `"SetTheme(Light)"`), so a fuzzy
`SetTheme` arg confirms which scheme it picked. Unknown actions and a missing
required `arg` return a 400 `{"ok": false, "error": …}`.

## Example session

```bash
PORT=8080   # any free port you like
cargo run -p garden-app -- --debug-port $PORT &

curl -s localhost:$PORT/state | jq '.panes[].title'     # what's open?
curl -s -X POST localhost:$PORT/mouse \
     -d '{"op":"drag","x":80,"y":30,"to":{"x":300,"y":90}}'   # select by drag
curl -s localhost:$PORT/state | jq '.panes[0].selection.text'  # what got selected?
curl -s -X POST localhost:$PORT/text -d '{"text":"replacement"}'
curl -s -X POST localhost:$PORT/key -d '{"key":"z","mods":["cmd"]}'  # undo
curl -s localhost:$PORT/screenshot -o shot.png          # see the result
curl -s localhost:$PORT/buffer/0 | diff - README.md     # buffer vs disk
```

## Notes & limitations

- **Shared desktop** (windowed mode only): the window is real and frontmost
  on launch, so the human's mouse/trackpad/keyboard can interleave with
  injected input (e.g. a stray trackpad scroll changes `scroll_top` between
  your commands). Read `/state` after acting instead of assuming coordinates
  from before — or run `--headless`, which has no such interference and is
  the right choice for scripted/agent sessions.
- One command per HTTP request; the connection closes after the response
  (`Connection: close`). Requests time out after ~5s if the event loop is
  unresponsive.
- `GET /state` **drains** the script's `print` output — two concurrent
  pollers will each see only part of it.
- The server has no authentication; it binds loopback only. Don't forward
  the port.

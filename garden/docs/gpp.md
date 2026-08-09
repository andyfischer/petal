# Garden Pane Protocol (GPP)

GPP lets a **child process drive the text content of a Garden pane**. The host
(garden-app) reuses its existing editor view as a passive render surface; the
subprocess pushes full-screen content and the host forwards a subscribed set of
keystrokes back. This is the "fat subprocess, thin host" model: all behavior
lives in the client, and the host just displays what it is told.

How much host behavior a client replaces is **layered and opt-in** — see
[Takeover layers](#takeover-layers) below. A light client (the default) keeps
Garden's pane behavior and just subscribes to a few keys; a heavy client takes
over the whole keyboard. But the **host command bar (`:`) and the global host
chords are reserved at every layer**, so the user can always run `:w`/`:q`/`:E`
and move focus between panes, no matter what the client does.

The reference **Lines-mode** client is `directory-browser` (`garden <dir>` opens
a navigable listing, in the spirit of vim's netrw; from an open editor, `:E` or
the `-` key does the same for the current file's directory).

`:Git` is backed by the **Panel-mode** app in `gpp-apps/git-viewers` (bin
`git-log`) — see [Panel mode](#panel-mode-script-push) below and the `:Git`
section of `docs/architecture.md`. `garden-diff` (`gpp-apps/garden-diff`) is the
panel-mode app behind **every** diff/review entry point — `:Diff [--stat]`,
`:Review`/`:Review2`/`:ReviewSplit`, `:PR`, `garden diff`, and `garden pr`: an
*editable* diff review: an editable unified stream (edit the diff to edit the
change), a read-only base side beside an editable working-tree side, a read-only
per-file stat view, and a `gh`-resolved PR mode that carries the PR description,
conversation, and inline review comments. It is the first non-read-only GPP app:
it introduced the `mutate`-based write-back (`mutate("apply", …)` → splice the
edits a view's projection resolved back into the files) and editable `edit_view`
panel regions. (It replaced two earlier read-only apps,
`git-diff` and `pr-browser`, which were retired once it covered their views.) All
panel-mode apps are [`petal_query`](../../petal-query/README.md) providers.
This document is the reference for writing future GPP clients.

Source: the `gpp` crate (shared contract), `garden-app/src/process_pane.rs`
(host side), and `gpp-apps/directory-browser/src/main.rs` (the reference Lines-mode
client); `gpp-apps/git-viewers` is the reference Panel-mode app (the step-by-step
guide is `docs/writing-gpp-apps.md`).
For how a process pane fits into the app, see the Garden Pane Protocol section of
`docs/architecture.md`.

## Transport

Newline-delimited JSON: **exactly one compact JSON object per line, with no
embedded newlines.** The host writes host → client messages to the child's
**stdin**; the client writes client → host messages to its **stdout**. The
child's **stderr** is free for its own logging (the host inherits it). Use
`gpp::write_message` / `gpp::read_message` for framing; `read_message` returns
`Ok(None)` at EOF.

Every message is a JSON-RPC 2.0 `Envelope`:

- **request** — `id` + `method` + `params`
- **notification** — `method` + `params` (no `id`)
- **response** — `id` + `result` (no `method`)

```rust
struct Envelope {
    jsonrpc: String,                  // "2.0"
    id: Option<u64>,                  // requests + responses
    method: Option<String>,           // requests + notifications
    params: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<RpcError>,          // { code: i64, message: String }
}
```

Absent fields are omitted on the wire, so the JSON matches the shape of each
message kind. Build envelopes with `Envelope::request(id, method, params)`,
`Envelope::notification(method, params)`, and `Envelope::response(id, result)`;
read typed params/results with `env.params_as::<T>()` / `env.result_as::<T>()`,
and test a method name with `env.is_method(gpp::method::RENDER)`.

All typed params use `#[serde(rename_all = "camelCase")]`, so snake_case Rust
fields map to camelCase JSON (`pane_id` ↔ `paneId`, `cursor_line` ↔ `cursorLine`).

## Key-name encoding

Keys are encoded as strings shared by host and client (`gpp::Key`, with
`to_name` / `from_name`):

- A **printable single character** is itself: `"j"`, `"/"`, `"G"`, `" "` (space).
  Letters are **case-sensitive** (`"g"` and `"G"` are distinct).
- **Named keys** use these exact strings: `"Enter"`, `"Tab"`, `"Backspace"`,
  `"Delete"`, `"Escape"`, `"Up"`, `"Down"`, `"Left"`, `"Right"`, `"Home"`,
  `"End"`, `"PageUp"`, `"PageDown"`.

A `key` notification also carries `ctrl` / `shift` / `cmd` booleans (default
false). A **reserved set** is **never forwarded** — these stay with the host
regardless of the client's keymap or takeover layer:

- `:` (with no Cmd/Ctrl) — opens the host command bar. A client cannot capture
  it even by listing `:` in its keymap; this is what guarantees `:w`/`:q`/`:E`
  keep working inside any pane.
- the Cmd/Ctrl editing chords (save, clipboard, select-all, …),
- `Ctrl+W` window navigation, and
- `Cmd`/`Ctrl`+`Q` quit.

## Takeover layers

A client declares how much host behavior to replace with the `takeover` field
of its `initialize` response (and may change it later via `setKeymap`). The
levels, lightest first:

| `takeover` | What the host forwards | What the host keeps |
| --- | --- | --- |
| `"keymap"` (default) | only the keys in `keymap` | scrolls the passive view for every other key; full command bar + chords |
| `"keyboard"` | every key except the reserved set | only the reserved set (command bar + global chords) |

`"keymap"` is the "lighter takeover": navigation-style Lines-mode clients (the
directory browser, the PR reviewer) use it, subscribing a handful of keys and
letting the host scroll and run commands. `"keyboard"` is the "almost-full takeover" for
full-screen TUI clients that want to drive all input themselves — but even they
hand `:` and the global chords back to the host. An absent `takeover` field
decodes as `"keymap"`, so older clients are unaffected.

## Messages

### Host → client

| Method (`gpp::method::*`) | Kind | Params | Meaning |
| --- | --- | --- | --- |
| `INITIALIZE` (`"initialize"`) | request (id 1) | `InitializeParams` | hand the client its pane id, size, args, cwd |
| `KEY` (`"key"`) | notification | `KeyParams` | a subscribed key was pressed while focused |
| `MOUSE` (`"mouse"`) | notification | `MouseParams` | a click landed in the pane (only if the client opted in — see [Mouse forwarding](#mouse-forwarding)) |
| `RESIZE` (`"resize"`) | notification | `ResizeParams` | the pane was resized |
| `SHUTDOWN` (`"shutdown"`) | notification | `{}` | the client should exit |

```rust
struct InitializeParams { pane_id: u64, rows: u32, cols: u32, args: Vec<String>, cwd: String }
struct KeyParams { key: String, ctrl: bool, shift: bool, cmd: bool }
struct ResizeParams { rows: u32, cols: u32 }
```

The client also exits on **stdin EOF** (the host drops stdin on shutdown).

### Client → host

| Method (`gpp::method::*`) | Kind | Params | Meaning |
| --- | --- | --- | --- |
| `INITIALIZE` (`"initialize"`) | response (id 1) | `InitializeResult` | report the pane name + initial keymap |
| `RENDER` (`"render"`) | notification | `RenderParams` | replace the full pane content |
| `SET_KEYMAP` (`"setKeymap"`) | notification | `SetKeymapParams` | replace the set of forwarded keys |
| `OPEN_PATH` (`"openPath"`) | notification | `OpenPathParams` | host swaps this pane for an editor on `path` (client is shut down) |
| `SET_STATUS` (`"setStatus"`) | notification | `SetStatusParams` | set the pane status text |

```rust
enum Takeover { Keymap, Keyboard }              // wire: "keymap" (default) / "keyboard"
struct InitializeResult {
    name: String,
    takeover: Takeover,            // how much host behavior to replace (default Keymap)
    keymap: Vec<String>,           // canonical key names (consulted only under Keymap)
    mouse: bool,                   // opt into mouse-click forwarding (default false)
}
struct RenderParams {
    lines: Vec<String>,            // the full content, one entry per row
    cursor_line: Option<usize>,    // 0-based row to select/highlight
    title: Option<String>,         // pane title (titlebar + status bar); None keeps the last
    status: Option<String>,
    styles: Option<Vec<Vec<StyleSpan>>>,      // per-line fg color spans; None/short = plain
    backgrounds: Option<Vec<Vec<BgSpan>>>,    // per-line bg bands; None/short = plain
}
struct StyleSpan { start: usize, end: usize, style: StyleKind } // char cols, end-exclusive
enum StyleKind { Added, Removed, Hunk, Title, Dim, Comment } // wire: "added", "removed", …
struct BgSpan { start: usize, end: usize, kind: BgKind }       // char cols, end-exclusive
enum BgKind { Added, Removed, Comment, Selected, Header } // wire: "added", "removed", …
struct SetKeymapParams {
    keys: Vec<String>,
    takeover: Option<Takeover>,    // change the takeover layer too, or None to leave it
    mouse: Option<bool>,           // change the mouse opt-in too, or None to leave it
}
struct OpenPathParams { path: String }
struct SetStatusParams { text: String }
```

`render` **replaces** all content; the host shows `lines` verbatim and selects
`cursorLine`. A process pane has no buffer path, so its **titlebar and status
bar show the GPP title**: the `initialize` response's `name` is used as the
title until the first `render` arrives, then each `render` with a `title`
overrides it (a `render` with `title: None` keeps the previous one). Clients
should therefore send a descriptive `title` — the browsers send the current
directory / git view (e.g. `git log — /path/to/repo`). `setKeymap` **replaces**
the forwarded-key set (the initial set
comes from the `initialize` response) and, if `takeover` / `mouse` are present,
switches the [takeover layer](#takeover-layers) / the mouse opt-in. `openPath`
ends the GPP session: the host drops the subprocess and reopens the pane as a
normal editor on `path`.

### Styled lines

`styles` optionally colors the rendered text: one span list per line (indexed
like `lines`; a missing/empty entry leaves that line plain), each span
`{start, end, style}` in char columns, end-exclusive. The six semantic styles
map to the host's theme rather than raw colors — `"added"` (diff green),
`"removed"` (diff red), `"hunk"` (hunk-header blue), `"title"` (accent),
`"dim"` (comment gray), and `"comment"` (review-comment accent) — so styled
panes match the user's palette. Omitting
`styles` entirely keeps today's plain rendering; the PR reviewer uses this to
color its file counts, diff gutters, and comment authors (`+`/`-`/`@@`/headers).

### Background bands

`backgrounds` optionally paints tinted **bands behind** the text: one span list
per line (indexed like `lines`), each span `{start, end, kind}` in char columns,
end-exclusive, drawn *under* the text (and under selection/caret). Unlike
`styles`, backgrounds are the primitive that makes a **rich diff/review UI**
possible — added/removed rows and inline comment blocks become tinted regions,
not just colored text. The five `BgKind`s map to translucent theme tints:
`"added"` (faint green), `"removed"` (faint red), `"comment"` (a muted band for
inline comment blocks), `"selected"` (the theme's selection tint, e.g. a
highlighted row), and `"header"` (a light neutral wash for section headers).
Because spans are **column-scoped**, a client composing several columns into one
flat line buffer can tint just one region — e.g. a reviewer can tint its
right-hand diff/comment area while leaving a left-hand file list untinted.
Omitting `backgrounds` entirely keeps today's plain rendering.

### Mouse forwarding

A client that sets `mouse: true` (in the `initialize` response, or later via
`setKeymap`) receives a `mouse` **notification** (host → client,
`gpp::method::MOUSE`) when a click lands in its focused pane:

```rust
struct MouseParams { line: usize, col: usize, kind: MouseKind }
enum MouseKind { Click, Double }   // wire: "click" / "double"
```

`line` is the 0-based **content row** — scroll-adjusted, an index into the last
`render`'s `lines` — and `col` the char column, clamped to the line. A
double-click's first press arrives as `"click"`, the second as `"double"`
(triple and beyond stay `"double"`). The host still handles focus changes and
wheel scrolling itself; a client that never opts in keeps the host's passive
click behavior (cursor placement), exactly as before. The browsers opt in:
click selects a row, double-click activates it (descend / open / show commit).

## Panel mode (script-push)

Everything above describes **Lines mode** (`ClientMode::Lines`, the default): the
client pushes text `render`s and the host forwards a subscribed key set. A client
may instead declare **Panel mode** (`mode: "panel"` in its `initialize` response),
which inverts the model into a "web page" shape — the client pushes a **Petal UI
script** the host runs in its in-process panel runtime, and drives it by answering
`query(kind, arg)` requests over the pipe. The full message reference is below;
the short version:

- The client replies to `initialize` with `mode: "panel"`, then immediately sends
  a **`setScript`** notification (`{ source }`) with its Petal drawer. The host
  compiles it (`PanelHost::from_source`) and renders/handles input as a
  [panel](petal-graphical-panels.md) — `takeover`/`keymap` are ignored (every
  non-reserved key/mouse/wheel goes to the script; `:` and the global chords stay
  reserved). A later `setScript` hot-reloads.
- The running script's `query(kind, arg)` calls arrive at the client as **`query`
  requests** (`{ kind, arg }`, with an id); the client answers with a
  **`queryResult`** response (`{ kind, arg, value?, error?, cacheControl? }`, the
  value a `HostData`-shaped JSON tree). While unanswered the script sees a pending
  value (spinner); the answer is cached host-side and resolves the pending on the
  next frame. The interaction/animation loop runs host-side with **no pipe
  traffic** — only data crosses the wire.
- **`cacheControl`** (optional, a `petal_query::CachePolicy`) tells the host how
  cacheable the answer is: `maxAgeMs` (freshness), `staleWhileRevalidateMs` (serve
  stale while a background refetch runs), or `noStore` (always revalidate). Absent
  = fresh forever (cache until `invalidate`), the historical default. The host's
  query cache (`petal_query::Cache`) honors it: a fresh answer is served without a
  refetch, a stale one is served *and* re-requested, an expired one falls back to
  a spinner while it refetches.
- `invalidate` (`{ kind, arg }`, client→host) drops a cached key so the script
  re-`query`s it — how a client pushes fresh data. `emit` (`{ event, arg }`,
  host→client) is the reverse push channel: the script's `emit(event, arg)`
  calls are drained once per frame tick and forwarded in call order, `arg`
  carrying any JSON tree (string/int/record/list/…). Fire-and-forget — a
  notification with no reply; a client that ignores `emit` keeps working
  unchanged. `openPath`/`setStatus` work as in Lines mode.
- **`mutate`** (`{ name, arg }`, host→client **request** with an id; answered by a
  **`mutateResult`** response `{ name, value?, error? }`) is the effectful,
  response-carrying call — the fourth quadrant beside `query` (cached pull) and
  `emit` (fire-and-forget push). Unlike `query` its result is **never cached**
  (`mutateResult` carries no `cacheControl`); unlike `emit` it returns a value.
  The host uses it for **browser-style navigation across a subprocess panel's
  screens**: a panel declares its navigable screens (`PanelUi::screen(name,
  source)`), and when the running script calls `navigate("b.ptl")` the host issues
  the built-in **`navigate` mutation** (`arg = { screen }`) to fetch that screen's
  source (`value = { screen, source }`), then drives its own history stack with it
  — the host owns the history and per-entry `state`; the client only supplies
  source. An app can register its own `navigate` handler (`Provider::on_mutation`)
  to add effects, or define other app-specific mutations — `garden-diff`'s
  `mutate("apply", { edits })` is the reference example: the drawer reads the
  write-backs the host's projection resolved for its editable `edit_view` region,
  and the subprocess splices them into the working-tree files (the first GPP
  write-back).

Panel-mode apps are [`petal_query`](../../petal-query/README.md) providers:
each declares a handler per `query(kind, arg)` returning a value + a `CachePolicy`,
and `petal-query` runs the loop above. The reference apps are in `gpp-apps/` —
`git-viewers` (bin `git-log` backing `:Git`, shelling git; full cache-policy
range), `garden-diff` (the *editable* diff review behind `:Diff`/`:Review*`/`:PR`
and the `garden diff`/`garden pr` CLIs — `edit_view` regions carrying editable
projections + a `mutate("apply", …)` write-back, the reference for a non-read-only panel app, and
network-backed via `gh` in PR mode), `session-retro` (a stateful provider with
`on_emit` persistence), `sqlite-browser` (a
read-only SQLite/Postgres browser + visualizer behind a `db::Backend` trait,
backed by `rusqlite` or the `postgres` crate per the launch arg), and
`gpp-test-app` (a fixture whose launch arg — `ok`, `runtime-error`,
`runtime-error-long`, `query-error` — drives the host into that exact panel
state, so the error card and other panel behavior can be reproduced for a
screenshot or test: `garden --subprocess gpp-test-app runtime-error`). This makes a GPP client
and an in-process panel the **same architecture** — a Petal UI script plus a query
provider — differing only in whether the provider is local Rust or a pipe proxy,
which is why `:Git`/`:Diff` ship as these panel-mode apps. **To build a panel-mode
app, follow the step-by-step guide [`docs/writing-gpp-apps.md`](writing-gpp-apps.md).**

A second panel-mode app, `gpp-apps/session-retro`, is an experimental **Claude
Code session retrospective**: it parses one transcript
(`~/.claude/projects/<slug>/<uuid>.jsonl`) and renders an annotated view — the
prompt-by-prompt step timeline with a per-step context bar, files touched, and
tool usage, split across a step-detail and a session-overview mode. It doubles as
a CLI (`session-retro --cli <session.jsonl>` prints the same report as text). All
parsing happens in the subprocess; the drawer reads it through a small set of
query kinds — `mode` (picker vs. report), `sessions` (the discovered-session list
for the picker, filtered by the arg), `open` (parse the transcript at `arg` and
make it current), `session` (the current report), `path` (the resolved transcript
path for the loading card), and `ui_state` (the persisted drawer state). The
drawer also pushes `emit("ui_state", …)` on divider drag-end, which the
subprocess persists atomically and re-serves via the `ui_state` query on the next
launch (the reference pattern for `emit`-based UI-state persistence in
[`writing-gpp-apps.md`](writing-gpp-apps.md)), and reads the live host palette via
`panel_theme()` so it recolors with the editor's color scheme. The transcript is
named either by a positional `.jsonl` path or by `--session-id <uuid>
[--project-name <path>]` (the project defaults to the launch cwd, and its slug is
Claude Code's path encoding — every non-`[A-Za-z0-9_]` char, so `/` and `.`,
becomes `-`); launched with **no** transcript it opens a searchable **session
picker** instead of erroring. Launch it as a panel with
`layout(process("/abs/session-retro", ["/abs/session.jsonl"]))`, or from the CLI
with `garden --subprocess session-retro` (picker), `… session-retro
<session.jsonl>`, or `… session-retro --session-id <uuid>`. Full user-facing
run/build instructions live in the README's Run section.

## Lifecycle / handshake

1. The host spawns the child, then writes an `initialize` **request** (id 1)
   with `InitializeParams`.
2. The client MUST reply with an `initialize` **response**
   (`Envelope::response(id, InitializeResult)`) **before sending any
   notification**. The host blocks reading exactly this one line during the
   handshake, so a client that renders first would deadlock it.
3. After responding, the client SHOULD immediately send a `render` notification
   with its initial content.
4. Thereafter the host forwards subscribed `key` notifications (and `resize`),
   and the client answers with `render` / `setKeymap` / `openPath` / `setStatus`
   notifications. The client may also push an unsolicited `render` at any time
   (the host drains process panes on its ~200ms poll tick).
5. The session ends when the host sends `shutdown`, closes stdin (EOF), or the
   client sends `openPath`. On any of these the client exits.

## Worked example — directory browser

Lines are shown pretty-printed for readability; on the wire each is one compact
line. `→` is host → client (stdin), `←` is client → host (stdout).

```jsonc
// 1. Handshake.
→ {"jsonrpc":"2.0","id":1,"method":"initialize",
   "params":{"paneId":0,"rows":40,"cols":120,"args":["/tmp/demo"],"cwd":"/tmp/demo"}}
← {"jsonrpc":"2.0","id":1,
   "result":{"name":"directory-browser","takeover":"keymap","keymap":["j","k","Up","Down","Enter","l","Right","h","Left","Backspace","-","g","G"," "]}}

// 2. Initial content. ".." sorts first, then dirs, then files; row 0 selected.
← {"jsonrpc":"2.0","method":"render",
   "params":{"lines":["> ../","  subdir/","  file_a.txt"],"cursorLine":0,"title":"/tmp/demo"}}

// 3. User presses j (subscribed) — selection moves down.
→ {"jsonrpc":"2.0","method":"key","params":{"key":"j"}}
← {"jsonrpc":"2.0","method":"render",
   "params":{"lines":["  ../","> subdir/","  file_a.txt"],"cursorLine":1,"title":"/tmp/demo"}}

// 4. Enter on "subdir/" — descend; re-render with the new listing.
→ {"jsonrpc":"2.0","method":"key","params":{"key":"Enter"}}
← {"jsonrpc":"2.0","method":"render",
   "params":{"lines":["> ../","  inner.txt"],"cursorLine":0,"title":"/tmp/demo/subdir"}}

// 5. Enter on a file — ask the host to open it. The host then shuts us down.
→ {"jsonrpc":"2.0","method":"key","params":{"key":"j"}}
← {"jsonrpc":"2.0","method":"render","params":{"lines":["  ../","> inner.txt"],"cursorLine":1}}
→ {"jsonrpc":"2.0","method":"key","params":{"key":"Enter"}}
← {"jsonrpc":"2.0","method":"openPath","params":{"path":"/tmp/demo/subdir/inner.txt"}}
→ {"jsonrpc":"2.0","method":"shutdown","params":{}}
```

After step 5 the pane is a normal editor showing `inner.txt`; the subprocess has
exited.

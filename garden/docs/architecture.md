# Garden architecture

Garden is a GPU-accelerated IDE written in Rust with [Petal](../../README.md)
as its scripting layer. The window layout is defined by a Petal script that
hot-reloads while the editor runs, and several panes (the git viewers, the
diff review, the start screen) are Petal-drawn panels whose data comes from a
subprocess.

This doc is the map: which crate owns what, and the design decisions that
shape them. The user-facing overview is the [README](../README.md).

## Design influences

- **Zed / GPUI**: the whole UI is drawn by the app as GPU primitives (quads,
  meshes, glyphs from an atlas), rebuilt into a scene on each dirty frame.
  Frames render only when something changed; there is no continuous loop.
- **Lapce / Helix**: a rope (`ropey`) as the text structure; edits grouped
  into transactions for undo/redo.
- **vim / neovim**: a thin editor core with a scripting layer in charge of
  configuration and composition, reached through a small explicit native
  API rather than by poking internals.
- **petal-sdl** (`../../integrations/petal-desktop-sdl`): the embedding
  precedent. A Petal `Env` with registered natives; the program is re-run and
  natives capture side effects; `env.hot_reload()` swaps the program while
  preserving `state`.

## Crate map

```
garden-core    text model: rope buffer, points, edits, undo, editable projections
garden-render  wgpu renderer: quads, meshes, images, canvases, text (glyphon)
garden-script  Petal embedding: layout tree from init.ptl, hot reload, the panel runtime
garden-app     app core (panes, editor views, vim, input, debug server) plus the
               window / terminal / headless frontends and ~/.garden/state
gpp            Garden Pane Protocol: the JSON-RPC contract for subprocess-backed panes
gpp-apps/      GPP client binaries, one crate per directory:
  directory-browser  netrw-style directory listing (`garden <dir>`, `:E`, `-`)
  git-viewers        the `git-log` app behind `:Git`
  garden-diff        the editable diff/review behind `:Diff`, `:Review`, `:PR`,
                     `garden diff`, `garden pr`
  sqlite-browser     read-only SQLite and Postgres browser
  main-menu          the start screen a bare `garden` opens
  screens-demo       worked example of multi-screen navigation (dev only)
  gpp-test-app       fixture that puts a pane into a chosen error state (dev only)
gpp-python/    a Python GPP client library and two sample apps
```

Dependency direction: `garden-app` depends on `garden-core`, `garden-render`,
`garden-script`, and `gpp`; those four do not depend on each other.
`garden-script` also depends on the `petal` and `petal-ui` crates one level
up in this repo (the language, and the shared input/draw/`ui`-prelude
contract every Petal embedder uses). The GPP clients depend only on
`petal-query`, which builds on `gpp`.

GPP is panel-only: every client pushes a Petal UI script that Garden runs
in-process, then answers its `query` / `mutate` / `navigate` requests over
the pipe. A GPP client and an in-process `panel(...)` pane are therefore the
same architecture, a Petal drawer plus a data provider, which is why `:Git`
and `:Diff` are themselves GPP apps. See [gpp.md](gpp.md) and
[writing-gpp-apps.md](writing-gpp-apps.md).

## garden-core: text model

Pure Rust, no graphics, built on `ropey`. `Buffer` holds the rope, the undo
stack, the file path, and the saved revision; `Point` is a line/char-column
position; `Selection` is an anchor and a head in either order.

Undo model: consecutive single-character insertions coalesce into one
transaction. Any cursor movement between edits, a switch between insertion
and deletion, or an explicit `end_undo_run()` (on save, or leaving vim Insert
mode) starts a new one. `Buffer::undo_index()` is the buffer's position in
its undo stack, stable across coalescing and moved by exactly one per
undo/redo, which is what lets a projection keep its own history in step.

`disk_changed()` and `reload()` back the external-file refresh: a clean
buffer whose file changed on disk is re-read silently; a dirty one is kept
with a warning.

### Editable projections (`garden-core/src/projection.rs`)

A projection is a document assembled out of other documents (a unified diff
mixing a file's current lines with the base lines it dropped, say) that can
be edited with the ordinary editor and written back exactly.

The decision worth knowing: **provenance, not alignment.** The obvious way to
fold an edited projection back is to diff the edited text against what was
projected and read the differences as intent. That is a guess, and it is
wrong in exactly the interesting cases. A `Projection` keeps a per-line
origin table instead and transforms it in lockstep with the buffer, so
saving is a fold, not a guess.

Each line's origin is one of: `Live { added }` (content the source holds),
`Ghost { text }` (content the base held and the source dropped; deleting the
projected line puts it back), `New` (typed fresh), or `Chrome { role, locked
}` (markers, titles, threaded comments: contributes nothing however it is
edited). A `Span` names the stretch of a source the projection covers and
may rewrite.

**The transform.** Every buffer mutation in `EditorView` funnels through one
choke point that reports the edit to the projection as a line splice.
Removed and inserted lines are paired positionally; a paired line was
retexted, a surplus removed line is hidden (resolved), a surplus inserted
line becomes `New`. The consequence is that no vim command needs projection
support: `dd`, `3dd`, `cc`, `V}d`, `p`, `J`, `x`, `.`-repeat, and insert-mode
typing all fold back correctly for free.

**The fold**, per span: a visible `Live` or `New` line contributes its buffer
text; a hidden `Live` contributes nothing (deleted from the source); a
visible `Ghost` contributes nothing (the deletion stands); a hidden `Ghost`
contributes its base text (the deletion was reverted); `Chrome` never
contributes.

**Markers.** `Decor` names the prefix each origin wears (`+`, `-`, space).
With `Decor::gutter` the markers live outside the buffer: the text is the
sources' own and `EditorView` draws the glyphs in a gutter column from the
origin table. That is what makes a projected diff edit like a file rather
than like a patch (`J` joins two added lines without dragging a `+` into the
seam). `garden-diff`'s editable views use it.

**Undo** restores the origins, not only the text: a `-` line brought back by
undoing its deletion is a deletion again. The projection keeps its own
history of inverse splices keyed to `Buffer::undo_index()`.

**Intents.** Some edits are requests about structure: deleting a hunk header
to revert the hunk, deleting a file header to drop that file's changes.
`EditorView::delete_lines` offers the edit to the projection first, which
answers `Pass`, `Refused(why)` (shown in the status bar), or `Claimed { ops }`.

## garden-render: GPU renderer

`wgpu` + `winit` + `glyphon` (cosmic-text shaping and a glyph atlas). The
renderer knows nothing about editors; it draws a `Scene` of primitives:
`Quad`, `Text`, `Mesh` (CPU-tessellated triangles with per-vertex color,
GPU-scissored and cut against a rounded-rect mask), `Image` (a cached PNG),
and the canvas ops (`Canvas`, `Target`, `Snapshot`, `Blur`, `CanvasDraw`)
that give panels offscreen layers.

Points worth knowing:

- **Draw order is scene order**, across kinds as well as within one. The
  primitive list is split into maximal same-kind runs and each run is drawn
  by its own pipeline at its point in the pass, so a panel can draw an
  overlay over its own labels.
- **Color is sRGB end to end.** The scene renders into a non-sRGB format, so
  alpha blending mixes gamma-encoded values, the space CSS, Core Graphics,
  and Figma blend in (50% black over white is `#808080`). Text agrees
  (glyphon in `ColorMode::Web`). `tests/srgb_compositing.rs` pins the
  numbers.
- **Canvases** are textures kept across frames by id and size. Drawing into
  one is its own render pass, interleaved in painter's order. Canvases hold
  premultiplied color; the blur is a separable Gaussian, downsampled for
  large radii.
- **One `GpuContext` per process.** The first window creates the wgpu device;
  later windows build a `Renderer::with_context` on it. Pipelines, atlas, and
  per-frame buffers stay per renderer.
- **Fonts** (`fonts.rs`): JetBrains Mono and Inter are embedded; any other
  family is resolved against the machine's font database on demand. The
  embedded roles are pinned to the shipped cuts because the editor's column
  arithmetic depends on their advances.
- `HeadlessRenderer` renders to an offscreen texture with no window, for the
  debug server's `/screenshot`. `cell_metrics()` measures the monospace cell
  by CPU shaping so windowless frontends share the windowed layout math.

## garden-script: Petal embedding

Owns the `petal::env::Env`. Loads `init.ptl`, registers natives, runs the
program, and extracts the declared layout. Watches the file and hot-reloads,
preserving Petal `state`.

```rust
pub enum LayoutNode {
    Row { children, ratios },      // side by side, left to right
    Column { children, ratios },   // stacked, top to bottom
    Editor { file },               // a text pane
    Process { command, args },     // a GPP subprocess-driven pane
    Panel { script },              // a Petal-drawn pane
}
```

`ScriptHost` is not `Send` (its `Env` holds interpreter state), so it is
created and polled on one thread. Each `ScriptHost` owns its own `Env` and
reads the emitted layout and theme back from that `Env`'s output buffers, so
several hosts (one per OS window) coexist with no shared capture slot.

### The script API

```petal
// init.ptl: declarative layout, re-evaluated on every hot reload
layout(
    row([
        column([ editor("src/main.rs"), editor("notes.md") ], [0.7, 0.3]),
        editor("README.md"),
    ], [0.6, 0.4])
)

color_scheme("dark")             // "dark", "light", "brown", "amiga"
color_theme({                    // override any subset of the scheme's colors
    window_bg: "#090a0d",
    text: "#e4eaf3",
    selection: "#4f8cc94d",      // "#rgb", "#rrggbb", or "#rrggbbaa"
})
```

- `editor(path?, config?)`: `config` keys are `line_numbers` (default false)
  and `wrap` (default true). Both round-trip through the layout rewrite; only
  non-default keys are emitted.
- `process(command, args?)`: a GPP pane. `args` is a list of strings.
- `panel(script)` / `panel(script, { screens: [...] })`: a Petal-drawn pane.
  `script` resolves relative to the layout script's directory. `screens`
  narrows the navigation allowlist (by default any `.ptl` in the sketch's
  own directory) to the listed names; it never widens access.
- `row(children, ratios?)` / `column(children, ratios?)`: plain records, so
  scripts can store and pass them.
- `layout(node)` converts the record tree to a `LayoutNode` eagerly.
  Structural problems are hard errors; a malformed `ratios` list degrades to
  an equal split with a warning.
- `color_theme(record)` captures plain rgba keyed by field name.
  `garden-script` must not depend on `garden-render`, so `garden-app` maps
  the values onto its `Theme`. A malformed color is a warning, not fatal.
- `color_scheme(name)` selects a whole built-in palette. This is the call the
  Color Scheme menu persists.

The panel runtime (`panel.rs`: `PanelHost`, `PanelCmd`, `PanelInput`) runs
one Petal VM per panel on petal-ui's input and draw natives, with Garden's
additions (`emit`, `mutate`, `navigate`, `text_view`, `edit_view`, `palette`,
`claim_key`, `request_frame`, the panel store). `query.rs` is the async
`query` / `invalidate` channel over Petal's pending values; `panel_trace.rs`
is the hit test behind the Petal IDE's direct manipulation; `inspect.rs`
re-exports Petal's IR/bytecode/AST rendering for the IR inspector. The
script-facing contract is [petal-graphical-panels.md](petal-graphical-panels.md).

Host introspection needs no native: `PanelHost` runs the env with Petal's
observation facility enabled, which records the last value bound to every
named term, and `observed_json` reports the script's whole logical state
keyed by function-qualified name. That map is what the debug server reports
as `panes[].panel.values` and what the `:State` overlay draws. Garden filters
it to the script's own bindings so the prelude's bindings do not bury yours.

### Layout as editable state: the transient overlay

The layout is code, but it is also state the editor mutates at runtime.
Every runtime change goes through `App::sync_layout`, which reconstructs the
whole tree from the live panes (keeping the active tree's rows, columns, and
ratios, replacing every leaf with `Pane::to_layout_node`) and saves it. The
live panes are the source of truth, so `:e`, File > Open, `:E`, `:Git`, a
browser opening a file, and the `Ctrl+W` split/only/close commands all keep
the saved layout in step with the screen.

`ScriptHost::save_layout` rewrites the source rather than regenerating it,
through Petal's goal-based editing (`../../docs/goal-based-editing.md`): the
node becomes a structured call tree and one goal, "there is a top-level call
`layout(<tree>)`", updates the existing call in place via a lossless CST
splice. Comments, a `color_theme` call, helper functions, and `state`
variables survive byte for byte.

The result is written to a per-window transient overlay,
`~/.garden/state/window-<id>/window.ptl`, and the host then watches that
file. The launch config (`init.ptl`) is left untouched. Without a script (the
plain-file `$EDITOR` shape) there is no file to write, so the change updates
the in-memory layout only.

### Permanent settings

Layout changes are per-window state in the overlay. Settings (the color
scheme today) are durable and belong in the user's hand-edited
`~/.garden/init.ptl`. `ScriptHost::save_setting(&[Goal])` reads the base
config, applies goals such as `Goal::should_call("color_scheme", ["light"])`
(updating an existing call or appending one, never duplicating), and writes
it back with everything else preserved.

## garden-app: app core and frontends

The crate splits into a frontend-independent core and three presentation
targets.

**`app/` is the core (`App`)**: panes, focus, input routing, the vim and
command-line layers, scene building, and debug-command handling. It never
touches a window, GPU, or terminal. Frontends feed it translated input and a
`Viewport`, and watch its redraw / quit / close / new-window flags. The
`impl` is split by concern: `panes` (build and reposition panes, poll the
script and files), `input` (key routing), `process` (GPP message handling),
`commands` (ex commands, search, menu actions), `mouse`, `scene`,
`debug_server`, `events` (the event log), `recents`, `lsp`, with the plain
data types in `types`.

**`frontend/` is the interface**: `trait Frontend { fn run(self, config) }`.
Event loops invert control differently per platform, so the frontend gets
the whole thread. `lib.rs` exposes `run()` (CLI parsing, layout resolution,
dispatch) and `main.rs` is a thin binary over it.

| Frontend | Flag | Notes |
|---|---|---|
| `window.rs` | default | winit `ApplicationHandler` over `garden_render::Renderer`. A `WindowRegistry` hosts several OS windows; on macOS a native menu bar (`menu.rs`, `muda`) dispatches to the focused window's `App::dispatch_menu`, which reuses the shortcut and ex-command paths. |
| `terminal.rs` | `--term` | crossterm TUI; a virtual 8x16 cell and a pure grid rasterizer (`grid.rs`). Ctrl+Q force-quits. |
| `headless.rs` | `--headless` | no UI; `--debug-port` is required. `/screenshot` lazily creates a `HeadlessRenderer`. |

### Startup

`garden [options] [file or directory]`. A file argument opens directly with
no script (the `$EDITOR` shape; `App` uses a fallback layout and skips
reload polling). A directory opens the `directory-browser` app. `garden open
<path>` is the unambiguous form. With no positional argument the main menu
(`gpp-apps/main-menu`) is the layout and `~/.garden/init.ptl` loads
config-only: its color scheme applies, its `layout(...)` does not. The
script takes the layout back on `--no-menu`, on `--init <path>`, or when the
`main-menu` binary is not installed. `garden petal-ide [file.ptl]` builds
`row([editor(file), panel(file)])` (see [petal-ide-mode.md](petal-ide-mode.md)).
`garden setup <cmd>` (`setup.rs`) seeds or resets `~/.garden` without
opening a window.

Garden's own GPP clients are resolved beside the running `garden` executable
(`current_exe`'s sibling), falling back to the bare name on `$PATH`, with
`GARDEN_GIT_LOG_BIN` / `GARDEN_DIFF_BIN` overrides. `garden --subprocess
<app> [args…]` runs any client as the whole layout.

### Multiple windows

The windowed frontend hosts several OS windows in one process (File > New
Window, `:windownew`, Cmd+Shift+N). Each window is an independent workspace:
its own `App`, `WindowState` (winit window plus `Renderer`), a never-reused
window id from the state database, and its own overlay and event-log rows.
What must be singular lives on the `Handler`: one `GpuContext`, one macOS
`MenuBar`, one `SharedClipboard`, one SQLite database (each window opens its
own connection; WAL plus a busy timeout), and one debug server (every
endpoint targets the focused window by default; `?window=<n>` selects one,
`GET /windows` lists them).

"Close window" (Cmd+W, last-pane `:q`) tears down one window; "quit" (Cmd+Q,
Ctrl+Q, `:wqa`) ends the process. The terminal and headless frontends are
single-window and treat close as exit.

### Core pieces

- **Layout solve** (`layout.rs`): walks `LayoutNode` against the viewport
  rect to a list of pane rects. Pure and unit-tested.
- **`EditorView`** (`editor_view.rs`): one per editor leaf. Holds the
  `Buffer`, cursor, selection, scroll, `VimState`, and rect; renders gutter,
  selection, text runs, caret, and scrollbars. Scrolling is pixel-smooth,
  anchored to a visual row plus a fractional offset so re-wrapping above the
  viewport cannot slide content under the user. Soft wrap is on by default
  (`:set wrap` / `:set nowrap`); motions stay linewise. Syntax highlighting
  comes from `syntax.rs` (tree-sitter, about 26 bundled grammars including
  Petal via `tree-sitter-petal`), cached per buffer revision.
- **Vim layer** (`vim.rs`): a pure key-at-a-time state machine over
  `EditorView`'s public API. Normal / Insert / Visual, motions with counts,
  operators, a register bridged to the system clipboard, undo/redo, search.
  See [keybindings.md](keybindings.md).
- **Search** (`search.rs`): smartcase plain-text matching with wraparound,
  whole-word boundaries for `*` / `#`, and `substitute_line` for `:s`.
- **Command line** (`command_line.rs`): parses the `:` commands (`:e`, `:w`,
  `:q`, `:wq`, `:wa`, `:wqa`, `:x`, `:noh`, `:report`, `:E`, `:Git`, `:Diff
  [--stat] [rev]`, `:Review*`, `:PR [n]`, `:set wrap|nowrap`, `:State`,
  `:back` / `:forward`, `:windownew`, a bare line address, `:s`) and the `/`
  and `?` prompts. `App` runs them.
- **Input** (`app/input.rs`): every key from every frontend and the debug
  server funnels through `App::apply_key`. Cmd shortcuts are global; Ctrl+C /
  X / V / A / Q alias them; Ctrl+W is the vim window prefix (focus moves via
  `window_nav.rs`, and the `o` / `s` / `v` / `c` / `q` layout commands persist
  through the overlay); everything else goes to the vim layer.
- **Fuzzy file finder** (`file_finder.rs`): Cmd/Ctrl+P. A pure subsequence
  scorer over `git ls-files` (falling back to a directory walk), a modal
  overlay, and `/state` reporting for tests.
- **Clipboard** (`clipboard.rs`): a `Clipboard` trait; `SystemClipboard`
  (arboard, degrading to in-process when unavailable) in the frontends,
  `InMemoryClipboard` in tests.
- **Debug server** (`debug.rs`): opt-in via `--debug-port`. Requests reach
  the loop that owns the `App` through a `RequestSink` (an `EventLoopProxy`
  for winit, an mpsc sender otherwise). `App::handle_debug` answers
  everything except `/screenshot`, which needs a renderer. Protocol:
  [debug-server.md](debug-server.md).
- **Frame scheduling**: the core sets a redraw flag on state change; each
  frontend drains it. All frontends poll `App::poll_script` (~200 ms) for
  hot reloads and `App::poll_files` for external file changes, and tick
  awake panels (see the sleep/wake rules in
  [petal-graphical-panels.md](petal-graphical-panels.md#animation-sleep-and-wake)).
- **Status bar**: the right slot shows `script_error` (a standing
  layout-script error) over `status_error` over `status_note`; the latter
  two clear on the next key press. All three appear in `/state`.
- **LSP** (`lsp/`, `app/lsp.rs`): an early client. Document sync and
  diagnostics for `.ptl` files; completion is not wired up.

### The Petal IDE binding

`App::sync_editor_panels` (`app/panes.rs`) recompiles each `panel(...)` pane
whose script path matches a live editor pane's file from that editor's
buffer, so editing `x.ptl` beside `panel("x.ptl")` updates the canvas as you
type, `state` preserved, with a compile error keeping the last good render.
The same pairing runs backwards for direct manipulation: pointing at a shape
highlights the `draw_*` call that drew it (Petal's runtime attribution,
`panel_trace.rs` for the hit test), Cmd/Ctrl-click jumps to it, and a
Cmd/Ctrl-drag proposes source edits through `petal::direct_manipulation`
that land in the editor buffer as one undo group. IDE mode also adds a
toolbar, play/pause (freezes panel ticks, leaves the editor live), and the IR
inspector (`petal_ide/`). User guide: [petal-ide-mode.md](petal-ide-mode.md).

### State: the Garden state directory

Per-machine state lives under `~/.garden/state`, managed by `state.rs`: one
SQLite database (`db.sqlite`, bundled `rusqlite`) plus `window-<id>/`
subdirectories.

- Window ids come from an `AUTOINCREMENT` table, so they are never reused.
  `main.rs` allocates one per launch and points the layout overlay at
  `window-<id>/window.ptl`.
- Migrations are an ordered, append-only `MIGRATIONS` list keyed by
  `user_version`. Adding a table is appending one string. Never edit or
  reorder existing entries.
- Best effort: a missing `$HOME` or an unopenable database logs a warning
  and launches anyway, with the overlay falling back to a sibling of the
  script.

The recents list the main menu reads also lives here (`recents.rs`).

### Event log and `:report`

Each window records the actions it processes into the same database
(`event_log.rs`, `app/events.rs`; migration v2). `App::log_event` buffers
in memory and the ~200 ms poll flushes every 5 s in one transaction; `Drop`
flushes the tail. Logged at the central dispatch points: every key, ex
command, text injection, mouse click, file open, layout change, script
reload or error, external-file change. `:report <text>` flushes, gathers the
previous five minutes of this window's events as context, and inserts a
`reports` row. A database error never takes the editor down.

## GPP: the host side

The wire protocol is [gpp.md](gpp.md). In-tree, the host pieces are:

- **`gpp` crate**: the envelope, the typed params and results, method and
  error constants, and newline-framed `write_message` / `read_message`. It
  depends only on serde, so any client can link it alone.
- **`ProcessPane`** (`process_pane.rs`): owns the child, a buffered writer
  over its stdin, and a reader thread forwarding every envelope to an mpsc
  channel. `spawn` does the synchronous handshake (writes `initialize`,
  blocks on exactly one response, then starts the reader). It mints request
  ids and keeps the pending-request table that routes an id-correlated
  response back to what it answered. `Drop` sends `shutdown`, closes stdin,
  and reaps the child.
- **`PanelView::pump_client`** applies a drained batch: a query response
  resolves the shared `petal_query::Cache`, `setScript` hot-swaps the drawer
  with state preserved, `invalidate` drops a key, and a client `emit` becomes
  a `ClientEvent` for the reserved `open_path` / `status` events.
- **Commands that spawn apps**: `:E` / `-` open `directory-browser` in place
  on the focused file's directory; `:Git` opens `git-log`; `:Diff`,
  `:Review*`, and `:PR` open `garden-diff`. Each pane renders and handles
  input as a panel but persists in the layout as a `process(...)` node, so a
  reload, split, or restore re-spawns the app.

## garden-diff: the review

`:Diff [--stat] [rev]`, `:Review` / `:Review2` / `:ReviewSplit [base]`, `:PR
[n]`, `garden diff`, and `garden pr` all run the `garden-diff` app rooted at
the focused file's repository. It is the only GPP app that is not read-only:
the after side is edited with real vim and written back on `^S`. Its
git/diff/splice logic is `src/diff_core.rs`.

Contract:

- `query("doc", scope)` returns `{ error, base, initial_mode, editable,
  markers_in_text, files, before: {text, styles}, after: {text, styles,
  projection}, unified: {text, styles, projection}, pr }`. `scope` is `""`
  for the whole review, `"commit:<sha>"` for one commit (read-only, since its
  line numbers address a blob rather than the checkout), or `"since:<sha>"`
  for its parent against the working tree. One `git diff <base>` is
  projected three ways. Cached fresh forever; the host drives editing and
  scrolling locally.
- `mutate("apply", { edits })` splices the `{source, start, end, lines}`
  write-backs the host's projection resolved into the working-tree files.
  The drawer then invalidates `doc` so the panes refresh.
- `query("commits", "")` returns the review's own commits (`git log
  <base>..HEAD`), fetched lazily when the COMMITS view opens.

Four view modes, switched by header pills: **UNIFIED** (default, editable; the
`+`/`-`/context stream with PR review comments woven in; soft-wrapped unless
the `wrap` pill turns it off), **SPLIT** (read-only before beside editable
after), **COMMITS** (scope control; right-click for a context menu), and
**STAT** (per-file changed-lines bars). `--stat` only picks the opening mode.

Both editable views are projections with gutter markers. The unified view's
gestures:

| Gesture | Result |
|---|---|
| delete a `+` line | the addition is dropped |
| delete a `-` line | the deletion is reverted; the base line comes back |
| retext a `+` or `-` line | the new text lands |
| delete a context line | removed from the file |
| type a bare line | added |
| edit a woven review-comment line | nothing; comments are chrome |
| `dd` on a `@@@ hunk:` header | the hunk is reverted |
| `dd` on a `@@@ file:` header | every hunk of that file is reverted |
| `dd` on the title line | refused, with a status message |

The after column supports the same text edits but has no `-` lines to revive
and its `@@@` markers are locked chrome (with only the new side in hand there
is nothing to revert a hunk to). Unsaved edits do not survive switching
views: a region's editor state is pruned when it is not declared for a
frame, and a projection cannot be re-seeded from its own edited text.

**PR mode** resolves the PR via `gh pr view`, diffs the base branch's
merge-base against the working tree (three-dot semantics, matching GitHub's
"Files changed"), and shows the description and conversation in a
collapsible band. Inline review comments (`gh api .../pulls/<n>/comments`,
best effort) are woven into the unified projection at the line they target,
with orphaned threads collected in a trailing block. Because edits write to
the working tree, the PR's head branch must be checked out.

The drawer publishes its hit targets (`split_x`, `unified_x`, `stat_x`,
`pill_y`, `after_x`, `unified_body_x`, `body_top`) as plain `let` bindings
that `/state` reports, so `tools/diff-review-integration-test.ts` clicks by
geometry it reads.

## git-log: the history browser

`:Git` runs `git-log` (`gpp-apps/git-viewers`): a commit list and the
selected commit's file list stacked on the left, the selected file's line
diff on the right. Three focusable regions cycle with Tab or a click;
`j`/`k`, arrows, PageUp/PageDown, Home/End move within the focused one; the
wheel scrolls whichever region the pointer is over. Dividers drag. Clicking a
hunk header expands the file to full context. A dirty working tree shows as
a synthetic first row.

The drawer (`src/git_panel.ptl`) bakes in nothing: `query("log", "")` returns
`{ repo, branch, worktree_dirty, commits }` (capped at 400 commits);
`query("commit", hash)` returns one commit's parsed diff (`"@worktree"` for
uncommitted changes, a `@full:` prefix for full context). Commit diffs are
immutable and stay cached; the log has a short `max_age` with a stale
window; the Refresh button invalidates the log and worktree keys. The git
plumbing in `src/lib.rs` is unit-tested against scratch repos, and
`tools/git-panel-integration-test.ts` drives the whole flow.

## Not yet

A file tree and tabs. Syntax-highlighting injections (HTML with JS/CSS,
Markdown fenced code) are not driven; the `LangDef` `injections` field is
reserved for them. LSP completion. Petal-defined keybindings and event hooks.

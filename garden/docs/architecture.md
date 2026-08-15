# Garden Architecture

Garden is a GPU-accelerated IDE (not a VSCode fork) written in Rust, with
[Petal](../README.md) as its embedded scripting layer. The window layout — and over
time, keybindings, commands, and UI behavior — is *defined by a Petal script*
and hot-reloads live while the editor runs.

## Design influences

- **Zed / GPUI**: the entire UI is drawn by the app itself as GPU primitives —
  solid quads + glyphs from a texture atlas — rebuilt into a scene each dirty
  frame. Frames are only rendered when something changed (no continuous loop).
- **Lapce / Helix**: a rope (`ropey`) as the core text structure; edits are
  grouped into transactions for undo/redo.
- **vim / neovim**: the editor is a thin core with a scripting layer in charge
  of configuration and UI composition. Neovim's lesson: expose the editor to
  the script through a small, explicit API surface (our native fns), not by
  letting the script poke internals.
- **petal-sdl** (`../integrations/petal-desktop-sdl`): the embedding precedent. A Petal `Env`
  with registered native fns; the program is (re-)run and native fns capture
  side effects into host-visible structures; `env.hot_reload()` swaps the
  program while preserving `state` variables.

## Crate map (cargo workspace)

```
garden-core    text model: rope buffer, points, edits, undo      (no UI deps)
garden-render  wgpu renderer: quads + text via glyphon           (no editor deps)
garden-script  Petal embedding: layout tree from init.ptl, hot reload
garden-app     frontend-independent app core + pluggable frontends
               (window / terminal / headless), input, wiring,
               and ~/.garden/state (SQLite via rusqlite; see State below)
gpp            Garden Pane Protocol: the shared JSON-RPC contract a
               subprocess uses to drive a pane's content over stdio
gpp-apps/       GPP client binaries (one crate per dir):
  directory-browser  (Lines mode) a navigable directory listing (vim-netrw style)
  git-viewers        (Panel mode) the reference script-push app — one bin,
                     `git-log` (backs `:Git`): it pushes a Petal drawer +
                     answers query() by shelling git
  garden-diff        (Panel mode) the one diff/review tool, backing `:Diff
                     [--stat]` / `:Review*` / `:PR` and the `garden diff` /
                     `garden pr` CLIs: an editable unified stream (edit the
                     diff to edit the change), a read-only BEFORE (base)
                     beside an editable AFTER (working tree) split, a
                     read-only per-file stat view, and a `gh`-resolved PR
                     mode (description, conversation, inline review comments
                     threaded into the unified view). Both editable views are
                     projections; `^S` writes back via mutate("apply", …)
  sqlite-browser     (Panel mode) a read-only SQLite *and* Postgres browser +
                     visualizer: table/view catalog with row counts, a
                     column-aligned data grid, and an Overview bar chart —
                     engine chosen by arg (file path vs postgres:// URL) behind
                     a db::Backend trait (rusqlite / postgres crate)
  gpp-test-app       (Panel mode) a fixture: the launch arg (ok / runtime-error
                     / runtime-error-long / query-error) drives the host into
                     that panel state, so the error card etc. can be reproduced
                     for a screenshot or test
```

GPP has two client modes (`gpp::ClientMode`): **Lines** (the original — push text
`render`s; `directory-browser` uses it) and **Panel** (push a Petal UI script the
host runs, drive it by answering `query` requests over the pipe; `git-viewers`,
`garden-diff`, and `sqlite-browser` use it). Panel mode makes a GPP client and an in-process panel the
same architecture — a Petal drawer + a query provider, local Rust or pipe-proxied
— which is why `:Git`/`:Diff` are now themselves panel-mode GPP apps.
See the "Panel mode" section of
`docs/gpp.md`, and the how-to in `docs/writing-gpp-apps.md`.

Dependency direction: `garden-app → {garden-core, garden-render, garden-script,
gpp}`. The four lower crates do not depend on each other; `garden-script`
additionally depends on the upstream `petal` and `petal-ui` crates (the language
core and the standard input/draw + `ui`-prelude contract for panels). The GPP
clients (`directory-browser`, `git-viewers`, `garden-diff`, and any future one)
depend only on `gpp`.
See the Garden Pane Protocol
section below for how the host drives a subprocess-backed pane.

---

## garden-core — text model

Pure Rust, no graphics. Built on `ropey`.

```rust
/// Line/column position. `col` is a char offset within the line (not bytes).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Point { pub line: usize, pub col: usize }

/// A contiguous selection; `anchor` is the fixed end, `head` the moving end
/// (the cursor). The two may be in either order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection { pub anchor: Point, pub head: Point }

impl Selection {
    pub fn new(anchor: Point, head: Point) -> Selection;
    pub fn is_empty(&self) -> bool;
    /// `(start, end)` in document order.
    pub fn ordered(&self) -> (Point, Point);
    /// Selected col range on one line, for per-line highlight rendering:
    /// `(start_col, end_col, includes_newline)`, or None off-selection.
    pub fn cols_on_line(&self, line: usize, line_len: usize) -> Option<(usize, usize, bool)>;
}

pub struct Buffer { /* ropey::Rope + undo stack + file path + saved undo index */ }

/// The buffer's position in its undo stack. Stable across edits that fold into
/// an open group or coalesce into a run, and moved by exactly one per undo/redo
/// — which is what lets a `Projection` (below) key its own history to the
/// buffer's and stay in step through both.
pub fn undo_index(&self) -> usize;

impl Buffer {
    pub fn new() -> Buffer;
    pub fn from_str(text: &str) -> Buffer;
    pub fn open(path: &Path) -> std::io::Result<Buffer>;
    pub fn save(&mut self) -> std::io::Result<()>;
    pub fn path(&self) -> Option<&Path>;
    pub fn is_dirty(&self) -> bool;  // derived: undo position != saved revision

    /// `Some(stamp)` if the file changed on disk since the last open/save/
    /// reload (an external edit); `None` for a pathless/unreadable file.
    pub fn disk_changed(&self) -> Option<DiskStamp>;
    /// Re-read the file, replacing content and clearing undo (clean + in
    /// sync). Caller re-clamps cursor/scroll. For refreshing a clean buffer.
    pub fn reload(&mut self) -> std::io::Result<()>;

    pub fn line_count(&self) -> usize;
    pub fn line(&self, idx: usize) -> String;       // without trailing '\n'
    pub fn line_len(&self, idx: usize) -> usize;     // chars, without '\n'
    pub fn clamp(&self, p: Point) -> Point;

    /// Insert text at `p`; returns the position just after the inserted text.
    pub fn insert(&mut self, p: Point, text: &str) -> Point;
    /// Delete [start, end); returns `start` clamped.
    pub fn delete(&mut self, start: Point, end: Point) -> Point;
    /// Delete [start, end) and insert `text` there as ONE undo transaction
    /// (typing over a selection); returns the position after the new text.
    pub fn replace(&mut self, start: Point, end: Point, text: &str) -> Point;
    /// The text in [start, end), including line breaks.
    pub fn text_range(&self, start: Point, end: Point) -> String;

    /// Undo/redo one transaction; returns a cursor position to restore, if any.
    pub fn undo(&mut self) -> Option<Point>;
    pub fn redo(&mut self) -> Option<Point>;
    /// Close the open coalescing run (save boundary; leaving vim Insert mode).
    pub fn end_undo_run(&mut self);
}
```

Undo model: consecutive single-char insertions coalesce into one transaction;
any cursor movement between edits, a deletion after insertions (and vice
versa), or an explicit `end_undo_run()` (save; leaving vim Insert mode) starts
a new transaction. Keep it simple — a `Vec<Transaction>` with an index is fine.

### Editable projections (`garden-core/src/projection.rs`)

A **projection** is a document assembled out of other documents — a unified diff
mixing a file's current lines with the base lines it dropped, a multi-file grep
result, a review thread woven through a patch — that can be *edited with the
ordinary editor* and written back exactly.

The design decision worth knowing: **provenance, not alignment.** The obvious way
to fold an edited projection back is to diff the edited text against what was
projected and read the differences as intent. That is what `garden-diff` used to
do (an LCS aligner plus heuristics for pairing deletions with insertions), and it
is fundamentally a guess — by the time the text is compared, *what edit happened*
has been thrown away. A `Projection` keeps a per-line origin table instead and
transforms it in lockstep with the buffer, so saving is a fold, not a guess.

```rust
pub enum LineOrigin {
    /// Content the source holds. `added` marks it as part of the change
    /// (not untouched context) — the distinction a revert acts on.
    Live { added: bool },
    /// Content the base held and the source dropped: a deleted line, shown so
    /// it can be read. It contributes nothing while visible; *deleting* the
    /// projected line is what puts `text` back.
    Ghost { text: String },
    /// Content typed fresh into the projection.
    New,
    /// Structure, not content: markers, titles, threaded comments. Contributes
    /// nothing however it is edited, so retyping a marker cannot corrupt a save.
    Chrome { role: ChromeRole, locked: bool },
}

/// A stretch of a source the projection covers and may rewrite: folding this
/// span's lines produces the replacement for `target`.
pub struct Span { pub source: u32, pub target: (usize, usize), pub group: Option<u32> }

impl Projection {
    /// Reconcile the table with a buffer edit that replaced buffer lines
    /// `[start, start + removed)` with `inserted` fresh ones.
    pub fn splice(&mut self, start: usize, removed: usize, inserted: usize, undo_index: usize);
    /// Walk the table's own history back to the buffer's undo position.
    pub fn sync_to(&mut self, undo_index: usize);
    /// Fold back into the sources: one `SourceEdit` per span.
    pub fn resolve(&self, lines: &[String]) -> Vec<SourceEdit>;
    /// Offer a structural edit before it lands as text (tier 2, below).
    pub fn intent(&mut self, intent: Intent, undo_index: usize) -> Outcome;
    /// Per-line style names, derived from the origins — so a band follows its
    /// line through an insertion instead of drifting off it.
    pub fn line_styles(&self) -> Vec<String>;
}
```

**Tier 1 — the transform.** Every buffer mutation in `EditorView` funnels through
one choke point (`edit`/`erase`/`insert_at`), which reports the edit to the
projection as a line splice. Removed and inserted lines are paired positionally;
a paired line was retexted, a surplus removed line is *hidden* (resolved), and a
surplus inserted line becomes a `New` entry inheriting its neighbour's span. The
consequence is that **no vim command needs projection support**: `dd`, `3dd`,
`cc`, `V}d`, `p`, `J`, `x`, `.`-repeat and insert-mode typing all fold back
correctly for free. `Buffer::insert`'s coalescing fast path is preserved through
the choke point, so a typing burst is still one undo step.

`line_splice` (in `editor_view.rs`) reads an edit bounded by line starts and made
of whole lines as a *whole-line* splice. That case matters: `dd` deletes
`[(l, 0), (l+1, 0))`, and reading it generically ("2 rows touched, 1 left") would
pair the wrong surviving line.

**The fold.** Per span, in table order:

| entry | contributes |
|---|---|
| visible `Live` | its current buffer text, undecorated |
| hidden `Live` | nothing — the line was deleted from the source |
| visible `Ghost` | nothing — the deletion still stands |
| hidden `Ghost` | its base text — the deletion was reverted |
| visible `New` | its buffer text, read the same way |
| `Chrome` | nothing, however it was edited |

`Decor` names the prefix each origin wears (`"+"`, `"-"`, `" "` for a diff) and
is *tolerant*: a line still wearing the decoration its origin expects is read
straight, and one that is not is read as if the user had typed it — so retyping
` three` as `-three` deletes the line rather than writing a literal `-three`.
`EditorView::open_seed` uses the same knowledge so vim's autoindent seeds an
opened line with the projection's "added" marker instead of copying its
neighbour's marker as if it were indentation.

`Decor::gutter` moves those markers **out of the buffer**: the text is the
sources' own, the glyphs are drawn by `EditorView` in a gutter column beside it
(`line_markers()`, derived from the table so a marker follows its line through
edits), and every text-level use of the prefixes turns off — nothing is stripped
on read, nothing is prepended on revert, and a typed line is taken literally.
This is what makes a projected diff edit like a *file* rather than like a patch:
with markers in the text, `J` joins `+one` and `+two` into `+one +two` and the
fold writes a literal `+` into the source. `garden-diff`'s editable views use
it; its read-only commit-scoped views are plain `text_view`s and so keep the
classic prefixed text (they have no projection, hence no gutter to draw into).

**Undo** restores the *origins*, not only the text: a `-` line brought back by
undoing its deletion must be a deletion again, so deleting it a second time
reverts the deletion rather than dropping an addition. The projection keeps its
own history of inverse splices keyed to `Buffer::undo_index()`, which is why that
accessor exists — coalescing and undo groups make one buffer transaction absorb
many edits, and counting steps would drift.

**Tier 2 — intents.** Some edits mean something the transform cannot express,
because they are requests about *structure*: deleting a hunk header to revert the
hunk, deleting a file header to drop that file's changes. `EditorView::delete_lines`
— which every line-wise delete in the editor reaches — offers the edit to the
projection first. It answers `Pass` (carry on with the text edit), `Refused(why)`
(shown in the status bar; a locked title cannot be deleted), or `Claimed { ops }`
(a list of `RowOp`s to perform instead, the table already patched). A projection
that does not care about structure never claims anything and still works.


## garden-render — GPU renderer

`wgpu` + `winit` + `glyphon` (cosmic-text shaping, glyph atlas management).
The renderer knows nothing about editors: it draws a `Scene` of primitives.

```rust
#[derive(Clone, Copy)] pub struct Color { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }
#[derive(Clone, Copy)] pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

pub struct Vertex { pub pos: (f32, f32), pub color: Color } // logical px + sRGB

pub enum Primitive {
    /// Solid rectangle (backgrounds, cursors, borders built from 4 thin quads).
    Quad { rect: Rect, color: Color },
    /// One run of monospace text starting at `pos` (top-left), clipped to `clip`.
    Text { pos: (f32, f32), text: String, color: Color, clip: Rect },
    /// Flat-shaded triangle list (CPU-tessellated geometry), GPU-scissored to
    /// `clip`. Lines/circles/polygons tessellate into this; panels are the
    /// first consumer (see `docs/petal-graphical-panels.md`).
    Mesh { vertices: Vec<Vertex>, clip: Rect },
}

pub struct Scene { pub bg: Color, pub primitives: Vec<Primitive> }

/// The GPU handles shared across every renderer in the process: one wgpu
/// instance/adapter/device/queue. Cheaply `Clone` (the handles are internally
/// ref-counted). Renderers layer their own pipelines, glyph atlas, and render
/// target on top — those stay per-renderer (per-frame buffers are grow-only
/// and must not be shared, or two windows rendering in the same tick corrupt
/// each other's frames).
pub struct GpuContext { /* instance, adapter, device, queue */ }
impl GpuContext {
    pub fn new_headless() -> Result<GpuContext, String>; // no display handle
}

pub struct Renderer { /* GpuContext + surface + per-window GpuCore (pipelines, atlas) */ }

impl Renderer {
    pub fn new(window: Arc<winit::window::Window>) -> Renderer; // pollster::block_on inside
    /// Build a renderer on an existing shared context (how a second OS window
    /// reuses the first's device instead of opening another). See Multi-window.
    pub fn with_context(context: &GpuContext, window: Arc<winit::window::Window>) -> Result<Renderer, String>;
    pub fn gpu_context(&self) -> GpuContext; // hand the shared context to another window
    pub fn resize(&mut self, width: u32, height: u32);
    pub fn render(&mut self, scene: &Scene);
    /// Render `scene` into an offscreen texture and read it back as tightly
    /// packed RGBA8 (sRGB-encoded bytes, PNG-ready), at the window's physical
    /// size. Blocks on the GPU readback. Used by the debug server's
    /// /screenshot endpoint — no screen-recording permission needed.
    pub fn capture(&mut self, scene: &Scene) -> Capture; // { width, height, rgba }
    /// Monospace cell metrics at the configured font size:
    /// (advance_width, line_height) in logical pixels. garden-app uses this for
    /// all layout math (cursor x = col * advance, click→col = x / advance).
    pub fn cell_size(&self) -> (f32, f32);
    pub fn scale_factor(&self) -> f64;
}

/// Surface-less renderer: offscreen capture only. Builds on a `GpuContext`
/// (its own by default, or a shared one via `with_context`); used by the
/// headless frontend for /screenshot. Errors instead of panicking — without a
/// GPU, only screenshots are lost.
pub struct HeadlessRenderer { /* per-renderer GpuCore, no surface/window */ }

impl HeadlessRenderer {
    pub fn new(logical_size: (f32, f32), scale_factor: f64) -> Result<HeadlessRenderer, String>;
    pub fn with_context(context: &GpuContext, logical_size: (f32, f32), scale_factor: f64) -> Result<HeadlessRenderer, String>;
    pub fn resize(&mut self, logical_size: (f32, f32));
    pub fn capture(&mut self, scene: &Scene) -> Capture;
    pub fn cell_size(&self) -> (f32, f32);
}

/// Cell metrics measured by CPU-only font shaping (no GPU, no window), so
/// windowless frontends share the windowed renderer's exact layout math.
pub fn cell_metrics() -> (f32, f32);
```

Implementation notes (as built — wgpu 29 + glyphon 0.11, pinned in lockstep
because glyphon releases require specific wgpu versions):
- Quads: one instanced draw call from a single per-instance buffer rebuilt
  per frame (the Zed approach, simplified); a unit quad is expanded in the
  vertex shader (`quad.wgsl`). Standard (non-premultiplied) alpha blending,
  matching what glyphon uses for text.
- Meshes (`mesh.rs`, `mesh.wgsl`): a `MeshPipeline` modeled on the quad
  pipeline, but a per-frame **vertex** buffer (grow-only) of CPU-tessellated
  triangles instead of instances. Each `Primitive::Mesh` is one scissored draw:
  a triangle can't be CPU-clipped to a rect the way a quad can, so the renderer
  sets a GPU scissor (`clip` → physical px, rounded outward like glyphon's text
  bounds, clamped to the target) per mesh. Colors are linearized on the CPU
  exactly like quads. The editor uses only `Quad`; `Mesh` exists for Petal
  panels' full draw API.
- **Draw order**: primitives composite in `Scene::primitives` order, across
  kinds as well as within one. The list is split into maximal same-kind runs
  and each run is drawn by its own pipeline at its own point in the pass (text
  through one glyphon renderer per run, all sharing a single atlas). Earlier
  this was three fixed passes — quads, meshes, then text — which made text
  composite on top of every shape regardless of position, so a panel could not
  draw an overlay over its own labels. Interleaving costs one pipeline switch
  per run, and a frame alternates on the order of a hundred times, so this is a
  few extra draw calls rather than one per primitive.
- **Color space**: `Color` values are sRGB (what you read off a hex picker).
  The surface is sRGB, so the renderer converts quad/clear colors to linear
  before writing (`Color::to_linear`); glyphon does the same internally for
  text. Skipping this conversion washes out all dark colors (~5x lighter) —
  it was a real bug once.
- Text: glyphon `TextArea` per `Primitive::Text` run (one per visible line);
  glyphon owns the shaping cache + atlas; shaping buffers are pooled across
  frames. JetBrains Mono Regular is embedded via `include_bytes!`
  (`assets/`, OFL license alongside) so there is no font discovery at
  startup.
- Target: ~200 visible lines re-rendered comfortably within a 60 fps frame.
- No animation loop: callers invoke `render()` only when state changed.

## garden-script — Petal embedding

Owns the `petal::env::Env`. Loads `init.ptl`, registers native fns, runs the
program, and extracts the declared layout. Watches the file and hot-reloads
(preserving Petal `state` vars via `env.hot_reload`).

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    /// Children side by side, left→right. `ratios` sums to ~1.0 (None = equal).
    Row { children: Vec<LayoutNode>, ratios: Option<Vec<f32>> },
    /// Children stacked top→bottom.
    Column { children: Vec<LayoutNode>, ratios: Option<Vec<f32>> },
    /// A text-editor pane, optionally pre-loaded with a file.
    Editor { file: Option<String> },
    /// A pane whose content is driven by a GPP subprocess: `command` is the
    /// client binary, `args` its arguments. See the Garden Pane Protocol below.
    Process { command: String, args: Vec<String> },
    /// A pane whose pixels are drawn by a Petal sketch run every frame.
    /// `script` is the sketch path. See `docs/petal-graphical-panels.md`.
    Panel { script: String },
}

pub struct ScriptHost { /* Env, ProgramId, StackKey, file signature, last layout */ }

impl ScriptHost {
    /// Compile + run `path`; error string on compile/runtime failure.
    pub fn load(path: &Path) -> Result<ScriptHost, String>;
    pub fn layout(&self) -> &LayoutNode;
    pub fn path(&self) -> &Path;
    /// Drain collected script output (print lines + garden-script warnings).
    pub fn take_output(&mut self) -> Vec<String>;
    /// Poll the script file (mtime + size). On change: recompile, hot-reload
    /// (preserving Petal `state` vars), re-run, re-extract. Returns Ok(true)
    /// if the layout changed. On script error, keep the old layout and
    /// return Err(msg) once (quiet until the file changes again).
    pub fn poll_reload(&mut self) -> Result<bool, String>;
    /// Persist a runtime layout change. Expresses `node` as a goal-based-editing
    /// call tree (`convert::layout_to_static_value`) and applies the goal "there is a
    /// top-level call `layout(<tree>)`" (preserving every other line), writes
    /// the result to the transient overlay (the path set by
    /// `set_transient_path`, else `transient_path`), and re-points the host to
    /// watch+hot-reload from it. The overlay's parent dir is created on demand.
    /// Returns the path.
    pub fn save_layout(&mut self, node: &LayoutNode) -> Result<PathBuf, String>;
    /// Set the exact file the overlay is written to (Garden uses the per-window
    /// `~/.garden/state/window-<id>/window.ptl`).
    pub fn set_transient_path(&mut self, path: PathBuf);
}

/// Fallback overlay sibling of a script path (`init.ptl` →
/// `init.transient.ptl`); a transient path maps to itself. Only used when no
/// explicit overlay was set via `set_transient_path` (tests / non-app usage).
pub fn transient_path(path: &Path) -> PathBuf;
```

`ScriptHost` is **not `Send`** (its `Env` holds non-`Send` interpreter state),
so create and poll it on one thread. The `layout(...)` capture is **not** a
shared thread-local: each `ScriptHost` owns its own `Env`, and the emitted
layout/theme values are read back from that `Env`'s symbol-keyed output buffers
(`run_and_extract` → `env.take_output_buffer`). That is why several
`ScriptHost`s — one per OS window (see [Multiple windows](#multiple-windows-one-process-n-windows)) —
coexist on the one event-loop thread with no shared capture slot to clobber.
(garden-script's only thread-locals are the `query`/`panel` provider slots,
swapped in and out around a single synchronous `env.run`.)

### Layout as editable state — the transient overlay

The layout is code, but it is also state the editor mutates at runtime. A
runtime rearrangement (`Ctrl-W o` — "expand the focused pane to fill the
window" — or `Ctrl-W s` / `Ctrl-W v`, which splice a stacked/side-by-side split
in at the focused pane via `window_nav::replace_leaf`) is persisted, not lost on
quit: `App::apply_runtime_layout` (in `app/input.rs`) calls
`ScriptHost::save_layout`, which **rewrites the source**

**The live panes are the source of truth, not a cached layout snapshot.** It is
not only rearrangements that mutate the layout: a pane's *content* also changes
out-of-band — `:e`/File ▸ Open swaps its file, File ▸ New empties it, `:E`/`-`
and `:Git` turn it into a GPP browser, and a browser selecting a file
(`openPath`) turns it back into an editor. Each of these calls `App::sync_layout`
(`app/input.rs`), which reconstructs the whole tree from the live panes —
`layout_from_panes` keeps the active tree's rows/columns/ratios but replaces
every leaf (in solver order) with `Pane::to_layout_node`, via
`window_nav::rebuild_leaves` — and saves it. So `script.layout()` and the
overlay never drift from what is on screen, and a later split/fullscreen/reload
can't resurrect stale content for the panes it leaves alone. `window_split`
likewise builds its new tree on `layout_from_panes`, not the script's snapshot.
`ScriptHost::save_layout` then **rewrites the source**
rather than regenerating it, through Petal's goal-based editing (see
`../docs/goal-based-editing.md`): the node is expressed as a structured
call tree (`convert::layout_to_static_value`) and one goal — "there is a top-level call
`layout(<tree>)`" — updates the existing call in place via a lossless CST
splice (appending a call if the script has none). Every other fragment in the
file — comments, a `color_theme` call, helper functions, `state` vars —
survives byte-for-byte.

The result is written to a **transient overlay** that is *per window*. At
launch `main.rs::load_script` allocates a fresh window id from the state
database (see [State](#state-the-garden-state-directory)) and points
`save_layout` at `~/.garden/state/window-<id>/window.ptl` via
`set_transient_path`; the directory is created on the first save. So each
window's runtime layout persists independently, alongside the user's other
local state rather than in the base script's directory. After a save the host
watches the overlay, so further hand-edits and runtime changes all flow through
the one file. The launch config itself (`init.ptl`) is left untouched — runtime
changes never feed back into `default_config_script`. Without a script (the
plain-file/`$EDITOR` shape) there is no file to write, so the change updates the
in-memory fallback layout only.

Petal-side API (native fns registered by garden-script):

```petal
// init.ptl — declarative layout, re-evaluated on every (hot) reload
layout(
    row([
        column([ editor("src/main.rs"), editor("notes.md") ], [0.7, 0.3]),
        editor("README.md"),
    ], [0.6, 0.4])
)

// Optional: override any subset of the dark theme's colors.
color_theme({
    window_bg: "#090a0d",
    text: "#e4eaf3",
    selection: "#4f8cc94d",   // "#rgb", "#rrggbb", or "#rrggbbaa" (alpha)
    titlebar_bg: "#0e1014",   // slim titlebar band + its text
    titlebar_text: "#9aa4b2",
    syntax_keyword: "#c678dd",
})
```

The windowed and headless frontends call `App::enable_titlebar`, reserving a
slim band (`TITLEBAR_H`) at the top of the drawable area that `build_scene`
paints with the focused document's name (clear of the macOS traffic lights). On
macOS the window itself is created with a transparent, full-size content view so
that band reads as a unified, slim title bar. The terminal frontend leaves it
off.

- `editor(path?, config?)` → record `{ kind: "editor", file: path|nil,
  line_numbers: bool, wrap: bool }`. `config` is an optional record with two
  recognized keys: `line_numbers` (a bool; default `false`, i.e. no line-number
  gutter) and `wrap` (a bool; default `true` — soft line wrapping, toggled at
  runtime by `:set wrap` / `:set nowrap`). Example:
  `editor("a.rs", { line_numbers: true, wrap: false })`. Both are per-pane state
  that round-trips through the live-pane layout rewrite (only non-default keys
  are emitted, so a plain pane stays a bare `editor()`; a file-less pane with a
  non-default key serializes as `editor(nil, { wrap: false })`).
- `process(command, args?)` → record `{ kind: "process", command: string,
  args: list|nil }`. A pane backed by a GPP subprocess (`command` spawned with
  `args`); the subprocess pushes content over the Garden Pane Protocol. `args`
  must be a list of strings or nil; a missing/non-string `command` is a hard
  error. Example: `process("directory-browser", ["src"])`.
- `panel(script)` / `panel(script, { screens: [...] })` → record
  `{ kind: "panel", script: string, screens: list|nil }`. A pane whose
  pixels are drawn by a Petal sketch run every frame; `script` is the sketch
  path, resolved relative to the layout script's directory. A missing/non-string
  `script` is a hard error. The optional config record's one key, `screens`, is
  an explicit navigation allowlist for the browser-style history API
  (`navigate(...)`): when present it *narrows* the default implicit allowlist
  (any `.ptl` in the sketch's own directory) to exactly the listed screen names;
  a listed entry still must pass the same traversal / `.ptl` / existence checks,
  so declaring it never widens access. Non-string `screens` entries are a hard
  error. Example: `panel("screens/home.ptl", { screens: ["home.ptl", "detail.ptl"] })`. The
  per-frame runtime lives in `garden-script`'s `panel` module
  (`PanelHost`/`PanelCmd`/`PanelInput`); the animation sleep/wake heuristics and
  rasterization live in `garden-app`'s `panel_view` (`PanelView`) + `panel_tess`
  (pure CPU tessellation of geometry into mesh triangles) — see
  `docs/petal-graphical-panels.md`. **The input/draw contract and the widget
  prelude come from the upstream `petal-ui` crate** (`../petal-ui`, the
  standard shared by every Petal embedder): `PanelHost` registers
  `petal_ui::input::register_input` + `draw::register_draw` + `register_prelude`
  (the `ui` module — hit-testing, `button`, `list_update`, `scroll_update`,
  `truncate_tail`, `wrap`, `preview`, `fit_parts`, `ensure_visible_px`,
  `text_width` — as an implicit import), drains
  `petal_ui::draw::take_draw_commands`, and projects each `DrawCommand` onto
  Garden's `PanelCmd` **carrying the full field set** — per-primitive alpha,
  corner `radius`, and stroke `width` — so a Garden panel renders the whole
  blessed draw vocabulary (translucent fills, rounded rects, thick strokes),
  not a truncated one; `panel_tess` rasterizes the extra fields (alpha via
  `Color::rgba`, `rect_rounded`, `width`-parameterized strokes). Host
  introspection needs no native at all: `PanelHost` runs the env with Petal's
  **observation** facility enabled (`env.observations_mut().enable()`), which
  records the last value bound to every named IR term, so
  `PanelHost::observed_json` can report a script's whole logical state — keyed by
  function-qualified source name (`sel`, `list_row.y`), values as real JSON — with
  the script doing nothing to publish it. That map is what the debug server
  reports as `panes[].panel.values` and what the `:State` overlay draws; Garden
  filters it to the script's own bindings (dropping `::`-qualified imports,
  `_`-prefixed plumbing, and function values) so the `petal-ui` prelude's ~95
  bindings don't bury the dozen that are yours. Beside that, Garden adds a
  small native set beside the standard one — `emit(event, arg)` (the fire-and-forget script→client push channel of
  panel-mode GPP: `PanelHost::take_emitted` drains each frame's events in call
  order as `(String, serde_json::Value)` pairs and `PanelView::tick` forwards
  them to the pane's subprocess as GPP `emit` notifications; no reply, and
  dropped in a panel with no attached client), the browser-style **history
  navigation** API — `navigate(screen)` / `navigate(screen, arg)` (push, the
  second form carrying the subject the target screen is for, read back there with
  `nav_arg()` and stored per history entry so back/forward keep it),
  `navigate_replace(screen)`
  (replace in place), `navigate_back()` / `navigate_forward()` — which push
  typed `NavIntent`s onto a separate `nav_events` channel (drained by
  `PanelHost::take_nav`, surfaced from `PanelView::tick` as
  `ClientEvent::Navigate`, and acted on by `App::drain_panel_nav` →
  `App::navigate_panel`; the same `navigate_panel` is also driven host-side by
  `App::nav_focused_panel` for the back/forward affordances — `Ctrl+[` / `Ctrl+]`
  and the `:back` / `:forward` ex commands — which walk the focused panel's stack);
  the app owns a per-pane history stack
  (`PanelView`'s `Vec<HistoryEntry>` + cursor), scopes Petal `state` **per
  history entry** so *back* restores the value a screen held when it was left
  (`PanelHost::restore_state` re-seeds the slot before the first frame, which
  the `StateInit`-skip preserves), and resolves each target against a
  script-directory whitelist — the panel's origin `.ptl` directory, optionally
  narrowed by an explicit `screens: [...]` list (traversal/absolute/symlink/
  off-list targets are refused) for an in-process `panel(...)` pane, while a
  **subprocess** (pushed-script) pane instead fetches the target's source from its
  client over the built-in `navigate` **mutation** (`ProcessPane::send_mutate` →
  `PanelView::client_fetch_screen`; the client's declared `PanelUi::screen`s are
  its allowlist) — the host still owns the history stack, the client only supplies
  source (a subprocess seed is source-backed via `set_origin_source` so *back* home
  rebuilds from the pushed source); the seed history entry rebuilds from the
  origin's real path so *back* home keeps hot-reload, and the
  `text_view`/`text_view_line_styles` pair (natively-selectable read-only text
  regions) with `text_view_scroll_to(id, line)` to drive one from outside (a
  file list beside a diff jumping it to the clicked file — a one-shot *action*
  the host applies and drops, so emit it only on the navigating frame) and
  `text_view_wrap(id, wrap)` to soft-wrap one (frame state, opt-in per region:
  wrapping suits a single full-width body but would slide a row-aligned pair of
  regions out of step), and the
  host-theme pair `palette()`/`panel_theme()` (the host UI
  theme as a record of `{r,g,b,a}` color records, sRGB 0–255, keyed by semantic
  name — `window_bg`/`panel`/`text`/`text_dim`/`accent`/`sel`/`hover`/`green`/
  `added_bg`/… ; injected read-only each frame via `PanelHost::set_theme`, called
  from `PanelView::tick` with `Theme::to_panel_theme`, so a live `POST /theme`
  recolors a drawer next frame and drawers paint in the app's colors instead of a
  hardcoded palette — the plain-data carrier is `garden_script::PanelTheme`).
  `palette()` is the shared accessor every panel-mode GPP app uses: it overlays
  the injected theme onto a built-in fallback so it is always complete (every key
  resolves with no guard), where bare `panel_theme()` returns exactly what was
  injected (empty when nothing was) — plus its monospace text metric
  (`bind_text_metrics`, advance ratio 0.6). The
  `host_data(kind, arg)` pull channel is **consumed from `petal_ui::host_data`**
  (garden prototyped it; it was blessed upstream, so the host registers
  `petal_ui::host_data::register_host_data` and installs its `DataProvider` via
  `swap_data_provider` rather than carrying a fork): a panel asks the optional
  host-attached provider (`PanelHost::set_data_provider`) for a plain-data value
  tree (`PanelData` — a re-export of `petal_ui::host_data::HostData` — → Petal
  record/list; nil without a provider) so it can fetch data on demand.
  Alongside it, the **async** `query(kind, arg)` / `invalidate(kind, arg)`
  channel (`garden-script/src/query.rs`) layers React-Query-style loading on
  Petal's **pending values**: a panel attaches a `QueryProvider`
  (`PanelHost::set_query_provider`) that reports `QueryState::{Loading, Ready,
  Errored}`; the native returns the resolved value when `Ready`, else a
  `Value::Pending` the script inspects with `is_loading`/`is_error`/`error_of`/
  `??`. This is Garden's prototype of a future upstream `petal-query` (the same
  path `host_data` took); the Git history browser (`:Git`, now the `git-log`
  panel-mode GPP app) is its first user, loading its log and diffs at runtime —
  through a local background-threaded provider when the drawer runs in-process, or
  a pipe-proxy provider (`ProcessQueryProvider`) when it is pushed by a
  `git-viewers` subprocess (the drawer is identical either way).
  Garden does *not* register the optional offscreen-canvas natives, so those
  commands never appear. **Input flows through `petal-ui`'s `InputState`
  end to end** (Phase 4): `PanelHost` owns one, `garden-app` translates its
  winit/debug-server events into the re-exported `InputEvent`s
  (`garden-script` re-exports `InputEvent`/`Modifiers`/`buttons`), and
  `frame`'s `begin_frame` derives the edge/level split — so panels read
  `mouse_released`, `key_released`, `mod_*`, `drag_active`/`drag_start_*`,
  `click_count`, `scroll_x`, and `text_input`, not just press edges. `PanelInput`
  is now a read-back snapshot of the bound uniforms (host introspection, surfaced
  at `/state`), not an input path. See `../docs/embedding-guide.md`
  and `docs/petal-graphical-panels.md`.
- `row(children, ratios?)` / `column(children, ratios?)` → records (real
  Petal `Value::Map`s, so scripts can store/pass them like any value;
  hand-written record literals of the same shape also work)
- `layout(node)` → converts the record tree to a `LayoutNode` **eagerly**
  (while the values are guaranteed live on the heap — see
  `garden-script/src/convert.rs`) and emits it into this `Env`'s per-symbol
  output buffer, drained by `run_and_extract`. Structural problems are hard
  errors; a malformed `ratios` list
  degrades to an equal split with a warning. `convert.rs::layout_to_static_value` is its
  inverse, used by `save_layout` to write the layout back out via goal-based
  editing.
- `color_theme(record)` → reads a record of `field: "#hexcolor"` pairs and
  captures a `garden_script::Theme`. Colors are hex strings (`"#rgb"`,
  `"#rrggbb"`, or `"#rrggbbaa"`; alpha defaults to opaque). Optional and
  independent of `layout`: a script may set only some keys or skip it
  entirely — every unset key keeps its built-in default. A malformed color is
  reported with a warning and skipped, not fatal.
  **Cross-crate boundary:** garden-script must not depend on garden-render, so
  it captures only plain rgba (`[f32; 4]`, 0.0..=1.0) keyed by field name;
  `garden-app`'s `theme::Theme::with_script_overrides` maps each onto a
  `garden_render::Color`.
- `color_scheme(name)` → captures a base-palette name (`"dark"`, `"light"`,
  `"brown"`, `"amiga"`) as `Option<String>` (`ScriptHost::scheme`). Unlike
  `color_theme` (per-key overrides), this selects a whole built-in palette;
  `garden-app` maps it onto `theme::ThemeScheme` at startup (`ThemeScheme::from_key`,
  unknown → default). This is the call the Color Scheme menu persists — see
  **Permanent settings** below.

### Permanent settings (goal-based editing)

Layout changes are ephemeral per-window state written to the transient overlay
(`save_layout`), but **settings** (the color scheme today, font size etc. later)
are durable and belong in the user's hand-edited `~/.garden/init.ptl`.
`ScriptHost::save_setting(&[Goal])` is the standard path for these: it reads the
**base config** (`config_path`, unaffected by `save_layout` re-pointing the
watched `path` to the overlay), applies Petal's
`petal::goal_based_editing::modify_source_with_goals` (re-exported as
`garden_script::{Goal, StaticValue}`), and writes it back — every comment, the `layout`
call, and helper code preserved. Each `Goal::should_call("color_scheme", ["light"])`
either updates an existing top-level call in place or appends a new one, so
repeated changes never duplicate. Startup always loads the base config (a fresh
window id each launch), so a persisted setting is read back next run.
`garden-app`'s `App::set_theme_scheme` applies the palette in memory and then
calls `App::persist_setting` to make it stick; see `../docs/goal-based-editing.md`.

Reference material: `../rust/src/{env.rs,native_fn.rs,value.rs,heap.rs,hot_reload.rs}`
and `../integrations/petal-desktop-sdl/src/{native_fns.rs,game_loop.rs}` (see `check_hot_reload`).

## garden-app — app core & pluggable frontends

Split into a frontend-independent core and an interface for presentation
targets:

- **`app/` — the core (`App`)**: panes, focus, input routing, the vim and
  command-line layers, scene building, and debug-command handling. It never
  touches a window, GPU, or terminal: frontends feed it translated input
  (`vim::Key` + `Mods`, logical-pixel mouse positions), give it a `Viewport`
  (logical size, cell metrics, scale), and watch its `take_redraw()` /
  `should_quit()` / `should_close()` / `take_new_window_request()` flags — the
  core can *ask* to close its window or open a new one, but only a frontend
  acts (an `App` never touches an OS window). Everything below operates on this core. The one `App`
  struct lives in `app/mod.rs`; its impl is split by concern across sibling
  modules — `panes` (build/reposition panes, poll the script and files),
  `input` (key routing + text injection), `process` (GPP message handling, the
  directory browser), `commands` (ex commands, search/substitute, native menu),
  `mouse`, `scene`, and `debug_server`, with the plain data types (`Viewport`,
  `Mods`, `MenuAction`, `ClickCounter`, `Pane`) in `types`.
- **`frontend/` — the interface**: `trait Frontend { fn run(self: Box<Self>,
  config: AppConfig) -> Result<(), String> }`. Event loops invert control
  differently per platform (winit owns the main thread via callbacks;
  crossterm and the headless loop poll), so the interface hands the whole
  thread to the frontend rather than abstracting the loop. The crate is
  **lib + bin** (`lib.rs` exposes `pub fn run()`; `main.rs` is a thin binary
  over it), so the window-orchestration logic is unit-testable outside winit's
  `ApplicationHandler`. `run()` parses the CLI, resolves the layout source into
  an `AppConfig`, and dispatches to one of three implementations:
  - **`frontend/window.rs`** (default): winit 0.30 `ApplicationHandler`
    presenting through `garden_render::Renderer`. The `Handler` holds a
    `WindowRegistry<WindowState>` (`frontend/registry.rs`) — a `WindowId`-keyed
    map with focus tracking — so one process hosts several OS windows (see
    [Multiple windows](#multiple-windows-one-process-n-windows)); events route
    by `WindowId`, `about_to_wait` polls every window (the loop wakes on the
    soonest deadline across them), and the process exits only when the last
    window closes or `should_quit` fires. On macOS it installs a native
    menu bar (`frontend/menu.rs`, `muda`) — File / Edit / View / Go / Git /
    Window, covering open/save, clipboard + find, wrap/line-number/theme
    toggles, the fuzzy finder and panel history, the git viewers (`:Git`,
    `:Diff`, `:Review`), and the `Ctrl+W` pane commands. The menu bar is
    process-global (one per process), so exactly one `MenuBar` lives on the
    `Handler`, drains once per `about_to_wait`, and routes each action to the
    **focused** window's `App::dispatch_menu`, which reuses the
    keyboard-shortcut and ex-command paths so a menu item and its shortcut do
    the same thing. `menu.rs` is a no-op stub off macOS.
  - **`frontend/terminal.rs`** (`--term`): a crossterm TUI, usable as
    `EDITOR="garden --term"`. Reports a virtual cell of 8×16 logical pixels
    per terminal cell and rasterizes each frame's `Scene` onto a character
    grid (`frontend/grid.rs`, pure and unit-tested — quads become cell
    backgrounds, text runs become glyphs; sub-cell chrome snaps to the cell
    it overlaps). Ctrl+Q force-quits; `/screenshot` answers with the grid as
    plain text.
  - **`frontend/headless.rs`** (`--headless`): no UI at all — the debug
    server is the only way in or out, so `--debug-port <n>` is required
    (the frontend errors out if it is omitted). Used for integration testing. Cell metrics
    come from `garden_render::cell_metrics()`; `/screenshot` lazily creates
    a `HeadlessRenderer` for true offscreen PNGs.

Startup: `garden [options] [file or directory]`. A file argument opens
directly in an editor pane with **no script** (the `$EDITOR` shape — `App`
then uses a fallback `LayoutNode` and skips reload polling); a single directory
opens the GPP directory browser. `garden open <path>` is the unambiguous form —
everything after `open` is a path, never a subcommand, so a file literally named
`git` or `setup` still opens. With no positional argument the **main menu**
(`gpp-apps/main-menu`, a `LayoutNode::Process` over the cwd) is the layout, and
`~/.garden/init.ptl` loads *config-only* — its color scheme and settings apply,
its `layout(...)` does not (`script_owns_layout = false`, the same shape as a
file argument). The script takes the layout back on `--no-menu`, on an explicit
`--init <path>`, or when the `main-menu` binary isn't installed
(`process_pane::client_bin_exists`), in which case the script loads as before:
`--init <path>` if given, else `~/.garden/init.ptl` if it exists, else a
single empty editor. `garden petal-ide [file.ptl]` (`resolve_petal_ide_subcommand`
in `main.rs`) is the **Petal IDE** launcher: it builds the fallback layout
`row([editor(file), panel(file)])` over one absolute path (seeding a starter
sketch for a new file, or `~/.garden/petal-ide/scratch.ptl` with no argument), so
editing the source live-updates the canvas beside it — see the **live editor↔panel
binding** under EditorView/panel below, and the user guide `docs/petal-ide-mode.md`.
Options: `--init <path>`, `--no-menu`, `--term`,
`--headless`, `--debug-port <n>` (start the debug server on port n; 0 = pick free;
no default). `garden setup <cmd>` (`setup.rs`) is the administrative side door — it never
opens a window: `initialize-config-if-missing` seeds `~/.garden` (idempotent;
run by `install-local.sh`) and `reset-config` restores the config files to
defaults while preserving `~/.garden/state` (window ids, overlays, event log).

### Multiple windows (one process, N windows)

The windowed frontend hosts several OS windows in one process (File ▸ New
Window / `:windownew`, Cmd+Shift+N). Each window is an independent workspace —
its own `App` (pane tree, focus, layout, script `Env`, undo, status line),
`WindowState` (winit `Window` + `Renderer`), and per-window persistence (a
fresh never-reused window id from `state.rs`, its own `window-<id>/window.ptl`
overlay and event-log rows). What macOS and the machine force to be singular
lives on the `Handler` as a small process shell:

- **One `GpuContext`** (`garden-render`): the first window creates the wgpu
  device; every later window builds its `Renderer` via `Renderer::with_context`
  on the same device (pipelines + glyph atlas stay per-window). See garden-render.
- **One `MenuBar`** (macOS): installed with the first window, drained once per
  tick, dispatched to the focused window.
- **One `SharedClipboard`** (`clipboard.rs`): a cheap `Rc<RefCell<…>>` handle
  every window's `App` clones, so a yank crosses windows even on the in-process
  fallback.
- **One SQLite database**: each window opens its own `Connection`; `State::open`
  enables WAL + a busy_timeout so concurrent per-window event-log flushes don't
  hit `SQLITE_BUSY`.
- **One debug server** (one port): every endpoint targets the focused window by
  default, `?window=<ordinal>` selects a specific one, `GET /windows` lists them
  (see `docs/debug-server.md`).

"Close window" and "quit process" are distinct: `KeyOutcome::CloseWindow` (Cmd+W,
last-pane `:q`/`:wq`/`Ctrl+W q`) tears down one window — the process lives on
until the last window goes — while `KeyOutcome::Quit` (Cmd+Q, Ctrl+Q, `:wqa`)
ends everything. The terminal and headless frontends stay single-window and
treat close as exit.

Core responsibilities (all frontend-independent):

- **Layout solve**: walk `LayoutNode` against the viewport rect → `Vec<(Rect, PaneId)>`
  for editor leaves (rows split x by ratio, columns split y). Pure function,
  unit-testable without a window.
- **EditorView** (one per editor leaf): `Buffer`, cursor `Point`, selection
  `anchor: Option<Point>` (selection = anchor..cursor), a `scroll: Scroll`
  position, a `vim: VimState`, and its current
  `Rect`. Renders: background quad, focused-pane border, gutter with line
  numbers, per-line selection highlights (with a half-cell tail for selected
  newlines), text lines clipped to the area right of the gutter, a block caret
  (Normal/Visual) or bar caret (Insert), and scrollbars when content overflows.
  Keeps the cursor visible when it moves, and pins the last line to the bottom
  (no scrolling into empty space).
  **Scrolling is pixel-smooth**, and `Scroll` is where that lives: `top`/`sub`
  (first visible buffer line + wrapped sub-row) name an *anchor visual row*,
  `frac` is how far into that row the viewport has moved (`0.0..1.0` rows), and
  `left` is a fractional display-column offset (only when not wrapping). The
  anchor is a row rather than an absolute pixel offset on purpose: rows above
  the viewport can re-wrap, the font size can change and lines can be
  inserted — an absolute offset would slide the content under the user on all
  three — and it keeps every scroll O(rows moved) instead of O(buffer).
  Rendering shifts each row up by `frac * cell_h`, draws one extra row to fill
  the sliver that opens at the bottom, and clips quads to the content band on
  the CPU (`push_clipped_quad`) since all quads share one instanced draw call
  and carry no scissor of their own; text is clipped by the GPU through
  `Primitive::Text`'s `clip`. Wheel deltas stay fractional all the way from
  winit (`MouseScrollDelta::PixelDelta` is *not* rounded to whole cells — that
  rounding is what made trackpad scrolling stutter and drop small deltas);
  keys, `zt`/`zz`/`zb` and cursor-visibility corrections still land on whole
  rows. Panel scripts, whose scroll state is their own, still see whole
  `scroll_y()` ticks — the host accumulates the fractions in
  `mouse::ScrollTicks` and hands over a tick at a time, so slow gestures
  accumulate rather than round to nothing.
  **Soft wrap** (`wrap`, default on for editor panes): long lines wrap to the
  pane width instead of scrolling horizontally, so a text buffer needs no
  horizontal scrollbar. A buffer line becomes several *visual rows* — `wrap_rows`
  breaks it at word boundaries (hard-breaking an over-long word), and rendering,
  cursor-visibility, click mapping, and vertical scrolling all address the
  `(line, sub)` visual-row space (`vpos_add`/`vpos_sub`). Cursor **motions stay
  linewise** (vim's `j`/`k` move whole buffer lines even when wrapped). `:set
  wrap` / `:set nowrap` toggles it per pane; turning it off restores the
  horizontal scrollbar + `scroll.left`. A GPP process pane forces wrap off (its
  render surface must stay 1:1 with the client's rows). **Scrollbars are
  draggable**: `scrollbar_geom` places the track+thumb (vertical in visual rows,
  horizontal only when not wrapping) and is shared by drawing and mouse
  hit-testing, so `App`'s `mouse::Drag::Scrollbar` maps a thumb drag straight to
  a scroll offset (`drag_scroll`) — unrounded, so the thumb follows the pointer
  by the pixel and the thumb itself glides with a sub-row scroll. **Syntax highlighting**: per-line token spans come from
  `syntax.rs` (tree-sitter-highlight), cached in a `RefCell`
  keyed by `Buffer::revision` and recomputed only when the content changes.
  Languages are a data-driven `REGISTRY` of `LangDef` entries (resolved from a
  file's name/extension by `Language::from_path`) — ~26 bundled grammars
  (Rust, Python, JS/TS/TSX, Go, C/C++, Java, C#, Ruby, PHP, HTML, CSS, Bash,
  YAML, Lua, Scala, Haskell, SQL, Zig, Nix, JSON, TOML, Markdown, and **Petal**
  itself — `.ptl`, via the `tree-sitter-petal` path dep whose reference grammar
  lives in `../editor-support/tree-sitter-petal`). Adding a language is one
  new `tree-sitter-*` dep plus one table row. Each grammar's
  compiled `HighlightConfiguration` is cached per language in `Highlighter`.
  Injections (HTML↔JS/CSS, Markdown inline, fenced code) are not driven yet —
  the single-grammar `tree_sitter_highlight::Highlighter` can't; the `LangDef`
  `injections` field is reserved for that.
  `build_scene` paints each visible line as multiple `Primitive::Text` runs,
  one per consecutive same-color stretch; token colors come from the
  `theme.syntax_*` fields. An unhighlighted file (unknown extension) renders as
  a single default-colored run per line, exactly as before.
- **Vim layer** (`vim.rs`): a pure, key-at-a-time state machine over
  `EditorView`'s public API — Normal/Insert/Visual modes, motions
  (`h/j/k/l/w/b/e/0/$/gg/G` with counts, `%` to the matching bracket — an
  inclusive motion, so `d%`/`c%`/`y%` and Visual `%` span the pair), operators
  (`d/c/y` + motion, plus
  `x/dd/D/cc/C/r`, `J` line join, and the `zt/zz/zb` scroll commands), a
  yank/paste register (`yy/yw/p/P`), Visual selection,
  undo/redo (`u` / `Ctrl+R`, count-aware; leaving Insert mode closes the
  buffer's coalescing run so one insert session is one undo step),
  and search (`n`/`N` repeat with counts, `*`/`#` for the word under the
  cursor forward/backward — whole-word matches only, vim's `\<...\>`;
  `/` and `?` return an action that opens the search prompt). The last
  search pattern (with its whole-word flag) lives in `VimState` per pane,
  beside the register.
  Buffers open in Normal mode. The register is bridged to the clipboard:
  yanks/deletes write through, and `p`/`P` paste the clipboard's text when it
  has any (characterwise if it no longer matches the register; the internal
  register is the fallback). Fully unit-tested without a window.
- **Search core** (`search.rs`): pure plain-text match finding —
  `find_next(buffer, from, pattern, forward, whole_word)` (strictly
  after/before the cursor, wrapping around the buffer) and
  `matches_in_lines(buffer, line_range, pattern, whole_word)` for viewport
  highlighting. Patterns are smartcase single-line substrings (no regex): an
  all-lowercase pattern matches case-insensitively, any uppercase makes it
  case-sensitive (ASCII-only folding, so multi-byte chars keep their columns);
  `whole_word` restricts matches to word boundaries (vim's `\<pat\>`, used by
  `*`/`#`). All columns are char offsets, matching `Point.col`, so non-ASCII
  text searches correctly. `EditorView::build_scene` draws a
  `theme::SEARCH_MATCH` quad behind each match on the visible lines while
  highlighting is armed.
  `substitute_line(line, pattern, replacement, global, ignore_case)` does the
  plain-text replace for `:s` (non-overlapping, first-or-all, char-safe;
  `ignore_case` backs the `i` flag).
- **Fuzzy file finder** (`file_finder.rs`): a pure subsequence matcher plus the
  modal `FileFinder` state the overlay drives. `fuzzy_match(query, text)` scores
  a smartcase subsequence (boundary, consecutive-run, and basename bonuses; a
  gap penalty favours contiguity), returning `None` for a non-match;
  `FileFinder` filters/ranks the candidate list on every query change, tracks
  the selection, and yields a scrolled `visible(max)` window for rendering.
  `gather_project_files(root, limit)` lists the project once, bounded by
  `limit`: `git ls-files --cached --others --exclude-standard` inside a git
  work tree (so `.gitignore` is honored with full fidelity), falling back to
  `gather_files` — a plain walk skipping VCS/build and hidden directories,
  never following directory symlinks — outside git.
  `App` opens it on `Cmd`/`Ctrl`+`P` (`app/input.rs`): `project_root` resolves
  the focused pane's enclosing `.git` repo (else its directory), the finder
  captures input modally, and `Enter` resolves the selected project-relative
  path against the root and reuses `App::open_path` (so it drops a focused
  browser back to an editor, like `:e`). `App::build_scene` draws a centered
  overlay panel (query line, match count, ranked rows with the selection
  highlighted); `/state` reports the open finder's query, selection, match
  count, and top matches for integration testing. All pure pieces are
  unit-tested without a window.
- **Clipboard** (`clipboard.rs`): a small `Clipboard` trait (`get`/`set`)
  keeps the core pure — `App` holds a `Box<dyn Clipboard>`. The frontends
  pass `SystemClipboard` (the OS pasteboard via `arboard`, constructed lazily
  and degrading to an in-process fallback when unavailable, so headless runs
  never crash); unit tests use `InMemoryClipboard`.
- **Command line** (`command_line.rs`): `:` opens an ex command line that
  takes over the status bar; `:e <file>` / `:w` / `:q` (closes the focused
  pane, quitting only from the last one — vim window semantics via
  `App::window_close`) / `:wq` / `:wa` /
  `:wqa` / `:noh[lsearch]` / `:report <text>` / `:E`(`:Explore`) / `:Git` /
  `:Diff [--stat] [rev]` (the `garden-diff` review — split / unified / stat, below) /
  `:Review`/`:Review2`/`:ReviewSplit` / `:PR [n]` (also `garden-diff`, below) / `:set wrap` / `:set nowrap`
  (toggle soft wrap on the focused editor pane; also the bare `:wrap`/`:nowrap`)
  / `:N` (a bare address — `:42`, `:$`, `:%` — jumps to that line, clamped,
  landing on its first non-blank column)
  and `:s` substitution
  are parsed (unit-tested) and run by `App`. `:report` files a bug/feature report
  with the recent event log as context (see [Event log](#event-log--actionevent-history--bug-reports)). Substitution (`Command::Substitute`,
  `:s/pat/rep/[flags]`, `:%s/...` for the whole buffer, `:N,Ms/...` for a
  line range — addresses are numbers, `.` (cursor line), or `$` (last line),
  resolved/clamped/reordered at run time) is plain-text: the char
  after `s` is the delimiter (vim style), an empty pattern reuses the pane's
  last search, flags are `g` (all matches on a line) and `i`/`I` (ignore /
  force case; sensitive is the default), and the rewrite of the affected line
  block applies as one undo
  transaction (`App::substitute` over `search::substitute_line`). The
  same mechanism hosts the `/` and `?` search prompts (a `kind` on
  `CommandLine` picks the prompt character and what Enter does); an open
  prompt shows in the debug `/state` JSON as `command_line` with its prefix
  (e.g. `"/foo"`).
- **Input**: keys funnel through `App::apply_key` (shared by every frontend
  and debug injection), already translated to the toolkit-independent
  `vim::Key` + `Mods`. Cmd shortcuts are global in every mode (Cmd+S save,
  Cmd+Shift+S save all, Cmd+Z / Shift+Cmd+Z undo/redo, Cmd+A select all,
  Cmd+C / Cmd+X / Cmd+V copy/cut/paste against the system clipboard — cut
  and paste-over-selection are each one undo transaction — Cmd+W close
  window, Cmd+Q quit); an
  open `:` / `/` / `?` line captures input; Ctrl+C / Ctrl+X / Ctrl+V /
  Ctrl+A / Ctrl+Q are global Mac-style aliases of the Cmd clipboard/select-
  all/quit shortcuts (sharing `clipboard_copy`/`clipboard_cut`/
  `clipboard_paste`), so they override vim's Ctrl meanings for those keys;
  Ctrl+W is the vim **window prefix** — it sets a one-shot pending flag, and
  the next key moves focus between panes (`h`/`j`/`k`/`l` by direction via
  `window_nav`, `w` to cycle), or runs a layout command persisted via the
  transient overlay (above): `o` ("only") collapses the layout to the focused
  pane, `s` / `v` split it into a stacked / side-by-side pair (splicing the
  new container in at the focused pane with `window_nav::replace_leaf`), and
  `c` / `q` close it (`window_nav::remove_leaf` drops the leaf and collapses a
  single-child container; `c` refuses the last pane with vim's E444, `q` quits
  from it — `:q` and `:wq` route through the same `App::window_close`, so they
  close a split first and only quit from the last pane). Any
  other key cancels the prefix harmlessly. Otherwise the key is dispatched to the
  vim layer, with the remaining Ctrl chords wrapped as `vim::Key::Ctrl` (they
  are mode- and count-sensitive — e.g. `3<C-r>` redo — so they belong to
  vim's state machine, not `App`). Mouse click focuses a pane + places the
  cursor, click-drag selects (shift-click extends); the scroll wheel scrolls
  the hovered pane on both axes.
- **Window navigation** (`window_nav.rs`): a pure geometric resolver —
  `neighbor(pane_rects, focus, Direction)` picks the pane to focus when moving
  `h/j/k/l`, preferring a pane whose perpendicular span overlaps the focused
  one, then the nearest along the travel axis. It also holds `replace_leaf`,
  the pure tree edit a split uses to swap the focused pane (the Nth leaf in
  solver order) for a two-child `row`/`column`. `App` owns the focus index and
  the Ctrl+W prefix state; this module only does the spatial/tree math, so it is
  unit-tested without a window.
- **Debug server** (`debug.rs`, opt-in via `--debug-port <n>`, required by
  `--headless`): a localhost HTTP server for live inspection
  (`/state`, `/scene`, `/buffer/<n>`, `/screenshot`) and input injection
  (`/key`, `/text`, `/mouse`, plus `/menu` for native menu-bar items, which
  AppKit delivers outside the injectable input path). Requests reach whichever loop owns the `App`
  through the `debug::RequestSink` trait — a winit `EventLoopProxy` for the
  windowed frontend, a plain mpsc sender for headless/terminal — and are
  answered over an mpsc channel. `App::handle_debug` answers everything
  except `/screenshot`, which needs a renderer and is intercepted per
  frontend. Full protocol: `docs/debug-server.md`.
- **Frame scheduling**: the core sets a redraw flag on state change; each
  frontend drains it (`take_redraw`) and repaints its own way. All frontends
  poll `App::poll_script` (~200ms) for layout hot reloads; on a change,
  panes are rebuilt reusing buffers for files still present. Script errors
  render in a status line, never crash.
- **Live editor↔panel binding** (the Petal IDE): `App::sync_editor_panels`
  (`app/panes.rs`), run at the top of every `tick_panels_pass`, recompiles each
  `panel(...)` pane whose resolved script path matches a **live editor pane's
  file** from that editor's current buffer text — `PanelView::reload_from_editor`
  → `PanelHost::reload_source` (`env.compile_program` + `transfer_state`,
  preserving Petal `state`, exactly like the disk `poll_reload`). So editing
  `x.ptl` next to `panel("x.ptl")` updates the canvas as you type, with no save
  round-trip; a compile error keeps the last good render and shows a banner. The
  pairing is by resolved path (unrelated panels — a clock, a GPP-pushed drawer —
  are untouched) and the recompile is hash-gated per panel, so unchanged buffers
  cost nothing. `garden petal-ide` is the launcher that arranges the split.
- **Direct manipulation (canvas → source)**: the same pairing rule, run
  backwards — pointing at a shape highlights the `draw_*` call that drew it.
  `sync_editor_panels` turns tracing on for every editor-paired panel
  (`PanelView::set_trace_origins`); `App::sync_trace_highlight` reconciles the
  pointer into the paired editor's `EditorView::trace_highlight` each tick, so the
  band follows the mouse and clears when it leaves a shape. It is a *separate*
  pass from the reload for two reasons: it must run while **paused**, where the
  rest of the panel tick does not (the frozen frame is still what the pointer is
  over), and it must go quiet while `PanelView::source_drifted()` — a failed
  reload means the running program's spans describe text the buffer has moved on
  from, so banding them would name the wrong line with total confidence. It also
  records the full resolved call in `App::trace` for the debug server's `/state`.

  **Acting on it**: Cmd/Ctrl-click routes through `App::jump_to_traced_code`
  (`app/mouse.rs`, checked before anything else consumes the press) — same hit
  test, but it moves the cursor to the call, scrolls it into view, and focuses the
  paired editor. Behind a modifier so a plain click stays the sketch's own
  (`mouse_pressed()` is part of the panel input contract); a press that hits no
  shape falls through to the ordinary click path.

  **Writing it back (drag to edit)**: the same press *armed* becomes a drag —
  `App::begin_manipulation` captures the grabbed shape (`Drag::Manipulate` +
  `ManipDrag`), each move states a **goal** through
  `PanelHost::propose_drag_edits`, and the returned `SourceRewrite`s are spliced
  into the paired editor's buffer (`App::apply_rewrites`, back-to-front, one undo
  group for the whole gesture). A gesture that never passes `DRAG_SLOP` releases
  as the jump above, so one modifier carries both. Three decisions worth knowing:
  the gesture is addressed by **command index**, not term id (the drag's own edit
  recompiles the sketch and invalidates every id, but the frame redraws the same
  shapes in the same order); goals are stated as *press-time value + total
  pointer delta*, read off the drawn command's own geometry rather than the
  argument literals, which is what makes a computed position draggable at all;
  and a computed position on a **looping** call is refused unless the grabbed
  shape is the last that call emitted, because the solver inverts against the
  last value each term took — the last iteration's. The edits land in the
  *buffer*, so the live binding re-renders the canvas under the pointer and
  nothing touches disk until the user saves.

  The attribution is the **runtime's**, not a re-run: Petal's lowerer stamps each
  instruction with its IR term, the VM hands that to every native, and an emitting
  native records the call chain that reached it
  (`ExecutionContext::trace_emit`/`emit_origins`, off by default). A *chain*
  rather than a leaf because each `draw_*` is itself a Petal function in the
  `petal-ui` prelude — `petal::provenance::pick_frame` walks out to the innermost
  frame in the file being edited. Spans, argument positions and literal values are
  then derived **lazily** (`petal::provenance::CallSite`) on the mouse move that
  asks, so a frame costs one short id list per shape and nothing else.

  Host side: `petal_ui::draw::take_draw_commands_traced` drains commands with
  their chains; `garden_script::panel_trace` does the hit test (a forward scan of
  the frame's command list tracking the clip rect, keeping the last shape covering
  the point — no spatial index, since the list is rebuilt every frame) and converts
  Petal's 1-based spans to the editor's 0-based ones; `PanelView::trace_at` joins
  them. `DrawTrace` also classifies each argument (`Literal`/`Binding`/`Computed`)
  with its editable span, and `garden_script::drag_handle` maps a `draw_*` name +
  its command to the arguments a pointer drag moves. The write-back side is
  `petal::direct_manipulation` (`propose_edits_batch` / `config let` policy),
  wrapped by `PanelHost::propose_drag_edits` into a `DragOutcome` —
  `Edits` / `Refused(why)` / `Stale`. See `docs/petal-ide-mode.md`.
- **Petal-IDE mode + toolbar/play-pause/IR inspector**: `garden petal-ide` also
  turns on IDE mode via `App::enable_ide` (from the windowed/headless frontends),
  which reserves a Rust-drawn **toolbar** band below the titlebar (`App::toolbar_h`,
  `build_toolbar`/`toolbar_buttons` in `app/scene.rs`, hit-tested in `app/mouse.rs`
  — the same shared-layout pattern as the split dividers). Its buttons dispatch
  through `App::dispatch_toolbar` (`app/commands.rs`), reusing the `MenuAction`
  paths. **Play/Pause** is an `App::paused` flag that early-returns from
  `tick_panels_pass`, freezing every panel tick and the IR refresh while leaving
  the editor live — and, on the way out, still running `sync_trace_highlight` so
  direct manipulation works on the frozen frame. The **IR inspector** (`ToggleIr`) opens a `panel(...)` pane on
  the seeded `~/.garden/petal-ide/ir_view.ptl` drawer; `App::attach_ir_providers`
  (run on every pane rebuild) recognizes that path and attaches an IR
  `DataProvider` over an `Rc<RefCell<IrState>>` shared with the app (the same
  shape as the GPP query cache). Each `tick_panels_pass` republishes the target
  editor's live buffer into that state (`App::refresh_ir_source`, hash-gated); the
  drawer pulls the selected stage's rendered text via `host_data("inspect", …)`,
  which `garden_script::inspect` (a re-export of the upstream `petal::inspect`
  packaging of `show-ir`/`show-bytecode`/`show-ast`) renders lazily. The shared
  state, provider, and the drawer live in `garden-app/src/petal_ide/`.
- **External file refresh**: on the same ~200ms tick the frontends call
  `App::poll_files`, which stamps each open file (`garden_core::DiskStamp` =
  mtime + size) and compares. A clean buffer is reloaded silently
  (`Buffer::reload` + `EditorView::reload` re-clamps the cursor); a dirty
  buffer is kept and a one-time warning lands in `App::status_note` (the
  per-pane `external_conflict` stamp dedupes it until the disk changes again
  or the buffer becomes clean and reloads). The status bar's right slot shows
  `script_error` (red, `script:`-prefixed — a standing layout-script reload
  error, cleared by the next clean reload) over `status_error` (red) over
  `status_note`; the latter two are transient action feedback (bad command,
  failed or successful save), cleared by the next key press. All three appear
  in `/state`.

### State — the Garden state directory

Per-machine persistent state lives under `~/.garden/state`, managed by
`garden-app/src/state.rs` (the `State` struct). It owns a single SQLite database
(`db.sqlite`, via `rusqlite` with the `bundled` feature — SQLite compiled from
source, no system library) plus per-window subdirectories `window-<id>/`.

- **Window ids** come from a `windows` table whose `INTEGER PRIMARY KEY
  AUTOINCREMENT` column is the id: machine-unique and never reused (it survives
  row deletion). `new_window_id()` inserts a row and returns `last_insert_rowid`;
  `main.rs::load_script` calls it once per launch and hands
  `window_overlay_path(id)` (`window-<id>/window.ptl`) to `set_transient_path`.
- **Migrations** are an ordered, append-only `MIGRATIONS: &[&str]`. `migrate`
  reads the DB's `user_version`, runs each entry whose 1-based index exceeds it
  (each in its own transaction, bumping `user_version` on commit), and is a
  no-op once current. Adding a table later = appending one string; existing
  databases run only the new tail. **Never edit or reorder existing entries.**
- **Best-effort**: a missing `$HOME` or an unopenable DB logs a warning and
  launches anyway, with layout changes falling back to the sibling overlay
  (`garden_script::transient_path`). The state layer is unit-tested against
  temp-dir databases (id monotonicity across reopen, incremental + idempotent
  migrations).

### Event log — action/event history + bug reports

For visibility and replay when something goes wrong, each window records the
actions and events it processes into the same state database, and `:report`
snapshots that history into a bug/feature report. The whole feature lives in
`garden-app/src/event_log.rs` (the `EventLog`) plus the thin `App` glue in
`app/events.rs`; the schema is migration **v2** (`events` + `reports` tables).

- **One connection, kept alive.** `main.rs::open_window_state` opens `State`
  once, allocates the window id, then `State::into_event_log(id)` hands the
  *same* `rusqlite::Connection` to an `EventLog` (rather than reopening the DB).
  The id names both the layout overlay and the event log's `window_id` column,
  so a session's events, reports, and layout share one identity. The log flows
  into the `App` via `AppConfig.event_log` → `App::set_event_log` (the frontends
  call it right after `App::new`; unit tests leave it `None`, disabling logging).
- **In-memory buffering, 5-second sync.** `App::log_event(category, detail)`
  pushes onto an in-memory `Vec` (no write per keystroke). Every frontend's
  ~200ms poll tick calls `App::poll_event_log` → `EventLog::maybe_flush`, which
  writes the buffer in one transaction only once `FLUSH_INTERVAL` (5s) has
  elapsed. `Drop` flushes the tail so a clean quit loses nothing. A DB error is
  logged and the buffer dropped — logging never takes the editor down.
- **What's logged.** The recording calls sit at the central dispatch points so
  coverage stays broad without threading the log everywhere: every key press
  (`App::apply_key`, the single funnel for all frontend + debug input), each ex
  command (`run_command`), text injection, mouse clicks, file opens, runtime
  layout changes, script reloads/errors, and external-file changes. Categories
  are stable tags (`key`, `command`, `mouse`, `file`, `layout`, `script`).
- **`:report <text>`.** `Command::Report` → `App::file_report` →
  `EventLog::file_report`: flush pending events, gather the previous
  `REPORT_WINDOW` (5 minutes) of this window's events as a formatted context
  block (timestamped via SQLite's `strftime`), and insert a `reports` row
  (`message` + `context`). The status bar acknowledges with the new report id
  and event count. With no event log attached (state unavailable) it reports a
  friendly error. Unit-tested against temp-dir databases (buffering, the flush
  timer, the 5-minute context window, drop-flush, and report capture).

## Garden Pane Protocol (GPP) — subprocess-backed panes

A GPP pane reuses the host's existing `EditorView`/`Buffer` purely as a **render
surface**: vim and editing are disabled, and a child process pushes the full
text to display. The model is "fat subprocess, thin host" — all logic lives in
the client; the host just shows what it is told and forwards a subscribed subset
of keystrokes. The first client is `directory-browser`, so `garden src/` opens a
navigable listing (like vim's netrw). The wire protocol is fully specified in
`docs/gpp.md`; this section covers the in-tree pieces.

How much host behavior a client replaces is a **layered, opt-in takeover**
(`gpp::Takeover`, declared in the `initialize` response): `Keymap` (the
default/lightest — only subscribed keys forwarded, the host scrolls for the
rest) or `Keyboard` (almost-full — every non-reserved key forwarded). The host
**command bar (`:`) and the global chords** (Cmd/Ctrl editing shortcuts, `Ctrl+W`
window nav, quit) are reserved at *every* layer and never reach the client, so
`:w`/`:q`/`:E` and window navigation keep working inside any process pane.
`App::open_path` (`:e`) drops the `ProcessPane` and turns the pane back into a
normal editor — the way out of a browser.

- **`gpp` crate (shared contract)**: the on-the-wire types and transport, used
  by both the host and every client. A JSON-RPC 2.0 `Envelope` (`request` /
  `notification` / `response` constructors); typed params per message
  (`InitializeParams`, `RenderParams`, `KeyParams`, `MouseParams`,
  `ResizeParams`, `SetKeymapParams`, `OpenPathParams`, `SetStatusParams`,
  plus `StyleSpan`/`StyleKind` for `render`'s optional per-line foreground color
  spans, `BgSpan`/`BgKind` for its optional per-line background bands (the
  column-scoped tints a rich diff/review client paints behind its rows — see
  `docs/gpp.md`), and `MouseKind` for click forwarding); method-name constants
  under `gpp::method`; a canonical `Key` enum with `to_name`/`from_name` for the
  shared key encoding; and newline-framed `write_message` / `read_message`
  helpers (one compact JSON object per line, `Ok(None)` at EOF). It has no
  Garden dependencies — it is a standalone library so external clients can
  depend on just it.

- **`directory-browser` (`gpp-apps/directory-browser`)**: the reference client.
  Split into a pure `Browser` core (listing, sorting, selection movement,
  activation — directories sort before files, `..` is prepended, the selected
  row is prefixed `"> "`) and a thin stdio `run` loop that wires the core to the
  GPP transport. It subscribes to navigation keys (`j/k/Up/Down`, `Enter/l/Right`
  to descend or open, `h/Left/Backspace/-` to go up, `g/G`, space), re-renders on
  each one, and on selecting a file sends `openPath` so the host swaps in a real
  editor.

- **`git-viewers` (`gpp-apps/git-viewers`)**: the reference **Panel-mode**
  (script-push) app — one bin (`git-log`) over the protocol loop + git/diff
  parsing in `src/lib.rs`, `include_str!`'ing its colocated production drawer.
  **`git-log`** backs `:Git` / `garden git log` — the history browser. It pushes
  `src/git_panel.ptl` (the same drawer the host would run in-process) and answers
  `query("log", "")` (commit history + repo/branch/dirty header) and
  `query("commit", arg)` (one commit's or the worktree's numbered diff; `@full:`
  for full context, `@worktree` for uncommitted changes) by shelling `git`. (The
  crate's second bin, the read-only `git-diff` viewer, was retired once
  `garden-diff` covered its views — including `--stat`.)

  Panel-mode apps never draw or handle keys themselves — the host runs the pushed
  drawer as an in-process [panel](petal-graphical-panels.md) and forwards the
  `query` calls it makes over the pipe (see the "Panel mode" section of
  `docs/gpp.md`, the design record in
  `docs/gpp.md`, and the how-to in
  `docs/writing-gpp-apps.md`). The git/diff invocations and the pure parsing live
  in `lib.rs` and are unit-tested against real temporary repositories. Like the
  other browsers they never send `openPath` — they are read-only viewers.

- **`garden-diff` (`gpp-apps/garden-diff`)**: the **editable** diff reviewer
  behind every diff/review entry point — `:Diff [--stat]`,
  `:Review`/`:Review2`/`:ReviewSplit`, `:PR [n]`, and the `garden diff [base|PR#]`
  / `garden pr [n|--local base]` CLIs. It is the unified successor to three
  retired paths: the read-only `git-diff` viewer (interactive **and** `--stat`),
  the in-host `:Review`/`:Review2` projection editors, and the read-only
  `pr-browser`. It pushes a fixed `src/garden_diff.ptl` drawer and answers
  `query("doc", "")` with the projected before/after/unified documents plus the
  per-file summary rows, by shelling `git`/`gh`; a **`mutate("apply", …)`** splices
  the write-backs the host's projection resolved into the working-tree files.
  Unlike every other GPP app it is not read-only — see its own section below
  (git/diff/splice logic in `src/diff_core.rs`).

- **`sqlite-browser` (`gpp-apps/sqlite-browser`)**: a **Panel-mode**
  [`petal_query::App`] that browses and visualizes a relational database. Despite
  the name it speaks to **two engines**: the launch argument is classified by
  `db::Source::from_arg` — a file path is **SQLite**
  (`process("/abs/sqlite-browser", ["/abs/db.sqlite"])`), a `postgres://…` URL is
  **Postgres** (`process("/abs/sqlite-browser", ["postgres://host/db"])`).
  Everything above the `db` layer is engine-agnostic: it asks a `db::Backend`
  trait for three shapes — the table/view catalog with row counts, a table's
  column schema (type, PK, NOT NULL), and a capped page of rows with every cell
  rendered to a display string — and never learns which engine answered. Two
  concrete backends implement it: `db::sqlite::SqliteBackend` (bundled
  `rusqlite`, opened **read-only**; exact `COUNT(*)` counts, `PRAGMA table_info`
  schema, NULL/blob-byte-count/int-REAL cell rendering) and
  `db::postgres::PostgresBackend` (synchronous `postgres` crate over native-TLS,
  session pinned `default_transaction_read_only`; system-catalog introspection —
  `pg_class`/`pg_attribute`/`pg_constraint` — with `reltuples` **estimated**
  counts to avoid an open-time full scan, objects outside the `public` schema
  listed qualified, and a generic `::text`-cast page query so any column type
  renders). The connection is cached in the per-run `State` and re-established via
  `Backend::is_live` if it drops. The `main` shapers turn the backend's output
  into the `catalog`/`table` query answers, including a monospace, column-aligned
  data grid pre-woven as per-line-styled `text_view` rows. The colocated
  `db_view.ptl` drawer paints a **two-column master-detail** view: the catalog on
  the left (an *Overview* entry, then each object with its row/column counts), and
  on the right either the selected table's **schema strip + data grid** or — for
  the Overview — a **horizontal bar chart** of every object's row count (the
  "visualizer"). Answers carry a short `max_age`+stale-while-revalidate policy so
  an externally-mutated database refreshes without a spinner. SQLite extraction is
  unit-tested against an in-memory DB and the shapers against a real on-disk file;
  the Postgres path has an env-gated round-trip test
  (`SQLITE_BROWSER_TEST_PG_URL=… cargo test -p sqlite-browser`).

- **`ProcessPane` host integration (`garden-app/src/process_pane.rs`)**: owns
  the child, a buffered writer over its stdin (host → client), and a background
  reader thread that forwards every envelope from stdout to an mpsc channel
  (client → host) — mirroring the thread+channel shape of `debug.rs`. `spawn`
  does the **synchronous handshake**: it writes the `initialize` request (id 1)
  and blocks reading exactly the one response the client must send first, *then*
  starts the reader thread (so the handshake line is never consumed by the
  thread). It exposes `send_key` / `send_resize` (host → client notifications),
  `try_drain` (non-blocking) and `drain_for(dur)` (block briefly for the reply
  after a key so it feels synchronous), plus `keymap()` / `set_keymap()` and
  `name()`. `Drop` sends `shutdown`, drops stdin (EOF), then kills+reaps the
  child as a backstop. All wire errors are logged, never panicked — a
  misbehaving client cannot take the editor down.

- **`App` wiring (`garden-app/src/app/`)**: a `Pane` (in `app/types.rs`) carries
  an `Option<ProcessPane>`; `is_process()` is true when present. On layout rebuild
  (`app/panes.rs`) a `PaneContent::Process { command, args }` slot either moves a
  matching live process across or spawns a fresh one (then drains its initial
  `render`). A focused process pane routes keys through `process_key`
  (`app/input.rs`), whose policy is a pure, unit-tested function
  `classify_process_key(key, mods, takeover, subscribed)`: `Cmd`/`Ctrl`+`Q`
  quits, `:` opens the host command line, the other Cmd/Ctrl chords stay with
  the host, and past that a `Keymap` client gets only its subscribed keys
  forwarded (other keys scroll the view) while a `Keyboard` client gets every
  remaining key. A forwarded key's reply is drained and applied immediately.
  `apply_process_messages` applies a drained batch —
  `render` replaces the surface buffer, selects `cursorLine`, and applies the
  optional per-line `styles` (semantic spans — added/removed/hunk/title/dim/
  comment — mapped to theme colors by `editor_view::style_color` and drawn
  instead of syntax highlighting) and `backgrounds` (per-line column-scoped
  bands — added/removed/comment/selected/header — mapped by `editor_view::bg_color`
  to translucent tints and drawn behind the text, under selection and the
  caret), `setKeymap`
  updates the forwarded set (and optionally the takeover layer and mouse
  opt-in), `setStatus`
  updates the status note, and `openPath`
  drops the `ProcessPane` (shutting the child down) and reopens the pane as a
  normal editor on the path. A client that opted into mouse forwarding
  (`initialize`'s `mouse: true`) receives clicks as `mouse` notifications with
  the scroll-adjusted content row (`app/mouse.rs::classify_mouse_down` routes
  them; both browsers use click-to-select, double-click-to-activate). `/state` reports each process pane's `takeover`
  beside its name and keymap. `poll_processes` drains every process pane on the
  same ~200ms tick as the script/file polls, so unsolicited renders appear
  without a keypress. `/state` reports each pane's `kind` (`"process"` vs
  `"editor"`) and, for a process pane, its `process` name + keymap.

- **`garden <dir>` startup (`garden-app/src/main.rs`)**: a single positional
  argument that is a directory resolves to a `LayoutNode::Process` running the
  `directory-browser` binary with that directory as its arg.
  `process_pane::directory_browser_bin()` resolves the client next to the
  running `garden` executable (`current_exe`'s sibling), falling back to the
  bare name on `$PATH`, so the in-tree build "just works". The same resolver
  backs the on-demand open below.

- **On-demand browser (`:E` / `-`)**: from an editor pane, the `:E` (alias
  `:Explore`) command and the `-` key open the directory browser *in place*,
  listing the focused file's parent directory (the cwd for a pathless buffer) —
  vim-netrw / vim-vinegar style. `App::open_directory_browser` spawns the same
  `directory-browser` client and swaps the focused pane to process-backed; the
  reverse trip (selecting a file) is the normal `openPath` flow above. `:E`
  parses to `Command::Explore` (`command_line.rs`); `-` returns
  `vim::Action::OpenDirectoryBrowser` from the Normal-mode handler (vim's `-`
  line motion is repurposed). On spawn failure the current buffer is kept and
  the error surfaces in the status bar.

- **GPP apps back `:Git` and every diff/review command**: those commands no
  longer open an in-process built-in panel — they spawn the panel-mode GPP apps
  `git-log` (`:Git`) and `garden-diff` (`:Diff`, `:Review*`, `:PR`; see the
  sections below), which push the drawer the host runs in-process and answer its
  `query` over the pipe. `process_pane::git_log_bin()` / `garden_diff_bin()`
  resolve the client beside the running `garden` executable (env overrides
  `GARDEN_GIT_LOG_BIN` / `GARDEN_DIFF_BIN`), by the same rules as
  `directory_browser_bin`.

## `:Diff` / `:Review` / `:PR` — the `garden-diff` review (the one diff tool)

`:Diff [--stat] [rev]`, `:Review`/`:Review2`/`:ReviewSplit [base]`, and
`:PR [number]` all replace the focused pane with the **`garden-diff`** panel-mode
GPP app (`gpp-apps/garden-diff`, bin `garden-diff`) via `App::open_garden_diff`;
the `garden diff [--stat] [base|PR#]` and `garden pr [n|--local base]` CLIs build
the same client as their startup layout. It is the unified successor to three
retired paths: the read-only `git-diff` viewer (both its interactive and `--stat`
views), the in-host `:Review`/`:Review2` projection editors, and the read-only
`pr-browser`. `open_garden_diff` launches it with `[dir]` plus the command's extra
args (a base ref and/or `--stat` for `:Diff`/`:Review*`, or `--pr [number]` for
`:PR`), rooted at the focused file's repository. Unlike every other GPP app it is
**not read-only** — the after side is edited with real vim and written back on
`^S`.

`garden-diff` owns its git/diff/splice logic in `src/diff_core.rs` (a subprocess
crate can't import the host binary's modules — the same reimplement-per-app
pattern as `git-viewers`); its marker format (`@@@ file:` / `@@@ hunk:`) and
region/splice semantics are the ones the retired in-host `:Review2` projection
used, so editing the after side and saving rewrites the working-tree files the
same way — only the editing surface is an `edit_view` panel region.

The app pushes a fixed drawer (`src/garden_diff.ptl`) the host runs as an
in-process [panel](petal-graphical-panels.md), and its query/mutate contract is:

- **`query("doc", scope)`** → `{ error, base, initial_mode, editable,
  markers_in_text, files:[{path,added,removed,binary}], before:{text,styles},
  after:{text,styles,projection}, unified:{text,styles,projection},
  pr:{present,…} }`. The argument is the review's **scope**: `""` for the whole
  thing, `"commit:<sha>"` for that commit alone, `"since:<sha>"` for its parent
  against the working tree. The host caches per argument, so switching back to a
  scope already looked at costs nothing.
  The app runs one `git diff <base>` and projects it three ways (`diff_core::build`):
  a read-only **before** (base side: context + removals, per-line styles) and two
  **editable projections** — the **after** column (working-tree side: context +
  additions) and the **unified** stream (classic `+`/`-`/context) — each shipping
  the provenance block `edit_view_projection` takes. The answer is cached
  fresh-forever; the host drives editing/scroll locally with no pipe traffic.
- **`mutate("apply", { edits })`** (host→client request, `on_mutation("apply", …)`)
  → the write-back for **either** editable view. The drawer sends the
  `{source, start, end, lines}` edits the host's projection resolved, not the
  region's text, so the client only splices them into the working-tree files
  (`diff_core::apply_edits`). The save then drops the cached `doc` and the drawer
  `invalidate`s it, so the panes refresh to the new working tree.
- **`query("commits", "")`** → `{ commits:[{sha,short,subject,author,date}] }` —
  the review's own commits, newest first (`git log <base>..HEAD`). Fetched
  lazily, the first time the COMMITS view is opened: a `git log` per launch is
  pure cost for the reviewers who never open it. An empty list is a normal
  answer (uncommitted work against the base), not an error.

**Scoping.** A `commit:` scope is **read-only** (`editable: false`): `git diff
<sha>^ <sha>` describes files as they were at that commit, so its line numbers
address a blob rather than the checkout, and folding edits back would splice the
past into the present at coincidentally-matching offsets. The drawer renders
those views as `text_view`s — which, having no projection, have no gutter, which
is why the host bakes their `+`/`-` markers into the line text
(`markers_in_text: true`) and ships positional `styles` alongside. A `since:`
scope still ends at the working tree, so it is an ordinary editable diff from a
nearer base. Root commits are diffed against git's empty-tree hash, so "just
this commit" works on a repository's first commit too.

Four **view modes**, switched by clickable header pills (a mouse gesture, so it
works even while the editor region is focused — a press outside a region hands
keyboard focus back to the script). `--stat` (from `:Diff --stat` /
`garden diff --stat`) only picks which one the drawer *opens* in, via the reply's
`initial_mode`:

- **UNIFIED** (default, editable): one column, the `+`/`-`/context stream, from
  the same parsed patch, with PR review comments threaded in (below), in an
  `edit_view` (region id 5). See "Editing the unified diff" below. Long lines
  **soft-wrap** to the column (`text_view_wrap(5, …)`), so a diff of prose or of
  code with long literals is readable without horizontal scrolling; the `wrap`
  header pill (unified only) turns it off when the exact columns matter. Only
  this view wraps — split's two regions are row-aligned, and wrapping one would
  slide it out of step with the other.
- **SPLIT**: the read-only BEFORE `text_view` beside the editable AFTER
  `edit_view` (region id 2 — a real host vim `EditorView`). Click to focus, edit
  with real vim, `^S` saves. Because `text_view` was already a real editor, making
  the after side editable was **routing** (the host forwards real vim keystrokes
  into an editable region), not a reimplementation — the editable side is Garden's
  actual vim.

  The after column is a **projection** too, and differs from the unified one only
  in what it shows: it is a picture of the new file, so it is *undecorated* (no
  `+`/`-`/space prefixes — a line typed into it is taken literally), every content
  line is `Live`, and there are no ghosts, because a line the change deleted is
  exactly what this column does not show. One span per hunk names the file range
  that hunk's lines occupy, so an edit folds straight back. Its `@@@` markers are
  **locked chrome** rather than the unified view's span/group headers: with only
  the new side in hand there is nothing to revert a hunk *to*, so `dd` on one is
  refused rather than half-reverting (dropping the additions, leaving the
  deletions in place). That gesture belongs to the unified view, which has both
  sides.

  Unsaved edits do **not** survive a round-trip through another view — a region's
  editor state is pruned when the region isn't declared for a frame, and a
  projection cannot be re-seeded from its own edited text (the origin table
  describes the *projected* lines, so the two would be out of step). Both editable
  views behave the same way here.
- **COMMITS** (read-only): the review's history and the scope control. A left
  click on a row scopes the diff to that commit; a **right** click opens a
  `context_menu` (the `petal-ui` prelude widget, on the panel right-button
  routing added alongside it) offering that commit, everything since it, or a
  return to the whole review. The scope change re-enters the drawer's loader
  with a new `doc` argument.
- **STAT** (read-only): the per-file changed-lines diagram — a totals line, a
  stacked added/removed bar, and one sqrt-scaled green/red bar per file (binary
  files labelled rather than barred) — drawn from the reply's `files` rows. This
  is the view the retired `git-diff --stat` app used to draw.

**Editing the unified diff.** Its `+`/`-`/space markers are **gutter chrome**
(`Decor::gutter`), not buffer text: the region holds the files' own lines and
the host draws the glyph beside each one from the origin table. That is what
lets the view be edited like a file rather than like a patch — `J` joins two
added lines without dragging a `+` into the seam, `0` reaches the indent, a
column selection takes no marker, and `/` matches what is on screen. The origin
alphabet below is unchanged; only where the marker is drawn moved.

The unified stream is not a picture of the new files
— it interleaves the base side (`-` lines) with the new one — which is what makes
the projection machinery load-bearing here: the reply carries a `projection` block
(one origin character per line plus the file range each hunk rewrites), the drawer
hands it to `edit_view_projection(5, …)`, and the host tracks where every line
came from as the buffer is edited. See "Editable projections" under `garden-core`.
The table below is therefore the unified view's; the after column supports the
same *text* edits, folded back the same way, but has no `-` lines to revive and no
revertable headers.

| gesture | result |
|---|---|
| keep a `+` line | still added |
| **delete** a `+` line | the addition is dropped |
| keep a `-` line | still deleted |
| **delete** a `-` line | the deletion is reverted — the base line comes back, at the point the diff showed it |
| retext a `+` or `-` line | the new text is what lands (a `-` line retexted to a space-prefixed one reverts it too) |
| delete a context line | removed from the file |
| type a bare line in | added, honouring a leading `+`/`-`/space if present |
| anything to a woven review-comment line | nothing — comments are recorded as chrome, never file content |
| **`dd` on a `@@@ hunk:` header** | the hunk is reverted: its additions go, its deletions come back as context |
| **`dd` on a `@@@ file:` header** | every hunk of that file is reverted |
| `dd` on the title line | refused, with a status message — it belongs to the view, not the change |

`^S` then sends `mutate("apply", { edits: edit_view_edits(id) })` — the
`{source, start, end, lines}` write-backs the host resolved — and the client
splices them (`diff_core::apply_edits`). Three things follow from this that did
not hold before:

- **No alignment.** The LCS aligner, the deletion/insertion pairing, and the
  block-extraction that recovered intent from edited text are gone (~250 lines),
  and with them the split view's separate marker/splice write-back: both views now
  save through the one `apply` path, so `diff_core` holds no code that reads
  edited text at all.
- **Markers are inert.** `@@@ file:` / `@@@ hunk:` are chrome in both views.
  Editing one no longer aborts the save ("markers were changed — save aborted");
  it does nothing at all. They stay because they are useful to read, and because
  *deleting* one is now a meaningful gesture (above) or an explicit refusal.
  Being inert is also what lets them carry whatever reads best: the **unified**
  view's hunk header names its file as well as its range
  (`@@@ hunk: src/a.rs @@ -1,3 +1,4 @@`), since that view is one long stream and
  the `@@@ file:` heading scrolls away above a long file. The split view's
  headers stay bare — the FILES column already answers "which file?".
- **Styles stop drifting.** The bands ride the origin table, so an inserted line
  moves the bands below it. Neither drawer region recomputes them from the live
  buffer text each frame, calls `text_view_line_styles`, or keeps a seed snapshot
  to survive a view switch — the host keys each region's buffer to its projection.

**PR mode** (`:PR`, `garden diff <PR#>`, or the `--pr [number]` flag): an all-digit
positional (or an explicit `--pr`) selects it. The app resolves the PR via
`gh pr view` (run with `.current_dir(dir)` — `gh` has no git-style `-C` flag),
then diffs the base branch's **merge-base** against the working tree (three-dot
semantics, matching GitHub's "Files changed"). Because the editable after side
writes to the *working tree*, the PR's head branch **must be the current
checkout** — otherwise `pr_diff_base` returns a `gh pr checkout <N>` hint instead
of a base ref. A collapsible discussion band shows the PR
number/title/author/`base ← head`/state plus the description **and the
conversation comments**, prebuilt client-side into one `discussion` string the
drawer scrolls in its own `text_view` region. **Inline review comments** (fetched
with `gh api repos/<slug>/pulls/<n>/comments`, a best-effort call whose failure
never costs the diff) are woven into the *unified* projection at the line they
target — new-file line for a `RIGHT` comment, old-file line for `LEFT`, with
threads whose anchor has fallen out of the diff collected in a trailing block so
none are dropped. They are deliberately **not** put on the split sides: that
column is the new file line for line, and prose interleaved into it is prose the
reader has to mentally subtract to see the file. They are safe *anywhere* now, in
the sense that mattered before — both projections record chrome as contributing no
file content however the user treats it. The projected titles carry a display
`label`, so PR mode names the base branch (e.g. `main`) rather than the opaque
merge-base SHA. An absent `--pr` number resolves the current branch's PR.

Like `git-log`, the pane **renders and handles input as an in-process
panel** but **persists in the layout as a `process(...)` node**: a
reload/split/window-restore re-spawns `garden-diff`, which re-pushes its drawer
(and re-reads the working tree — a saved edit is already on disk). `garden diff
[base|PR#]` is the CLI counterpart. The protocol surface this app required was
the `mutate("apply", …)` message, making `edit_view` panel
regions editable, and the `edit_view_projection` / `edit_view_edits` pair (all
documented in `docs/writing-gpp-apps.md`). `diff_core` is
unit-tested (`gpp-apps/garden-diff/src/diff_core/tests.rs`, against scratch repos
like the `git-viewers` tests), and the whole loop — load, pill switching, an edit
typed into the after column, a refused delete of one of its markers, a `-` line
deleted in the unified view, a hunk reverted by deleting its header, a refused
delete of the title, `^S`, the file on disk — is driven end-to-end by `tools/diff-review-integration-test.ts`
over the debug server. The drawer publishes its hit targets (`split_x` /
`unified_x` / `stat_x` / `pill_y` for the pills; `after_x` / `unified_body_x` /
`body_top` for the body regions, emitted from inside the branch that lays them
out, since the notice and PR bands move them) as plainly-named `let` bindings,
which the host **observes** and reports at `/state` → `panes[].panel.values`, so
that test clicks by geometry it reads rather than hard-codes.

## `:Git` — the `git-log` panel-mode GPP app

`:Git` (→ `Command::Git` → `App::open_git_viewer`) replaces the focused pane with
the `git-log` panel-mode GPP app (`gpp-apps/git-viewers`, bin `git-log`), a
history browser: a two-line commit list and the selected commit's file list
stacked in a left column, the selected file's line diff (numbered old/new gutter,
add/del row tinting) on the right. Three focusable regions cycle with Tab or a
click; `j`/`k`/arrows/PageUp/PageDown/Home/End move within the focused region, and
the wheel scrolls whichever region the pointer is over. A dirty working tree
(tracked changes) shows as a synthetic first row. Like `:Diff`, the pane renders
and handles input as an in-process panel but persists as a `process(...)` node
(a reload/split/restore re-spawns `git-log`, which re-pushes its drawer).

Three mouse interactions beyond selection (all driven by the drawer, persisted
only in panel state — not the layout):

- **Draggable dividers**: the vertical divider (left column ↔ diff) and the
  horizontal divider (commit list ↔ file list) resize by dragging. The drawer
  keeps the two split positions as fractions (`left_frac`/`commits_frac`) and
  tracks a `drag_div` while the button is held; geometry is recomputed from the
  updated fractions the same frame, so the drag has no lag.
- **Click-to-expand diff**: clicking a hunk header (`▸`/`▾`) uncollapses the
  file's diff to **full context** — the whole file's source around every hunk —
  and clicking it again collapses back. This re-fetches the commit with a large
  `-U` via the `@full:` prefix (see below); expansion resets when a different
  commit is selected.
- **Wrapped file names**: because the panel renderer draws all text at one
  monospace size (no per-run size), long paths in the file list *wrap* across
  two lines (`wrap2`) rather than shrinking, keeping the basename visible.

The drawer (`git-viewers/src/git_panel.ptl`) bakes in **nothing**: it loads all
its data at runtime through the `query(kind, arg)` native — Garden's React-Query
prototype on Petal's **pending values** (`garden-script/src/query.rs`; see
Petal's `pending-values` design notes). The host runs the drawer in-process;
the `git-log` app answers the `query` requests it makes over the pipe (the same
`(kind, arg)` a local Rust provider would answer, so the drawer is identical to a
built-in panel's), doing the git work in `git-viewers/src/lib.rs`:

- **The query round-trip**: the drawer's `query(...)` calls arrive at the app as
  `query` requests over the pipe (a pipe-proxy `ProcessQueryProvider` host-side);
  while a fetch is in flight `query` returns a `Value::Pending`, and the drawer
  inspects it with the language's meta functions — `is_ready`/`is_loading`/
  `is_error`/`error_of`/`??` — to render a spinner or an error, never blocking a
  frame. The panel's per-frame re-run *is* the retry loop, so the next frame after
  the answer lands simply shows the data.
- **`query("log", "")`** → `{ repo, branch, worktree_dirty, commits: […] }` (one
  `git log`, capped at 400 commits, + branch/dirty probes). The drawer caches it
  in `state` and stops polling once ready.
- **`query("commit", hash)`** → one commit's diff (`git show` parsed into
  per-file, per-line old/new numbers, capped at 3000 lines/file, 400
  files/commit). `"@worktree"` selects the uncommitted diff; a `@full:` prefix
  requests the full-context diff (`git show -U100000`) that click-to-expand uses.
  Immutable commit diffs stay cached across a refresh.
- **Refresh**: the header's ⟳ Refresh button `invalidate`s the log / worktree keys
  and clears the drawer's "loaded" markers, so the polls resume and **git is
  re-run** for fresh data.

The git plumbing and pure parsing in `git-viewers/src/lib.rs` are unit-tested
against scratch repos (`cargo test -p git-viewers`); the whole `:Git` flow is
driven end to end over the debug server by
`tools/git-panel-integration-test.ts` (see `docs/testing.md`).

## Retired: the in-host projection framework

`:Review`/`:Review2`/`:ReviewSplit`, `:PR`, and `garden pr [--local]` used to run
an in-host **projectional editor** (`garden-app/src/projection/` + `diff.rs`): a
transformed view of `git diff <base>` in a normal editor pane, with `:w` splicing
the edits back into the sources, `:Revert` restoring a hunk, and a paired
read-only "before" pane kept scroll-aligned by the *n*-th shared marker. It was
removed once `garden-diff` covered every entry point — the same projection now
lives in the subprocess (`gpp-apps/garden-diff/src/diff_core.rs`), with an
`edit_view` panel region as the editing surface and `^S` as the write-back, so
there is exactly one implementation of the review. `:Revert` went with it (the
after side is a normal vim buffer — undo, or reload the diff). The names it went
by before removal — `garden-app/src/projection/`, `app::panes::review_split_tests`,
and `scripts/review-editor-integration-test.sh` — are what to search the history
for if the general source↔view machinery is ever wanted again.

## Roadmap (post-v1)

Syntax-highlighting injections (HTML↔JS/CSS, Markdown inline, fenced code) and
optional runtime-loaded grammars beyond the bundled set, shared buffers across
panes, command palette driven by Petal commands, Petal-defined keybindings and
event hooks (the vim autocmd idea), LSP client, file tree pane, tabs.

Designs queued up for post-v1, grounded in a survey of Zed/Lapce/Helix/neovim:
Helix-style ChangeSet transactions + multi-range selections, undo as a revision
tree, neovim-style extmarks, and a handle-based scripting API with an event
registry.

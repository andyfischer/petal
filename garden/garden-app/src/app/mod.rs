//! The frontend-independent application core: pane management, input
//! routing, the vim/command-line layers, scene building, and debug-command
//! handling.
//!
//! [`App`] never touches a window, a GPU, or a terminal. A frontend (see
//! [`crate::frontend`]) owns the event loop and the presentation target; it
//! feeds translated input into `App`, watches the [`take_redraw`](App::take_redraw)
//! / [`should_quit`](App::should_quit) / [`should_close`](App::should_close)
//! flags, and presents the
//! [`Scene`](garden_render::Scene)s that [`build_scene`](App::build_scene) produces.
//!
//! The impl is split by concern across sibling modules, all operating on the
//! one [`App`] struct defined here:
//!
//! - [`panes`] — building/repositioning panes from the layout, polling the
//!   script and on-disk files for changes
//! - [`input`] — key routing (vim layer, clipboard shortcuts, the `Ctrl+W`
//!   window prefix) and text injection
//! - [`process`] — applying GPP subprocess messages and the directory browser
//! - [`commands`] — ex commands, search/substitute, the native menu
//! - [`recents`] — recording what the user opened into [`crate::recents`]
//! - [`mouse`] — pointer hit-testing, drag selection, scrolling
//! - [`scene`] — building the render frame
//! - [`debug_server`] — answering debug-server requests against live state

use std::path::PathBuf;
use std::time::Instant;

use garden_render::Rect;
use garden_script::{LayoutNode, ScriptHost};

use crate::clipboard::Clipboard;
use crate::command_line::CommandLine;
use crate::editor_view::EditorView;
use crate::event_log::EventLog;
use crate::file_finder::FileFinder;
use crate::petal_ide::IdeState;
use crate::theme;

mod commands;
mod debug_server;
mod events;
mod input;
mod lsp;
mod mouse;
mod panes;
mod process;
mod recents;
mod scene;
mod types;

pub use types::{
    ClickCounter, KeyPhase, MenuAction, Mods, Pane, ToolbarAction, Viewport, MENU_ACTIONS,
};
pub(crate) use types::{KeyOutcome, ToolbarButton};

#[cfg(test)]
mod tests;

/// Outer margin around the pane area, logical pixels.
const MARGIN: f32 = 6.0;

/// Height of the custom titlebar drawn across the top of the window, logical
/// pixels. Slim enough to read as a unified macOS title bar while clearing the
/// traffic-light controls that float over it (see [`App::enable_titlebar`]).
pub const TITLEBAR_H: f32 = 32.0;

/// Horizontal space the macOS traffic-light controls occupy at the left of the
/// titlebar; the title text starts after it.
const TRAFFIC_LIGHTS_W: f32 = 78.0;

/// Height of the Petal-IDE toolbar band drawn below the titlebar, logical
/// pixels. Reserved only in IDE mode (see [`App::enable_ide`]).
pub const TOOLBAR_H: f32 = 34.0;

pub struct App {
    /// The layout script, when one is loaded. `None` in plain-file mode
    /// (e.g. `garden --term notes.txt` with no `init.ptl`).
    ///
    /// A loaded script normally supplies both the layout and the theme. In
    /// **config-only** mode ([`script_owns_layout`](Self::script_owns_layout) is
    /// false) the script is still consulted for the theme, color scheme, and
    /// permanent settings (`~/.garden/init.ptl`), but its `layout(...)` is
    /// ignored — the layout comes from [`fallback_layout`](Self::fallback_layout)
    /// instead. This is how `garden README.md` still picks up your init.ptl
    /// colors while opening the file you named.
    script: Option<ScriptHost>,
    /// Layout used when no script owns the layout — either no script is loaded
    /// (plain-file / `$EDITOR` mode) or the script is config-only (a file
    /// argument alongside `init.ptl`; see [`script`](Self::script)).
    fallback_layout: LayoutNode,
    /// Whether the loaded [`script`](Self::script) owns the layout. True for a
    /// bare `garden` (init.ptl drives the panes); false when a file argument
    /// makes init.ptl config-only. Always false without a script.
    script_owns_layout: bool,
    viewport: Viewport,
    panes: Vec<Pane>,
    focus: usize,
    /// Mouse position in logical pixels.
    mouse: (f32, f32),
    /// Active mouse drag (left button held): either a text selection or a
    /// scrollbar drag. See [`mouse::Drag`](crate::app::mouse::Drag).
    drag: Option<mouse::Drag>,
    /// The panel pane holding an unreleased **right**-button press, so the
    /// release reaches the same script that saw the press even if the pointer
    /// has since left the pane. The right button starts no drag of its own —
    /// it exists to open context menus — so it needs no [`mouse::Drag`] state.
    right_press: Option<usize>,
    /// Leftover sub-row/sub-column wheel motion owed to *panel scripts*.
    /// See [`mouse::ScrollTicks`](crate::app::mouse::ScrollTicks).
    scroll_ticks: mouse::ScrollTicks,
    /// Draggable split-pane dividers, recomputed with the pane rects. Empty for
    /// a single-pane layout.
    dividers: Vec<crate::layout::Divider>,
    /// An in-progress divider drag (resizing a split), independent of the
    /// text/scrollbar [`drag`](Self::drag).
    divider_drag: Option<DividerDrag>,
    /// Whether to overlay each panel pane's observed values (every name its last
    /// frame bound, plus the frame count) — the Petal-IDE state inspector,
    /// toggled with `:State`. Off by default.
    show_panel_state: bool,
    /// A live layout override used only while a divider is being dragged, so the
    /// split resizes on screen without persisting to disk on every mouse move
    /// (the final layout is saved on release). `None` outside a drag, when
    /// [`layout`](Self::layout) falls back to the script/fallback tree.
    live_layout: Option<LayoutNode>,
    /// Last layout-script reload error, shown (red, `script:`-prefixed) in the
    /// status bar for as long as the script stays broken; cleared by the next
    /// successful reload. Unlike [`status_error`](Self::status_error) it
    /// survives keypresses — the script is still broken until it reloads.
    script_error: Option<String>,
    /// Transient error from the last user action (bad ex command, failed save,
    /// pattern not found); shown in the status bar (red) until the next key
    /// press starts a new action.
    status_error: Option<String>,
    /// The current panel script error (compile or runtime), reconciled from the
    /// live panes every tick by [`sync_panel_error`](Self::sync_panel_error).
    /// A panel paints its own error banner, but that banner is *inside* one pane
    /// on a canvas nobody may be looking at; this is the single obvious place —
    /// alongside [`script_error`](Self::script_error) — where "my panel is
    /// broken" is reported, and it is what `/state`'s `status_error` falls back
    /// to. Derived state: it clears itself the moment the panel compiles and
    /// renders again, and unlike [`status_error`](Self::status_error) a keypress
    /// does not dismiss it (the panel is still broken).
    panel_error: Option<String>,
    /// Transient informational message (file written / reloaded from disk,
    /// external-change warning); shown in the status bar in a non-error color
    /// until the next key press.
    status_note: Option<String>,
    /// Active `:` command line, when one is open.
    command_line: Option<CommandLine>,
    /// Active fuzzy file finder (`Cmd`/`Ctrl`+`P`), when one is open. Modal: it
    /// captures input until `Enter` opens the selection or `Escape` cancels.
    file_finder: Option<FileFinder>,
    /// Project root the open finder gathered from; a selected relative path is
    /// resolved against it before opening. See [`crate::file_finder`].
    file_finder_root: PathBuf,
    /// Language servers and the documents synced to them. Servers are spawned
    /// lazily on the first eligible file, so a session that opens none costs
    /// nothing. Driven by [`App::poll_lsp`]; see [`crate::lsp`].
    lsp: lsp::LspManager,
    /// Set after `Ctrl+W` is pressed: the next key is read as a window command
    /// (h/j/k/l to move focus, `w` to cycle) rather than reaching the vim
    /// layer. Cleared as soon as that key arrives. See [`crate::window_nav`].
    window_cmd_pending: bool,
    /// Clipboard for Cmd+C/X/V and the vim register write-through: the OS
    /// pasteboard in the running app, an in-memory one in tests.
    clipboard: Box<dyn Clipboard>,
    /// Set whenever state changed in a way the frontend should repaint;
    /// drained with [`take_redraw`](App::take_redraw).
    needs_redraw: bool,
    /// Set by Cmd+Q / Ctrl+Q / `:wqa`; the frontend shuts the process down
    /// when it sees it.
    quit: bool,
    /// Set by Cmd+W and by `:q` / `:wq` / `Ctrl+W q` on the last pane: this
    /// window should close. Distinct from `quit` — with multiple OS windows
    /// the process lives on; today's single-window frontends treat it as exit.
    close_window: bool,
    /// Set by `:windownew` / File ▸ New Window: the user asked for a new OS
    /// window. The App core is frontend-independent and cannot create OS
    /// windows itself, so this is surfaced as an intent the windowed frontend
    /// polls via [`take_new_window_request`](App::take_new_window_request)
    /// (the single-window frontends drain it and report an error instead).
    new_window_requested: bool,
    /// Active color theme: the built-in default overlaid with the script's
    /// `color_theme` overrides. Rebuilt whenever the script's theme revision
    /// changes (see [`poll_script`](App::poll_script)).
    theme: theme::Theme,
    /// Built-in palette selected for this window before script color overrides.
    theme_scheme: theme::ThemeScheme,
    /// Last theme revision seen from the script, to detect colors-only edits.
    last_theme_rev: u64,
    /// Per-session event log (actions/events buffered to the state DB), when
    /// the state database is available. Attached after construction by the
    /// frontend via [`set_event_log`](App::set_event_log); `None` in unit tests
    /// and when state is unavailable, in which case logging and `:report` are
    /// silently disabled.
    event_log: Option<EventLog>,
    /// Recently-opened files/projects/PRs, when the state database is
    /// available. Attached after construction by the frontend via
    /// [`set_recents`](App::set_recents); `None` in unit tests and when state
    /// is unavailable, in which case nothing is recorded — recording is
    /// bookkeeping and must never stand between the user and their file.
    recents: Option<crate::recents::Recents>,
    /// Whether this process may open a **native modal** file picker
    /// ([`crate::file_dialog`]). Only the windowed frontend turns it on (via
    /// [`enable_native_dialogs`](App::enable_native_dialogs)) — a modal in
    /// `--term`, `--headless`, or a unit test has no window to attach to and
    /// would block the thread with nobody able to dismiss it, so the
    /// `open_file_dialog` mutation reports that instead of hanging. Unlike
    /// [`top_inset`](App::top_inset), headless does *not* opt in: it draws a
    /// titlebar but has no desktop session.
    native_dialogs: bool,
    /// Height of the custom titlebar reserved at the top of the drawable area,
    /// logical pixels. Zero (the default) draws no titlebar; the windowed and
    /// headless frontends enable it via [`enable_titlebar`](App::enable_titlebar).
    /// The terminal frontend leaves it off.
    top_inset: f32,
    /// Monotonically increasing count of scenes built (presented, captured, or
    /// dumped) — the debug server's global frame number (`X-Garden-Frame`,
    /// `GET /frame`, the top-level `frame` in `/state`). Bumped by
    /// [`build_scene`](App::build_scene); a `Cell` because scene building is
    /// deliberately `&self`. Unlike a panel's per-pane `frame_count` (which
    /// resets when its script hot-reloads), this never resets while the
    /// process lives, so clients can order captures against it reliably.
    frame: std::cell::Cell<u64>,
    /// Files that must never be overwritten in place: saving a pane whose file
    /// is in this set opens a "save as" filename prompt instead. The Petal-IDE
    /// default (scratch) mode adds its scratch file here so edits are saved to a
    /// user-named file rather than clobbering the scratch. Attached after
    /// construction by the frontend via
    /// [`set_save_as_paths`](App::set_save_as_paths); empty in unit tests.
    save_as_paths: std::collections::HashSet<PathBuf>,
    /// Petal-IDE session state (the IR inspector + its shared render cache),
    /// turned on by the frontend via [`enable_ide`](App::enable_ide) for a
    /// `garden petal-ide` launch. `None` in a normal window; its presence gates
    /// the toolbar, play/pause, and the IR inspector panel.
    ide: Option<IdeState>,
    /// Height reserved for the IDE toolbar band below the titlebar, logical
    /// pixels. Zero unless [`enable_ide`](App::enable_ide) turned it on.
    toolbar_h: f32,
    /// Whether canvas re-rendering is paused (the toolbar's ▶/⏸). While paused,
    /// panels don't tick and the IR source isn't refreshed — the last frame
    /// holds on screen, but the editor stays fully live. Toggled from the
    /// toolbar or the `TogglePlay` menu action.
    paused: bool,
    /// The full direct-manipulation trace under the pointer, kept for the debug
    /// server's `/state`. The editor renders only the call span (each pane's
    /// [`trace_highlight`](crate::EditorView::trace_highlight)); this carries the
    /// rest — callee, and each argument's source kind, literal, and editable span
    /// — so automation can assert what a drag mode would rewrite without decoding
    /// pixels. There is one pointer, so there is one of these.
    trace: Option<TraceDetail>,
    /// The in-progress **drag-to-edit** gesture, when one is under way: a shape
    /// on a traced canvas is being pulled, and each move rewrites the numbers
    /// that placed it. Held beside [`App::drag`] (which is `Copy`) because a
    /// gesture carries a list of arguments, not just an index.
    manip: Option<mouse::ManipDrag>,
}

/// The traced draw call under the pointer, with the file it belongs to — the
/// `/state` view of [`App::trace`](App::trace).
#[derive(Debug, Clone)]
pub(in crate::app) struct TraceDetail {
    /// The paired editor's file, resolved — which pane's coordinates the spans
    /// are in.
    pub file: PathBuf,
    pub trace: garden_script::DrawTrace,
}

/// An in-progress split-divider drag. Captured on press over a divider; each
/// move resizes the split relative to the `baseline` (so it never drifts), and
/// the release persists the final layout.
struct DividerDrag {
    /// Path + boundary of the split being resized (see [`layout::Divider`]).
    path: Vec<usize>,
    before: usize,
    /// The split axis's usable extent, for converting pixels to a ratio delta.
    span: f32,
    /// `true` for a vertical divider (drag along x); `false` for horizontal.
    vertical: bool,
    /// Mouse coordinate along the split axis at the moment of the press.
    start_px: f32,
    /// The layout at drag start; each move resizes a fresh clone of this, so the
    /// result depends only on the total drag distance (no per-frame drift).
    baseline: LayoutNode,
}

impl App {
    pub fn new(
        script: Option<ScriptHost>,
        fallback_layout: LayoutNode,
        script_owns_layout: bool,
        viewport: Viewport,
        clipboard: Box<dyn Clipboard>,
    ) -> App {
        // The base scheme comes from the script's `color_scheme(...)` call (a
        // persisted settings choice), falling back to the built-in dark palette.
        // This runs even in config-only mode (a file argument), so `init.ptl`
        // colors apply whether or not the script also owns the layout.
        let theme_scheme = script
            .as_ref()
            .and_then(ScriptHost::scheme)
            .and_then(theme::ThemeScheme::from_key)
            .unwrap_or(theme::ThemeScheme::Dark);
        let theme = build_theme(&script, theme_scheme);
        let last_theme_rev = script.as_ref().map(ScriptHost::theme_rev).unwrap_or(0);
        // Without a script there is nothing to own the layout; normalize so the
        // flag is only ever true alongside a loaded script.
        let script_owns_layout = script.is_some() && script_owns_layout;
        let mut app = App {
            script,
            fallback_layout,
            script_owns_layout,
            viewport,
            panes: Vec::new(),
            focus: 0,
            mouse: (0.0, 0.0),
            drag: None,
            right_press: None,
            scroll_ticks: mouse::ScrollTicks::default(),
            dividers: Vec::new(),
            divider_drag: None,
            show_panel_state: false,
            live_layout: None,
            script_error: None,
            status_error: None,
            panel_error: None,
            status_note: None,
            command_line: None,
            file_finder: None,
            file_finder_root: PathBuf::from("."),
            lsp: lsp::LspManager::default(),
            window_cmd_pending: false,
            clipboard,
            needs_redraw: true,
            quit: false,
            close_window: false,
            new_window_requested: false,
            theme,
            theme_scheme,
            last_theme_rev,
            event_log: None,
            recents: None,
            native_dialogs: false,
            top_inset: 0.0,
            frame: std::cell::Cell::new(0),
            save_as_paths: std::collections::HashSet::new(),
            ide: None,
            toolbar_h: 0.0,
            paused: false,
            trace: None,
            manip: None,
        };
        app.rebuild_panes();
        app
    }

    /// Reserve space for the custom titlebar and draw it. Called by the
    /// windowed and headless frontends (which present [`build_scene`](App::build_scene)
    /// output) so the chrome matches; the terminal frontend leaves it off.
    pub fn enable_titlebar(&mut self) {
        self.top_inset = TITLEBAR_H;
        self.reposition_panes();
        self.needs_redraw = true;
    }

    /// Allow the app core to open a native modal file picker. Called by the
    /// **windowed** frontend only — it is the one frontend with a desktop
    /// session to show a modal in and an event loop to return to.
    pub fn enable_native_dialogs(&mut self) {
        self.native_dialogs = true;
    }

    /// Turn on Petal-IDE mode: record the target program the IR inspector
    /// compiles (and the seeded IR-drawer path a panel pane is matched against),
    /// and reserve the toolbar band below the titlebar. Called by the windowed
    /// and headless frontends after [`enable_titlebar`](Self::enable_titlebar)
    /// for a `garden petal-ide` launch; the terminal frontend leaves it off.
    pub fn enable_ide(&mut self, target: PathBuf, ir_view_path: PathBuf) {
        self.ide = Some(IdeState::new(target, ir_view_path));
        self.toolbar_h = TOOLBAR_H;
        self.reposition_panes();
        self.needs_redraw = true;
    }

    /// True once the user asked to quit; the frontend exits its loop.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// True once the user asked to close this window (not the whole process);
    /// the frontend tears the window down. Single-window frontends currently
    /// treat it like [`should_quit`](Self::should_quit).
    pub fn should_close(&self) -> bool {
        self.close_window
    }

    /// The global frame number: how many scenes [`build_scene`](App::build_scene)
    /// has built so far (each build — presented, captured, or dumped — is one
    /// frame). See the `frame` field docs for why this exists.
    pub fn frame(&self) -> u64 {
        self.frame.get()
    }

    /// How many panes this window holds. Used by the debug server's `/windows`
    /// listing (the frontend can't see the private pane vec).
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Drive every panel pane: hot-reload its script, bind pane-local mouse
    /// position, and run a frame while the panel is awake. Sets the redraw flag
    /// if anything changed. Returns whether **any** panel is still animating, so
    /// a frontend with a real loop can keep a ~60fps tick going (and fall back to
    /// the slow poll cadence once everything sleeps). See
    /// [`crate::panel_view`] for the sleep/wake model.
    pub fn tick_panels(&mut self) -> bool {
        self.tick_panels_pass().1
    }

    /// One tick pass over every panel (the body of [`tick_panels`](Self::tick_panels)):
    /// hot-reload, bind the mouse, run a frame while awake. Returns
    /// `(changed, animating)` — whether any panel's **drawn output** changed
    /// this pass (the settle criterion; a frame that redrew identical commands
    /// does not count), and whether any panel is still awake. Sets the redraw
    /// flag exactly as `tick_panels` always has.
    fn tick_panels_pass(&mut self) -> (bool, bool) {
        if !self.panes.iter().any(|p| p.is_panel()) {
            return (false, false);
        }
        // Paused (the toolbar's ⏸): freeze all canvas re-rendering and IR refresh.
        // The last frame stays on screen and the editor remains fully live, so you
        // can edit and then hit ▶ to see the accumulated change at once.
        //
        // Direct manipulation keeps working, though, which is why the trace
        // reconcile runs *before* this returns: the frozen frame is exactly what
        // the pointer is over, its command list and program are both still here,
        // and "freeze a moment, then point at it" is the gesture pausing exists
        // for. Skipping it would not merely disable the highlight — it would
        // strand the last one on screen, pointing confidently at the wrong line.
        if self.paused {
            if self.sync_trace_highlight() {
                self.needs_redraw = true;
            }
            return (false, false);
        }
        // Petal-IDE: republish the target program's live buffer to the IR
        // inspector so its IR/bytecode/AST track edits (a no-op outside IDE mode).
        if self.refresh_ir_source() {
            self.needs_redraw = true;
        }
        // Petal-IDE live binding: before ticking, drive each panel from its
        // paired editor buffer so an unsaved edit shows on the canvas this frame.
        let live_changed = self.sync_editor_panels();
        let now = Instant::now();
        let (mx, my) = self.mouse;
        // The current host theme, injected read-only into every panel each frame
        // so a drawer's `panel_theme()` reflects the live scheme (built once).
        let panel_theme = self.theme.to_panel_theme();
        let mut animating = false;
        let mut changed = live_changed;
        let mut redraw = live_changed;
        for pane in &mut self.panes {
            let Some(panel) = pane.panel.as_mut() else {
                continue;
            };
            // Only in-process (file-backed) panels reload from disk: a subprocess
            // panel's source arrives over the pipe via `setScript`, and its
            // "path" is the client *binary*. `PanelHost::poll_reload` no-ops for
            // a source-backed host, so this is safe to call on every panel.
            if panel.poll_reload(now) {
                redraw = true;
                changed = true;
            }
            panel.set_theme(panel_theme.clone());
            panel.set_mouse((mx - pane.rect.x) as i32, (my - pane.rect.y) as i32);
            if panel.tick(now, pane.rect, self.viewport.cell) {
                redraw = true;
                changed |= panel.last_tick_changed();
            }
            if panel.is_awake(now) {
                animating = true;
            }
        }
        // Act on any navigation intents raised this pass. A swap rebuilds the
        // panel's host, so the new screen renders on the next pass — count it as
        // `changed` so the settle loop runs that pass. In-process panels have no
        // subprocess client, so this is the only place their nav is drained.
        for i in 0..self.panes.len() {
            if self.drain_panel_nav(i) {
                changed = true;
                redraw = true;
            }
        }
        if self.sync_panel_error() {
            redraw = true;
        }
        if redraw {
            self.needs_redraw = true;
        }
        (changed, animating)
    }

    /// Reconcile [`panel_error`](Self::panel_error) with the live panes: a panel
    /// that fails to compile (at load, on a hot reload, or from a paired editor
    /// buffer) or raises at runtime reports here, and the report clears itself as
    /// soon as the panel runs clean again. Each *new* message is also written to
    /// the event log and stderr, because the failure people actually hit is a
    /// hot reload that silently keeps running the old program: without a line
    /// somewhere, an edit that doesn't compile is indistinguishable from an edit
    /// that had no effect. Returns whether the reported error changed.
    fn sync_panel_error(&mut self) -> bool {
        let current = self.panes.iter().find_map(|pane| {
            let panel = pane.panel.as_ref()?;
            let err = panel.error()?;
            // The headline only: a Petal runtime error carries a multi-line
            // source excerpt, and the status bar is one line.
            let headline = err.lines().next().unwrap_or(err);
            Some(format!("panel {}: {headline}", panel.script()))
        });
        if current == self.panel_error {
            return false;
        }
        if let Some(msg) = current.clone() {
            eprintln!("garden: {msg}");
            self.log_event("panel", msg);
        }
        self.panel_error = current;
        true
    }

    /// The error to show as *the* current error: the last user-action error if
    /// one is standing, else the live panel error. One place to look, for the
    /// status bar and for a headless client polling `/state`.
    pub(crate) fn effective_status_error(&self) -> Option<&str> {
        self.status_error.as_deref().or(self.panel_error.as_deref())
    }

    /// The debug capture consistency contract: run panel frames until the drawn
    /// output is steady, so a scene built now (for `/screenshot` or `/scene`)
    /// reflects **all previously injected input**, in every frontend.
    ///
    /// A panel consumes queued input on its next frame, and a script may take a
    /// frame or two more to propagate it through chained `state` variables
    /// (frame N sets a value, frame N+1 draws from it). Each settle pass runs
    /// one frame of every awake panel; the loop stops as soon as a pass changes
    /// no panel's draw commands — a fixed point, since a further frame would see
    /// no new input and ~zero `dt` — or after `SETTLE_MAX_PASSES` for panels
    /// that animate continuously and never reach one (for those the capture is
    /// simply the latest complete frame, which is still self-consistent: the
    /// renderer draws the whole scene atomically into an offscreen target).
    /// Passes run back-to-back, so `dt ≈ 0` and time-based animation is not
    /// fast-forwarded; asleep panels never tick — their cached commands are
    /// already steady. Out of scope: data a panel-mode GPP client fetches
    /// asynchronously (`query` round-trips) — poll `/state` for that.
    pub fn settle_panels(&mut self) {
        /// Upper bound on settle passes, to stay responsive with panels that
        /// redraw differently every frame (clocks, spinners). Generous next to
        /// the 1–2 frames real input propagation takes.
        const SETTLE_MAX_PASSES: usize = 10;
        for _ in 0..SETTLE_MAX_PASSES {
            let (changed, _) = self.tick_panels_pass();
            if !changed {
                break;
            }
        }
    }

    /// Advance every panel by `n` frames of exactly `dt` seconds each, ignoring
    /// the sleep/wake window — the debug server's `POST /tick`. Deterministic
    /// panel time for a harness driving a game or an animation, which otherwise
    /// has to post a no-op keypress per frame just to make one happen. Returns
    /// how many panel frames actually ran.
    pub fn advance_panels(&mut self, n: u32, dt: f64) -> u64 {
        let cell = self.viewport.cell;
        let panel_theme = self.theme.to_panel_theme();
        let mut frames = 0u64;
        for _ in 0..n {
            let now = Instant::now();
            for pane in &mut self.panes {
                let Some(panel) = pane.panel.as_mut() else {
                    continue;
                };
                panel.set_theme(panel_theme.clone());
                panel.tick_with_dt(now, dt, pane.rect, cell);
                frames += 1;
            }
            for i in 0..self.panes.len() {
                self.drain_panel_nav(i);
            }
        }
        if frames > 0 {
            self.needs_redraw = true;
        }
        frames
    }

    /// Restart every panel pane from its source, discarding Petal `state` — the
    /// debug server's `POST /panel/reset`, and the same operation the toolbar's
    /// Reset performs. `state` deliberately survives hot reload, which makes
    /// iterating on *seeded* data impossible in place: you edit the generator,
    /// the old seed is restored, and only killing the process helps. Returns
    /// how many panels restarted.
    pub fn reset_panel_state(&mut self) -> usize {
        let now = Instant::now();
        let mut count = 0;
        for pane in &mut self.panes {
            // A GPP-pushed panel has no file to reload from — its "path" is the
            // client binary — so restarting it would only record a load error.
            if let Some(pv) = pane.panel.as_mut().filter(|pv| pv.is_file_backed()) {
                pv.restart(now);
                count += 1;
            }
        }
        if count > 0 {
            self.paused = false;
            self.needs_redraw = true;
        }
        count
    }

    /// Run one frame of the focused pane's panel immediately, if it is one, so
    /// input routed to a panel takes effect at once — deterministic for the
    /// debug server (`POST /key` then `GET /state` sees the update) and snappy
    /// for the user, rather than waiting for the next animation tick. The panel
    /// is woken first so the frame is guaranteed to run.
    pub(in crate::app) fn tick_focused_panel(&mut self) {
        self.tick_panel_at(self.focus);
    }

    /// The focused pane's panel, if it is one — for routing keys to its embedded
    /// text-view regions.
    pub(in crate::app) fn focused_panel_mut(
        &mut self,
    ) -> Option<&mut crate::panel_view::PanelView> {
        self.panes
            .get_mut(self.focus)
            .and_then(|p| p.panel.as_mut())
    }

    /// The `text_view` region that currently owns keyboard focus in the focused
    /// panel, if any — the target of the region clipboard/select-all chords.
    pub(in crate::app) fn focused_panel_region(&self) -> Option<i64> {
        self.panes
            .get(self.focus)
            .and_then(|p| p.panel.as_ref())
            .and_then(|p| p.focused_region())
    }

    /// Run one immediate frame of pane `idx` if it is a panel (a no-op otherwise),
    /// binding the current mouse position and waking it first. Shared by
    /// [`tick_focused_panel`](Self::tick_focused_panel) and the scroll path (which
    /// targets the pane under the pointer, not necessarily the focused one).
    pub(in crate::app) fn tick_panel_at(&mut self, idx: usize) {
        self.tick_panel_frame(idx);
        // A frame may have raised a `navigate*` intent. Act on it now (an
        // in-process panel has no subprocess client, so `poll_script_clients`
        // never drains it) and render the swapped-in screen immediately, so the
        // debug server's `POST /key` → `GET /state` contract sees the new screen.
        if self.drain_panel_nav(idx) {
            self.tick_panel_frame(idx);
        }
    }

    /// Run one immediate frame of pane `idx` if it is a panel, binding the current
    /// mouse and waking it first. The frame body of [`tick_panel_at`], separated so
    /// a screen swap can re-run it without recursing through the nav drain.
    fn tick_panel_frame(&mut self, idx: usize) {
        let now = Instant::now();
        let (mx, my) = self.mouse;
        let panel_theme = self.theme.to_panel_theme();
        if let Some(pane) = self.panes.get_mut(idx) {
            if let Some(panel) = pane.panel.as_mut() {
                panel.note_activity(now);
                panel.set_theme(panel_theme);
                panel.set_mouse((mx - pane.rect.x) as i32, (my - pane.rect.y) as i32);
                if panel.tick(now, pane.rect, self.viewport.cell) {
                    self.needs_redraw = true;
                }
            }
        }
    }

    /// Drain and act on any browser-history navigation intents pane `idx` raised
    /// this frame (`navigate`/`navigate_replace`/`navigate_back`/`navigate_forward`).
    /// In-process `panel(...)` panes have no subprocess client, so their intents are
    /// routed here — the counterpart to [`poll_script_clients`](Self::poll_script_clients)
    /// for subprocess-backed panels. Returns whether the running screen changed.
    pub(in crate::app) fn drain_panel_nav(&mut self, idx: usize) -> bool {
        let events = match self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
            Some(pv) => pv.take_nav_events(),
            None => return false,
        };
        if events.is_empty() {
            return false;
        }
        self.handle_client_events(idx, events);
        true
    }

    /// Stamp activity on every panel (any user input restarts the 10s wake
    /// window). Cheap no-op when there are no panels.
    pub(in crate::app) fn wake_panels(&mut self) {
        if !self.panes.iter().any(|p| p.is_panel()) {
            return;
        }
        let now = Instant::now();
        for pane in &mut self.panes {
            if let Some(panel) = pane.panel.as_mut() {
                panel.note_activity(now);
            }
        }
        self.needs_redraw = true;
    }

    /// Drain the redraw flag. Returns true if state changed since the last
    /// call and the frontend should repaint.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.needs_redraw)
    }

    /// Drain the new-window intent (`:windownew`, File ▸ New Window). The App
    /// can't create OS windows — it is frontend-independent — so the windowed
    /// frontend polls this and spawns a window when it returns true; the same
    /// take-style contract as [`take_redraw`](App::take_redraw).
    pub fn take_new_window_request(&mut self) -> bool {
        std::mem::take(&mut self.new_window_requested)
    }

    /// Report an error in the status bar (red, cleared by the next key press).
    /// For frontends surfacing conditions the core can't act on itself — e.g.
    /// a new-window request reaching a single-window frontend.
    pub(crate) fn set_status_error(&mut self, message: impl Into<String>) {
        self.status_error = Some(message.into());
        self.needs_redraw = true;
    }

    /// The logical drawable size last reported by the frontend.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Last known mouse position in logical pixels.
    pub fn mouse(&self) -> (f32, f32) {
        self.mouse
    }

    /// The frontend's drawable area changed (window resize, terminal resize).
    pub fn set_viewport_size(&mut self, w: f32, h: f32) {
        self.viewport.size = (w, h);
        self.reposition_panes();
        self.needs_redraw = true;
    }

    pub fn theme_scheme(&self) -> theme::ThemeScheme {
        self.theme_scheme
    }

    pub fn set_theme_scheme(&mut self, scheme: theme::ThemeScheme) {
        self.theme_scheme = scheme;
        self.theme = build_theme(&self.script, self.theme_scheme);
        self.status_note = Some(format!("theme: {}", scheme.label()));
        self.needs_redraw = true;
        // Persist the choice to the permanent init.ptl so it survives a restart.
        self.persist_setting(
            &[garden_script::Goal::should_call(
                "color_scheme",
                [scheme.key()],
            )],
            "color scheme",
        );
    }

    /// Write a permanent settings change to the user's `init.ptl` via
    /// goal-based editing — the standard path for durable settings (see
    /// [`ScriptHost::save_setting`]). A no-op without a loaded script (the
    /// plain-file/`$EDITOR` shape has no config to write). The in-memory setting
    /// has already been applied by the caller; this only makes it stick. A write
    /// failure surfaces in the status bar but is otherwise non-fatal.
    fn persist_setting(&mut self, goals: &[garden_script::Goal], what: &str) {
        let Some(script) = self.script.as_mut() else {
            return;
        };
        match script.save_setting(goals) {
            Ok(_) => self.log_event("settings", format!("persisted {what}")),
            Err(err) => {
                self.status_error = Some(format!("could not save {what}: {err}"));
            }
        }
    }

    fn layout(&self) -> &LayoutNode {
        // While a divider is being dragged, the live override reflects the
        // in-progress resize (persisted only on release).
        if let Some(live) = &self.live_layout {
            return live;
        }
        match &self.script {
            // Config-only script (a file argument): its `layout(...)` is ignored;
            // the file panes in `fallback_layout` win. See [`script`](Self::script).
            Some(script) if self.script_owns_layout => script.layout(),
            _ => &self.fallback_layout,
        }
    }

    fn status_height(&self) -> f32 {
        self.viewport.cell.1 + 8.0
    }

    /// Rect available for panes (viewport minus margins, titlebar + IDE
    /// toolbar, and the status bar).
    fn pane_area(&self) -> Rect {
        let (w, h) = self.viewport.size;
        let top = self.top_inset + self.toolbar_h + MARGIN;
        Rect {
            x: MARGIN,
            y: top,
            w: (w - 2.0 * MARGIN).max(0.0),
            h: (h - top - MARGIN - self.status_height()).max(0.0),
        }
    }

    fn focused_visible_lines(&self) -> usize {
        match self.panes.get(self.focus) {
            Some(pane) => EditorView::visible_lines(pane.rect, self.viewport.cell.1),
            None => 1,
        }
    }
}

/// The active theme: the built-in defaults overlaid with the script's
/// `color_theme` overrides, or just the defaults when no script is loaded.
fn build_theme(script: &Option<ScriptHost>, scheme: theme::ThemeScheme) -> theme::Theme {
    match script {
        Some(script) => theme::Theme::scheme(scheme).with_script_overrides(script.theme()),
        None => theme::Theme::scheme(scheme),
    }
}

/// Outcome of [`save_all_panes`]: the first save failure formatted for the
/// status bar (if any), and how many dirty save-protected panes were skipped
/// rather than overwritten.
struct SaveAllOutcome {
    first_error: Option<String>,
    skipped_protected: usize,
}

/// Save every dirty pane that has a file path; panes without a path are
/// skipped. A pane whose path is in `protected` (the Petal-IDE scratch, see
/// [`App::save_as_paths`]) is *not* overwritten — it's left dirty and counted,
/// so a caller can divert it to the save-as prompt instead. Later panes still
/// get saved even after an earlier failure.
fn save_all_panes(
    panes: &mut [Pane],
    protected: &std::collections::HashSet<PathBuf>,
) -> SaveAllOutcome {
    let mut first_error = None;
    let mut skipped_protected = 0;
    for pane in panes {
        if !pane.view.buffer.is_dirty() || pane.view.buffer.path().is_none() {
            continue;
        }
        // Mirror `save_focused`'s G1 guard: never silently clobber a
        // save-protected scratch buffer from save-all.
        if pane
            .view
            .buffer
            .path()
            .is_some_and(|p| protected.contains(p))
        {
            skipped_protected += 1;
            continue;
        }
        if let Err(err) = pane.view.save() {
            first_error.get_or_insert(format!("save failed: {err}"));
        }
    }
    SaveAllOutcome {
        first_error,
        skipped_protected,
    }
}

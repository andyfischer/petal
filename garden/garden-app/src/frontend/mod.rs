//! Pluggable presentation targets.
//!
//! A frontend owns its event loop and its presentation device, and drives the
//! frontend-independent [`App`](crate::app::App) core: it translates native
//! input into [`vim::Key`](crate::vim::Key) presses and logical-pixel mouse
//! positions, presents the [`Scene`](garden_render::Scene)s the core builds,
//! and watches the core's redraw/quit flags. Three implementations exist:
//!
//! - [`window::WindowFrontend`] — winit window + wgpu GPU renderer (default).
//! - [`headless::HeadlessFrontend`] — no UI at all; the debug server is the
//!   only way in or out. Used for testing.
//! - [`terminal::TerminalFrontend`] — a crossterm TUI in the controlling
//!   terminal, suitable for `EDITOR="garden --term"`.
//!
//! Event loops invert control differently per platform (winit owns the main
//! thread via callbacks; crossterm and the headless loop poll), so the
//! interface hands the whole thread to the frontend rather than abstracting
//! the loop itself.

pub mod grid;
pub mod headless;
#[cfg(target_os = "macos")]
pub mod macos_icon;
pub mod menu;
pub(crate) mod registry;
pub mod terminal;
pub mod window;

use std::time::Duration;

use garden_script::{LayoutNode, ScriptHost};

use crate::event_log::EventLog;

/// How often every frontend polls the layout script for hot reloads.
pub const RELOAD_POLL: Duration = Duration::from_millis(200);

/// Everything `main` resolves before choosing a frontend.
pub struct AppConfig {
    /// The loaded layout script, if any.
    pub script: Option<ScriptHost>,
    /// Layout used when no script owns the layout (plain-file / EDITOR usage, or
    /// a file argument that makes the script config-only).
    pub fallback_layout: LayoutNode,
    /// Whether [`script`](Self::script) owns the layout. True for a bare
    /// `garden`; false when a file argument makes init.ptl config-only (theme
    /// still applies, but the file panes win). Ignored without a script.
    pub script_owns_layout: bool,
    /// Start the debug server on this port (0 picks a free one).
    pub debug_port: Option<u16>,
    /// The per-window event log, when the state database is available. `None`
    /// disables logging (and `:report`), but never blocks the editor.
    pub event_log: Option<EventLog>,
    /// Save-protected files: saving a pane whose file is one of these prompts
    /// for a filename ("save as") instead of overwriting it. Populated for the
    /// Petal-IDE default (scratch) mode; empty otherwise.
    pub save_as_paths: std::collections::HashSet<std::path::PathBuf>,
    /// The Petal-IDE target program, when launched via `garden petal-ide`. `Some`
    /// turns on IDE mode (the top toolbar, play/pause, the IR inspector), with
    /// this path as the program the IR panel inspects. `None` for a normal window.
    pub ide_target: Option<std::path::PathBuf>,
}

/// One presentation target. `run` takes over the calling thread, constructs
/// the [`App`](crate::app::App) core with its own viewport metrics, and runs
/// until the user quits.
pub trait Frontend {
    fn run(self: Box<Self>, config: AppConfig) -> Result<(), String>;
}

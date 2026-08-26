//! Plain data types shared across the app core: the presentation geometry a
//! frontend renders with, modifier and menu inputs, the multi-click detector,
//! and the [`Pane`] record. These carry no behavior beyond their own
//! invariants, so they live apart from the [`App`](super::App) impl.

use std::time::{Duration, Instant};

use garden_render::Rect;
use garden_script::LayoutNode;

use crate::editor_view::EditorView;
use crate::panel_view::PanelView;
use crate::theme::ThemeScheme;

/// The presentation geometry a frontend renders with: logical size, monospace
/// cell metrics, and the physical-per-logical scale factor. All of the core's
/// layout math runs in these units.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Logical size of the drawable area (width, height).
    pub size: (f32, f32),
    /// Monospace cell metrics: (advance_width, line_height).
    pub cell: (f32, f32),
    /// Physical pixels per logical pixel.
    pub scale: f64,
}

/// Modifier state accompanying one key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub cmd: bool,
    pub ctrl: bool,
    pub shift: bool,
    /// `Alt`/`Option`. Carried all the way into a panel script's `mod_alt()`,
    /// which is why it exists: a panel whose bare letters are content needs a
    /// modifier the host doesn't already own.
    pub alt: bool,
}

impl Mods {
    /// The chord as petal-ui's modifier bitmask — `1=shift 2=ctrl 4=alt 8=cmd`,
    /// the same encoding a script reads through `modifiers` and the debug
    /// server reports as `panel.input.modifiers`. Used to match a panel's
    /// [key claims](crate::panel_view::PanelView::claims_key).
    pub fn bits(self) -> u8 {
        (self.shift as u8) | (self.ctrl as u8) << 1 | (self.alt as u8) << 2 | (self.cmd as u8) << 3
    }

    /// Whether a **command** modifier is held (`Cmd`/`Ctrl`/`Alt`). Shift is
    /// excluded: it only shifts the character.
    pub fn any_command(self) -> bool {
        self.cmd || self.ctrl || self.alt
    }
}

/// A native-menu command (the macOS menu bar). Built by the windowed
/// frontend and routed into the core via [`App::dispatch_menu`](super::App::dispatch_menu),
/// which reuses the same paths as the keyboard shortcuts and ex commands so
/// behavior stays identical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    // File
    NewFile,
    /// Open a new OS window (`:windownew`); fulfilled by the windowed frontend.
    NewWindow,
    OpenFile(std::path::PathBuf),
    OpenFolder(std::path::PathBuf),
    Save,
    SaveAll,
    CloseWindow,
    Quit,
    // Edit
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// Open the `/` search prompt (Edit ▸ Find…).
    Find,
    /// Jump to the next/previous match of the last search (vim's `n` / `N`).
    FindNext,
    FindPrev,
    // View
    SetTheme(ThemeScheme),
    /// Toggle soft wrap on the focused pane (`:set wrap` / `:set nowrap`).
    ToggleWrap,
    /// Toggle the focused pane's line-number gutter.
    ToggleLineNumbers,
    /// Toggle the panel state inspector (`:ToggleState`).
    ToggleStateInspector,
    /// Petal-IDE: freeze / resume canvas re-rendering (toolbar ▶ / ⏸).
    TogglePlay,
    /// Petal-IDE: open / close the IR inspector panel (toolbar IR button).
    ToggleIr,
    // Go
    /// Open the fuzzy file finder (Cmd+P).
    GoToFile,
    /// Panel history navigation (`:back` / `:forward`).
    Back,
    Forward,
    /// Open the directory browser on the focused file's directory (`:E`).
    ExploreDirectory,
    // Git
    GitLog,
    GitDiff,
    GitDiffStat,
    /// Open the projectional review editor against the default base (`:Review`).
    ReviewChanges,
    // Window (pane management, vim's Ctrl+W commands)
    SplitDown,
    SplitRight,
    CloseOtherPanes,
    ClosePane,
    NextPane,
}

/// Catalog of `/menu` debug-server action names, each paired with the kind of
/// string argument it takes (`None` = no arg, `Some("path")` for the Open items,
/// `Some("scheme")` for `SetTheme`). The debug server's `GET /menu` lists these,
/// and `POST /menu` resolves a name through [`MenuAction::from_debug_request`].
/// Keep this in sync with that match — the `menu_catalog_round_trips` test below
/// asserts every entry here parses.
pub const MENU_ACTIONS: &[(&str, Option<&str>)] = &[
    ("NewFile", None),
    ("NewWindow", None),
    ("OpenFile", Some("path")),
    ("OpenFolder", Some("path")),
    ("Save", None),
    ("SaveAll", None),
    ("CloseWindow", None),
    ("Quit", None),
    ("Undo", None),
    ("Redo", None),
    ("Cut", None),
    ("Copy", None),
    ("Paste", None),
    ("SelectAll", None),
    ("Find", None),
    ("FindNext", None),
    ("FindPrev", None),
    ("SetTheme", Some("scheme")),
    ("ToggleWrap", None),
    ("ToggleLineNumbers", None),
    ("ToggleStateInspector", None),
    ("TogglePlay", None),
    ("ToggleIr", None),
    ("GoToFile", None),
    ("Back", None),
    ("Forward", None),
    ("ExploreDirectory", None),
    ("GitLog", None),
    ("GitDiff", None),
    ("GitDiffStat", None),
    ("ReviewChanges", None),
    ("SplitDown", None),
    ("SplitRight", None),
    ("CloseOtherPanes", None),
    ("ClosePane", None),
    ("NextPane", None),
];

impl MenuAction {
    /// Resolve a `/menu` debug request — a case-insensitive variant name plus an
    /// optional string argument — into a [`MenuAction`], so the debug server can
    /// inject the menu clicks the native menu bar produces (muda accelerators and
    /// clicks can't otherwise be driven headlessly). The Open items take a
    /// filesystem path in place of the native file picker; `SetTheme` takes a
    /// theme key or label. Names mirror the [`MENU_ACTIONS`] catalog.
    pub fn from_debug_request(name: &str, arg: Option<&str>) -> Result<MenuAction, String> {
        use MenuAction::*;
        let need_arg = || arg.ok_or_else(|| format!("menu action {name:?} requires an \"arg\""));
        Ok(match name.to_ascii_lowercase().as_str() {
            "newfile" => NewFile,
            "newwindow" => NewWindow,
            "openfile" => OpenFile(std::path::PathBuf::from(need_arg()?)),
            "openfolder" => OpenFolder(std::path::PathBuf::from(need_arg()?)),
            "save" => Save,
            "saveall" => SaveAll,
            "closewindow" => CloseWindow,
            "quit" => Quit,
            "undo" => Undo,
            "redo" => Redo,
            "cut" => Cut,
            "copy" => Copy,
            "paste" => Paste,
            "selectall" => SelectAll,
            "find" => Find,
            "findnext" => FindNext,
            "findprev" => FindPrev,
            "settheme" => {
                let key = need_arg()?;
                SetTheme(
                    ThemeScheme::ALL
                        .iter()
                        .copied()
                        .find(|s| {
                            s.key().eq_ignore_ascii_case(key) || s.label().eq_ignore_ascii_case(key)
                        })
                        .ok_or_else(|| format!("unknown theme scheme {key:?}"))?,
                )
            }
            "togglewrap" => ToggleWrap,
            "togglelinenumbers" => ToggleLineNumbers,
            "togglestateinspector" => ToggleStateInspector,
            "toggleplay" => TogglePlay,
            "toggleir" => ToggleIr,
            "gotofile" => GoToFile,
            "back" => Back,
            "forward" => Forward,
            "exploredirectory" => ExploreDirectory,
            "gitlog" => GitLog,
            "gitdiff" => GitDiff,
            "gitdiffstat" => GitDiffStat,
            "reviewchanges" => ReviewChanges,
            "splitdown" => SplitDown,
            "splitright" => SplitRight,
            "closeotherpanes" => CloseOtherPanes,
            "closepane" => ClosePane,
            "nextpane" => NextPane,
            _ => return Err(format!("unknown menu action {name:?}")),
        })
    }
}

/// A clickable control in the Petal-IDE toolbar (the band below the titlebar).
/// Dispatched by [`App::dispatch_toolbar`](super::App::dispatch_toolbar) when a
/// press lands on its button; the same actions are reachable from the native
/// menu / debug `/menu` (see [`MenuAction`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    /// Freeze / resume canvas re-rendering (▶ / ⏸).
    TogglePlay,
    /// Open or close the IR inspector panel.
    ToggleIr,
    /// Toggle the live-state inspector overlay (`:State`).
    ToggleState,
    /// Restart the canvas sketch from scratch (reset its Petal `state`).
    ResetSketch,
}

/// A laid-out toolbar button: where it sits, what it does, its label, and
/// whether it reads as active (lit). Produced by
/// [`App::toolbar_buttons`](super::App::toolbar_buttons) and consumed by both
/// the scene builder (draw) and the pointer handler (hit-test), so geometry and
/// behavior can never drift apart.
pub(crate) struct ToolbarButton {
    pub rect: Rect,
    pub action: ToolbarAction,
    pub label: &'static str,
    pub active: bool,
}

/// Presses within this window of the previous one chain into a multi-click.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);
/// ... and within this distance of it, per axis, in logical pixels.
const MULTI_CLICK_SLOP: f32 = 6.0;

/// Detects double/triple clicks from timing and proximity. Each frontend that
/// receives real mouse input owns one and passes the resulting count into
/// [`App::mouse_down`](super::App::mouse_down); the time is injected so counting
/// stays testable (the debug server's `/mouse` injects an explicit count instead).
#[derive(Default)]
pub struct ClickCounter {
    /// `(when, x, y)` of the previous press, plus the running count.
    last: Option<(Instant, f32, f32)>,
    count: u32,
}

impl ClickCounter {
    pub fn new() -> ClickCounter {
        ClickCounter::default()
    }

    /// Record a press at `(x, y)` at time `now`; returns the click count this
    /// press reaches (1 for a fresh click, 2 for a double-click, ...).
    pub fn click(&mut self, x: f32, y: f32, now: Instant) -> u32 {
        let chained = self.last.is_some_and(|(when, lx, ly)| {
            now.duration_since(when) <= MULTI_CLICK_WINDOW
                && (x - lx).abs() <= MULTI_CLICK_SLOP
                && (y - ly).abs() <= MULTI_CLICK_SLOP
        });
        self.count = if chained { self.count + 1 } else { 1 };
        self.last = Some((now, x, y));
        self.count
    }
}

pub struct Pane {
    pub rect: Rect,
    pub file: Option<String>,
    pub view: EditorView,
    /// The on-disk version we've already warned about while this pane was
    /// dirty, so [`App::poll_files`](super::App::poll_files) flags an external
    /// edit only once per distinct disk change. `None` while in sync (or after
    /// a reload).
    pub external_conflict: Option<garden_core::DiskStamp>,
    /// The `(command, args)` a script-client pane was spawned for, so a rebuild
    /// can reuse the live child instead of respawning it. `None` for editor
    /// panes.
    pub command_args: Option<(String, Vec<String>)>,
    /// The Petal runtime driving this pane, when it is a `panel(...)` pane. The
    /// pane's [`view`](Self::view) is then only a passive title/status carrier;
    /// the panel paints the pane's pixels (see [`crate::panel_view`]).
    pub panel: Option<PanelView>,
    /// When set, the pane rejects edits: navigation, selection, and scrolling
    /// work, but any keystroke that would mutate the buffer is a no-op. It is
    /// transient — a layout rebuild drops the pane back to a plain editor.
    pub read_only: bool,
}

impl Pane {
    /// An editor pane around an already-prepared `view` — a reused buffer, a
    /// freshly opened file, or an error-message surface. The counterpart for
    /// construction to [`set_editor`](Self::set_editor); both guarantee the
    /// process/conflict fields start cleared.
    pub fn editor(rect: Rect, file: Option<String>, view: EditorView) -> Pane {
        Pane {
            rect,
            file,
            view,
            external_conflict: None,
            command_args: None,
            panel: None,
            read_only: false,
        }
    }

    /// A panel pane: a Petal-scripted graphics surface. The `view` is a passive
    /// carrier (seeded with the script's name as its title); all pixels come
    /// from `panel`. The construction counterpart used when rebuilding panes.
    pub fn panel(rect: Rect, mut view: EditorView, panel: PanelView) -> Pane {
        view.set_external_title(Some(panel_title(panel.script())));
        Pane {
            rect,
            file: None,
            view,
            external_conflict: None,
            command_args: None,
            panel: Some(panel),
            read_only: false,
        }
    }

    /// A **GPP script-client pane**: the client pushed the Petal `panel` and
    /// answers its `query`s over the pipe (the [`ProcessPane`] is held *inside*
    /// `panel`), so the pane renders/handles input as a panel but persists as a
    /// `process(command, args)` node — `command_args` is what round-trips it.
    ///
    /// [`ProcessPane`]: crate::process_pane::ProcessPane
    pub fn script_client(
        rect: Rect,
        mut view: EditorView,
        panel: PanelView,
        command: String,
        args: Vec<String>,
    ) -> Pane {
        view.set_external_title(Some(panel_title(panel.script())));
        Pane {
            rect,
            file: None,
            view,
            external_conflict: None,
            command_args: Some((command, args)),
            panel: Some(panel),
            read_only: false,
        }
    }

    /// Whether this pane's pixels are drawn by a Petal panel script.
    pub fn is_panel(&self) -> bool {
        self.panel.is_some()
    }

    /// The layout leaf describing this pane's *current* content: a process /
    /// script-client pane round-trips through its spawned command + args; a
    /// built-in panel through its script; any other pane through its file
    /// (possibly `None` for a scratch buffer). This is what lets the live panes —
    /// not a stale snapshot — define the persisted layout (see
    /// [`App::layout_from_panes`](super::App)).
    ///
    /// `command_args` is checked before `panel` so a **GPP client**
    /// (which renders as a panel but carries its spawned command) persists — and
    /// re-spawns — as a `process(...)` node, re-pushing its script on reload,
    /// rather than as an unloadable panel path.
    pub fn to_layout_node(&self) -> LayoutNode {
        if let Some((command, args)) = &self.command_args {
            return LayoutNode::Process {
                command: command.clone(),
                args: args.clone(),
            };
        }
        if let Some(panel) = &self.panel {
            // Persist by the panel's ORIGIN screen, not its live (possibly
            // navigated) one, so a reload rebuilds the declared node rather than
            // resurrecting a navigated screen as the layout-declared script.
            return LayoutNode::Panel {
                script: panel.origin_script().to_string(),
                // Persist the explicit navigation allowlist so a saved layout
                // keeps `screens: [...]` across a reload.
                screens: panel.screens().to_vec(),
            };
        }
        LayoutNode::Editor {
            file: self.file.clone(),
            line_numbers: self.view.show_line_numbers,
            wrap: self.view.wrap,
        }
    }

    /// Turn this pane into a plain editor on `file` (`None` = scratch buffer).
    /// Any GPP subprocess is dropped (its `Drop` shuts the child down) and all
    /// process/conflict state is cleared. This is the **one** way to make a pane
    /// — editor or browser — an editor, so every field stays consistent (a
    /// browser opening a file, `:e`, File ▸ New all funnel through here).
    pub fn set_editor(&mut self, file: Option<String>) {
        // `line_numbers` is a pane property, not a file property: it must survive
        // a content change (`:e`, File ▸ New, exiting a browser) so a later
        // `sync_layout` doesn't persist the gutter back off.
        let line_numbers = self.view.show_line_numbers;
        self.command_args = None;
        self.panel = None;
        self.read_only = false;
        self.external_conflict = None;
        self.view = EditorView::open(file.as_deref());
        self.view.show_line_numbers = line_numbers;
        self.file = file;
    }

}

/// A panel pane's titlebar/status label: the script's file name (its full path
/// when it has no file component).
fn panel_title(script: &str) -> String {
    std::path::Path::new(script)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(script)
        .to_string()
}

/// Which half of a key press is being delivered.
///
/// Every frontend delivers [`Tap`](KeyPhase::Tap) — a press and its release in
/// the same frame — because none of them report key-up. `Down`/`Up` come from
/// the debug server's `POST /key {"op": "down"}` so a driver can *hold* a key
/// against a panel script, which is the only way `key_down(k)` is observable
/// from a later `GET /state`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyPhase {
    #[default]
    Tap,
    Down,
    Up,
}

/// What a key press did, so callers know whether to exit or redraw.
pub(crate) enum KeyOutcome {
    /// Quit the whole process (Cmd/Ctrl+Q, `:wqa`).
    Quit,
    /// Close this window only — with multiple OS windows the process may
    /// live on (Cmd+W; `:q`/`:wq`/`Ctrl+W q` on the last pane).
    CloseWindow,
    Handled,
    Ignored,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_catalog_round_trips() {
        // Every name the `GET /menu` catalog advertises must parse — this guards
        // MENU_ACTIONS against drifting from the from_debug_request match.
        for (name, arg_kind) in MENU_ACTIONS {
            let arg = arg_kind.map(|k| if k == "scheme" { "dark" } else { "/tmp/x" });
            MenuAction::from_debug_request(name, arg)
                .unwrap_or_else(|e| panic!("catalog action {name:?} failed to parse: {e}"));
        }
    }

    #[test]
    fn menu_action_name_is_case_insensitive() {
        assert_eq!(
            MenuAction::from_debug_request("save", None).unwrap(),
            MenuAction::Save
        );
        assert_eq!(
            MenuAction::from_debug_request("SAVE", None).unwrap(),
            MenuAction::Save
        );
        assert_eq!(
            MenuAction::from_debug_request("GitLog", None).unwrap(),
            MenuAction::GitLog
        );
    }

    #[test]
    fn menu_action_requires_and_resolves_args() {
        assert!(MenuAction::from_debug_request("OpenFile", None).is_err());
        assert_eq!(
            MenuAction::from_debug_request("OpenFile", Some("/a/b")).unwrap(),
            MenuAction::OpenFile("/a/b".into())
        );
        // SetTheme accepts either a scheme key ("light") or its label ("Paper").
        assert_eq!(
            MenuAction::from_debug_request("SetTheme", Some("light")).unwrap(),
            MenuAction::SetTheme(ThemeScheme::Light)
        );
        assert_eq!(
            MenuAction::from_debug_request("SetTheme", Some("Paper")).unwrap(),
            MenuAction::SetTheme(ThemeScheme::Light)
        );
        assert!(MenuAction::from_debug_request("SetTheme", Some("bogus")).is_err());
    }

    #[test]
    fn unknown_menu_action_errs() {
        assert!(MenuAction::from_debug_request("Frobnicate", None).is_err());
    }
}

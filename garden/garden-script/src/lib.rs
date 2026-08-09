//! garden-script — the Petal embedding for Garden.
//!
//! Owns the Petal [`Env`](petal::env::Env), loads the layout script
//! (`init.ptl`), registers Garden's native fns, runs the program, and extracts
//! the declared [`LayoutNode`] tree. The script file is watched by simple
//! mtime + size polling: call [`ScriptHost::poll_reload`] periodically and the
//! host recompiles + hot-reloads on change, preserving Petal `state` variables
//! via `env.transfer_state`.
//!
//! Petal-side API (see `docs/architecture.md`):
//!
//! ```petal
//! layout(
//!     row([
//!         column([ editor("src/main.rs"), editor("notes.md") ], [0.7, 0.3]),
//!         editor("README.md"),
//!     ], [0.6, 0.4])
//! )
//! ```
//!
//! - `editor()` / `editor(path)` / `editor(path, { line_numbers: true, wrap: false })`
//!   → record `{ kind: "editor", file: path|nil, line_numbers: bool, wrap: bool }`.
//!   The optional config record's keys are `line_numbers` (default `false`) and
//!   `wrap` (default `true`).
//! - `process(command)` / `process(command, args)` → record
//!   `{ kind: "process", command: string, args: list|nil }`
//! - `row(children)` / `row(children, ratios)` → record
//!   `{ kind: "row", children: list, ratios: list|nil }`
//! - `column(children)` / `column(children, ratios)` → same with kind `"column"`
//! - `layout(node)` → emits the record tree into a symbol-keyed output buffer
//!   on the [`Env`]; the host drains it after the run and converts it to a
//!   [`LayoutNode`]. This is Petal's canonical "observe what the script called"
//!   mechanism — see `../docs/embedding-guide.md`.
//!
//! ## Layout as live state
//!
//! The layout is *code*, but it is also state the editor mutates at runtime
//! (e.g. expanding a pane to fill the window). Such changes are persisted by
//! rewriting the script's `layout(...)` call in place and saving the result to
//! a **transient overlay** file (set via [`ScriptHost::set_transient_path`];
//! Garden uses `~/.garden/state/window-<id>/window.ptl`) which is normally
//! git-ignored. [`ScriptHost::save_layout`] does this through Petal's
//! goal-based editing (see `../docs/goal-based-editing.md`): the new
//! [`LayoutNode`] is expressed as a structured call tree
//! ([`convert::layout_to_static_value`]) and a single goal — "there is a top-level call
//! `layout(<tree>)`" — updates the existing call in place (every other line of
//! the file is kept verbatim) or appends one. The result is written to the
//! transient file and the host re-points to watch it.
//!
//! Script `print(...)` output is echoed to stdout immediately by the Petal
//! runtime; the same lines (plus garden-script warnings) are also collected per
//! run and retrievable via [`ScriptHost::take_output`].

mod convert;
pub mod inspect;
mod native_fns;
mod panel;
mod panel_trace;
mod query;
mod theme;

pub use panel::{
    buttons, DataProvider, DecorSpec, InputEvent, Modifiers, NavIntent, PanelCmd, PanelData,
    PanelHost, PanelInput, PanelTheme, ProjectionSpec, KEY_NAMES,
};
pub use panel_trace::{
    drag_handle, hit_test, ArgSource, CallRef, CodeSpan, DragHandle, DragOutcome, DrawOrigin,
    DrawTrace, SourceRewrite, TracedArg,
};
pub use query::{HostData, QueryProvider, QueryState};
pub use theme::Theme;

/// The goal-based source-editing vocabulary, re-exported for callers that
/// persist permanent settings through [`ScriptHost::save_setting`] (so
/// `garden-app` need not depend on `petal` directly). See
/// `../docs/goal-based-editing.md`.
pub use petal::goal_based_editing::{Goal, StaticValue};

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::Value;

/// A node in the declarative pane layout produced by the Petal script.
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutNode {
    /// Children side by side, left→right. `ratios` sums to ~1.0 (None = equal).
    Row {
        children: Vec<LayoutNode>,
        ratios: Option<Vec<f32>>,
    },
    /// Children stacked top→bottom.
    Column {
        children: Vec<LayoutNode>,
        ratios: Option<Vec<f32>>,
    },
    /// A text-editor pane, optionally pre-loaded with a file. `line_numbers`
    /// (default `false`) toggles the line-number gutter; `wrap` (default `true`)
    /// controls soft line wrapping. Both are per-pane config options that
    /// round-trip through the layout script's `editor(path, { ... })` config so
    /// `:set nowrap` / line-number toggles survive a restart.
    Editor {
        file: Option<String>,
        line_numbers: bool,
        wrap: bool,
    },
    /// A pane backed by a subprocess speaking GPP (Garden Pane Protocol)
    /// over stdio. `command` is the executable; `args` are passed to it.
    Process { command: String, args: Vec<String> },
    /// A pane whose pixels are drawn by a Petal script run every frame.
    /// `script` is the path to that script. See `docs/petal-graphical-panels.md`
    /// and [`crate::panel`].
    ///
    /// `screens` is an **optional** explicit navigation allowlist for the
    /// browser-style history API (`navigate(...)`): when non-empty it *narrows*
    /// the default implicit allowlist (any `.ptl` in the panel's own script
    /// directory) to exactly the listed screen names — an off-list target is
    /// refused even if it sits in the directory. An **empty** vec means the
    /// screens list was not declared, so the implicit script-directory default
    /// applies unchanged. Declaring a list never *widens* the default: a listed
    /// entry still must pass the same traversal / `.ptl` / existence checks.
    Panel {
        script: String,
        screens: Vec<String>,
    },
}

/// File identity used for change detection: (modified time, size in bytes).
type FileSig = (SystemTime, u64);

/// Hosts the Petal environment for one layout script.
///
/// Created with [`ScriptHost::load`]; the current layout is always available
/// via [`ScriptHost::layout`] (on reload errors the previous good layout is
/// kept). The observable-call state lives on the owned [`Env`] (in symbol-keyed
/// output buffers), not in any global, so a host carries all its own state.
pub struct ScriptHost {
    env: Env,
    program_id: ProgramId,
    stack_id: StackKey,
    path: PathBuf,
    /// The permanent config file the host was loaded from (e.g.
    /// `~/.garden/init.ptl`). Unlike [`path`](Self::path) — which
    /// [`save_layout`](Self::save_layout) re-points to the transient overlay —
    /// this always names the user's hand-edited source, so permanent settings
    /// ([`save_setting`](Self::save_setting)) land where they survive a restart.
    config_path: PathBuf,
    layout: LayoutNode,
    /// Theme captured from the last successful run (default = no overrides).
    theme: Theme,
    /// Base color-scheme name captured from the last run's `color_scheme(...)`
    /// call, if any. The application maps this onto its built-in palette.
    scheme: Option<String>,
    /// Bumped whenever `theme` changes across a reload, so the application can
    /// restyle on a colors-only edit (one that leaves the layout identical).
    theme_rev: u64,
    /// Last observed (mtime, size); `None` if the file could not be stat'ed.
    /// Updated even when a reload attempt fails, so unchanged broken content
    /// is not re-reported on every poll.
    last_sig: Option<FileSig>,
    /// Explicit file the transient overlay is written to. `None` falls back to
    /// a sibling of the base script (see [`transient_path`]); Garden sets this
    /// to the window's overlay (`~/.garden/state/window-<id>/window.ptl`) so
    /// runtime layout changes live with the user's other local state instead of
    /// polluting the base script's folder.
    transient_target: Option<PathBuf>,
    output: Vec<String>,
}

impl std::fmt::Debug for ScriptHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptHost")
            .field("path", &self.path)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl ScriptHost {
    /// Compile + run the script at `path`; returns an error string on read,
    /// compile, or runtime failure (including layout validation errors). A
    /// script that never calls `layout(...)` is not an error — it falls back to
    /// a default empty layout (see [`run_and_extract`](Self::run_and_extract)).
    pub fn load(path: &Path) -> Result<ScriptHost, String> {
        let source = fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;

        let mut env = Env::new();
        native_fns::register_all(&mut env);

        let program_id = env.load_program(&source)?;
        let stack_id = env.create_stack(program_id)?;

        let mut host = ScriptHost {
            env,
            program_id,
            stack_id,
            path: path.to_path_buf(),
            config_path: path.to_path_buf(),
            layout: LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
            theme: Theme::default(),
            scheme: None,
            theme_rev: 0,
            last_sig: stat_sig(path),
            transient_target: None,
            output: Vec::new(),
        };

        host.layout = host.run_and_extract()?;
        Ok(host)
    }

    /// The current layout tree (the last one that loaded successfully).
    pub fn layout(&self) -> &LayoutNode {
        &self.layout
    }

    /// The theme overrides captured from the last successful run. Empty when
    /// the script never called `color_theme`; the application layer overlays
    /// these onto its built-in defaults.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The base color-scheme name from the last run's `color_scheme(...)` call,
    /// or `None` if the script selected none. The application maps this onto its
    /// built-in `ThemeScheme` at startup so `init.ptl` controls the initial
    /// palette; a menu change is persisted back with [`save_setting`].
    pub fn scheme(&self) -> Option<&str> {
        self.scheme.as_deref()
    }

    /// A counter that increments each time the captured theme changes across a
    /// reload. The application watches it to restyle live even when a script
    /// edit changes only colors and leaves the layout the same.
    pub fn theme_rev(&self) -> u64 {
        self.theme_rev
    }

    /// Path of the watched script file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Set the exact file [`save_layout`](Self::save_layout) writes the
    /// transient overlay to (its parent directory is created on demand). Garden
    /// points this at the per-window overlay,
    /// `~/.garden/state/window-<id>/window.ptl`. Without it the overlay is a
    /// sibling of the base script (see [`transient_path`]).
    pub fn set_transient_path(&mut self, path: PathBuf) {
        self.transient_target = Some(path);
    }

    /// The explicit transient-overlay target, if one was set via
    /// [`set_transient_path`](Self::set_transient_path). Garden uses its parent
    /// (the per-window state directory) to place other generated scripts — e.g.
    /// the `:Diff` viewer's panel — alongside the layout overlay.
    pub fn transient_path(&self) -> Option<&Path> {
        self.transient_target.as_deref()
    }

    /// Drain the collected script output (Petal `print` lines and garden-script
    /// warnings) accumulated since the last call. The Petal runtime already
    /// echoes these lines to stdout as they happen.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }

    /// Poll the script file for changes (mtime + size). On change: recompile,
    /// hot-reload (preserving Petal `state` vars), re-run, and re-extract the
    /// layout.
    ///
    /// Returns `Ok(true)` if the layout changed, `Ok(false)` if the file is
    /// unchanged or the new layout is identical. On any error the previous
    /// layout is kept and `Err(msg)` is returned once; subsequent polls stay
    /// quiet until the file changes again.
    pub fn poll_reload(&mut self) -> Result<bool, String> {
        let sig = match stat_sig(&self.path) {
            Some(sig) => sig,
            None => {
                if self.last_sig.take().is_some() {
                    return Err(format!("cannot stat {}", self.path.display()));
                }
                return Ok(false);
            }
        };
        if self.last_sig == Some(sig) {
            return Ok(false);
        }
        // Record the new signature before attempting the reload so that a
        // broken file is only reported once.
        self.last_sig = Some(sig);

        let source = fs::read_to_string(&self.path)
            .map_err(|e| format!("failed to read {}: {}", self.path.display(), e))?;

        // Compile first: a compile error must not disturb the running program.
        let new_program = self.env.compile_program(self.program_id, &source)?;
        self.env.transfer_state(self.stack_id, new_program)?;

        let new_layout = self.run_and_extract()?;
        let changed = new_layout != self.layout;
        self.layout = new_layout;
        Ok(changed)
    }

    /// Persist a runtime layout change to the transient sibling script.
    ///
    /// The current script source is read and a goal-based edit (see
    /// [`save_setting`](Self::save_setting)) replaces its `layout(...)` call in
    /// place with `node` expressed as a call tree — every other fragment of the
    /// file (comments, `color_theme`, helper code) is preserved byte-for-byte;
    /// a script with no `layout(...)` call gets one appended. The result is
    /// written to the file set by
    /// [`set_transient_path`](Self::set_transient_path) (Garden uses the
    /// window's `~/.garden/state/window-<id>/window.ptl`), or beside the base
    /// script when none is set (see [`transient_path`]). The host then watches
    /// and hot-reloads from it, so subsequent edits and further runtime changes
    /// flow through the same overlay file.
    ///
    /// Returns the path of the transient file written. On any failure the host
    /// is left untouched (the old layout and watched path stay in place).
    pub fn save_layout(&mut self, node: &LayoutNode) -> Result<PathBuf, String> {
        let source = fs::read_to_string(&self.path)
            .map_err(|e| format!("failed to read {}: {}", self.path.display(), e))?;

        let goal = Goal::should_call("layout", [convert::layout_to_static_value(node)]);
        let new_source = petal::goal_based_editing::modify_source_with_goals(&source, &[goal])
            .map_err(|e| e.to_string())?;

        let target = self
            .transient_target
            .clone()
            .unwrap_or_else(|| transient_path(&self.path));
        if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
        }
        fs::write(&target, &new_source)
            .map_err(|e| format!("failed to write {}: {}", target.display(), e))?;

        // Reload from the rewritten source so the Env, captured layout, and theme
        // all reflect the change immediately (and the generated source is
        // validated by actually running it). Watch the transient file from now
        // on so the next poll sees no spurious change.
        let new_program = self.env.compile_program(self.program_id, &new_source)?;
        self.env.transfer_state(self.stack_id, new_program)?;
        self.layout = self.run_and_extract()?;

        self.path = target.clone();
        self.last_sig = stat_sig(&target);
        Ok(target)
    }

    /// Persist a **permanent** settings change to the base config file
    /// ([`config_path`](Self::config_path)) via goal-based editing.
    ///
    /// This is the standard path for durable, hand-editable settings — a color
    /// scheme, a font size — as opposed to [`save_layout`](Self::save_layout),
    /// which writes ephemeral per-window layout to the transient overlay. Each
    /// [`Goal`] declares an outcome ("there is a call `color_scheme("light")`");
    /// the goal engine updates an existing top-level call in place or appends a
    /// new one, preserving every surrounding comment and the layout call. See
    /// `../docs/goal-based-editing.md`.
    ///
    /// The write targets the user's original `init.ptl`, not the overlay, so the
    /// change is read back on the next launch (startup always loads the base
    /// config). When the base file is still the watched file (no runtime layout
    /// change has re-pointed the host to the overlay), the change signature is
    /// refreshed so this write does not trigger a redundant hot-reload — the
    /// caller is expected to have already applied the setting in memory.
    ///
    /// Returns the path written on success; the host is left untouched on error.
    pub fn save_setting(&mut self, goals: &[Goal]) -> Result<PathBuf, String> {
        let source = fs::read_to_string(&self.config_path)
            .map_err(|e| format!("failed to read {}: {}", self.config_path.display(), e))?;
        let new_source = petal::goal_based_editing::modify_source_with_goals(&source, goals)
            .map_err(|e| e.to_string())?;
        fs::write(&self.config_path, &new_source)
            .map_err(|e| format!("failed to write {}: {}", self.config_path.display(), e))?;
        if self.config_path == self.path {
            self.last_sig = stat_sig(&self.path);
        }
        Ok(self.config_path.clone())
    }

    /// The permanent config file this host was loaded from (the user's
    /// `init.ptl`). [`save_setting`](Self::save_setting) writes here; it is not
    /// affected by [`save_layout`](Self::save_layout) re-pointing the watched
    /// [`path`](Self::path) to the transient overlay.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Run the (already loaded / hot-reloaded) program and read back what the
    /// script declared. The `layout`/`color_theme`/`color_scheme` native fns
    /// emit their arguments into symbol-keyed output buffers on the [`Env`]
    /// (see [`native_fns`]); this drains those buffers and interprets them —
    /// converting the layout record tree to a [`LayoutNode`], parsing the theme
    /// colors, and reading the scheme name. Petal output is drained into
    /// `self.output`.
    ///
    /// The theme is independent of the layout: a script need not call
    /// `color_theme`, in which case `self.theme` becomes the empty default and
    /// the application keeps its built-in colors. Only a successful run updates
    /// `self.theme`; on error it is left untouched (with the previous layout).
    ///
    /// A script that never calls `layout(...)` yields a default empty layout (a
    /// single blank editor pane) rather than an error.
    fn run_and_extract(&mut self) -> Result<LayoutNode, String> {
        // Intern the buffer symbols (idempotent — same ids the native fns use).
        let layout_sym = self.env.intern_symbol(native_fns::LAYOUT_SYM);
        let theme_sym = self.env.intern_symbol(native_fns::THEME_SYM);
        let scheme_sym = self.env.intern_symbol(native_fns::SCHEME_SYM);

        // Clear any values left by an earlier run (e.g. one that errored after
        // emitting) so a run that doesn't call `layout(...)` reads as empty.
        self.env.clear_output_buffer(layout_sym);
        self.env.clear_output_buffer(theme_sym);
        self.env.clear_output_buffer(scheme_sym);

        let run_result = self.env.run(self.stack_id);
        self.output.append(&mut self.env.take_output());
        run_result?;

        let mut warnings = Vec::new();

        // Layout: last `layout(...)` call wins. A script that never called it is
        // not an error — fall back to a default empty layout (a single blank
        // editor pane) so an `init.ptl` that only sets a theme, or is empty,
        // still loads. The drained values reference the Env heap, so decode
        // against `self.env.heap()` before the next run mutates it.
        let layout = match self.env.take_output_buffer(layout_sym).last().copied() {
            Some(value) => convert::convert_layout(value, self.env.heap(), &mut warnings)
                .map_err(|e| format!("layout: {e}"))?,
            None => LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            },
        };

        // Color scheme: last emitted name wins; a non-string is an error.
        self.scheme = match self.env.take_output_buffer(scheme_sym).last().copied() {
            Some(Value::String(id)) => Some(self.env.heap().get_string(id).to_string()),
            Some(other) => {
                return Err(format!(
                    "color_scheme expects a scheme name string, got {}",
                    other.type_name()
                ))
            }
            None => None,
        };

        // Theme: last emitted record wins; malformed colors degrade to warnings.
        let new_theme = match self.env.take_output_buffer(theme_sym).last().copied() {
            Some(value) => convert::convert_theme(value, self.env.heap(), &mut warnings)?,
            None => Theme::default(),
        };
        if new_theme != self.theme {
            self.theme = new_theme;
            // Bump the revision so the application restyles even when the
            // layout is unchanged (a colors-only hot edit).
            self.theme_rev = self.theme_rev.wrapping_add(1);
        }

        for warning in warnings {
            self.output
                .push(format!("[garden-script] warning: {warning}"));
        }
        Ok(layout)
    }
}

/// Best-effort (mtime, size) signature for change detection.
fn stat_sig(path: &Path) -> Option<FileSig> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

/// The fallback transient-overlay sibling of a base script path: the same
/// directory and stem with a `.transient.ptl` extension (`init.ptl` →
/// `init.transient.ptl`); a path that is already a `.transient.ptl` maps to
/// itself, so repeated saves stay on the one overlay file.
///
/// Only used when no explicit overlay was set via
/// [`ScriptHost::set_transient_path`] — Garden always sets one
/// (`~/.garden/state/window-<id>/window.ptl`), so this covers tests and plain
/// non-app usage.
pub fn transient_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("init.ptl");
    if name.ends_with(".transient.ptl") {
        return path.to_path_buf();
    }
    let stem = name.strip_suffix(".ptl").unwrap_or(name);
    path.with_file_name(format!("{stem}.transient.ptl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_sibling_of_init() {
        assert_eq!(
            transient_path(Path::new("/home/u/.garden/init.ptl")),
            PathBuf::from("/home/u/.garden/init.transient.ptl")
        );
    }

    #[test]
    fn transient_of_transient_is_itself() {
        let p = Path::new("/x/init.transient.ptl");
        assert_eq!(transient_path(p), p.to_path_buf());
    }

    #[test]
    fn transient_of_arbitrary_stem() {
        assert_eq!(
            transient_path(Path::new("work.ptl")),
            PathBuf::from("work.transient.ptl")
        );
    }

    #[test]
    fn save_layout_preserves_other_fragments() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(
            &base,
            "// my config\ncolor_theme({ text: \"#fff\" })\n\nlayout(editor())\n\n// end\n",
        )
        .unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        let written = host
            .save_layout(&LayoutNode::Editor {
                file: Some("x.rs".into()),
                line_numbers: false,
                wrap: true,
            })
            .unwrap();
        assert_eq!(
            fs::read_to_string(&written).unwrap(),
            "// my config\ncolor_theme({ text: \"#fff\" })\n\nlayout(editor(\"x.rs\"))\n\n// end\n"
        );
    }

    #[test]
    fn save_layout_appends_when_no_layout_call() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "x = 1\n").unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        let written = host
            .save_layout(&LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            })
            .unwrap();
        assert_eq!(
            fs::read_to_string(&written).unwrap(),
            "x = 1\n\nlayout(editor())\n"
        );
    }

    /// A nested layout round-trips through save_layout and the normal load
    /// path, and the generated source is pretty-printed one child per line.
    #[test]
    fn save_layout_round_trips_nested_layout() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        let node = LayoutNode::Column {
            children: vec![
                LayoutNode::Row {
                    children: vec![
                        LayoutNode::Editor {
                            file: Some("x.rs".into()),
                            line_numbers: true,
                            wrap: true,
                        },
                        LayoutNode::Process {
                            command: "directory-browser".into(),
                            args: vec!["src".into()],
                        },
                        LayoutNode::Panel {
                            script: "examples/panels/sketch.ptl".into(),
                            screens: Vec::new(),
                        },
                    ],
                    ratios: Some(vec![0.5, 0.25, 0.25]),
                },
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: true,
                },
            ],
            ratios: Some(vec![0.8, 0.2]),
        };

        let mut host = ScriptHost::load(&base).unwrap();
        let written = host.save_layout(&node).unwrap();
        // save_layout re-runs the generated source, so the captured layout
        // proving the round-trip already went through the real load path.
        assert_eq!(host.layout(), &node);

        let text = fs::read_to_string(&written).unwrap();
        let expected = "\
layout(column([
    row([
      editor(\"x.rs\", { line_numbers: true }),
      process(\"directory-browser\", [\"src\"]),
      panel(\"examples/panels/sketch.ptl\"),
    ], [0.5, 0.25, 0.25]),
    editor(),
  ], [0.8, 0.2]))\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn save_layout_round_trips_wrap_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        // wrap off (non-default) emits a config record; line_numbers stays off so
        // `wrap` is the only key. A file-less pane still gets a positional `nil`
        // before the config, like line_numbers does.
        let node = LayoutNode::Row {
            children: vec![
                LayoutNode::Editor {
                    file: Some("x.rs".into()),
                    line_numbers: false,
                    wrap: false,
                },
                LayoutNode::Editor {
                    file: None,
                    line_numbers: false,
                    wrap: false,
                },
            ],
            ratios: None,
        };

        let mut host = ScriptHost::load(&base).unwrap();
        let written = host.save_layout(&node).unwrap();
        assert_eq!(host.layout(), &node); // full load path re-derived the node

        let text = fs::read_to_string(&written).unwrap();
        let expected = "\
layout(row([
    editor(\"x.rs\", { wrap: false }),
    editor(nil, { wrap: false }),
  ]))\n";
        assert_eq!(text, expected);
    }

    /// A `panel(script, { screens: [...] })` node threads its explicit
    /// allowlist through the full load path, and `save_layout` round-trips it as
    /// a config record (an empty list stays absent — just `panel(script)`).
    #[test]
    fn panel_screens_round_trip_through_load_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(
            &base,
            "layout(panel(\"sketch.ptl\", { screens: [\"a.ptl\", \"b.ptl\"] }))\n",
        )
        .unwrap();

        let host = ScriptHost::load(&base).unwrap();
        assert_eq!(
            host.layout(),
            &LayoutNode::Panel {
                script: "sketch.ptl".into(),
                screens: vec!["a.ptl".into(), "b.ptl".into()],
            }
        );

        // Re-save the same node and confirm the serialized form preserves the
        // screens allowlist and re-loads identically.
        let mut host = host;
        let node = LayoutNode::Panel {
            script: "sketch.ptl".into(),
            screens: vec!["a.ptl".into(), "b.ptl".into()],
        };
        let written = host.save_layout(&node).unwrap();
        assert_eq!(host.layout(), &node);
        let text = fs::read_to_string(&written).unwrap();
        assert_eq!(
            text,
            "layout(panel(\"sketch.ptl\", { screens: [\"a.ptl\", \"b.ptl\"] }))\n"
        );
    }

    /// A panel with no declared screens serializes without a config record, so a
    /// plain `panel(script)` is unchanged by the round trip.
    #[test]
    fn panel_without_screens_serializes_bare() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        let node = LayoutNode::Panel {
            script: "sketch.ptl".into(),
            screens: Vec::new(),
        };
        let written = host.save_layout(&node).unwrap();
        assert_eq!(host.layout(), &node);
        let text = fs::read_to_string(&written).unwrap();
        assert_eq!(text, "layout(panel(\"sketch.ptl\"))\n");
    }

    /// Layout paths with quotes/backslashes are escaped by the goal engine and
    /// survive the save → reload round trip.
    #[test]
    fn save_layout_escapes_special_characters() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        let node = LayoutNode::Editor {
            file: Some("a\"b\\c".into()),
            line_numbers: false,
            wrap: true,
        };
        let mut host = ScriptHost::load(&base).unwrap();
        host.save_layout(&node).unwrap();
        assert_eq!(host.layout(), &node);
    }

    #[test]
    fn script_without_layout_call_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        // A script that never calls `layout(...)` — only a comment and a var.
        fs::write(&base, "// no layout here\nx = 1\n").unwrap();

        let host = ScriptHost::load(&base).unwrap();
        assert_eq!(
            host.layout(),
            &LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            }
        );
    }

    #[test]
    fn save_layout_writes_transient_and_repoints() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        {
            let mut f = fs::File::create(&base).unwrap();
            write!(
                f,
                "// keep this comment\nlayout(column([editor(\"a\"), editor(\"b\")], [0.5, 0.5]))\n"
            )
            .unwrap();
        }

        let mut host = ScriptHost::load(&base).unwrap();
        let only = LayoutNode::Editor {
            file: Some("a".into()),
            line_numbers: false,
            wrap: true,
        };
        let written = host.save_layout(&only).unwrap();

        // The transient sibling was written and is now the watched path.
        assert_eq!(written, base.with_file_name("init.transient.ptl"));
        assert_eq!(host.path(), written.as_path());
        assert_eq!(host.layout(), &only);

        // The base file is untouched; the comment survives into the transient.
        let base_text = fs::read_to_string(&base).unwrap();
        assert!(
            base_text.contains("column("),
            "base should be unchanged: {base_text}"
        );
        let trans_text = fs::read_to_string(&written).unwrap();
        assert!(
            trans_text.contains("// keep this comment"),
            "got: {trans_text}"
        );
        assert!(
            trans_text.contains("layout(editor(\"a\"))"),
            "got: {trans_text}"
        );

        // A second save stays on the same transient file.
        let again = host
            .save_layout(&LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            })
            .unwrap();
        assert_eq!(again, written);
    }

    #[test]
    fn save_layout_honors_transient_path_override() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor(\"a\"))\n").unwrap();

        // Point the overlay at a path in a not-yet-existing dir — save creates
        // the parent directory on demand, mirroring the per-window overlay.
        let overlay = dir.path().join("state").join("window-3").join("window.ptl");
        let mut host = ScriptHost::load(&base).unwrap();
        host.set_transient_path(overlay.clone());

        let written = host
            .save_layout(&LayoutNode::Editor {
                file: Some("b".into()),
                line_numbers: false,
                wrap: true,
            })
            .unwrap();
        assert_eq!(written, overlay);
        assert!(written.exists(), "overlay written at the explicit path");
        assert_eq!(host.path(), written.as_path());

        // A second save stays on the same overlay file.
        let again = host
            .save_layout(&LayoutNode::Editor {
                file: None,
                line_numbers: false,
                wrap: true,
            })
            .unwrap();
        assert_eq!(again, written);
    }

    #[test]
    fn captures_color_scheme_name() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "color_scheme(\"light\")\nlayout(editor())\n").unwrap();

        let host = ScriptHost::load(&base).unwrap();
        assert_eq!(host.scheme(), Some("light"));
    }

    #[test]
    fn no_color_scheme_call_leaves_scheme_unset() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        let host = ScriptHost::load(&base).unwrap();
        assert_eq!(host.scheme(), None);
    }

    #[test]
    fn save_setting_updates_existing_call_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(
            &base,
            "// my config\ncolor_scheme(\"dark\")\nlayout(editor(\"a\"))\n",
        )
        .unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        let written = host
            .save_setting(&[Goal::should_call("color_scheme", ["light"])])
            .unwrap();

        // Written to the permanent base config, not a transient sibling.
        assert_eq!(written, base);
        let text = fs::read_to_string(&base).unwrap();
        assert!(text.contains("color_scheme(\"light\")"), "got: {text}");
        assert!(!text.contains("\"dark\""), "old value replaced: {text}");
        // The comment and layout call are preserved untouched.
        assert!(text.contains("// my config"), "got: {text}");
        assert!(text.contains("layout(editor(\"a\"))"), "got: {text}");
    }

    #[test]
    fn save_setting_appends_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "layout(editor())\n").unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        host.save_setting(&[Goal::should_call("color_scheme", ["brown"])])
            .unwrap();

        let text = fs::read_to_string(&base).unwrap();
        assert!(text.contains("color_scheme(\"brown\")"), "got: {text}");
        assert!(text.contains("layout(editor())"), "layout kept: {text}");
    }

    #[test]
    fn save_setting_targets_base_config_after_a_layout_save() {
        // A runtime layout change re-points the watched path to the transient
        // overlay; a later permanent setting must still land in the base config.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("init.ptl");
        fs::write(&base, "color_scheme(\"dark\")\nlayout(editor(\"a\"))\n").unwrap();

        let mut host = ScriptHost::load(&base).unwrap();
        let overlay = dir.path().join("window.ptl");
        host.set_transient_path(overlay.clone());
        host.save_layout(&LayoutNode::Editor {
            file: Some("b".into()),
            line_numbers: false,
            wrap: true,
        })
        .unwrap();
        assert_eq!(host.path(), overlay.as_path());

        let written = host
            .save_setting(&[Goal::should_call("color_scheme", ["light"])])
            .unwrap();

        assert_eq!(written, base, "setting persisted to the base config");
        assert_eq!(host.config_path(), base.as_path());
        let base_text = fs::read_to_string(&base).unwrap();
        assert!(
            base_text.contains("color_scheme(\"light\")"),
            "got: {base_text}"
        );
    }
}

//! Building and maintaining the pane set: solving the layout tree into pane
//! rects, reusing live editor/process state across a rebuild, and polling the
//! layout script and on-disk files for external changes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use garden_core::Point;
use garden_render::Rect;
use garden_script::{DrawTrace, PanelHost};

use crate::editor_view::EditorView;
use crate::layout::{self, PaneContent, PaneSlot};
use crate::panel_view::PanelView;
use crate::process_pane::ProcessPane;

use super::{build_theme, App, Pane, TraceDetail};

impl App {
    /// Rebuild panes from the layout tree, reusing existing state across a
    /// reload: an editor pane's view (buffer, cursor, scroll) when its file
    /// matches, and a process pane's live subprocess when its command+args
    /// match (so a reposition/reload never respawns the child).
    pub(in crate::app) fn rebuild_panes(&mut self) {
        let cell = self.viewport.cell;
        let slots = layout::solve(self.layout(), self.pane_area());
        // Panel script paths are resolved relative to the layout script's
        // directory, so `panel("clock.ptl")` sits next to `init.ptl`.
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let mut old: Vec<Pane> = self.panes.drain(..).collect();

        // Build each new pane, collecting any process panes that spawned during
        // the rebuild so their initial render can be drained afterwards (the
        // borrow on `self` must end first).
        let mut new_panes = Vec::with_capacity(slots.len());
        let mut spawned: Vec<usize> = Vec::new();
        // Panel-mode (script-push) clients spawned this rebuild, primed after the
        // borrow on `self` ends (like `spawned`, but their first data comes from
        // the query round-trip rather than an initial `render`).
        let mut spawned_clients: Vec<usize> = Vec::new();
        // Panel scripts that failed to compile/load this rebuild, reported once
        // the borrow on `self` ends.
        let mut panel_errors: Vec<String> = Vec::new();
        for (idx, PaneSlot { rect, content }) in slots.into_iter().enumerate() {
            let pane = match content {
                PaneContent::Editor {
                    file,
                    line_numbers,
                    wrap,
                } => {
                    let reused = old
                        .iter()
                        .position(|p| !p.is_process() && p.file == file)
                        .map(|i| old.remove(i).view);
                    let mut view = reused.unwrap_or_else(|| EditorView::open(file.as_deref()));
                    // Apply the (possibly reload-updated) per-pane config.
                    view.show_line_numbers = line_numbers;
                    view.wrap = wrap;
                    Pane::editor(rect, file, view)
                }
                PaneContent::Process { command, args } => {
                    // Move a matching live process across untouched.
                    let reused = old
                        .iter()
                        .position(|p| process_matches(p, &command, &args))
                        .map(|i| old.remove(i));
                    if let Some(mut pane) = reused {
                        pane.rect = rect;
                        pane
                    } else {
                        let mut view = EditorView::open(None);
                        let (rows, cols) = process_dims(&view, rect, cell);
                        match ProcessPane::spawn(&command, &args, idx as u64, rows, cols) {
                            Ok(process) => match process.mode() {
                                // A panel-mode client pushes a Petal UI script the
                                // host runs; a Lines client pushes text `render`s.
                                gpp::ClientMode::Panel => {
                                    spawned_clients.push(idx);
                                    build_script_client_pane(rect, view, process, command, args)
                                }
                                gpp::ClientMode::Lines => {
                                    spawned.push(idx);
                                    Pane::process(rect, view, process, command, args)
                                }
                            },
                            Err(err) => {
                                // Never crash: show the error in a plain editor.
                                let mut msg = format!("could not start {command}: {err}");
                                if err.kind() == std::io::ErrorKind::NotFound {
                                    msg.push_str(
                                        "\n\nThe pane's program was not found. If this is the \
                                         directory browser, build the whole workspace with \
                                         `cargo build` so `directory-browser` is placed next to \
                                         `garden`, or set GARDEN_DIRECTORY_BROWSER_BIN to its path.",
                                    );
                                }
                                view.set_external_content(&msg, None);
                                Pane::editor(rect, None, view)
                            }
                        }
                    }
                }
                PaneContent::Panel { script, screens } => {
                    // Move a matching live panel across untouched (keeps it
                    // animating and preserves its Petal `state` — and its whole
                    // navigation history). Match on the panel's ORIGIN screen, not
                    // its live `script()`: after a `navigate(...)` the live screen
                    // diverges from the layout-declared node, and keying on it here
                    // would drop the navigated panel and lose its history stack.
                    let reused = old
                        .iter()
                        .position(|p| {
                            p.panel
                                .as_ref()
                                .is_some_and(|pv| pv.origin_script() == script)
                        })
                        .map(|i| old.remove(i));
                    if let Some(mut pane) = reused {
                        pane.rect = rect;
                        // The layout may have re-declared the allowlist (a live
                        // edit to `screens: [...]`), so refresh it on reuse.
                        if let Some(pv) = pane.panel.as_mut() {
                            pv.set_screens(screens.clone());
                        }
                        pane
                    } else {
                        // A `panel(...)` layout node names a Petal script on disk,
                        // loaded — and hot-reloaded — from there. (The built-in
                        // `:Git` / `:Diff` viewers are no longer panels: they are
                        // subprocess-backed script clients, persisted as
                        // `process(...)` nodes.)
                        let path = resolve_script(&script, script_dir.as_deref());
                        match PanelHost::load(&path) {
                            Ok(host) => {
                                let mut view = PanelView::new(host, script.clone(), Instant::now());
                                view.set_screens(screens.clone());
                                Pane::panel(rect, EditorView::open(None), view)
                            }
                            Err(err) => {
                                // Never crash: show the error in a plain editor.
                                // Also report it everywhere a failure is looked
                                // for — the pane title, the status bar, and the
                                // launch log — because the fallback pane is
                                // otherwise indistinguishable from an empty
                                // buffer, and a headless/scripted client reading
                                // `status_error` would see a clean start.
                                let msg = format!("could not load panel {script}: {err}");
                                let mut view = EditorView::open(None);
                                view.set_external_content(&msg, None);
                                view.set_external_title(Some(format!("panel error: {script}")));
                                panel_errors.push(msg);
                                Pane::editor(rect, None, view)
                            }
                        }
                    }
                }
            };
            new_panes.push(pane);
        }

        self.panes = new_panes;
        // A panel that doesn't compile is the single most common authoring
        // error; make it loud rather than leaving a blank-looking pane.
        if let Some(first) = panel_errors.first() {
            for msg in &panel_errors {
                eprintln!("garden: {msg}");
            }
            self.status_error = Some(first.clone());
        }
        self.focus = self.focus.min(self.panes.len().saturating_sub(1));
        self.drag = None;
        self.window_cmd_pending = false;
        self.recompute_dividers();
        // Petal-IDE: (re)attach the IR data provider to any inspector panel and
        // refresh the toolbar's IR-open state.
        self.attach_ir_providers();

        // Drain each freshly-spawned process's initial messages so its first
        // listing shows immediately.
        for idx in spawned {
            if let Some(process) = self.panes.get(idx).and_then(|p| p.process.as_ref()) {
                let msgs = process.drain_for(Duration::from_millis(250));
                self.apply_process_messages(idx, msgs);
            }
        }
        // Prime each freshly-spawned script client so its first frame has data.
        for idx in spawned_clients {
            self.prime_script_client(idx);
        }
    }

    /// Re-solve pane rects without touching buffers (e.g. after a resize).
    /// Process-backed panes are told their new size so the subprocess can
    /// re-render at the right dimensions.
    pub(in crate::app) fn reposition_panes(&mut self) {
        let cell = self.viewport.cell;
        let slots = layout::solve(self.layout(), self.pane_area());
        if slots.len() == self.panes.len() {
            for (pane, slot) in self.panes.iter_mut().zip(slots) {
                pane.rect = slot.rect;
                let (rows, cols) = process_dims(&pane.view, pane.rect, cell);
                if let Some(process) = pane.process.as_mut() {
                    process.send_resize(rows, cols);
                }
            }
            self.recompute_dividers();
        } else {
            self.rebuild_panes();
        }
    }

    /// Recompute the draggable split dividers from the current layout + area.
    pub(in crate::app) fn recompute_dividers(&mut self) {
        self.dividers = layout::solve_dividers(self.layout(), self.pane_area());
    }

    /// Poll the layout script for changes (no-op without a script). Sets the
    /// redraw flag when the layout, colors, or the error state changed.
    pub fn poll_script(&mut self) {
        let Some(script) = self.script.as_mut() else {
            return;
        };
        let outcome = script.poll_reload();
        // The theme can change on any reload, including one that keeps the
        // layout identical (a colors-only edit), so check it independently.
        let theme_rev = script.theme_rev();
        // A config-only script (file argument) is still watched so live edits to
        // init.ptl colors apply, but its `layout(...)` must not replace the file
        // panes — the theme block below runs regardless.
        let owns_layout = self.script_owns_layout;
        match outcome {
            // Any successful reload clears the standing script error — even a
            // config-only script's (whose layout(...) is ignored below).
            Ok(true) => {
                self.script_error = None;
                if owns_layout {
                    self.log_event("script", "layout reloaded");
                    self.rebuild_panes();
                }
                self.needs_redraw = true;
            }
            Ok(false) => {}
            Err(err) => {
                self.log_event("script", format!("reload error: {err}"));
                self.script_error = Some(err);
                self.needs_redraw = true;
            }
        }
        if theme_rev != self.last_theme_rev {
            self.last_theme_rev = theme_rev;
            self.theme = build_theme(&self.script, self.theme_scheme);
            self.needs_redraw = true;
        }
    }

    /// Poll each open file for external modification (called on the same
    /// reload tick as [`poll_script`](App::poll_script), in every frontend).
    /// A clean buffer is silently reloaded from disk; a dirty buffer is kept
    /// and a one-time warning is surfaced in the status bar — re-armed only
    /// when the file changes again, or resolved by reloading once the buffer
    /// becomes clean (e.g. the user undoes or saves).
    pub fn poll_files(&mut self) {
        for i in 0..self.panes.len() {
            let pane = &mut self.panes[i];
            // Process and panel panes have no file buffer to stamp.
            if pane.is_process() || pane.is_panel() {
                continue;
            }
            let Some(stamp) = pane.view.buffer.disk_changed() else {
                pane.external_conflict = None;
                continue;
            };
            let note = if pane.view.buffer.is_dirty() {
                // Don't clobber unsaved edits; warn once per disk version.
                if pane.external_conflict == Some(stamp) {
                    continue;
                }
                pane.external_conflict = Some(stamp);
                format!(
                    "{} changed on disk (unsaved changes kept)",
                    pane.view.display_name()
                )
            } else if pane.view.reload().is_ok() {
                pane.external_conflict = None;
                format!("{} reloaded from disk", pane.view.display_name())
            } else {
                continue;
            };
            self.log_event("file", note.clone());
            self.status_note = Some(note);
            self.needs_redraw = true;
        }
    }
}

/// A process pane's size in (rows, cols) for `rect` at `cell` metrics. `view`
/// supplies the gutter-aware visible column count, so the subprocess renders at
/// the same width the host would draw text. Shared by pane spawn, resize, and
/// the on-demand `:E` / `-` browser open.
pub(in crate::app) fn process_dims(view: &EditorView, rect: Rect, cell: (f32, f32)) -> (u32, u32) {
    let rows = EditorView::visible_lines(rect, cell.1) as u32;
    let cols = view.visible_cols(rect, cell.0) as u32;
    (rows, cols)
}

/// Resolve a panel script path: returned as-is if absolute, else joined onto
/// the layout script's directory (so `panel("clock.ptl")` resolves next to
/// `init.ptl`), falling back to the path as-is (cwd-relative) without one.
impl App {
    /// The **Petal-IDE live binding**: for every `panel(...)` pane whose script
    /// resolves to the same path as a live editor pane's file, recompile the
    /// panel from that editor's current buffer text (preserving Petal `state`),
    /// so the canvas tracks the editor as you type — no save round-trip. This is
    /// what makes `garden petal-ide`'s editor|canvas split feel live, but it is a
    /// general rule: any layout that puts `editor("x.ptl")` beside
    /// `panel("x.ptl")` gets the same binding. Panels with no matching editor (a
    /// clock, a GPP-pushed drawer) are untouched, and the recompile is hash-gated
    /// per panel so an unchanged buffer costs nothing. Returns whether any panel
    /// reloaded (a redraw is wanted); also sets the redraw flag directly.
    pub(in crate::app) fn sync_editor_panels(&mut self) -> bool {
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));

        // Resolved script paths of panels that a matching editor could drive.
        let panel_paths: HashSet<PathBuf> = self
            .panes
            .iter()
            .filter_map(|p| p.panel.as_ref())
            .map(|pv| resolve_script(pv.origin_script(), script_dir.as_deref()))
            .collect();
        if panel_paths.is_empty() {
            // No panels left (e.g. the paired panel pane just closed). Clear any
            // error-line highlight a panel had set on an editor before returning,
            // or it would linger forever with no panel left to reconcile it away.
            let mut cleared = false;
            for pane in &mut self.panes {
                if pane.is_panel() || pane.is_process() {
                    continue;
                }
                if pane.view.error_line.is_some() {
                    pane.view.error_line = None;
                    cleared = true;
                }
                if pane.view.trace_highlight.is_some() {
                    pane.view.trace_highlight = None;
                    cleared = true;
                }
            }
            if cleared {
                self.needs_redraw = true;
            }
            return cleared;
        }

        // Current buffer text of editor panes whose file matches one of those —
        // read only for matched paths, so unrelated editors cost nothing.
        let mut sources: HashMap<PathBuf, String> = HashMap::new();
        for pane in &self.panes {
            if pane.is_panel() || pane.is_process() {
                continue;
            }
            let Some(file) = &pane.file else { continue };
            let path = resolve_script(file, script_dir.as_deref());
            if panel_paths.contains(&path) {
                sources
                    .entry(path)
                    .or_insert_with(|| pane.view.buffer.text());
            }
        }
        if sources.is_empty() {
            return false;
        }

        let now = Instant::now();
        let mut changed = false;
        for pane in &mut self.panes {
            let Some(panel) = pane.panel.as_mut() else {
                continue;
            };
            // The live editor→panel binding tracks the ORIGIN screen. A navigated
            // panel is showing a different screen, so recompiling its host from the
            // origin editor's buffer would push the wrong source into it — skip it
            // until it navigates back home.
            if panel.is_navigated() {
                continue;
            }
            let path = resolve_script(panel.origin_script(), script_dir.as_deref());
            if let Some(src) = sources.get(&path) {
                changed |= panel.reload_from_editor(src, now);
                // A panel with a paired live editor is one you can point at to
                // find the code behind a shape, so it traces its draw calls back
                // to source. Like the live binding itself this is a general rule
                // rather than a `petal-ide` special case: any layout that puts an
                // editor and a panel on one file gets direct manipulation.
                panel.set_trace_origins(true);
            }
        }

        changed |= self.sync_trace_highlight();

        // Reconcile each paired editor's error line with its panel's current
        // error, so a compile/runtime error highlights the offending source line
        // in the editor. Runs every frame (the panel's error persists until a
        // clean compile), so a fixed buffer clears the highlight on the next tick.
        let panel_errors: HashMap<PathBuf, Option<usize>> = self
            .panes
            .iter()
            .filter_map(|p| p.panel.as_ref())
            .map(|panel| {
                let path = resolve_script(panel.origin_script(), script_dir.as_deref());
                (path, panel.error().and_then(parse_error_line))
            })
            .collect();
        for pane in &mut self.panes {
            if pane.is_panel() || pane.is_process() {
                continue;
            }
            let Some(file) = pane.file.clone() else {
                continue;
            };
            let path = resolve_script(&file, script_dir.as_deref());
            if let Some(err_line) = panel_errors.get(&path) {
                if pane.view.error_line != *err_line {
                    pane.view.error_line = *err_line;
                    changed = true;
                }
            }
        }

        if changed {
            self.needs_redraw = true;
        }
        changed
    }

    /// Reconcile the **direct-manipulation highlight**: the shape under the
    /// pointer on a traced canvas lights up the source that drew it, in the
    /// editor paired with that panel. Recomputed from the live pointer, so it
    /// follows the mouse and clears itself the moment it leaves a shape.
    ///
    /// Split out of [`sync_editor_panels`](Self::sync_editor_panels) because it
    /// must also run while **paused**, where the rest of the panel tick does not:
    /// the frozen frame is still the thing being pointed at.
    ///
    /// Also records the full trace (arguments and all) in [`App::trace`] for the
    /// debug server. Returns whether anything changed.
    pub(in crate::app) fn sync_trace_highlight(&mut self) -> bool {
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let (mx, my) = self.mouse;

        // Traced panels and what the pointer is over on each. A path absent from
        // this map has no highlight to show — which is how the highlight clears
        // when the paired panel closes, stops tracing, or drifts out of sync.
        let mut hits: HashMap<PathBuf, Option<DrawTrace>> = HashMap::new();
        for pane in &self.panes {
            let Some(panel) = pane.panel.as_ref() else {
                continue;
            };
            if !panel.traces_origins() {
                continue;
            }
            // While the buffer doesn't compile, the canvas keeps showing the last
            // *good* frame — correct, and the reason the animation never stops.
            // But that frame's spans describe the text that compiled, and the
            // editor is showing text that has moved on: insert a line above and
            // every span is off by one, so the band would sit on unrelated code
            // with total confidence. Report no hit until it compiles again.
            //
            // A *runtime* error is not this case (the running program is still
            // the source on screen), which is why this reads the reload error
            // specifically rather than `error()`.
            if panel.source_drifted() {
                hits.insert(
                    resolve_script(panel.origin_script(), script_dir.as_deref()),
                    None,
                );
                continue;
            }
            let path = resolve_script(panel.origin_script(), script_dir.as_deref());
            // Only the pane actually under the pointer reports a hit; every other
            // traced pane reports "no shape", which is what clears a stale
            // highlight when the mouse leaves the canvas.
            let hit = pane
                .rect
                .contains(mx, my)
                .then(|| panel.trace_at(pane.rect, mx, my))
                .flatten();
            hits.insert(path, hit);
        }

        let mut changed = false;
        let mut detail = None;
        for pane in &mut self.panes {
            if pane.is_panel() || pane.is_process() {
                continue;
            }
            let Some(file) = pane.file.clone() else {
                continue;
            };
            let path = resolve_script(&file, script_dir.as_deref());
            let hit = hits.get(&path).and_then(|t| t.as_ref());
            let span = hit.and_then(|t| t.call).map(|c| {
                (
                    Point::new(c.start_line, c.start_col),
                    Point::new(c.end_line, c.end_col),
                )
            });
            if span.is_some() {
                if let Some(t) = hit {
                    detail = Some(TraceDetail {
                        file: path,
                        trace: t.clone(),
                    });
                }
            }
            if pane.view.trace_highlight != span {
                pane.view.trace_highlight = span;
                changed = true;
            }
        }
        // Compared by span so an unchanged hover doesn't churn; the rest of the
        // trace is derived from the same call, so the span is a faithful key.
        if self.trace.as_ref().map(|d| d.trace.call) != detail.as_ref().map(|d| d.trace.call) {
            changed = true;
        }
        self.trace = detail;
        changed
    }
}

/// Extract the 0-based line index from a Petal error message. Compile and
/// runtime errors carry a bracketed `[line N, column M]` marker (module-
/// qualified errors read `foo.ptl [line N, column M]`), whose `N` is the
/// offending source line, so `error_line` can highlight it. We anchor on the
/// `[line ` bracket rather than the first bare `line ` substring, or an
/// identifier ending in "line" (e.g. `deadline`) would hijack the match.
/// Returns `None` when the message has no such marker.
fn parse_error_line(err: &str) -> Option<usize> {
    let idx = err.find("[line ")?;
    let rest = &err[idx + "[line ".len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
        .map(|n| n - 1)
}

#[cfg(test)]
mod parse_error_line_tests {
    use super::parse_error_line;

    #[test]
    fn reads_the_bracketed_line_marker() {
        assert_eq!(
            parse_error_line("Unexpected '}' [line 5, column 3]"),
            Some(4)
        );
    }

    #[test]
    fn handles_a_module_qualified_marker() {
        assert_eq!(
            parse_error_line("Unknown name ui.ptl [line 12, column 1]"),
            Some(11)
        );
    }

    #[test]
    fn an_identifier_ending_in_line_does_not_hijack_the_match() {
        // `deadline ` contains "line "; only the bracketed marker should count.
        assert_eq!(
            parse_error_line("undefined variable `deadline` [line 7, column 2]"),
            Some(6)
        );
    }

    #[test]
    fn none_without_a_marker() {
        assert_eq!(parse_error_line("some failure with no position"), None);
        assert_eq!(parse_error_line("mentions deadline but no marker"), None);
    }
}

impl App {
    /// Publish the Petal-IDE target program's live editor buffer into the shared
    /// inspector state (hash-gated by [`IrState::set_source`]), so the IR panel's
    /// IR/bytecode/AST re-render as you type. Returns whether the source changed.
    /// A no-op outside IDE mode, or when the target editor isn't open. Uses
    /// interior mutability on the shared cache, so it takes `&self`.
    pub(in crate::app) fn refresh_ir_source(&self) -> bool {
        let Some(ide) = self.ide.as_ref() else {
            return false;
        };
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let source = self.panes.iter().find_map(|pane| {
            if pane.is_panel() || pane.is_process() {
                return None;
            }
            let file = pane.file.as_ref()?;
            (resolve_script(file, script_dir.as_deref()) == ide.target)
                .then(|| pane.view.buffer.text())
        });
        match source {
            Some(src) => ide.ir.borrow_mut().set_source(&src),
            None => false,
        }
    }

    /// Attach the IR `host_data` provider to any Petal-IDE inspector panel — a
    /// panel whose origin script is the seeded `ir_view.ptl` — so its
    /// `host_data(...)` calls reach the shared render cache. Idempotent; run after
    /// every pane rebuild (fresh panels have no provider; a reused panel keeps its
    /// own, and re-attaching over the same shared cache is harmless). Also
    /// refreshes [`IdeState::ir_open`] for the toolbar's IR-button highlight.
    pub(in crate::app) fn attach_ir_providers(&mut self) {
        let Some(ide) = self.ide.as_ref() else {
            return;
        };
        let ir_path = ide.ir_view_path.clone();
        let shared = ide.ir.clone();
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let mut open = false;
        for pane in &mut self.panes {
            let Some(pv) = pane.panel.as_mut() else {
                continue;
            };
            if resolve_script(pv.origin_script(), script_dir.as_deref()) == ir_path {
                pv.set_data_provider(crate::petal_ide::ir_data_provider(shared.clone()));
                open = true;
            }
        }
        if let Some(ide) = self.ide.as_mut() {
            ide.ir_open = open;
        }
    }

    /// Index of the open IR inspector pane (the panel on the seeded `ir_view.ptl`),
    /// if any. Used to toggle it open/closed and to exclude it from a sketch reset.
    pub(in crate::app) fn ir_panel_index(&self) -> Option<usize> {
        let ide = self.ide.as_ref()?;
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        self.panes.iter().position(|p| {
            p.panel.as_ref().is_some_and(|pv| {
                resolve_script(pv.origin_script(), script_dir.as_deref()) == ide.ir_view_path
            })
        })
    }
}

pub(in crate::app) fn resolve_script(script: &str, base: Option<&std::path::Path>) -> PathBuf {
    let p = PathBuf::from(script);
    match base {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p,
    }
}

/// Build a **panel-mode GPP pane** from a just-spawned client: wait briefly for
/// its initial `setScript` push, compile the pushed Petal source into a
/// `PanelHost`, attach a pipe-backed [`ProcessQueryProvider`] over a fresh shared
/// cache, and wrap it as a script-client pane (renders as a panel, persists as a
/// `process(...)` node). Any failure degrades to a plain editor showing the
/// error, never a crash.
pub(in crate::app) fn build_script_client_pane(
    rect: Rect,
    mut view: EditorView,
    process: ProcessPane,
    command: String,
    args: Vec<String>,
) -> Pane {
    let source = match wait_for_set_script(&process, Duration::from_millis(500)) {
        Some(source) => source,
        None => {
            view.set_external_content(
                &format!("{command}: panel-mode GPP client sent no script"),
                None,
            );
            return Pane::editor(rect, None, view);
        }
    };
    match PanelHost::from_source(&command, &source) {
        Ok(mut host) => {
            let shared = crate::script_client::new_shared();
            host.set_query_provider(Box::new(crate::script_client::ProcessQueryProvider::new(
                shared.clone(),
            )));
            let mut panel = PanelView::new(host, format!("gpp:{command}"), Instant::now());
            panel.attach_client(process, shared, command.clone());
            // The home screen is the pushed source, not an on-disk file, so mark
            // the seed history entry source-backed — otherwise *back* to origin
            // would try to rebuild from the synthetic `gpp:<cmd>` path.
            panel.set_origin_source(source);
            Pane::script_client(rect, view, panel, command, args)
        }
        Err(err) => {
            view.set_external_content(&format!("{command}: pushed script failed: {err}"), None);
            Pane::editor(rect, None, view)
        }
    }
}

/// Block up to `timeout` for a panel-mode client's initial `setScript`
/// notification, returning its source (ignoring any other early envelopes).
fn wait_for_set_script(process: &ProcessPane, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        for env in process.drain_for(remaining) {
            if env.is_method(gpp::method::SET_SCRIPT) {
                if let Ok(params) = env.params_as::<gpp::SetScriptParams>() {
                    return Some(params.source);
                }
            }
        }
    }
}

/// Whether `pane` is a live process pane — Lines (`process`) or panel-mode
/// script client (`panel` + `command_args`) — spawned for the same command+args,
/// so it can be moved across a rebuild rather than respawned (keeping the child,
/// its pushed script, and the panel's scroll/selection state).
fn process_matches(pane: &Pane, command: &str, args: &[String]) -> bool {
    (pane.process.is_some() || pane.panel.as_ref().is_some_and(|pv| pv.has_client()))
        && pane
            .command_args
            .as_ref()
            .is_some_and(|(c, a)| c == command && a.as_slice() == args)
}

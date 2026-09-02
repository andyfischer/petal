//! Answering debug-server requests against live state: each [`DebugCmd`] maps
//! to a JSON or text [`Reply`], reusing the same input paths as the frontends
//! so injected input behaves identically. `Screenshot` is the one command the
//! core cannot answer (it needs a renderer); each frontend intercepts it first.

use garden_render::{Primitive, Rect, Scene, TextStyle};
use garden_script::ArgSource;
use serde_json::{json, Value};

use crate::debug::{self, DebugCmd, Reply};
use crate::editor_view::EditorView;
use crate::theme::ThemeScheme;
use crate::vim;

use super::{App, KeyPhase, MenuAction, MENU_ACTIONS};

/// Every script `print(...)` line this session has produced, with the cursor a
/// draining read has reached.
///
/// `GET /state` used to *drain* the panels' output straight into its reply,
/// which made it quietly single-reader: two pollers each saw part of a panel's
/// print lines and neither saw all of them, and an observer could not run
/// alongside a driver at all. The lines are now accumulated here and *read*
/// from, so the same line can be handed to several clients; the draining mode
/// survives as the default (`?output=new`) because a poll loop wants only what
/// is new, but it is one mode of three rather than the only behaviour.
#[derive(Debug, Default)]
pub(in crate::app) struct OutputLog {
    lines: std::collections::VecDeque<String>,
    /// Absolute index of `lines[0]` — absolute numbering survives the cap, so a
    /// cursor a client is holding stays meaningful (and can be seen to have
    /// fallen off the back, since it will be below `first`).
    first: u64,
    /// The absolute index a draining read starts from.
    cursor: u64,
}

impl OutputLog {
    /// Lines kept for re-reading. A panel that prints every frame would
    /// otherwise grow this without bound; the panels' own buffers cap at 200
    /// each, so this is generous next to what can arrive between two reads.
    const CAP: usize = 2000;

    fn push(&mut self, line: String) {
        if self.lines.len() == Self::CAP {
            self.lines.pop_front();
            self.first += 1;
        }
        self.lines.push_back(line);
    }

    /// The absolute index one past the last line held.
    fn next(&self) -> u64 {
        self.first + self.lines.len() as u64
    }

    /// Answer one read, moving the cursor only for the draining mode.
    fn read(&mut self, mode: debug::OutputRead) -> Vec<String> {
        let from = match mode {
            debug::OutputRead::Drain => {
                let next = self.next();
                std::mem::replace(&mut self.cursor, next)
            }
            debug::OutputRead::All => self.first,
            // A cursor from before the retained window reads from the oldest
            // line still held rather than silently returning nothing.
            debug::OutputRead::From(n) => n.max(self.first),
        };
        let skip = from.saturating_sub(self.first) as usize;
        self.lines.iter().skip(skip).cloned().collect()
    }
}

impl App {
    /// Handle one debug command against live state. `Screenshot` is the one
    /// command the core cannot answer — capturing needs a renderer, so each
    /// frontend intercepts it before delegating here.
    pub fn handle_debug(&mut self, cmd: DebugCmd) -> Result<Reply, String> {
        match cmd {
            DebugCmd::State { values, output } => {
                Ok(Reply::Json(self.state_json_filtered(&values, output)))
            }
            DebugCmd::Tick {
                n,
                dt,
                advance_clock,
            } => {
                let frames = self.advance_panels(n, dt, advance_clock);
                Ok(Reply::Json(json!({
                    "ok": true,
                    "panel_frames": frames,
                    "n": n,
                    "dt": dt,
                    // Whether these frames also moved the script clock, and
                    // where it now stands for each panel — a harness asserting
                    // on `time()`-driven animation needs both.
                    "advance_clock": advance_clock,
                    "clocks": self.panel_clocks(),
                    "frame": self.frame(),
                    // Where each panel's own clock now stands, so a caller can
                    // assert the advance landed without a second request.
                    "panels": self.panel_frame_counts(),
                })))
            }
            DebugCmd::PanelReset => {
                let count = self.reset_panel_state();
                Ok(Reply::Json(json!({"ok": true, "panels_reset": count})))
            }
            DebugCmd::Seed { seed } => {
                let count = self.seed_panels(seed);
                Ok(Reply::Json(
                    json!({"ok": true, "seed": seed, "panels": count}),
                ))
            }
            DebugCmd::Scene { pane } => {
                // Same consistency contract as /screenshot: settle panel frames
                // first, so the dumped primitives reflect all injected input.
                self.settle_panels();
                let view = self.scene_view(pane)?;
                let scene = self.build_scene();
                let mut json = scene_json_view(&scene, view);
                json["frame"] = json!(self.frame());
                Ok(Reply::Json(json))
            }
            DebugCmd::Screenshot { .. } => {
                Err("screenshot is not supported by this frontend".to_string())
            }
            DebugCmd::Windows => Err("window listing is answered by the frontend".to_string()),
            DebugCmd::Frame { min } => {
                let frame = self.frame();
                let mut json = json!({"ok": true, "frame": frame});
                if let Some(min) = min {
                    json["reached"] = json!(frame >= min);
                }
                Ok(Reply::Json(json))
            }
            DebugCmd::BufferText { pane } => self
                .panes
                .get(pane)
                .map(|p| Reply::Text(p.view.buffer.to_string()))
                .ok_or_else(|| format!("no pane {pane} (have {})", self.panes.len())),
            DebugCmd::Key { key, mods, op } => {
                let parsed = debug::parse_key(&key).ok_or(format!("unknown key {key:?}"))?;
                let phase = match op {
                    debug::KeyOp::Tap => KeyPhase::Tap,
                    debug::KeyOp::Down => KeyPhase::Down,
                    debug::KeyOp::Up => KeyPhase::Up,
                };
                self.apply_key_phase(parsed, debug::mods_from_names(&mods), phase);
                Ok(Reply::Json(self.input_ack()))
            }
            DebugCmd::Text { text } => {
                self.insert_text(&text)?;
                Ok(Reply::Json(self.input_ack()))
            }
            DebugCmd::Command { command } => {
                // The ex command as typed, without the leading `:`. Driving the
                // command line per-character through `/key` works too, but is
                // slow and easy to get wrong — a test that means "run `:Diff
                // main`" should be able to say so.
                let parsed = crate::command_line::parse(&command);
                self.run_command(parsed);
                Ok(Reply::Json(self.input_ack()))
            }
            DebugCmd::Theme { scheme } => {
                let scheme = parse_theme_scheme(&scheme)
                    .ok_or_else(|| format!("unknown theme scheme {scheme:?}"))?;
                self.set_theme_scheme(scheme);
                Ok(Reply::Json(json!({
                    "ok": true,
                    "theme": {"key": scheme.key(), "label": scheme.label()},
                })))
            }
            DebugCmd::MenuList => Ok(Reply::Json(menu_catalog_json())),
            DebugCmd::Menu { action, arg } => {
                let parsed = MenuAction::from_debug_request(&action, arg.as_deref())?;
                // Echo the resolved variant so the client can confirm what fired
                // (e.g. which theme a fuzzy `SetTheme` arg picked).
                let label = format!("{parsed:?}");
                self.dispatch_menu(parsed);
                let mut ack = self.input_ack();
                ack["action"] = json!(label);
                Ok(Reply::Json(ack))
            }
            DebugCmd::Mouse {
                op,
                x,
                y,
                to,
                lines,
                cols,
                mods,
                clicks,
                button,
            } => {
                // Button 1 is the right button — the context gesture, which has
                // its own (much smaller) routing: panels only, no drag, no
                // cursor placement. See `App::mouse_down_right`.
                let right = button == 1;
                match op.as_str() {
                    "click" if right => {
                        self.mouse_down_right(x, y);
                        self.mouse_up_right();
                    }
                    "down" if right => self.mouse_down_right(x, y),
                    "up" if right => self.mouse_up_right(),
                    "click" => {
                        self.mouse_down(x, y, mods, clicks);
                        self.mouse_up();
                    }
                    "down" => self.mouse_down(x, y, mods, clicks),
                    "move" => self.mouse_moved(x, y),
                    "up" => self.mouse_up(),
                    "drag" => {
                        let (to_x, to_y) = to.ok_or("\"drag\" requires a \"to\" point")?;
                        self.mouse_down(x, y, mods, clicks);
                        self.mouse_moved(to_x, to_y);
                        self.mouse_up();
                    }
                    "scroll" => {
                        self.mouse = (x, y);
                        if lines != 0.0 {
                            self.handle_scroll(lines);
                        }
                        if cols != 0.0 {
                            self.handle_scroll_h(cols);
                        }
                    }
                    other => return Err(format!("unknown mouse op {other:?}")),
                }
                Ok(Reply::Json(self.input_ack()))
            }
        }
    }

    /// Small acknowledgment for input injection: where the focused cursor and
    /// selection ended up.
    fn input_ack(&self) -> Value {
        let pane = self.panes.get(self.focus);
        json!({
            "ok": true,
            "focus": self.focus,
            "cursor": pane.map(|p| point_json(p.view.cursor)),
            "selection": pane.and_then(|p| selection_json(&p.view)),
        })
    }

    /// Every panel's script and current frame number — the `/tick` reply's
    /// receipt that panel time actually moved.
    fn panel_frame_counts(&self) -> Vec<Value> {
        self.panes
            .iter()
            .enumerate()
            .filter_map(|(i, pane)| {
                let pv = pane.panel.as_ref()?;
                Some(json!({"pane": i, "script": pv.script(), "frame": pv.frame_count()}))
            })
            .collect()
    }

    /// The identity of *this* Garden process, at `/state`'s root. Several
    /// concurrent headless sessions are the normal case for agent work, and
    /// `localhost:<port>` has resolved to the wrong one; a response that names
    /// the process, its port, and the scripts it is running is self-diagnosing.
    fn identity_json(&self) -> Value {
        let panels: Vec<Value> = self
            .panes
            .iter()
            .enumerate()
            .filter_map(|(i, pane)| {
                let pv = pane.panel.as_ref()?;
                Some(json!({
                    "pane": i,
                    "script": pv.script(),
                    "path": pv.origin_path().map(absolute_path),
                }))
            })
            .collect();
        json!({
            "pid": std::process::id(),
            "port": crate::debug::server_port(),
            "layout": self.script.as_ref().map(|s| absolute_path(s.path())),
            "cwd": std::env::current_dir().ok().map(|p| p.display().to_string()),
            // Which *build* answered, not just which process: an agent reading
            // /state on every step can tell a stale binary without a second
            // request. The full feature list is at `GET /version`.
            "build": crate::version::build_json(),
            "panels": panels,
        })
    }

    /// The unfiltered `/state` document — the form the in-memory tests assert
    /// on (the debug server itself always goes through the filtered form).
    #[cfg(test)]
    pub(in crate::app) fn state_json(&mut self) -> Value {
        self.state_json_filtered(&debug::ValueFilter::default(), debug::OutputRead::default())
    }

    /// The pane rect a `?pane=<n>` selector names, in logical window
    /// coordinates — the crop `GET /screenshot?pane=<n>` takes and the origin
    /// `GET /scene?pane=<n>` rebases onto. `None` for no selector.
    pub fn pane_capture_rect(&self, pane: Option<usize>) -> Result<Option<Rect>, String> {
        match pane {
            None => Ok(None),
            Some(i) => self
                .panes
                .get(i)
                .map(|p| Some(p.rect))
                .ok_or_else(|| format!("no pane {i} (have {})", self.panes.len())),
        }
    }

    /// Reseed every panel's `random()` stream (`POST /seed`). Returns how many
    /// panels were reseeded. Takes effect on their next frame, so a harness
    /// seeds, then ticks, then captures.
    fn seed_panels(&mut self, seed: u64) -> usize {
        let mut count = 0;
        for pane in &mut self.panes {
            if let Some(panel) = pane.panel.as_mut() {
                panel.set_seed(seed);
                count += 1;
            }
        }
        count
    }

    /// Where each panel's own `time()` clock stands, by pane index, and whether
    /// it is the virtual (tick-driven) clock or the wall clock.
    fn panel_clocks(&self) -> Value {
        let clocks: Vec<Value> = self
            .panes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.panel.as_ref().map(|pv| (i, pv)))
            .map(|(i, pv)| json!({"pane": i, "time": pv.clock(), "virtual": pv.is_virtual_clock()}))
            .collect();
        json!(clocks)
    }

    /// Resolve a `?pane=` selector into the view `/scene` describes.
    fn scene_view(&self, pane: Option<usize>) -> Result<SceneView, String> {
        Ok(SceneView {
            pane,
            bounds: self.pane_capture_rect(pane)?,
        })
    }

    pub(in crate::app) fn state_json_filtered(
        &mut self,
        values: &debug::ValueFilter,
        output: debug::OutputRead,
    ) -> Value {
        let (w, h) = self.viewport.size;
        let cell = self.viewport.cell;
        // A panel's `print(...)` lines, drained before the panes are described so
        // they can be reported alongside the layout script's own output. Panel
        // print is a script author's only debug channel and used to reach stdout
        // and nowhere else — invisible to a headless client driving `/state`.
        // Collected into the session log (never straight into the reply), so a
        // second reader can still see them — see [`OutputLog`].
        let fresh: Vec<String> = self
            .panes
            .iter_mut()
            .filter_map(|pane| pane.panel.as_mut())
            .flat_map(|pv| pv.take_output())
            .collect();
        let script_path = self.script.as_ref().map(|s| s.path().display().to_string());
        if let Some(script) = self.script.as_mut() {
            for line in script.take_output() {
                self.output_log.push(line);
            }
        }
        for line in fresh {
            self.output_log.push(line);
        }
        let output_lines = self.output_log.read(output);
        let output_first = self.output_log.first;
        let output_next = self.output_log.next();
        let panes: Vec<Value> = self
            .panes
            .iter()
            .enumerate()
            .map(|(i, pane)| {
                json!({
                    "index": i,
                    "kind": if pane.is_panel() { "panel" } else { "editor" },
                    "panel": pane.panel.as_ref().map(|pv| json!({
                        "script": pv.script(),
                        // The GPP client app driving this pane (its spawn
                        // command), or null for an in-process panel.
                        "client": pv.client_name(),
                        "awake": pv.is_awake(std::time::Instant::now()),
                        "frame": pv.frame_count(),
                        // Every value the panel's last good frame bound, keyed
                        // by function-qualified source name — how a harness
                        // asserts an interactive panel's logical state
                        // (selection, scroll, hit rects) without decoding
                        // pixels. The script does nothing to publish these; see
                        // `PanelHost::observed_json` for the naming rule and
                        // what is filtered out. Narrow the map with
                        // `?values=` / `?values_prefix=`; unfiltered it is
                        // every binding the script made, seeded data included.
                        "values": filter_values(pv.observed(), values),
                        // Which frame `values` came from, and whether that is
                        // the frame that just ran. A key missing from a stale
                        // map means "the frame that would have bound it
                        // failed", not "that branch never ran" — the two read
                        // identically without this.
                        "values_frame": pv.observed_frame(),
                        "values_stale": pv.observed_frame() != Some(pv.frame_count() - 1),
                        // How far the *failing* frame got before it raised, when
                        // the last frame raised: the partial bindings, beside
                        // (never on top of) the last good ones.
                        "values_partial": pv.partial_observed().map(|(frame, map)| json!({
                            "frame": frame,
                            "values": filter_values(map, values),
                        })),
                        "error": pv.error(),
                        "input": panel_input_json(pv.input_snapshot()),
                    })),
                    "file": pane.file,
                    "title": pane.view.title(),
                    "mode": pane.view.vim.mode.label(),
                    // Mid-command buffered keys, or null at a clean command
                    // boundary. `mode` reads NORMAL even while a count/operator/
                    // prefix is pending, so a harness that ignores this can see
                    // a later command resolve against stale state and look
                    // broken; a single Escape restores a clean slate.
                    "pending": vim_pending_json(&pane.view.vim.pending()),
                    "dirty": pane.view.buffer.is_dirty(),
                    "cursor": point_json(pane.view.cursor),
                    "selection": selection_json(&pane.view),
                    // The direct-manipulation highlight: the source range of the
                    // shape the pointer is over on a paired canvas. Exposed so a
                    // headless session can assert the canvas→source mapping
                    // without decoding pixels. `null` when nothing is traced.
                    "trace_highlight": pane.view.trace_highlight.map(|(s, e)| json!({
                        "start": point_json(s),
                        "end": point_json(e),
                    })),
                    "scroll_top": pane.view.scroll.top,
                    "scroll_sub": pane.view.scroll.sub,
                    // Sub-row offset in rows, 0.0..1.0 — the smooth part of the
                    // scroll position, which the two fields above can't show.
                    "scroll_frac": pane.view.scroll.frac,
                    "scroll_left": pane.view.scroll.left,
                    "wrap": pane.view.wrap,
                    "line_count": pane.view.buffer.line_count(),
                    "visible_lines": EditorView::visible_lines(pane.rect, cell.1),
                    "rect": rect_json(pane.rect),
                })
            })
            .collect();
        let identity = self.identity_json();
        json!({
            "ok": true,
            // Which Garden answered — see `identity_json`.
            "identity": identity,
            // The global frame counter (scenes built so far); each panel also
            // reports its own per-script `frame` below.
            "frame": self.frame(),
            "window": {"width": w, "height": h, "scale": self.viewport.scale},
            "cell": {"width": cell.0, "height": cell.1},
            "focus": self.focus,
            "theme": {
                "key": self.theme_scheme().key(),
                "label": self.theme_scheme().label(),
            },
            "script_error": self.script_error,
            // The one place to look for "something is broken": a standing user
            // -action error, else the live panel error (compile or runtime).
            // `panel_error` reports the same thing on its own for a client that
            // wants to tell the two apart.
            "status_error": self.effective_status_error(),
            "panel_error": self.panel_error,
            "status_note": self.status_note,
            "command_line": self.command_line.as_ref().map(|c| c.display()),
            "file_finder": self.file_finder.as_ref().map(|ff| json!({
                "query": ff.query(),
                "selected": ff.selected_index(),
                "match_count": ff.match_count(),
                "matches": ff.match_paths(20),
                "selected_path": ff.selected_path(),
            })),
            "window_cmd_pending": self.window_cmd_pending,
            // The whole direct-manipulation trace under the pointer, not just the
            // highlighted range: which call, and for each argument where it is
            // written, what literal it resolves to, and how safely a drag could
            // rewrite it. Per-pane `trace_highlight` is what the editor *draws*;
            // this is what a drag mode would *act on*, exposed so it can be
            // asserted end-to-end. `null` when the pointer is over no shape.
            "trace": self.trace.as_ref().map(|d| json!({
                "file": d.file.display().to_string(),
                "callee": d.trace.callee,
                "call": d.trace.call.map(code_span_json),
                "args": d.trace.args.iter().map(|a| json!({
                    "index": a.index,
                    // "literal" | "binding" | "computed" — a `binding` edit
                    // moves every shape reading that definition.
                    "source": match a.source {
                        ArgSource::Literal => "literal",
                        ArgSource::Binding => "binding",
                        ArgSource::Computed => "computed",
                    },
                    "value": a.value,
                    "is_int": a.is_int,
                    // Where the argument is written in the call...
                    "span": a.span.map(code_span_json),
                    // ...vs. the range a rewrite must replace, which for a
                    // binding is at its definition, not at this call.
                    "editable_span": a.editable_span.map(code_span_json),
                })).collect::<Vec<_>>(),
            })),
            "lsp": self.lsp_state_json(),
            // Glyph-atlas pressure as of the last frame any renderer in this
            // process prepared, or null if nothing has been drawn yet (a
            // headless session that has taken no screenshot). A nonzero
            // `overflows`/`dropped_batches` means text was silently missing
            // from a frame — the one condition under which a harness should
            // discard a run as invalid rather than score the screenshot.
            "text_atlas": garden_render::last_atlas_stats().map(|s| json!({
                "runs": s.runs,
                "distinct_sizes": s.distinct_sizes,
                "dropped_batches": s.dropped_batches,
                "overflows": s.overflows,
            })),
            // Font specs a panel asked for that this machine cannot draw. They
            // degrade to the default monospace face and keep drawing, which is
            // right and completely silent: the text is there, it is legible,
            // and it is not the typeface anyone asked for.
            "unresolved_fonts": garden_render::fonts::unresolved_specs(),
            // `output` is every script `print(...)` line the requested read
            // covers — the layout script's and every panel's, in one place,
            // because "where did my print go?" has exactly one answer. The
            // default read is still the draining one; `?output=all` and
            // `?output=<cursor>` do not move the cursor. See [`OutputLog`].
            "script": {
                "path": script_path,
                "output": output_lines,
                // Absolute line numbering, so a second client can resume with
                // `?output=<output_next>` instead of racing the drain — and can
                // tell that lines fell off the back of the buffer, because its
                // cursor will be below `output_first`.
                "output_first": output_first,
                "output_next": output_next,
            },
            "panes": panes,
        })
    }
}

/// The native-menu action catalog for `GET /menu`: each entry is the action
/// name `POST /menu` accepts and the kind of `arg` it needs (`null` = none).
fn menu_catalog_json() -> Value {
    let actions: Vec<Value> = MENU_ACTIONS
        .iter()
        .map(|(name, arg)| json!({"action": name, "arg": arg}))
        .collect();
    json!({"ok": true, "actions": actions})
}

fn parse_theme_scheme(name: &str) -> Option<ThemeScheme> {
    ThemeScheme::ALL.iter().copied().find(|scheme| {
        scheme.key().eq_ignore_ascii_case(name) || scheme.label().eq_ignore_ascii_case(name)
    })
}

fn point_json(p: garden_core::Point) -> Value {
    json!({"line": p.line, "col": p.col})
}

/// A traced source range, in the editor's own 0-based line/column coordinates —
/// shaped like the `trace_highlight` ranges so both read the same way.
fn code_span_json(s: garden_script::CodeSpan) -> Value {
    json!({
        "start": {"line": s.start_line, "col": s.start_col},
        "end": {"line": s.end_line, "col": s.end_col},
    })
}

/// The panel's last-frame input snapshot (for debugging input dispatch) — the
/// full standard contract the script saw: pressed/released edges, held state,
/// the drag gesture, click chain, modifiers, and typed text.
fn panel_input_json(input: &garden_script::PanelInput) -> Value {
    json!({
        "mouse": [input.mouse_x, input.mouse_y],
        "keys_down": input.keys_down,
        "keys_pressed": input.keys_pressed,
        "keys_released": input.keys_released,
        "mouse_buttons_down": input.mouse_buttons_down,
        "mouse_buttons_pressed": input.mouse_buttons_pressed,
        "mouse_buttons_released": input.mouse_buttons_released,
        "scroll": [input.scroll_x, input.scroll_y],
        "modifiers": input.modifiers,
        "drag_active": input.drag_active,
        "drag_start": [input.drag_start_x, input.drag_start_y],
        "click_count": input.click_count,
        "text": input.text,
    })
}

/// A path as an absolute string, for the identity block. Scripts are often
/// launched by a relative path (`--init layout.ptl`), and "which file is this
/// Garden running" is not answerable by a relative one unless you also know its
/// working directory. Falls back to the path as written if it cannot be
/// resolved (a deleted or not-yet-created file).
fn absolute_path(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        })
        .display()
        .to_string()
}

/// Apply a `/state` value filter to one panel's observation map.
fn filter_values(observed: &serde_json::Map<String, Value>, filter: &debug::ValueFilter) -> Value {
    if filter.is_all() {
        return Value::Object(observed.clone());
    }
    Value::Object(
        observed
            .iter()
            .filter(|(k, _)| filter.matches(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

fn rect_json(r: Rect) -> Value {
    json!({"x": r.x, "y": r.y, "w": r.w, "h": r.h})
}

/// A pane's pending vim state, or `null` at a clean command boundary. Only the
/// buffered fields are included, so a non-null object is always a live partial
/// command a test can act on (or clear with Escape). See [`vim::VimPending`].
fn vim_pending_json(pending: &vim::VimPending) -> Value {
    if pending.is_clean() {
        return Value::Null;
    }
    let mut obj = serde_json::Map::new();
    if let Some(count) = pending.count {
        obj.insert("count".into(), json!(count));
    }
    if let Some(op) = pending.operator {
        obj.insert("operator".into(), json!(op.to_string()));
    }
    if pending.g_pending {
        obj.insert("g_pending".into(), json!(true));
    }
    if pending.z_pending {
        obj.insert("z_pending".into(), json!(true));
    }
    if pending.replace_pending {
        obj.insert("replace_pending".into(), json!(true));
    }
    if let Some((till, forward)) = pending.find_pending {
        obj.insert(
            "find_pending".into(),
            json!({"till": till, "forward": forward}),
        );
    }
    if let Some(around) = pending.object_pending {
        obj.insert(
            "object_pending".into(),
            json!(if around { "around" } else { "inner" }),
        );
    }
    Value::Object(obj)
}

/// Selection as JSON, with the selected text capped to keep responses small.
fn selection_json(view: &EditorView) -> Option<Value> {
    const TEXT_CAP: usize = 10_000;
    let sel = view.selection()?;
    let mut text = view.selected_text();
    let truncated = text.chars().count() > TEXT_CAP;
    if truncated {
        text = text.chars().take(TEXT_CAP).collect();
    }
    Some(json!({
        "anchor": point_json(sel.anchor),
        "head": point_json(sel.head),
        "text": text,
        "truncated": truncated,
    }))
}

/// The axis-aligned bounding box of a mesh's vertices — the rect a layout
/// assertion means when it asks where a rounded rect or a circle is. Empty for
/// an empty mesh.
fn mesh_bounds(vertices: &[garden_render::Vertex]) -> Rect {
    let Some(first) = vertices.first() else {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    };
    let (mut min_x, mut min_y) = first.pos;
    let (mut max_x, mut max_y) = first.pos;
    for v in &vertices[1..] {
        min_x = min_x.min(v.pos.0);
        min_y = min_y.min(v.pos.1);
        max_x = max_x.max(v.pos.0);
        max_y = max_y.max(v.pos.1);
    }
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// The colour covering the most area of a mesh. Vertices carry per-vertex
/// colours, so a mesh has no single colour in general (a gradient has none at
/// all) — but every mesh Garden and petal-ui actually emit is a solid fill or a
/// fill plus a differently-coloured border, and "the colour with the most
/// pixels" is the fill in both. Triangles are weighted by area so a hairline
/// border never outvotes the body it surrounds.
fn dominant_color(vertices: &[garden_render::Vertex]) -> Option<garden_render::Color> {
    let mut totals: Vec<(garden_render::Color, f32)> = Vec::new();
    for tri in vertices.chunks_exact(3) {
        let (ax, ay) = tri[0].pos;
        let (bx, by) = tri[1].pos;
        let (cx, cy) = tri[2].pos;
        let area = ((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)).abs() * 0.5;
        // A triangle's colour is its first vertex's; a gradient triangle is
        // therefore reported by one of its corners rather than not at all.
        let color = tri[0].color;
        match totals.iter_mut().find(|(c, _)| *c == color) {
            Some((_, total)) => *total += area,
            None => totals.push((color, area)),
        }
    }
    totals
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
}

/// The largest number of per-shape entries one mesh reports, so a pathological
/// frame cannot produce an unbounded dump.
const MESH_SHAPE_CAP: usize = 256;

/// Break a batched mesh back into the shapes that went into it.
///
/// A panel accumulates consecutive fills into ONE mesh primitive to keep the
/// draw-call count down, so the whole-mesh bounding box is often the whole
/// panel and useless for asserting that (say) a scrollbar exists. The batch is
/// built in draw order, and each `draw_*` call emits its triangles in a single
/// colour, so a run of consecutive same-colour triangles is one shape's
/// tessellation. Two adjacent fills of the *identical* colour merge into one
/// entry — the one case this cannot separate, and a harmless one, since they
/// are indistinguishable on screen too.
fn mesh_shapes(vertices: &[garden_render::Vertex], view: SceneView) -> Vec<Value> {
    let mut shapes: Vec<Value> = Vec::new();
    let mut run_start = 0usize;
    let tris: Vec<&[garden_render::Vertex]> = vertices.chunks_exact(3).collect();
    let color_json = |c: &garden_render::Color| json!([c.r, c.g, c.b, c.a]);
    for i in 0..=tris.len() {
        let same = i < tris.len() && tris[i][0].color == tris[run_start][0].color;
        if same {
            continue;
        }
        if i > run_start {
            let run: Vec<garden_render::Vertex> = tris[run_start..i]
                .iter()
                .flat_map(|t| t.iter().copied())
                .collect();
            if shapes.len() < MESH_SHAPE_CAP {
                shapes.push(json!({
                    "rect": rect_json(view.rect(mesh_bounds(&run))),
                    "color": color_json(&tris[run_start][0].color),
                    "triangles": i - run_start,
                }));
            }
        }
        run_start = i;
    }
    shapes
}

/// Does anything of `rect` survive `clip`?
fn survives_clip(rect: Rect, clip: Rect) -> bool {
    let x = rect.x.max(clip.x);
    let y = rect.y.max(clip.y);
    let right = (rect.x + rect.w).min(clip.x + clip.w);
    let bottom = (rect.y + rect.h).min(clip.y + clip.h);
    right > x && bottom > y
}

/// The box a text run occupies: its measured advance width by its line box.
///
/// Both axes are exact. Vertically the line box is `pos.y ..
/// pos.y + size * LINE_HEIGHT_RATIO`; horizontally the width is what the
/// shaper will actually lay down for this style
/// ([`garden_render::measure_text`]), which is why `visible` below is now a
/// real intersection test rather than the "not provably gone" it used to be —
/// a run that starts inside its clip and runs off the right edge, and a run
/// that reaches in from the left, are both answered properly.
fn text_run_rect(pos: (f32, f32), text: &str, size: f32, style: TextStyle) -> Rect {
    Rect::new(
        pos.0,
        pos.1,
        garden_render::measure_text(text, size, style),
        size * garden_render::LINE_HEIGHT_RATIO,
    )
}

/// Which slice of a scene `/scene` is describing: the whole window, or one
/// pane rebased onto its own origin (`?pane=N`).
///
/// Every harness that screenshots a single pane also has to crop the scene to
/// it and subtract its origin, and getting that offset wrong is silent — the
/// assertions still pass, against the wrong rectangle. So the host does it,
/// and does it the same way for both endpoints.
#[derive(Clone, Copy, Default)]
struct SceneView {
    /// The pane the coordinates are relative to, or `None` for window
    /// coordinates.
    pane: Option<usize>,
    /// The pane's rect in window coordinates: primitives outside it are
    /// dropped, and everything kept is translated by its origin.
    bounds: Option<Rect>,
}

impl SceneView {
    /// Translate a window-space rect into this view's coordinates.
    fn rect(&self, r: Rect) -> Rect {
        match self.bounds {
            Some(b) => Rect::new(r.x - b.x, r.y - b.y, r.w, r.h),
            None => r,
        }
    }

    /// Translate a window-space point into this view's coordinates.
    fn pos(&self, p: (f32, f32)) -> (f32, f32) {
        match self.bounds {
            Some(b) => (p.0 - b.x, p.1 - b.y),
            None => p,
        }
    }

    /// A primitive's clip, narrowed to the pane: a pane-relative dump must not
    /// report a clip reaching past the pane it was cropped to.
    fn clip(&self, clip: Rect) -> Rect {
        let clip = match self.bounds {
            Some(b) => intersect(clip, b),
            None => clip,
        };
        self.rect(clip)
    }

    /// Does this primitive belong in the dump at all? Anything wholly outside
    /// the pane is another pane's (or the chrome's).
    fn keeps(&self, rect: Rect) -> bool {
        match self.bounds {
            Some(b) => survives_clip(rect, b),
            None => true,
        }
    }
}

/// The overlap of two rects, or an empty rect at the first's origin when they
/// do not meet.
fn intersect(a: Rect, b: Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.w).min(b.x + b.w);
    let bottom = (a.y + a.h).min(b.y + b.h);
    Rect::new(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
}

/// What a text run is actually rasterized in, as opposed to what it asked for.
///
/// `family` is the face the shaper gets — for a spec this machine cannot draw
/// that is the default monospace, which is the single most useful thing a
/// harness can be told, because the drawing looks fine and is simply in the
/// wrong typeface. `weight`/`italic` are the *cut* fontdb picked, which for a
/// family with no bold (the embedded JetBrains Mono) or no italic is not the
/// one requested; `synthetic_bold` says the weight is being faked by
/// over-drawing rather than shaped.
fn resolved_font_json(style: TextStyle) -> Value {
    let (cut_weight, cut_italic) =
        garden_render::fonts::shaping_cut(style.font, style.weight, style.italic);
    json!({
        "family": garden_render::fonts::family_of(style.font),
        "weight": cut_weight,
        "italic": cut_italic,
        "synthetic_bold": garden_render::fonts::needs_synthetic_bold(
            style.font,
            style.weight,
            style.italic,
        ),
    })
}

/// The whole-window dump — `scene_json_view` with no pane selector. Tests
/// describe a scene by hand and read it back through this.
#[cfg(test)]
fn scene_json(scene: &Scene) -> Value {
    scene_json_view(scene, SceneView::default())
}

fn scene_json_view(scene: &Scene, view: SceneView) -> Value {
    let color_json = |c: &garden_render::Color| json!([c.r, c.g, c.b, c.a]);
    let prims: Vec<Value> = scene
        .primitives
        .iter()
        .enumerate()
        .filter_map(|(id, p)| {
            let mut prim = match p {
                Primitive::Quad { rect, color } => {
                    if !view.keeps(*rect) {
                        return None;
                    }
                    json!({
                        "type": "quad", "rect": rect_json(view.rect(*rect)),
                        "color": color_json(color),
                    })
                }
                Primitive::Text {
                    pos,
                    text,
                    color,
                    clip,
                    size,
                    style,
                } => {
                    let box_ = text_run_rect(*pos, text, *size, *style);
                    if !view.keeps(box_) {
                        return None;
                    }
                    let at = view.pos(*pos);
                    json!({
                        "type": "text", "pos": [at.0, at.1],
                        "text": text,
                        "color": color_json(color), "clip": rect_json(view.clip(*clip)),
                        "size": size,
                        // Whether the run survives its clip. A scrolling list
                        // that clips to its viewport emits the rows above and
                        // below it too; without this a headless test could not
                        // tell a drawn row from a clipped-away one, which is
                        // what pushed drawers into culling straddling rows
                        // themselves. Exact on both axes now that the advance
                        // is measured.
                        "visible": survives_clip(box_, *clip),
                        // The shaped advance width, so a scene can be compared
                        // against a reference layout by extent and not just by
                        // origin — and so "is this centered?" is answerable at
                        // all.
                        "advance": box_.w,
                        // The typographic axes, unconditionally: absent fields
                        // conflated "default" with "not applicable" and every
                        // consumer had to special-case the gap.
                        "weight": style.weight,
                        "italic": style.italic,
                        "spacing": style.spacing,
                        // …and the face this run is really drawn in, which is
                        // not always the one it named. See `resolved_font_json`.
                        "font": resolved_font_json(*style),
                    })
                }
                // A mesh's per-vertex data would bloat the dump, but reporting
                // only a triangle count made `/scene` useless for the panels
                // that need it most: a rounded rect, a circle and a triangle
                // are all meshes, so a design that fills with rounded rects had
                // *no* assertable geometry at all. Summarize each mesh by what
                // a layout assertion actually wants — where it is and what
                // colour it is.
                Primitive::Mesh { vertices, clip } => {
                    let bounds = mesh_bounds(vertices);
                    if !view.keeps(bounds) {
                        return None;
                    }
                    json!({
                        "type": "mesh", "triangles": vertices.len() / 3,
                        "clip": rect_json(view.clip(*clip)),
                        "rect": rect_json(view.rect(bounds)),
                        "visible": survives_clip(bounds, *clip),
                        "color": dominant_color(vertices).map(|c| color_json(&c)),
                        // …and, since consecutive fills are batched into one
                        // mesh, the individual shapes that went into it.
                        "shapes": mesh_shapes(vertices, view),
                    })
                }
                Primitive::Image {
                    rect,
                    source,
                    alpha,
                    clip,
                    mask,
                } => {
                    if !view.keeps(*rect) {
                        return None;
                    }
                    json!({
                        "type": "image", "rect": rect_json(view.rect(*rect)), "source": source,
                        "alpha": alpha, "clip": rect_json(view.clip(*clip)),
                        "visible": survives_clip(*rect, *clip),
                        // The rounded cut its corners survive: 0 for a square
                        // image, so a client can assert an avatar is a circle.
                        "radius": mask.radius.max(0.0),
                    })
                }
                // The layer ops. A canvas's contents are drawn in *canvas*
                // coordinates between a `target` entry naming it and the one
                // that switches back, so a pane-relative view reports those
                // primitives where they sit on the canvas, not on the pane.
                Primitive::Canvas { id, size } => json!({
                    "type": "canvas", "canvas": id, "size": [size.0, size.1],
                }),
                Primitive::Target { id } => json!({ "type": "target", "canvas": id }),
                Primitive::Snapshot { id, from, clip } => {
                    let at = view.pos(*from);
                    json!({
                        "type": "snapshot", "canvas": id, "from": [at.0, at.1],
                        "clip": rect_json(view.clip(*clip)),
                    })
                }
                Primitive::Blur { id, radius } => json!({
                    "type": "blur", "canvas": id, "radius": radius,
                }),
                Primitive::CanvasDraw {
                    id,
                    rect,
                    alpha,
                    clip,
                    mask,
                } => {
                    if !view.keeps(*rect) {
                        return None;
                    }
                    json!({
                        "type": "canvas_draw", "canvas": id,
                        "rect": rect_json(view.rect(*rect)),
                        "alpha": alpha, "clip": rect_json(view.clip(*clip)),
                        "visible": survives_clip(*rect, *clip),
                        "radius": mask.radius.max(0.0),
                    })
                }
            };
            // The primitive's index in the draw-command stream — stable for a
            // given scene, and the handle a client diffs two scenes by. It is
            // the *unfiltered* index, so a pane-relative dump and a whole-window
            // one name the same primitive by the same id.
            prim["id"] = json!(id);
            Some(prim)
        })
        .collect();
    let mut out = json!({"ok": true, "bg": color_json(&scene.bg), "primitives": prims});
    if let (Some(pane), Some(bounds)) = (view.pane, view.bounds) {
        // Which pane the coordinates are relative to, and where it sits in the
        // window — the two numbers a client would otherwise have to guess to
        // line a pane-only screenshot up with a pane-relative scene.
        out["pane"] = json!({"index": pane, "rect": rect_json(bounds)});
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::InMemoryClipboard;
    use crate::debug::ValueFilter;
    use garden_render::{Color, Vertex};
    use garden_script::LayoutNode;
    use std::io::Write;

    /// A one-panel app running `source`, settled so the panel has run a frame.
    /// The temp file is returned because it must outlive the app that reads it.
    fn panel_app(source: &str) -> (App, tempfile::NamedTempFile) {
        let mut file = tempfile::NamedTempFile::with_suffix(".ptl").unwrap();
        write!(file, "{source}").unwrap();
        let mut app = App::new(
            None,
            LayoutNode::Panel {
                script: file.path().to_string_lossy().into_owned(),
                screens: Vec::new(),
            },
            true,
            crate::app::Viewport {
                size: (800.0, 600.0),
                cell: (8.0, 16.0),
                scale: 1.0,
            },
            Box::new(InMemoryClipboard::default()),
        );
        app.settle_panels();
        (app, file)
    }

    fn state_with(app: &mut App, query: &str) -> Value {
        let cmd = crate::debug::route_for_test("GET", query, b"").expect("routes");
        match app.handle_debug(cmd).expect("state") {
            Reply::Json(v) => v,
            _ => panic!("/state must answer JSON"),
        }
    }

    /// The single panel's block in a `/state` reply.
    fn panel_of(state: &Value) -> &Value {
        state["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["panel"].is_object())
            .map(|p| &p["panel"])
            .expect("a panel pane")
    }

    /// `panel.values` is unusable at full size on a real app — every colour
    /// constant and every copy of a seeded list. `?values=` and
    /// `?values_prefix=` narrow it to the handful a test is asserting on.
    #[test]
    fn state_values_can_be_filtered() {
        let (mut app, _f) = panel_app(
            "let obs_sel = 3\nlet obs_scroll = 4\nlet palette = [1, 2, 3]\nlet spacing = 7\n",
        );

        let all = state_with(&mut app, "/state");
        let values = panel_of(&all)["values"].as_object().unwrap();
        for key in ["obs_sel", "obs_scroll", "palette", "spacing"] {
            assert!(values.contains_key(key), "unfiltered /state keeps {key}");
        }

        let named = state_with(&mut app, "/state?values=obs_sel,spacing");
        let values = panel_of(&named)["values"].as_object().unwrap();
        assert_eq!(values.len(), 2, "only the named values: {values:?}");
        assert_eq!(values["obs_sel"], 3);
        assert_eq!(values["spacing"], 7);

        let prefixed = state_with(&mut app, "/state?values_prefix=obs_");
        let values = prefixed["panes"][0]["panel"]["values"].as_object().unwrap();
        assert_eq!(values.len(), 2, "only the obs_ mirror values: {values:?}");
        assert!(values.contains_key("obs_sel") && values.contains_key("obs_scroll"));

        let dropped = state_with(&mut app, "/state?values=none");
        assert!(panel_of(&dropped)["values"].as_object().unwrap().is_empty());
    }

    /// A key from an erroring frame used to come back *absent*, which reads as
    /// "that branch never ran". The values now carry the frame they came from,
    /// are flagged stale, and the failing frame's partial bindings are reported
    /// beside them.
    #[test]
    fn values_are_stamped_with_their_frame_and_keep_a_failing_frame() {
        let (mut app, _f) = panel_app(
            "state n = 0\nn = n + 1\nlet alive = n * 10\n\
             if n == 2 then\n  let doomed = 99\n  assert(false)\nend\n",
        );

        let good = panel_of(&state_with(&mut app, "/state")).clone();
        assert_eq!(good["values"]["alive"], 10);
        assert_eq!(good["values_frame"], 0, "the first frame is frame 0");
        assert_eq!(good["values_stale"], false);
        assert!(good["values_partial"].is_null(), "no failure yet");

        // Frame 1 raises. The last good values stay (they match what is on
        // screen) but are now flagged stale, and the failed frame is reported.
        app.advance_panels(1, 0.016, true);
        let broken = panel_of(&state_with(&mut app, "/state")).clone();
        assert!(broken["error"].is_string(), "the frame should have raised");
        assert_eq!(broken["values"]["alive"], 10, "last good values survive");
        assert_eq!(broken["values_frame"], 0);
        assert_eq!(
            broken["values_stale"], true,
            "an absent key must not read as 'that branch never ran'"
        );
        assert_eq!(broken["values_partial"]["frame"], 1);
        assert_eq!(
            broken["values_partial"]["values"]["alive"], 20,
            "how far the failing frame got, kept beside the good values"
        );
    }

    /// `POST /tick` advances panel time with no injected input at all — the
    /// thing every animation test was faking with no-op keypresses.
    #[test]
    fn tick_advances_panel_frames_without_input() {
        let (mut app, _f) = panel_app("state n = 0\nn = n + 1\nlet seen = n\n");
        let before = panel_of(&state_with(&mut app, "/state"))["frame"]
            .as_i64()
            .unwrap();

        let reply = match app
            .handle_debug(DebugCmd::Tick {
                n: 30,
                dt: 0.016,
                advance_clock: true,
            })
            .expect("tick")
        {
            Reply::Json(v) => v,
            _ => panic!("/tick must answer JSON"),
        };
        assert_eq!(reply["panel_frames"], 30);

        let state = state_with(&mut app, "/state");
        let after = panel_of(&state);
        assert_eq!(after["frame"].as_i64().unwrap(), before + 30);
        assert_eq!(
            after["values"]["seen"].as_i64().unwrap(),
            before + 30,
            "the script's own counter advanced once per ticked frame"
        );
    }

    /// A ticked panel keeps ticking past the wake window: `/tick` ignores it
    /// (and re-stamps activity), so a long deterministic run doesn't stall.
    #[test]
    fn tick_ignores_the_sleep_window() {
        let (mut app, _f) = panel_app("state n = 0\nn = n + 1\nlet seen = n\n");
        // Force every panel asleep by moving its activity stamp into the past.
        for pane in &mut app.panes {
            if let Some(pv) = pane.panel.as_mut() {
                pv.sleep_for_test();
            }
        }
        assert_eq!(
            app.advance_panels(5, 0.016, true),
            5,
            "asleep panels still tick"
        );
        let state = state_with(&mut app, "/state");
        assert_eq!(panel_of(&state)["awake"], true);
    }

    /// Petal `state` surviving hot reload is right, and is exactly what makes
    /// iterating on *seeded* data impossible in place. `/panel/reset` restarts
    /// the panel from source so the seed is regenerated.
    #[test]
    fn panel_reset_discards_petal_state() {
        let (mut app, _f) = panel_app("state n = 0\nn = n + 1\nlet seen = n\n");
        app.advance_panels(4, 0.016, true);
        assert!(
            panel_of(&state_with(&mut app, "/state"))["values"]["seen"]
                .as_i64()
                .unwrap()
                >= 5
        );

        match app.handle_debug(DebugCmd::PanelReset).expect("reset") {
            Reply::Json(v) => assert_eq!(v["panels_reset"], 1),
            _ => panic!("/panel/reset must answer JSON"),
        }
        app.settle_panels();
        let state = state_with(&mut app, "/state");
        let after = panel_of(&state);
        assert_eq!(
            after["values"]["seen"], 1,
            "state restarted from the script's own initializer"
        );
    }

    /// Two Gardens on neighbouring ports have been confused for each other;
    /// `/state` now says which process and which scripts answered.
    #[test]
    fn state_identifies_the_process_and_its_panels() {
        let (mut app, file) = panel_app("let x = 1\n");
        let state = state_with(&mut app, "/state");
        let id = &state["identity"];
        assert_eq!(id["pid"], std::process::id());
        let panels = id["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(
            panels[0]["path"].as_str().unwrap(),
            std::fs::canonicalize(file.path())
                .unwrap()
                .display()
                .to_string(),
            "the panel's script path is in /state's root, absolute"
        );
        // …and which *build* answered, so a stale binary is visible to any
        // client already reading /state.
        assert!(!id["build"]["version"].as_str().unwrap().is_empty());
        assert!(!id["build"]["build_date"].as_str().unwrap().is_empty());
    }

    /// A clipped scrolling list emits the rows outside its viewport too; the
    /// dump has to say which of them the clip actually keeps, or a headless
    /// test cannot tell a drawn row from a clipped-away one.
    #[test]
    fn scene_marks_text_clipped_away_as_not_visible() {
        let clip = Rect::new(0.0, 100.0, 200.0, 40.0);
        let run = |y: f32| Primitive::Text {
            pos: (10.0, y),
            text: "row".to_string(),
            color: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            clip,
            size: 16.0,
            style: TextStyle::default(),
        };
        let scene = Scene {
            bg: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            // Above the clip, inside it, straddling its bottom edge, below it.
            primitives: vec![run(60.0), run(110.0), run(130.0), run(180.0)],
        };
        let json = scene_json(&scene);
        let visible: Vec<&Value> = json["primitives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| &p["visible"])
            .collect();
        assert_eq!(
            visible,
            vec![&json!(false), &json!(true), &json!(true), &json!(false)],
            "a straddling run is still (partly) visible; the ones outside are not"
        );
    }

    /// The horizontal half of `visible` used to be a guess (only the run's
    /// start was judged), so a run scrolled off the right of its clip read as
    /// visible. With a measured advance it is a real intersection test.
    #[test]
    fn scene_measures_the_horizontal_clip_too() {
        let clip = Rect::new(100.0, 0.0, 200.0, 40.0);
        let white = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let run = |x: f32| Primitive::Text {
            pos: (x, 10.0),
            text: "abcd".to_string(),
            color: white,
            clip,
            size: 16.0,
            style: TextStyle::default(),
        };
        let scene = Scene {
            bg: white,
            // Wholly left of the clip, reaching in from the left (its measured
            // advance carries it over x=100), inside, and wholly right of it.
            primitives: vec![run(-100.0), run(80.0), run(150.0), run(400.0)],
        };
        let json = scene_json(&scene);
        let visible: Vec<&Value> = json["primitives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| &p["visible"])
            .collect();
        assert_eq!(
            visible,
            vec![&json!(false), &json!(true), &json!(true), &json!(false)]
        );
    }

    /// A scene comparison that can only see origins is blind to the bugs that
    /// matter, so every run reports the face it is really drawn in, its
    /// typographic axes (always, not only when non-default), its measured
    /// advance, and a stable id.
    #[test]
    fn scene_text_reports_its_face_metrics_and_id() {
        let white = Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let clip = Rect::new(0.0, 0.0, 800.0, 600.0);
        let styled = TextStyle {
            weight: 700,
            italic: false,
            spacing: 2.0,
            font: garden_render::fonts::resolve("ui"),
        };
        let scene = Scene {
            bg: white,
            primitives: vec![
                Primitive::Quad {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    color: white,
                },
                Primitive::Text {
                    pos: (10.0, 20.0),
                    text: "Title".to_string(),
                    color: white,
                    clip,
                    size: 20.0,
                    style: styled,
                },
                Primitive::Text {
                    pos: (10.0, 60.0),
                    text: "Title".to_string(),
                    color: white,
                    clip,
                    size: 20.0,
                    style: TextStyle::default(),
                },
            ],
        };
        let json = scene_json(&scene);
        let prims = json["primitives"].as_array().unwrap();
        // Rule (d): the id is the primitive's index in the draw stream.
        for (i, p) in prims.iter().enumerate() {
            assert_eq!(p["id"], json!(i));
        }
        let ui = &prims[1];
        let mono = &prims[2];
        // Rule (b): present on every run, default or not — "absent" used to
        // conflate "default" with "not applicable".
        assert_eq!(mono["weight"], 400);
        assert_eq!(mono["italic"], false);
        assert_eq!(mono["spacing"], 0.0);
        assert_eq!(ui["weight"], 700);
        assert_eq!(ui["spacing"], 2.0);
        // Rule (a): the resolved face, and the *cut* the shaper will use — the
        // embedded monospace has no bold, so its bold is over-drawn.
        assert!(ui["font"]["family"].is_string());
        assert_ne!(ui["font"]["family"], mono["font"]["family"]);
        assert_eq!(mono["font"]["synthetic_bold"], false);
        // Rule (c): a measured advance, in the run's own face.
        let ui_w = ui["advance"].as_f64().unwrap();
        let mono_w = mono["advance"].as_f64().unwrap();
        assert!(ui_w > 0.0 && mono_w > 0.0);
        assert!(
            (ui_w - mono_w).abs() > 1.0,
            "a proportional bold run and a monospace one must not measure alike \
             ({ui_w} vs {mono_w})"
        );
        // Letter-spacing is part of the advance, since it is part of the pen.
        assert!(ui_w > 5.0 * 2.0);
    }

    /// Every harness that screenshots one pane also crops the scene to it and
    /// subtracts its origin. The host does both now, the same way, so the two
    /// cannot drift apart.
    #[test]
    fn a_pane_scoped_scene_is_cropped_and_rebased() {
        let (mut app, _f) = panel_app("draw_rect(0, 0, 10, 10, 1, 2, 3)\n");
        let pane = app.pane_capture_rect(Some(0)).unwrap().unwrap();
        assert!(
            pane.x > 0.0 || pane.y > 0.0,
            "the pane is inset by the chrome"
        );

        let full = match app.handle_debug(DebugCmd::Scene { pane: None }).unwrap() {
            Reply::Json(v) => v,
            _ => panic!("/scene answers JSON"),
        };
        let scoped = match app.handle_debug(DebugCmd::Scene { pane: Some(0) }).unwrap() {
            Reply::Json(v) => v,
            _ => panic!("/scene answers JSON"),
        };
        assert_eq!(scoped["pane"]["index"], 0);
        assert_eq!(scoped["pane"]["rect"]["x"], pane.x);

        let scoped_prims = scoped["primitives"].as_array().unwrap();
        assert!(!scoped_prims.is_empty());
        // Ids are the *unfiltered* indices, so the same primitive is named the
        // same way in both dumps — and its rect differs by exactly the origin.
        let full_prims = full["primitives"].as_array().unwrap();
        for p in scoped_prims {
            let id = p["id"].as_u64().unwrap() as usize;
            let same = &full_prims[id];
            assert_eq!(same["id"], p["id"]);
            if let (Some(a), Some(b)) = (p["rect"]["x"].as_f64(), same["rect"]["x"].as_f64()) {
                assert!(
                    (b - a - pane.x as f64).abs() < 0.01,
                    "rebased by the pane origin"
                );
            }
        }
        assert!(
            scoped_prims.len() < full_prims.len(),
            "the chrome outside the pane is dropped"
        );
    }

    /// 60 ticks at dt=0.016 are 0.96 seconds of script time, whatever the wall
    /// clock did while they ran. Without that no golden image of a moving UI is
    /// stable, because `time()` never lands on the same value twice.
    #[test]
    fn ticking_advances_the_script_clock_by_the_supplied_dt() {
        let (mut app, _f) = panel_app("let t = time()\ndraw_rect(0, 0, 1, 1, 1, 2, 3)\n");
        let before = panel_of(&state_with(&mut app, "/state"))["values"]["t"]
            .as_f64()
            .unwrap();
        let reply = match app
            .handle_debug(DebugCmd::Tick {
                n: 60,
                dt: 0.016,
                advance_clock: true,
            })
            .expect("tick")
        {
            Reply::Json(v) => v,
            _ => panic!("/tick answers JSON"),
        };
        assert_eq!(reply["clocks"][0]["virtual"], true);
        let after = panel_of(&state_with(&mut app, "/state"))["values"]["t"]
            .as_f64()
            .unwrap();
        // The 60 ticks contribute exactly 0.96; the settle frames each side of
        // them contribute their own (sub-millisecond, wall-measured) dt, which
        // is why this is a tolerance rather than an equality.
        assert!(
            (after - before - 0.96).abs() < 0.01,
            "time() advanced by {} over 60 ticks of 0.016",
            after - before
        );
    }

    /// `{"advance_clock": false}` keeps the wall clock, for a caller that wants
    /// ticked frames to be extra frames of real time.
    #[test]
    fn ticking_can_leave_the_clock_alone() {
        let (mut app, _f) = panel_app("let t = time()\ndraw_rect(0, 0, 1, 1, 1, 2, 3)\n");
        app.handle_debug(DebugCmd::Tick {
            n: 60,
            dt: 1.0,
            advance_clock: false,
        })
        .expect("tick");
        let t = panel_of(&state_with(&mut app, "/state"))["values"]["t"]
            .as_f64()
            .unwrap();
        assert!(t < 1.0, "wall-clock time() must not jump 60 seconds: {t}");
    }

    /// Seeding is what makes generated placeholder content comparable between
    /// two renders of the same script.
    #[test]
    fn seeding_makes_generated_content_reproducible() {
        let src = "state n = random(0, 1000000)\nlet seen = n\ndraw_rect(0, 0, 1, 1, 1, 2, 3)\n";
        let (mut app, _f) = panel_app(src);
        let draw = |app: &mut App| {
            // Reset first: a restart rebuilds the panel host (and with it the
            // engine's clock-derived seed), so the seed has to be published
            // after it.
            app.handle_debug(DebugCmd::PanelReset).expect("reset");
            app.handle_debug(DebugCmd::Seed { seed: 42 }).expect("seed");
            app.settle_panels();
            panel_of(&state_with(app, "/state"))["values"]["seen"].clone()
        };
        let first = draw(&mut app);
        let second = draw(&mut app);
        assert_eq!(first, second, "the same seed must draw the same content");
    }

    /// `/state` used to *drain* the output, which made it single-reader: an
    /// observer running beside a driver saw half the lines and neither saw all.
    #[test]
    fn state_output_can_be_reread_from_a_cursor() {
        let (mut app, _f) = panel_app("state n = 0\nn = n + 1\nprint(str(n))\n");
        app.advance_panels(2, 0.016, true);

        let first = state_with(&mut app, "/state");
        let lines = first["script"]["output"].as_array().unwrap().clone();
        assert!(
            lines.len() >= 3,
            "three frames have printed by now: {lines:?}"
        );
        let next = first["script"]["output_next"].as_u64().unwrap();

        // The default read drains, as it always did.
        let second = state_with(&mut app, "/state");
        assert!(second["script"]["output"].as_array().unwrap().len() < lines.len());

        // …but a second client can still see everything, and resume.
        let all = state_with(&mut app, "/state?output=all");
        assert_eq!(
            all["script"]["output"].as_array().unwrap()[..lines.len()],
            lines[..]
        );
        let resumed = state_with(&mut app, &format!("/state?output={next}"));
        assert_eq!(
            resumed["script"]["output_next"].as_u64().unwrap()
                - resumed["script"]["output"].as_array().unwrap().len() as u64,
            next,
            "a cursor read starts exactly where the last one stopped"
        );
        // Reading with a cursor must not move the draining cursor.
        let after = state_with(&mut app, "/state?output=all");
        assert!(after["script"]["output"].as_array().unwrap().len() >= lines.len());
    }

    /// Every panel fill is a mesh, so a `/scene` that reported only a triangle
    /// count left a rounded-rect design with no assertable geometry at all.
    #[test]
    fn scene_reports_mesh_bounds_and_dominant_color() {
        let fill = Color {
            r: 0.2,
            g: 0.4,
            b: 0.6,
            a: 1.0,
        };
        let border = Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        // A 100x50 body (two triangles) plus a hairline sliver in another color.
        let v = |x: f32, y: f32, c: Color| Vertex::new((x, y), c);
        let vertices = vec![
            v(10.0, 20.0, fill),
            v(110.0, 20.0, fill),
            v(10.0, 70.0, fill),
            v(110.0, 20.0, fill),
            v(110.0, 70.0, fill),
            v(10.0, 70.0, fill),
            v(10.0, 20.0, border),
            v(110.0, 20.0, border),
            v(10.0, 20.5, border),
        ];
        let scene = Scene {
            bg: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            primitives: vec![Primitive::Mesh {
                vertices,
                clip: Rect::new(0.0, 0.0, 800.0, 600.0),
            }],
        };
        let json = scene_json(&scene);
        let mesh = &json["primitives"][0];
        assert_eq!(mesh["rect"]["x"], 10.0);
        assert_eq!(mesh["rect"]["y"], 20.0);
        assert_eq!(mesh["rect"]["w"], 100.0);
        assert_eq!(mesh["rect"]["h"], 50.0);
        assert_eq!(
            mesh["color"],
            json!([fill.r, fill.g, fill.b, fill.a]),
            "the fill wins on area, not the hairline border"
        );

        // Consecutive fills are batched into one mesh, so the per-shape
        // breakdown is what proves a particular element (a scrollbar, say) is
        // there at all.
        let shapes = mesh["shapes"].as_array().unwrap();
        assert_eq!(shapes.len(), 2, "body and border are separate shapes");
        assert_eq!(shapes[0]["rect"]["h"], 50.0);
        assert_eq!(shapes[0]["color"], json!([fill.r, fill.g, fill.b, fill.a]));
        assert_eq!(shapes[1]["rect"]["h"], 0.5);
        assert_eq!(
            shapes[1]["color"],
            json!([border.r, border.g, border.b, border.a])
        );
    }

    #[test]
    fn value_filter_matches_function_qualified_keys() {
        let filter = ValueFilter::from_query_for_test(&[("values", "sel")]);
        assert!(filter.matches("sel"));
        assert!(
            filter.matches("list_row.sel"),
            "a caller shouldn't need to know the enclosing function"
        );
        assert!(!filter.matches("selection"));
        assert!(!filter.matches("theme"));
    }
}

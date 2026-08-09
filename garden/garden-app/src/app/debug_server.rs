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

use super::{App, MenuAction, Mods, MENU_ACTIONS};

impl App {
    /// Handle one debug command against live state. `Screenshot` is the one
    /// command the core cannot answer — capturing needs a renderer, so each
    /// frontend intercepts it before delegating here.
    pub fn handle_debug(&mut self, cmd: DebugCmd) -> Result<Reply, String> {
        match cmd {
            DebugCmd::State => Ok(Reply::Json(self.state_json())),
            DebugCmd::Scene => {
                // Same consistency contract as /screenshot: settle panel frames
                // first, so the dumped primitives reflect all injected input.
                self.settle_panels();
                let scene = self.build_scene();
                let mut json = scene_json(&scene);
                json["frame"] = json!(self.frame());
                Ok(Reply::Json(json))
            }
            DebugCmd::Screenshot => Err("screenshot is not supported by this frontend".to_string()),
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
            DebugCmd::Key { key, mods } => {
                let parsed = debug::parse_key(&key).ok_or(format!("unknown key {key:?}"))?;
                let has = |names: &[&str]| mods.iter().any(|m| names.contains(&m.as_str()));
                self.apply_key(
                    parsed,
                    Mods {
                        cmd: has(&["cmd", "super", "meta"]),
                        ctrl: has(&["ctrl", "control"]),
                        shift: has(&["shift"]),
                    },
                );
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

    pub(in crate::app) fn state_json(&mut self) -> Value {
        let (w, h) = self.viewport.size;
        let cell = self.viewport.cell;
        let panes: Vec<Value> = self
            .panes
            .iter()
            .enumerate()
            .map(|(i, pane)| {
                json!({
                    "index": i,
                    "kind": if pane.is_process() {
                        "process"
                    } else if pane.is_panel() {
                        "panel"
                    } else {
                        "editor"
                    },
                    "process": pane.process.as_ref().map(|proc| json!({
                        "name": proc.name(),
                        "takeover": proc.takeover(),
                        "keymap": proc.keymap(),
                    })),
                    "panel": pane.panel.as_ref().map(|pv| json!({
                        "script": pv.script(),
                        "awake": pv.is_awake(std::time::Instant::now()),
                        "frame": pv.frame_count(),
                        // Every value the panel's last good frame bound, keyed
                        // by function-qualified source name — how a harness
                        // asserts an interactive panel's logical state
                        // (selection, scroll, hit rects) without decoding
                        // pixels. The script does nothing to publish these; see
                        // `PanelHost::observed_json` for the naming rule and
                        // what is filtered out.
                        "values": Value::Object(pv.observed().clone()),
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
        json!({
            "ok": true,
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
            "status_error": self.status_error,
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
            "script": self.script.as_mut().map(|script| json!({
                "path": script.path().display().to_string(),
                "output": script.take_output(),
            })),
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

fn scene_json(scene: &Scene) -> Value {
    let color_json = |c: &garden_render::Color| json!([c.r, c.g, c.b, c.a]);
    let prims: Vec<Value> = scene
        .primitives
        .iter()
        .map(|p| match p {
            Primitive::Quad { rect, color } => json!({
                "type": "quad", "rect": rect_json(*rect), "color": color_json(color),
            }),
            Primitive::Text {
                pos,
                text,
                color,
                clip,
                size,
                style,
            } => {
                let mut run = json!({
                    "type": "text", "pos": [pos.0, pos.1], "text": text,
                    "color": color_json(color), "clip": rect_json(*clip), "size": size,
                });
                // The typographic axes appear only when a run actually uses
                // one, so an assertion over ordinary text sees the shape it
                // always did.
                if *style != TextStyle::default() {
                    run["weight"] = json!(style.weight);
                    run["italic"] = json!(style.italic);
                    run["spacing"] = json!(style.spacing);
                }
                run
            }
            // A mesh's per-vertex data would bloat the dump; report its size.
            Primitive::Mesh { vertices, clip } => json!({
                "type": "mesh", "triangles": vertices.len() / 3, "clip": rect_json(*clip),
            }),
            Primitive::Image {
                rect,
                source,
                alpha,
                clip,
            } => json!({
                "type": "image", "rect": rect_json(*rect), "source": source,
                "alpha": alpha, "clip": rect_json(*clip),
            }),
        })
        .collect();
    json!({"ok": true, "bg": color_json(&scene.bg), "primitives": prims})
}

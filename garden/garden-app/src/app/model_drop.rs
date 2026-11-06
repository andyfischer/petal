//! Structural direct manipulation in Petal IDE (declarative dragging, D2 P4).
//!
//! In drag mode a plain press on a traced canvas starts a DM v2 drag, which
//! edits the *numbers* that placed a shape. A press on one of the sketch's
//! own `ui/drag` draggables is different: the sketch already knows what the
//! drag means (which model states it can reach), so drag mode hands it the
//! pointer for the gesture. The sketch previews the drag as it does outside
//! drag mode, and its drop — announced as `{key, model, drop}` — becomes a
//! proposed edit: when the model is a top-level `let` literal of the sketch,
//! the literal is rewritten to read as the drop (a reorder is one removal and
//! one insertion; comments stay), into the paired editor's buffer like any
//! drag-to-edit.
//!
//! `:DragDebug` (Cmd/Ctrl+Shift+G) switches on `ui/drag`'s `drag_debug`
//! overlay in every panel: the target table, the drop-zone map and the spec
//! tree with the chosen branch.

use super::App;

impl App {
    /// A drag-mode press at (`x`, `y`) on traced canvas `idx`: if it lands on
    /// a `ui/drag` draggable, the gesture is the sketch's — remember it and
    /// say so (the caller then routes the press to the panel as usual).
    pub(in crate::app) fn model_drag_press(&mut self, idx: usize, x: f32, y: f32) -> bool {
        let rect = self.panes[idx].rect;
        let Some(panel) = self.panes[idx].panel.as_mut() else {
            return false;
        };
        if !panel.model_drag_at(rect, x, y) {
            return false;
        }
        // Drops from before the gesture (outside drag mode) are not this one.
        panel.take_model_drops();
        self.model_drag = Some(idx);
        self.log_event("dm", "press on a ui/drag draggable: the sketch drags".to_string());
        true
    }

    /// The release of a sketch drag on canvas `idx` reached the sketch: turn
    /// its drop into an edit of the paired editor's buffer.
    pub(in crate::app) fn end_model_drag(&mut self, idx: usize) {
        if self.model_drag != Some(idx) {
            return;
        }
        self.model_drag = None;
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let Some(panel) = self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) else {
            return;
        };
        let drops = panel.take_model_drops();
        let path = super::panes::resolve_script(panel.origin_script(), script_dir.as_deref());
        let Some(drop) = drops.last() else {
            // Cancelled, or released on nothing: nothing to write.
            return;
        };
        let Some(editor) = self.editor_pane_for(&path) else {
            self.status_note = Some("the dropped model has no editor open to write it to".to_string());
            return;
        };
        let source = self.panes[editor].view.buffer.text();
        match garden_script::propose_model_drop(&source, drop) {
            garden_script::ModelDropOutcome::Edits(rewrites) => {
                self.apply_rewrites(editor, &rewrites);
                if let Some(pane) = self.panes.get_mut(editor) {
                    pane.view.buffer.end_undo_run();
                }
                self.log_event("dm", "ui/drag drop written to the literal".to_string());
            }
            garden_script::ModelDropOutcome::Refused(why) => {
                self.status_note = Some(format!("drop not written: {why}"));
            }
        }
        self.needs_redraw = true;
    }

    /// Switch `ui/drag`'s debug overlay on or off in every panel.
    pub(in crate::app) fn set_drag_debug(&mut self, on: bool) {
        self.ui_debug_drag = on;
        for pane in &mut self.panes {
            if let Some(panel) = pane.panel.as_mut() {
                panel.set_ui_debug("drag", on);
            }
        }
        self.status_note = Some(if on {
            "drag debug on: ui/drag shows its targets, drop zones and spec".to_string()
        } else {
            "drag debug off".to_string()
        });
        self.wake_panels();
        let panels: Vec<usize> = (0..self.panes.len()).filter(|&i| self.panes[i].is_panel()).collect();
        for i in panels {
            self.tick_panel_at(i);
        }
        self.needs_redraw = true;
    }

    /// `:DragDebug [on|off]` (no argument toggles).
    pub(in crate::app) fn drag_debug_command(&mut self, arg: &str) {
        match arg.trim() {
            "" | "toggle" => self.set_drag_debug(!self.ui_debug_drag),
            "on" => self.set_drag_debug(true),
            "off" => self.set_drag_debug(false),
            other => self.status_error = Some(format!("E: :DragDebug takes on or off, not {other}")),
        }
    }
}

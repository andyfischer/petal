//! Structural direct manipulation on a panel (declarative dragging, D2 P4).
//!
//! A panel that drags with `ui/drag` announces every drop on the
//! `drag_drops` output channel as `{key, model, drop}`. When the model is a
//! top-level `let` literal of the sketch, the drop can be written back to the
//! source: [`propose_model_drop`] diffs the model against the drop into goal
//! edits ([`petal::drop_edit`]) — a reorder is one removal and one insertion,
//! comments and layout elsewhere untouched — and hands back the change as a
//! [`SourceRewrite`] for the paired editor's buffer, the same currency a DM v2
//! drag pays in.
//!
//! Petal IDE uses it in drag mode: a press on a `ui/drag` draggable
//! ([`PanelHost::model_drag_at`]) goes to the sketch instead of starting a
//! DM v2 drag, the sketch previews the drag as it always does, and the drop
//! ([`PanelHost::take_model_drops`]) becomes the proposed edit. Nothing here
//! needs `config let`.

use super::PanelHost;
use crate::panel_trace::{CodeSpan, SourceRewrite};

/// Drops kept between [`PanelHost::take_model_drops`] calls; older ones go.
const MAX_DROPS: usize = 16;

/// One `ui/drag` drop, as the sketch announced it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelDrop {
    /// The dragged element's place key.
    pub key: serde_json::Value,
    /// The model the drag started from (`drag_session`'s first argument).
    pub model: serde_json::Value,
    /// The model the drop commits.
    pub drop: serde_json::Value,
}

/// What a drop on a literal model resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelDropOutcome {
    /// Apply this, and the literal reads as the drop. (One rewrite: the span
    /// of the source that changed.)
    Edits(Vec<SourceRewrite>),
    /// Why there is no edit (the model is no literal of this file, it is
    /// ambiguous, a value has no literal spelling, nothing changed).
    Refused(String),
}

impl PanelHost {
    /// Move the drops the last frame announced into the queue.
    pub(super) fn collect_model_drops(&mut self) {
        for d in petal_ui::drag::take_drops(&mut self.core.env) {
            self.model_drops.push(ModelDrop {
                key: d["key"].clone(),
                model: d["model"].clone(),
                drop: d["drop"].clone(),
            });
        }
        let extra = self.model_drops.len().saturating_sub(MAX_DROPS);
        self.model_drops.drain(..extra);
    }

    /// The `ui/drag` drops announced since the last call, oldest first.
    pub fn take_model_drops(&mut self) -> Vec<ModelDrop> {
        std::mem::take(&mut self.model_drops)
    }

    /// Whether the topmost target the last frame registered at panel-local
    /// (`x`, `y`) is a `ui/drag` draggable (`draggable(key, …)` registers
    /// `"drag:<key>"`): a press there belongs to the sketch's own drag.
    pub fn model_drag_at(&mut self, x: f64, y: f64) -> bool {
        let targets = petal_ui::hit::targets(&mut self.core.env);
        targets
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.modal && t.contains(x, y))
            .max_by_key(|(i, t)| (t.layer, *i))
            .is_some_and(|(_, t)| t.id.as_deref().is_some_and(|id| id.starts_with("drag:")))
    }

    /// Switch the debug overlay `name` on or off for the sketch
    /// (`ui_debug(name)`; `"drag"` draws `ui/drag`'s `drag_debug`).
    pub fn set_ui_debug(&mut self, name: &str, on: bool) {
        self.core.input.set_debug(name, on);
    }

    /// Whether the debug overlay `name` is on.
    pub fn ui_debug(&self, name: &str) -> bool {
        self.core.input.debug_on(name)
    }
}

/// Turn `drop` into an edit of `source` (the paired editor's text): the one
/// top-level `let` literal the model is, rewritten to read as the drop. The
/// rewrite's span is in `source`'s 0-based line/column coordinates.
pub fn propose_model_drop(source: &str, drop: &ModelDrop) -> ModelDropOutcome {
    let edit = match petal::drop_edit::propose_drop(source, &drop.model, &drop.drop) {
        Ok(e) => e,
        Err(e) => return ModelDropOutcome::Refused(e.message),
    };
    if edit.goals.is_empty() {
        return ModelDropOutcome::Refused("the drop leaves the model as it was".to_string());
    }
    let (span, new_text) = changed_span(source, &edit.source);
    let steps: Vec<String> = edit.goals.iter().map(goal_label).collect();
    ModelDropOutcome::Edits(vec![SourceRewrite {
        span,
        new_text,
        file: None,
        variable: Some(edit.binding.clone()),
        shared: false,
        config: false,
        description: format!("drop into `{}`: {}", edit.binding, steps.join(", ")),
    }])
}

/// The smallest span of `old` that, replaced by the returned text, gives
/// `new`: everything outside the common prefix and suffix.
fn changed_span(old: &str, new: &str) -> (CodeSpan, String) {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suf = a.len().min(b.len()) - pre;
    let suf = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suf)
        .take_while(|(x, y)| x == y)
        .count();
    let (start_line, start_col) = line_col(&a, pre);
    let (end_line, end_col) = line_col(&a, a.len() - suf);
    let text: String = b[pre..b.len() - suf].iter().collect();
    (
        CodeSpan {
            start_line,
            start_col,
            end_line,
            end_col,
        },
        text,
    )
}

fn line_col(chars: &[char], offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for c in &chars[..offset] {
        if *c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn goal_label(g: &petal::goal_based_editing::Goal) -> String {
    use petal::goal_based_editing::Goal;
    match g {
        Goal::ShouldRemove { path } => format!("remove {path}"),
        Goal::ShouldInsert { path, .. } => format!("insert {path}"),
        Goal::ShouldSetPath { path, .. } => format!("set {path}"),
        Goal::ShouldSetValue { name, .. } => format!("set {name}"),
        Goal::ShouldAppend { path, .. } => format!("append to {path}"),
        Goal::ShouldCall { function, .. } => format!("call {function}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changed_span_is_the_difference() {
        let (span, text) = changed_span("let A = [1, 2, 3]\n", "let A = [2, 1, 3]\n");
        assert_eq!(
            (span.start_line, span.start_col, span.end_line, span.end_col),
            (0, 9, 0, 13)
        );
        assert_eq!(text, "2, 1");
        let (span, text) = changed_span("a\nb\nc\n", "a\nc\n");
        assert_eq!((span.start_line, span.start_col, span.end_line, span.end_col), (1, 0, 2, 0));
        assert_eq!(text, "");
    }
}

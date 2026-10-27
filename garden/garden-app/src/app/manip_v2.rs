//! Drag-to-edit, v2: the half of a direct-manipulation drag that Petal's v2
//! engine answers by re-running the frame (see
//! `garden_script::PanelHost::begin_drag_v2` and
//! docs/dev/direct-manipulation-v2-plan.md §4.4).
//!
//! The gesture, as the app sees it:
//!
//! - **Press**: the panel keeps the frame's checkpoint for the drag
//!   (`pin_dm_checkpoint`), and the paired editor's text is snapshotted. Every
//!   span the engine answers with is in that snapshot.
//! - **First move past the slop**: the session opens (the one pause in the
//!   gesture, capped at `garden_script::OPEN_WALL`). The panel's live binding
//!   is held from here to the release, so the program the session measured
//!   stays the one running.
//! - **Every move**: a linear preview (no runs). The editor shows the
//!   predicted numbers (rewritten from the snapshot, so nothing accumulates),
//!   and the canvas paints the grabbed shape moved under the pointer.
//! - **Release**: verify, round and rank. The top alternative is written; the
//!   others stay available to `:DragNext` / `:DragPrev` / `:DragPick N`, and
//!   the status bar lists what else could have moved, which `:DragAllow`
//!   restricts the next drag to (client disambiguation).

use std::path::PathBuf;

use garden_script::{DragAlternative, DragCandidate, DragFilter, DragOutcomeV2, DragSelector, SourceRewrite};

use super::App;

/// Which engine a drag-to-edit gesture uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DmEngine {
    /// v2 for every shape it can grab (anything with a position), v1 only
    /// for what v2 cannot (none of today's shapes). One engine means one
    /// answer shape: ranked alternatives, the candidate report and the
    /// `tunable` / `rigid` hints, none of which v1 has (it reads `rigid` as
    /// "computed" and ignores `tunable`'s bounds). See petal-ide-mode.md,
    /// "Which engine".
    #[default]
    Auto,
    V1,
    V2,
}

impl DmEngine {
    pub(crate) fn name(self) -> &'static str {
        match self {
            DmEngine::Auto => "auto",
            DmEngine::V1 => "v1",
            DmEngine::V2 => "v2",
        }
    }
}

/// The answer to the last v2 drag, kept so its alternatives can be cycled.
pub(crate) struct DragChoices {
    /// The editor pane the sketch is in.
    pub(crate) editor: usize,
    /// The canvas pane the shape was dragged on (where the result panel sits).
    pub(crate) pane: usize,
    /// Each candidate's literal in the editor's *current* text (line, start
    /// col, end col): its press-time span moved through the written
    /// alternative and any hint toggles since. `None` outside the sketch.
    pub(crate) marks: Vec<Option<(usize, usize, usize)>>,
    /// Which candidates the written alternative edited.
    pub(crate) written_marks: Vec<bool>,
    /// A hint was toggled since the drag: the alternatives were solved
    /// without it, so they are shown but no longer written.
    pub(crate) hints_changed: bool,
    /// Why the drag was declined, when it was.
    pub(crate) refused: Option<String>,
    /// Each touched file's text when the drag was pressed: `None` is the
    /// sketch (the editor's buffer), `Some(path)` an imported module.
    pub(crate) snapshots: Vec<(Option<PathBuf>, String)>,
    pub(crate) alternatives: Vec<DragAlternative>,
    pub(crate) candidates: Vec<DragCandidate>,
    /// The alternative now written.
    pub(crate) current: usize,
    /// Which axes the drag moved along (x, y): what "also movable" means.
    pub(crate) axes: (bool, bool),
    /// The sketch's text after writing it: cycling only while the buffer
    /// still says this, so a hand edit since is never clobbered.
    pub(crate) written: String,
}

/// Replace the editor's whole text with `target` as one edit spanning only
/// what differs (common prefix and suffix kept), so the cursor, the undo
/// group and the projection see a single splice.
pub(crate) fn replace_differing(view: &mut crate::editor_view::EditorView, target: &str) {
    let current = view.buffer.text();
    if current == target {
        return;
    }
    let a: Vec<char> = current.chars().collect();
    let b: Vec<char> = target.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let point = |chars: &[char], offset: usize| {
        let before = &chars[..offset];
        let line = before.iter().filter(|c| **c == '\n').count();
        let col = before.iter().rev().take_while(|c| **c != '\n').count();
        garden_core::Point::new(line, col)
    };
    let start = point(&a, prefix);
    let end = point(&a, a.len() - suffix);
    let middle: String = b[prefix..b.len() - suffix].iter().collect();
    view.edit(start, end, &middle);
    view.cursor = view.buffer.clamp(start);
    view.anchor = None;
    view.desired_col = None;
}

/// The rewrites of `alt` for `file` (`None`: the sketch), re-addressed as
/// the sketch's so [`garden_script::apply_rewrites_to`] applies them.
fn rewrites_for(rewrites: &[SourceRewrite], file: &Option<PathBuf>) -> Vec<SourceRewrite> {
    rewrites
        .iter()
        .filter(|r| &r.file == file)
        .map(|r| SourceRewrite {
            file: None,
            ..r.clone()
        })
        .collect()
}

/// What the status bar says about alternative `k` of `n`.
fn describe(alt: &DragAlternative, k: usize, n: usize) -> String {
    let mut s = alt
        .rewrites
        .iter()
        .map(|r| r.description.replace('`', ""))
        .collect::<Vec<_>>()
        .join(", ");
    if !alt.verified {
        s = format!("unverified: {s}");
    }
    if alt.moves_others > 0 {
        s.push_str(&format!(
            " — moves {} other shape{}",
            alt.moves_others,
            if alt.moves_others == 1 { "" } else { "s" }
        ));
    }
    if alt.stateful {
        s.push_str(" — feeds `state`");
    }
    if n > 1 {
        s.push_str(&format!(" [{} of {n}: :DragNext]", k + 1));
    }
    s
}

/// The candidates worth naming to the user: eligible, and measured to move
/// the shape along an axis the drag moved it on (`axes`: x, y).
fn movable(candidates: &[DragCandidate], axes: (bool, bool)) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let useful = |c: &&DragCandidate| c.eligible && ((axes.0 && c.moves_x) || (axes.1 && c.moves_y));
    for c in candidates.iter().filter(useful) {
        let label = c.selector.label();
        if !out.contains(&label) {
            out.push(label);
        }
    }
    out
}

impl App {
    /// Write a v2 drag's previewed numbers into the sketch's buffer, from the
    /// press-time snapshot `s0`.
    pub(in crate::app) fn write_preview_v2(&mut self, editor: usize, s0: &str, rewrites: &[SourceRewrite]) {
        let Ok(text) = garden_script::apply_rewrites_to(s0, rewrites) else {
            return;
        };
        if let Some(pane) = self.panes.get_mut(editor) {
            replace_differing(&mut pane.view, &text);
        }
        let preview = rewrites
            .iter()
            .filter(|r| r.file.is_none())
            .map(|r| r.description.replace('`', ""))
            .collect::<Vec<_>>();
        if !preview.is_empty() {
            self.status_note = Some(format!("{} (preview)", preview.join(", ")));
        }
    }

    /// The release of a v2 drag: write the top alternative (or put the text
    /// back) and keep the answer for cycling.
    pub(in crate::app) fn settle_drag_v2(
        &mut self,
        editor: usize,
        pane: usize,
        s0: String,
        outcome: DragOutcomeV2,
        (dx, dy): (f64, f64),
    ) {
        self.dm_gen += 1;
        let axes = (dx.round() != 0.0, dy.round() != 0.0);
        match outcome {
            DragOutcomeV2::Alternatives {
                alternatives,
                candidates,
            } => {
                // Every file any alternative touches, as it stood at the press.
                let mut snapshots: Vec<(Option<PathBuf>, String)> = vec![(None, s0)];
                for alt in &alternatives {
                    for r in &alt.rewrites {
                        if snapshots.iter().any(|(f, _)| f == &r.file) {
                            continue;
                        }
                        if let Some(path) = &r.file {
                            if let Some(text) = self.file_text(path) {
                                snapshots.push((r.file.clone(), text));
                            }
                        }
                    }
                }
                let open = self.dm_mode || alternatives.len() > 1;
                let mut choices = DragChoices {
                    editor,
                    pane,
                    marks: Vec::new(),
                    written_marks: Vec::new(),
                    hints_changed: false,
                    refused: None,
                    snapshots,
                    alternatives,
                    candidates,
                    current: 0,
                    axes,
                    written: String::new(),
                };
                self.write_choice(&mut choices, 0);
                self.dm_choices = Some(choices);
                self.dm_panel_open = open;
            }
            DragOutcomeV2::Refused { why, candidates } => {
                if let Some(pane) = self.panes.get_mut(editor) {
                    replace_differing(&mut pane.view, &s0);
                }
                let others = movable(&candidates, axes);
                self.status_note = Some(if others.is_empty() {
                    why.clone()
                } else {
                    format!("{why} — could move: {}", others.join(", "))
                });
                let marks = candidates
                    .iter()
                    .map(|c| c.span.map(|s| (s.start_line, s.start_col, s.end_col)))
                    .collect();
                let written_marks = vec![false; candidates.len()];
                self.dm_panel_open = true;
                self.dm_choices = Some(DragChoices {
                    editor,
                    pane,
                    marks,
                    written_marks,
                    hints_changed: false,
                    refused: Some(why),
                    written: s0.clone(),
                    snapshots: vec![(None, s0)],
                    alternatives: Vec::new(),
                    candidates,
                    current: 0,
                    axes,
                });
            }
            DragOutcomeV2::Stale => {
                if let Some(pane) = self.panes.get_mut(editor) {
                    replace_differing(&mut pane.view, &s0);
                }
            }
        }
    }

    /// The current text of `path`: an open editor's buffer, else the disk.
    fn file_text(&self, path: &std::path::Path) -> Option<String> {
        match self.editor_on(path) {
            Some(i) => Some(self.panes[i].view.buffer.text()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    fn editor_on(&self, path: &std::path::Path) -> Option<usize> {
        let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let want = canon(path);
        self.panes.iter().position(|p| {
            !p.is_panel() && p.file.as_ref().is_some_and(|f| canon(std::path::Path::new(f)) == want)
        })
    }

    /// Write alternative `k` of `choices`, from the snapshots: the sketch into
    /// its buffer, an imported module into its editor if one is open, else to
    /// its file.
    fn write_choice(&mut self, choices: &mut DragChoices, k: usize) {
        let Some(alt) = choices.alternatives.get(k) else {
            return;
        };
        for (file, s0) in &choices.snapshots {
            let Ok(text) = garden_script::apply_rewrites_to(s0, &rewrites_for(&alt.rewrites, file)) else {
                continue;
            };
            match file {
                None => {
                    if let Some(pane) = self.panes.get_mut(choices.editor) {
                        replace_differing(&mut pane.view, &text);
                    }
                    choices.written = text;
                }
                Some(path) => match self.editor_on(path) {
                    Some(i) => replace_differing(&mut self.panes[i].view, &text),
                    None => {
                        if let Err(e) = std::fs::write(path, &text) {
                            self.status_error = Some(format!("E: could not write {}: {e}", path.display()));
                        }
                    }
                },
            }
        }
        choices.current = k;
        // Where each candidate's literal is in the text just written.
        let sketch: Vec<&SourceRewrite> = alt.rewrites.iter().filter(|r| r.file.is_none()).collect();
        choices.written_marks = choices
            .candidates
            .iter()
            .map(|c| {
                c.span.is_some_and(|s| {
                    sketch
                        .iter()
                        .any(|r| r.span.start_line == s.start_line && r.span.start_col == s.start_col)
                })
            })
            .collect();
        choices.marks = choices
            .candidates
            .iter()
            .map(|c| {
                let s = c.span?;
                let edits: Vec<garden_script::dm_hints::LineEdit> = sketch
                    .iter()
                    .filter(|r| r.span.start_line == s.start_line && r.span.end_line == s.start_line)
                    .map(|r| garden_script::dm_hints::LineEdit {
                        start: r.span.start_col,
                        end: r.span.end_col,
                        text: r.new_text.clone(),
                    })
                    .collect();
                let (a, b) = garden_script::dm_hints::map_range((s.start_col, s.end_col), &edits);
                Some((s.start_line, a, b))
            })
            .collect();
        self.dm_gen += 1;
        let mut note = describe(alt, k, choices.alternatives.len());
        let used: Vec<String> = alt
            .rewrites
            .iter()
            .filter_map(|r| r.variable.clone())
            .collect();
        let others: Vec<String> = movable(&choices.candidates, choices.axes)
            .into_iter()
            .filter(|l| !used.contains(l))
            .collect();
        if !others.is_empty() {
            note.push_str(&format!(" — also movable: {} (:DragAllow)", others.join(", ")));
        }
        self.status_note = Some(note);
        self.log_event("mouse", format!("drag-to-edit v2: alternative {}", k + 1));
    }

    /// `:DragNext` / `:DragPrev` / `:DragPick N`: write another alternative of
    /// the last v2 drag. `step` is relative, `pick` absolute (0-based).
    pub(in crate::app) fn cycle_drag_alternative(&mut self, step: isize, pick: Option<usize>) {
        let Some(mut choices) = self.dm_choices.take() else {
            self.status_error = Some("E: no drag to choose an alternative of".to_string());
            return;
        };
        let n = choices.alternatives.len();
        let unchanged = self
            .panes
            .get(choices.editor)
            .is_some_and(|p| p.view.buffer.text() == choices.written);
        if n == 0 {
            self.status_error = Some("E: the last drag has no alternatives".to_string());
            self.dm_choices = Some(choices);
            return;
        }
        if !unchanged {
            // The code was edited since: the snapshots no longer describe it,
            // so the answer is forgotten rather than written over the edit.
            self.status_error = Some("E: the code changed since the drag".to_string());
            self.dm_panel_open = false;
            self.dm_gen += 1;
            return;
        }
        if choices.hints_changed {
            // The alternatives were solved without the hints marked since;
            // writing one would also undo those marks (it is written from the
            // press-time text).
            self.status_error =
                Some("E: hints changed since the drag: drag again to re-solve with them".to_string());
            self.dm_choices = Some(choices);
            return;
        }
        let k = match pick {
            Some(k) if k < n => k,
            Some(_) => {
                self.status_error = Some(format!("E: the drag has {n} alternatives"));
                self.dm_choices = Some(choices);
                return;
            }
            None => (choices.current as isize + step).rem_euclid(n as isize) as usize,
        };
        if let Some(pane) = self.panes.get_mut(choices.editor) {
            pane.view.buffer.begin_undo_group();
        }
        self.write_choice(&mut choices, k);
        if let Some(pane) = self.panes.get_mut(choices.editor) {
            pane.view.buffer.end_undo_run();
        }
        self.dm_choices = Some(choices);
        self.needs_redraw = true;
    }

    /// `:DragAllow sel…` / `:DragDeny sel…` (empty clears): which literals
    /// the next v2 drags may move.
    pub(in crate::app) fn set_drag_filter(&mut self, allow: bool, args: &str) {
        let mut parsed = Vec::new();
        for word in args.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()) {
            match DragSelector::parse(word) {
                Some(s) => parsed.push(s),
                None => {
                    self.status_error = Some(format!(
                        "E: not a selector: {word} (a name, line:N, fn:NAME or config)"
                    ));
                    return;
                }
            }
        }
        if allow {
            self.dm_filter.allow = parsed;
        } else {
            self.dm_filter.deny = parsed;
        }
        self.dm_gen += 1;
        self.status_note = Some(describe_filter(&self.dm_filter));
    }

    /// `:DragEngine auto|v1|v2`.
    pub(in crate::app) fn set_drag_engine(&mut self, arg: &str) {
        self.dm_engine = match arg.trim() {
            "" => {
                self.status_note = Some(format!("drag engine: {}", self.dm_engine.name()));
                return;
            }
            "auto" => DmEngine::Auto,
            "v1" => DmEngine::V1,
            "v2" => DmEngine::V2,
            other => {
                self.status_error = Some(format!("E: not an engine: {other} (auto, v1 or v2)"));
                return;
            }
        };
        self.status_note = Some(format!("drag engine: {}", self.dm_engine.name()));
    }

    /// The `/state` view of the drag settings and the last v2 answer.
    pub(in crate::app) fn dm_state_json(&self) -> serde_json::Value {
        use serde_json::json;
        let span = |s: &garden_script::CodeSpan| {
            json!({
                "start": {"line": s.start_line, "col": s.start_col},
                "end": {"line": s.end_line, "col": s.end_col},
            })
        };
        let mut out = json!({
            "engine": self.dm_engine.name(),
            "allow": self.dm_filter.allow.iter().map(DragSelector::label).collect::<Vec<_>>(),
            "deny": self.dm_filter.deny.iter().map(DragSelector::label).collect::<Vec<_>>(),
            "active": self.manip_is_v2(),
            "last": self.dm_choices.as_ref().map(|c| json!({
                "current": c.current,
                "alternatives": c.alternatives.iter().map(|a| json!({
                    "description": a.description,
                    "verified": a.verified,
                    "score": a.score,
                    "moves_others": a.moves_others,
                    "config_only": a.config_only,
                    "stateful": a.stateful,
                    "rewrites": a.rewrites.iter().map(|r| json!({
                        "span": span(&r.span),
                        "new_text": r.new_text,
                        "file": r.file.as_ref().map(|f| f.display().to_string()),
                        "variable": r.variable,
                        "config": r.config,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "candidates": c.candidates.iter().enumerate().map(|(i, k)| json!({
                    "label": k.label,
                    "state": self.cand_state(c, i).name(),
                    "tunable": k.tunable,
                    "rigid": k.rigid,
                    "mark": c.marks.get(i).copied().flatten().map(|(l, s, e)| json!({"line": l, "start": s, "end": e})),
                    "selector": k.selector.label(),
                    "value": k.value,
                    "config": k.config,
                    "eligible": k.eligible,
                    "why_not": k.why_not,
                    "moves": k.moves,
                    "moves_x": k.moves_x,
                    "moves_y": k.moves_y,
                    "span": k.span.as_ref().map(span),
                    "file": k.file.as_ref().map(|f| f.display().to_string()),
                })).collect::<Vec<_>>(),
            })),
        });
        if let (Some(o), serde_json::Value::Object(extra)) = (out.as_object_mut(), self.dm_mode_json()) {
            o.extend(extra);
        }
        out
    }
}

/// The status line for a filter change.
pub(in crate::app) fn describe_filter(f: &DragFilter) -> String {
    if f.is_default() {
        return "drags may move any literal (preferring config)".to_string();
    }
    let mut parts = Vec::new();
    if !f.allow.is_empty() {
        parts.push(format!(
            "drags move only {}",
            f.allow.iter().map(DragSelector::label).collect::<Vec<_>>().join(", ")
        ));
    }
    if !f.deny.is_empty() {
        parts.push(format!(
            "never {}",
            f.deny.iter().map(DragSelector::label).collect::<Vec<_>>().join(", ")
        ));
    }
    parts.join("; ")
}

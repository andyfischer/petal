//! Petal IDE's **drag mode**: direct manipulation without a modifier, and the
//! first-draft UI around a solved drag.
//!
//! - **The mode** (`:DragMode`, the toolbar's Drag button, Cmd/Ctrl+Shift+D).
//!   While it is on, a plain press on a traced canvas is a drag-to-edit
//!   gesture (what Cmd-press is outside it), and the sketch's own mouse is
//!   suspended: its frames see the pointer off the canvas, and no press,
//!   wheel or right click reaches it. Keys still do.
//! - **Affordances.** The shape under the pointer is outlined and labelled
//!   with the call that drew it; the pointer becomes a grab hand; a badge on
//!   the canvas says the mode is on.
//! - **The result panel.** After a solved drag (always in drag mode, and
//!   after a Cmd-drag with several answers), a card over the canvas lists the
//!   ranked alternatives (click one to write it, through `:DragPick`'s
//!   machinery) and the numbers behind the shape with their state, plus the
//!   session's `:DragAllow` / `:DragDeny` selectors as clickable chips.
//! - **Hints by mouse.** In drag mode the editor underlines the drag's
//!   candidate numbers (and every `tunable` / `rigid` literal) in their
//!   state's color, and a gutter column marks the lines they are on. Clicking
//!   an underlined number (Alt-click: any number), its state chip in the
//!   panel, or a gutter marker cycles free → tunable → rigid → free by
//!   rewriting the source with the real hint builtins (`dm_hints`), as one
//!   undo step. The canvas recompiles from it like any edit, so the next
//!   drag's solver reads the hint.

use std::cmp::Reverse;
use std::hash::{Hash, Hasher};

use garden_core::Point;
use garden_render::{Color, Primitive, Rect, TextStyle, FONT_SIZE};
use garden_script::DragSelector;

use crate::dm_hints::{self, HintState, LineEdit, Literal};
use crate::editor_view::{DmGutterMark, DmUnderline};

use super::manip_v2::{describe_filter, DragChoices};
use super::App;

/// Where a suspended sketch is told the pointer is: far off its canvas.
pub(in crate::app) const OFF_CANVAS: i32 = -100_000;

/// Candidate rows the result panel lists before it says "and N more".
const MAX_CANDIDATE_ROWS: usize = 10;

/// The pointer's shape, for the windowed frontend to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerCursor {
    Default,
    /// Over a shape drag mode would grab.
    Grab,
    /// Pulling one.
    Grabbing,
    /// Over something clickable in the drag UI.
    Pointer,
}

impl PointerCursor {
    pub fn name(self) -> &'static str {
        match self {
            PointerCursor::Default => "default",
            PointerCursor::Grab => "grab",
            PointerCursor::Grabbing => "grabbing",
            PointerCursor::Pointer => "pointer",
        }
    }
}

/// What a number behind a dragged shape is, as the UI shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum CandState {
    /// Movable, unhinted.
    Free,
    /// `tunable(…)`: preferred.
    Tunable,
    /// `rigid(…)`: never moved.
    Rigid,
    /// A `config let` knob: preferred.
    Config,
    /// Not editable for another reason (library code, a loop bound, denied…).
    Fixed,
}

impl CandState {
    pub(in crate::app) fn name(self) -> &'static str {
        match self {
            CandState::Free => "free",
            CandState::Tunable => "tunable",
            CandState::Rigid => "rigid",
            CandState::Config => "config",
            CandState::Fixed => "fixed",
        }
    }

    pub(in crate::app) fn color(self) -> Color {
        match self {
            CandState::Free => Color::rgb(0.95, 0.74, 0.30),
            CandState::Tunable => Color::rgb(0.36, 0.82, 0.48),
            CandState::Rigid => Color::rgb(0.93, 0.38, 0.38),
            CandState::Config => Color::rgb(0.32, 0.74, 0.92),
            CandState::Fixed => Color::rgb(0.52, 0.54, 0.60),
        }
    }

    fn of_hint(h: HintState) -> CandState {
        match h {
            HintState::Free => CandState::Free,
            HintState::Tunable => CandState::Tunable,
            HintState::Rigid => CandState::Rigid,
        }
    }
}

/// The accent drag mode paints its chrome in.
fn accent() -> Color {
    Color::rgb(0.98, 0.62, 0.20)
}

/// Something a click on the result panel does.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::app) enum DmAction {
    Close,
    /// Write alternative `k` (0-based).
    Pick(usize),
    /// Cycle candidate `i`'s hint.
    CycleHint(usize),
    /// Toggle candidate `i`'s selector in the session allow / deny list.
    ToggleAllow(usize),
    ToggleDeny(usize),
    ClearFilters,
}

impl DmAction {
    /// The `/state` spelling.
    fn label(&self) -> String {
        match self {
            DmAction::Close => "close".into(),
            DmAction::Pick(k) => format!("pick:{}", k + 1),
            DmAction::CycleHint(i) => format!("hint:{i}"),
            DmAction::ToggleAllow(i) => format!("only:{i}"),
            DmAction::ToggleDeny(i) => format!("never:{i}"),
            DmAction::ClearFilters => "clear".into(),
        }
    }
}

/// One laid-out piece of the result panel.
pub(in crate::app) struct DmCell {
    pub rect: Rect,
    pub text: String,
    pub color: Color,
    pub bg: Option<Color>,
    pub border: Option<Color>,
    pub action: Option<DmAction>,
}

/// The result panel, laid out: shared by drawing and hit-testing so the two
/// cannot drift (the toolbar's rule).
pub(in crate::app) struct DmOverlay {
    pub card: Rect,
    pub cells: Vec<DmCell>,
}

/// The shape drag mode would grab (or is pulling), in window coordinates.
pub(in crate::app) struct DmHover {
    pub pane: usize,
    pub index: usize,
    pub rect: Rect,
    pub label: String,
}

/// A run of text in a panel row, before layout.
struct Seg {
    text: String,
    color: Color,
    /// Drawn as a chip on this background.
    chip: Option<Color>,
    border: bool,
    action: Option<DmAction>,
}

impl Seg {
    fn text(text: impl Into<String>, color: Color) -> Seg {
        Seg {
            text: text.into(),
            color,
            chip: None,
            border: false,
            action: None,
        }
    }

    fn chip(text: impl Into<String>, fg: Color, bg: Option<Color>, action: Option<DmAction>) -> Seg {
        Seg {
            text: text.into(),
            color: fg,
            border: bg.is_none(),
            chip: Some(bg.unwrap_or(Color::rgba(0.0, 0.0, 0.0, 0.0))),
            action,
        }
    }
}

/// One panel row: left-flowing and right-aligned segments, and what a click
/// anywhere else on the row does.
struct Row {
    left: Vec<Seg>,
    right: Vec<Seg>,
    action: Option<DmAction>,
    bg: Option<Color>,
}

impl Row {
    fn new(left: Vec<Seg>) -> Row {
        Row {
            left,
            right: Vec::new(),
            action: None,
            bg: None,
        }
    }
}

/// A number as the panel shows it: an integer without its `.0`.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The literal on `line` that a candidate's span (starting at `col`) is.
fn literal_on(line: &str, col: usize) -> Option<Literal> {
    dm_hints::literals(line)
        .into_iter()
        .find(|l| !l.bound && l.start <= col && col < l.end.max(l.start + 1))
}

impl App {
    /// Turn drag mode on or off.
    pub(in crate::app) fn set_dm_mode(&mut self, on: bool) {
        self.dm_mode = on;
        self.dm_gen += 1;
        if !on {
            self.dm_panel_open = false;
        }
        self.status_note = Some(if on {
            "drag mode: drag a shape to edit its code; click a number to mark it tunable / rigid"
                .to_string()
        } else {
            "drag mode off: Cmd-drag still edits, plain clicks go to the sketch".to_string()
        });
        self.log_event("dm", format!("drag mode {}", if on { "on" } else { "off" }));
        // The suspended (or restored) pointer reaches the sketch next frame.
        self.wake_panels();
        self.needs_redraw = true;
    }

    /// `:DragMode [on|off]` (no argument toggles).
    pub(in crate::app) fn dm_mode_command(&mut self, arg: &str) {
        match arg.trim() {
            "" | "toggle" => self.set_dm_mode(!self.dm_mode),
            "on" => self.set_dm_mode(true),
            "off" => self.set_dm_mode(false),
            other => self.status_error = Some(format!("E: :DragMode takes on or off, not {other}")),
        }
    }

    /// Whether a traced canvas (one paired with an editor) is open: what the
    /// Cmd+Shift+D chord needs before it means anything.
    pub(in crate::app) fn has_traced_panel(&self) -> bool {
        self.panes
            .iter()
            .any(|p| p.panel.as_ref().is_some_and(|panel| panel.traces_origins()))
    }

    /// The traced canvas pane under (`x`, `y`).
    fn traced_panel_at(&self, x: f32, y: f32) -> Option<usize> {
        let idx = self.pane_at(x, y)?;
        self.panes[idx]
            .panel
            .as_ref()
            .filter(|p| p.traces_origins())
            .map(|_| idx)
    }

    /// The shape drag mode outlines: the one being pulled, else (in drag
    /// mode, with nothing else under the pointer) the one a press would grab.
    pub(in crate::app) fn dm_hover(&self) -> Option<DmHover> {
        let (pane, index) = match self.manip_target() {
            Some((pane, index, _)) => (pane, index),
            None => {
                if !self.dm_mode || self.drag.is_some() {
                    return None;
                }
                let (mx, my) = self.mouse;
                if self.dm_overlay().is_some_and(|o| o.card.contains(mx, my)) {
                    return None;
                }
                let pane = self.traced_panel_at(mx, my)?;
                let p = &self.panes[pane];
                let panel = p.panel.as_ref()?;
                if panel.source_drifted() {
                    return None;
                }
                let index = panel
                    .drag_target_v2_at(p.rect, mx, my)
                    .or_else(|| panel.drag_target_at(p.rect, mx, my).map(|(i, _)| i))?;
                (pane, index)
            }
        };
        let p = self.panes.get(pane)?;
        let panel = p.panel.as_ref()?;
        let (x, y, w, h) = panel.dm_bounds(index)?;
        let rect = Rect {
            x: p.rect.x + x as f32 - 3.0,
            y: p.rect.y + y as f32 - 3.0,
            w: w as f32 + 6.0,
            h: h as f32 + 6.0,
        };
        let label = match panel.trace_of(index) {
            Some(t) => format!(
                "{}{}",
                t.callee.as_deref().unwrap_or("shape"),
                t.call.map(|c| format!(" · line {}", c.start_line + 1)).unwrap_or_default()
            ),
            None => "shape".to_string(),
        };
        Some(DmHover {
            pane,
            index,
            rect,
            label,
        })
    }

    /// What a candidate of the last drag is now, reading its literal in the
    /// editor's current text (a hint toggled since the drag shows at once).
    pub(in crate::app) fn cand_state(&self, c: &DragChoices, i: usize) -> CandState {
        let cand = &c.candidates[i];
        let text_state = c.marks.get(i).copied().flatten().and_then(|(line, s, _)| {
            let view = &self.panes.get(c.editor)?.view;
            (line < view.buffer.line_count())
                .then(|| literal_on(&view.buffer.line(line), s))
                .flatten()
                .map(|l| l.state())
        });
        match text_state {
            Some(HintState::Tunable) => CandState::Tunable,
            Some(HintState::Rigid) => CandState::Rigid,
            // Unmarked since the drag measured it.
            Some(HintState::Free) if cand.rigid || cand.tunable => CandState::Free,
            None if cand.rigid => CandState::Rigid,
            None if cand.tunable => CandState::Tunable,
            _ if !cand.eligible => CandState::Fixed,
            _ if cand.config => CandState::Config,
            _ => CandState::Free,
        }
    }

    /// The candidates the panel lists (in the sketch, or movable elsewhere),
    /// best first, and how many were left out: (listed, more, in libraries).
    pub(in crate::app) fn listed_candidates(c: &DragChoices) -> (Vec<usize>, usize, usize) {
        let library = |i: &usize| {
            let k = &c.candidates[*i];
            !k.eligible && k.why_not.as_deref().is_some_and(|w| w.contains("library"))
        };
        let mut all: Vec<usize> = (0..c.candidates.len()).collect();
        let lib = all.iter().filter(|i| library(i)).count();
        all.retain(|i| !library(i));
        all.sort_by_key(|&i| {
            let k = &c.candidates[i];
            (!(k.eligible && k.moves), !k.moves, !c.written_marks.get(i).copied().unwrap_or(false), i)
        });
        let more = all.len().saturating_sub(MAX_CANDIDATE_ROWS);
        all.truncate(MAX_CANDIDATE_ROWS);
        (all, more, lib)
    }

    /// The result panel, laid out over its canvas; `None` while closed.
    pub(in crate::app) fn dm_overlay(&self) -> Option<DmOverlay> {
        if !self.dm_panel_open {
            return None;
        }
        let c = self.dm_choices.as_ref()?;
        let pane = self.panes.get(c.pane).filter(|p| p.is_panel())?;
        let theme = &self.theme;
        let (cw, ch) = self.viewport.cell;
        let dim = theme.text_dim;
        let mut rows: Vec<Row> = Vec::new();

        let n = c.alternatives.len();
        let mut head = Row::new(vec![
            Seg::text("Drag result", theme.text),
            Seg::text(
                match (n, &c.refused) {
                    (_, Some(_)) => "declined".to_string(),
                    (1, _) => "1 answer".to_string(),
                    (n, _) => format!("{n} alternatives"),
                },
                dim,
            ),
        ]);
        head.right = vec![Seg::chip("close", dim, None, Some(DmAction::Close))];
        rows.push(head);
        if let Some(why) = &c.refused {
            rows.push(Row::new(vec![Seg::text(why.clone(), theme.error_text)]));
        }
        if n > 0 {
            rows.push(Row::new(vec![if c.hints_changed {
                Seg::text("Alternatives: hints changed since, drag again to re-solve", accent())
            } else {
                Seg::text("Alternatives: click one to write it instead", dim)
            }]));
        }
        for (k, alt) in c.alternatives.iter().enumerate() {
            let desc = alt
                .rewrites
                .iter()
                .map(|r| r.description.replace('`', ""))
                .collect::<Vec<_>>()
                .join(", ");
            let current = k == c.current;
            let fg = if c.hints_changed { dim } else { theme.text };
            let mut row = Row::new(vec![
                Seg::text(if current { ">" } else { " " }, accent()),
                Seg::text(format!("{}", k + 1), dim),
                Seg::text(desc, fg),
            ]);
            let others = match alt.moves_others {
                0 => "only this shape".to_string(),
                1 => "moves 1 other".to_string(),
                m => format!("moves {m} others"),
            };
            row.right = vec![
                Seg::text(others, dim),
                if alt.verified {
                    Seg::chip("verified", Color::rgb(0.06, 0.08, 0.06), Some(CandState::Tunable.color()), None)
                } else {
                    Seg::chip("unverified", Color::rgb(0.08, 0.06, 0.02), Some(CandState::Free.color()), None)
                },
            ];
            if !c.hints_changed {
                row.action = Some(DmAction::Pick(k));
            }
            if current {
                row.bg = Some(theme.selection);
            }
            rows.push(row);
        }

        let (listed, more, lib) = Self::listed_candidates(c);
        if !listed.is_empty() {
            rows.push(Row::new(vec![Seg::text(
                "Numbers behind it: click a state to cycle free > tunable > rigid",
                dim,
            )]));
        }
        for &i in &listed {
            let k = &c.candidates[i];
            let state = self.cand_state(c, i);
            let mark = c.marks.get(i).copied().flatten();
            // The number as the code says it now (the drag may have written it).
            let now = mark.and_then(|(line, s, _)| {
                let view = &self.panes.get(c.editor)?.view;
                (line < view.buffer.line_count())
                    .then(|| literal_on(&view.buffer.line(line), s))
                    .flatten()
                    .map(|l| l.text)
            });
            let value = now.unwrap_or_else(|| fmt_num(k.value));
            let label = match &k.variable {
                Some(v) => format!("{v} = {value}"),
                None => value,
            };
            let place = match (mark, &k.file) {
                (Some((line, _, _)), _) => format!("line {}", line + 1),
                (None, Some(f)) => f
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                (None, None) => String::new(),
            };
            let axes = match (k.moves_x, k.moves_y) {
                (true, true) => "moves x y",
                (true, false) => "moves x",
                (false, true) => "moves y",
                // Measured and found not to move the shape, or never measured.
                _ if k.eligible => "no effect",
                _ => "",
            };
            let mut left = vec![
                Seg::chip(
                    state.name(),
                    Color::rgb(0.05, 0.05, 0.06),
                    Some(state.color()),
                    mark.map(|_| DmAction::CycleHint(i)),
                ),
                Seg::text(label, theme.text),
                Seg::text(place, dim),
            ];
            if c.written_marks.get(i).copied().unwrap_or(false) {
                left.push(Seg::text("written", accent()));
            }
            if !axes.is_empty() {
                left.push(Seg::text(axes, dim));
            }
            if let (CandState::Fixed, Some(why)) = (state, &k.why_not) {
                left.push(Seg::text(why.clone(), dim));
            }
            let mut row = Row::new(left);
            let lit = |on: bool| on.then_some(theme.selection);
            row.right = vec![
                Seg::chip(
                    "only",
                    theme.text,
                    lit(self.dm_filter.allow.contains(&k.selector)),
                    Some(DmAction::ToggleAllow(i)),
                ),
                Seg::chip(
                    "never",
                    theme.text,
                    lit(self.dm_filter.deny.contains(&k.selector)),
                    Some(DmAction::ToggleDeny(i)),
                ),
            ];
            rows.push(row);
        }
        if more > 0 || lib > 0 {
            let mut parts = Vec::new();
            if more > 0 {
                parts.push(format!("{more} more not shown"));
            }
            if lib > 0 {
                parts.push(format!("{lib} in library code (never edited)"));
            }
            rows.push(Row::new(vec![Seg::text(parts.join("; "), dim)]));
        }
        let mut session = Row::new(vec![
            Seg::text("Session:", dim),
            Seg::text(describe_filter(&self.dm_filter), theme.text),
        ]);
        if !self.dm_filter.is_default() {
            session.right = vec![Seg::chip("clear", theme.text, None, Some(DmAction::ClearFilters))];
        }
        rows.push(session);

        // Layout: a card along the canvas's bottom edge.
        let pad = 8.0;
        let row_h = ch + 8.0;
        let w = (pane.rect.w - 16.0).min(78.0 * cw).max(120.0);
        let h = rows.len() as f32 * row_h + 2.0 * pad;
        let card = Rect {
            x: pane.rect.x + 8.0,
            y: (pane.rect.y + pane.rect.h - 8.0 - h).max(pane.rect.y + 4.0),
            w,
            h,
        };
        let seg_w = |s: &Seg| s.text.chars().count() as f32 * cw + if s.chip.is_some() { cw } else { 0.0 };
        let mut cells = Vec::new();
        for (r, row) in rows.into_iter().enumerate() {
            let y = card.y + pad + r as f32 * row_h;
            if row.action.is_some() || row.bg.is_some() {
                cells.push(DmCell {
                    rect: Rect {
                        x: card.x + 3.0,
                        y,
                        w: card.w - 6.0,
                        h: row_h,
                    },
                    text: String::new(),
                    color: theme.text,
                    bg: row.bg,
                    border: None,
                    action: row.action,
                });
            }
            let place = |s: Seg, x: f32, max_w: f32, cells: &mut Vec<DmCell>| {
                let mut text = s.text;
                let budget = ((max_w - if s.chip.is_some() { cw } else { 0.0 }) / cw).floor().max(0.0) as usize;
                if text.chars().count() > budget {
                    text = text.chars().take(budget.saturating_sub(1)).collect::<String>() + "…";
                }
                let width = text.chars().count() as f32 * cw + if s.chip.is_some() { cw } else { 0.0 };
                cells.push(DmCell {
                    rect: Rect {
                        x,
                        y: y + 2.0,
                        w: width,
                        h: row_h - 4.0,
                    },
                    text,
                    color: s.color,
                    bg: s.chip.filter(|c| c.a > 0.0),
                    border: s.border.then_some(theme.border),
                    action: s.action,
                });
                width
            };
            let mut right_x = card.x + card.w - pad;
            for s in row.right.into_iter().rev() {
                let width = seg_w(&s);
                right_x -= width;
                place(s, right_x, width, &mut cells);
                right_x -= cw;
            }
            let mut x = card.x + pad;
            for s in row.left {
                let room = right_x - x;
                if room < cw * 2.0 {
                    break;
                }
                let width = place(s, x, room, &mut cells);
                x += width + cw;
            }
        }
        Some(DmOverlay { card, cells })
    }

    /// Draw drag mode's chrome over the panes: the mode badge, the hover
    /// outline and the result panel.
    pub(in crate::app) fn build_dm_chrome(&self, prims: &mut Vec<Primitive>) {
        let (cw, ch) = self.viewport.cell;
        let text = |prims: &mut Vec<Primitive>, pos: (f32, f32), s: &str, color: Color, clip: Rect| {
            prims.push(Primitive::Text {
                pos,
                text: s.to_string(),
                color,
                clip,
                size: FONT_SIZE,
                style: TextStyle::default(),
            });
        };
        if self.dm_mode {
            for p in self.panes.iter().filter(|p| p.panel.as_ref().is_some_and(|x| x.traces_origins())) {
                let label = "DRAG MODE";
                let w = (label.len() as f32 + 2.0) * cw;
                let badge = Rect {
                    x: p.rect.x + p.rect.w - w - 8.0,
                    y: p.rect.y + 8.0,
                    w,
                    h: ch + 6.0,
                };
                prims.push(Primitive::Quad {
                    rect: badge,
                    color: accent(),
                });
                text(prims, (badge.x + cw, badge.y + 3.0), label, Color::rgb(0.08, 0.06, 0.03), p.rect);
            }
        }
        if let Some(h) = self.dm_hover() {
            let clip = self.panes[h.pane].rect;
            let r = h.rect;
            let t = 2.0;
            for e in [
                Rect { h: t, ..r },
                Rect { y: r.y + r.h - t, h: t, ..r },
                Rect { w: t, ..r },
                Rect { x: r.x + r.w - t, w: t, ..r },
            ] {
                push_clipped(prims, e, clip, accent());
            }
            if self.manip_target().is_none() {
                let label = format!("drag to edit: {}", h.label);
                let w = (label.chars().count() as f32 + 1.0) * cw;
                let above = r.y - ch - 6.0;
                let y = if above >= clip.y { above } else { r.y + r.h + 2.0 };
                let tag = Rect {
                    // Kept inside the canvas: shifted left near its right edge.
                    x: r.x.min(clip.x + clip.w - w - 4.0).max(clip.x),
                    y,
                    w,
                    h: ch + 4.0,
                };
                push_clipped(prims, tag, clip, Color::rgba(0.05, 0.06, 0.08, 0.92));
                text(prims, (tag.x + cw / 2.0, tag.y + 2.0), &label, accent(), clip);
            }
        }
        if let Some(o) = self.dm_overlay() {
            let card = o.card;
            prims.push(Primitive::Quad {
                rect: Rect {
                    x: card.x - 1.0,
                    y: card.y - 1.0,
                    w: card.w + 2.0,
                    h: card.h + 2.0,
                },
                color: accent(),
            });
            prims.push(Primitive::Quad {
                rect: card,
                color: Color {
                    a: 0.97,
                    ..self.theme.pane_bg_focused
                },
            });
            let (mx, my) = self.mouse;
            for cell in &o.cells {
                let hot = cell.action.is_some() && cell.rect.contains(mx, my);
                if let Some(bg) = cell.bg {
                    push_clipped(prims, cell.rect, card, bg);
                }
                if hot && cell.text.is_empty() && cell.bg.is_none() {
                    push_clipped(prims, cell.rect, card, Color { a: 0.5, ..self.theme.selection });
                }
                if let Some(b) = cell.border.or(hot.then_some(accent()).filter(|_| !cell.text.is_empty())) {
                    let r = cell.rect;
                    for e in [
                        Rect { h: 1.0, ..r },
                        Rect { y: r.y + r.h - 1.0, h: 1.0, ..r },
                        Rect { w: 1.0, ..r },
                        Rect { x: r.x + r.w - 1.0, w: 1.0, ..r },
                    ] {
                        push_clipped(prims, e, card, if hot { accent() } else { b });
                    }
                }
                if !cell.text.is_empty() {
                    let pad = if cell.bg.is_some() || cell.border.is_some() { cw / 2.0 } else { 0.0 };
                    text(
                        prims,
                        (cell.rect.x + pad, cell.rect.y + (cell.rect.h - ch) / 2.0),
                        &cell.text,
                        cell.color,
                        card,
                    );
                }
            }
        }
    }

    /// The pointer shape for where the mouse is.
    pub fn pointer_cursor(&self) -> PointerCursor {
        if self.manip_target().is_some() {
            return PointerCursor::Grabbing;
        }
        let (x, y) = self.mouse;
        if let Some(o) = self.dm_overlay() {
            if o.card.contains(x, y) {
                let hot = o.cells.iter().any(|c| c.action.is_some() && c.rect.contains(x, y));
                return if hot { PointerCursor::Pointer } else { PointerCursor::Default };
            }
        }
        if !self.dm_mode {
            return PointerCursor::Default;
        }
        if self.dm_hover().is_some() {
            return PointerCursor::Grab;
        }
        if let Some(idx) = self.pane_at(x, y) {
            let p = &self.panes[idx];
            if !p.is_panel() && p.view.dm_gutter.is_some() {
                let cell = self.viewport.cell;
                if let Some(line) = p.view.dm_gutter_line_at(p.rect, cell, x, y) {
                    if p.view.dm_gutter.as_ref().is_some_and(|g| g.iter().any(|m| m.line == line)) {
                        return PointerCursor::Pointer;
                    }
                } else {
                    let at = p.view.position_for_click(p.rect, cell, x, y);
                    if p.view
                        .dm_underlines
                        .iter()
                        .any(|u| u.line == at.line && u.start <= at.col && at.col < u.end)
                    {
                        return PointerCursor::Pointer;
                    }
                }
            }
        }
        PointerCursor::Default
    }

    /// A left press, as drag mode and the result panel see it. `true` when
    /// consumed.
    pub(in crate::app) fn dm_mouse_down(&mut self, x: f32, y: f32, mods: super::Mods) -> bool {
        if let Some(o) = self.dm_overlay() {
            if o.card.contains(x, y) {
                let action = o
                    .cells
                    .iter()
                    .rev()
                    .find(|c| c.action.is_some() && c.rect.contains(x, y))
                    .and_then(|c| c.action.clone());
                if let Some(a) = action {
                    self.dm_act(a);
                }
                return true;
            }
        }
        if !self.dm_mode {
            return false;
        }
        let Some(idx) = self.pane_at(x, y) else {
            return false;
        };
        if self.traced_panel_at(x, y).is_some() {
            // A press on bare canvas is swallowed too: the sketch's mouse is
            // suspended while the mode is on.
            self.begin_manipulation(x, y);
            return true;
        }
        if self.panes[idx].is_panel() || self.panes[idx].view.dm_gutter.is_none() {
            return false;
        }
        let rect = self.panes[idx].rect;
        let cell = self.viewport.cell;
        if let Some(line) = self.panes[idx].view.dm_gutter_line_at(rect, cell, x, y) {
            self.focus = idx;
            self.dm_toggle_line(idx, line);
            return true;
        }
        let p = self.panes[idx].view.position_for_click(rect, cell, x, y);
        let text = self.panes[idx].view.buffer.line(p.line);
        let Some(lit) = dm_hints::literal_at(&text, p.col) else {
            return false;
        };
        let (s, e) = lit.outer();
        let marked = self.panes[idx]
            .view
            .dm_underlines
            .iter()
            .any(|u| u.line == p.line && u.start < e && s < u.end);
        if mods.alt || marked {
            self.focus = idx;
            self.dm_mark_literal(idx, p.line, &lit, None);
            return true;
        }
        false
    }

    /// Act on a click on the result panel.
    fn dm_act(&mut self, action: DmAction) {
        self.log_event("dm", format!("panel: {}", action.label()));
        match action {
            DmAction::Close => self.dm_panel_open = false,
            DmAction::Pick(k) => self.cycle_drag_alternative(0, Some(k)),
            DmAction::CycleHint(i) => {
                let Some(c) = self.dm_choices.as_ref() else { return };
                let editor = c.editor;
                let Some((line, s, _)) = c.marks.get(i).copied().flatten() else { return };
                let Some(view) = self.panes.get(editor).map(|p| &p.view) else { return };
                if line >= view.buffer.line_count() {
                    return;
                }
                match literal_on(&view.buffer.line(line), s) {
                    Some(lit) => self.dm_mark_literal(editor, line, &lit, None),
                    None => self.status_error = Some("E: that number is no longer in the code".to_string()),
                }
            }
            DmAction::ToggleAllow(i) | DmAction::ToggleDeny(i) => {
                let allow = matches!(action, DmAction::ToggleAllow(_));
                let Some(sel) = self.dm_choices.as_ref().and_then(|c| c.candidates.get(i)).map(|k| k.selector.clone())
                else {
                    return;
                };
                self.dm_toggle_filter(allow, sel);
            }
            DmAction::ClearFilters => {
                self.dm_filter = garden_script::DragFilter::default();
                self.dm_gen += 1;
                self.status_note = Some(describe_filter(&self.dm_filter));
            }
        }
        self.needs_redraw = true;
    }

    /// Add `sel` to the session allow (or deny) list, or take it out again.
    /// Adding to one list takes it off the other.
    fn dm_toggle_filter(&mut self, allow: bool, sel: DragSelector) {
        let (list, other) = if allow {
            (&mut self.dm_filter.allow, &mut self.dm_filter.deny)
        } else {
            (&mut self.dm_filter.deny, &mut self.dm_filter.allow)
        };
        if let Some(p) = list.iter().position(|s| *s == sel) {
            list.remove(p);
        } else {
            list.push(sel.clone());
            other.retain(|s| *s != sel);
        }
        self.dm_gen += 1;
        self.status_note = Some(format!("{} (from the next drag)", describe_filter(&self.dm_filter)));
    }

    /// Mark literal `lit` on `line` of editor pane `editor` as `to` (`None`:
    /// the next state in the cycle).
    pub(in crate::app) fn dm_mark_literal(&mut self, editor: usize, line: usize, lit: &Literal, to: Option<HintState>) {
        let to = to.unwrap_or(lit.state().next());
        let edits = dm_hints::mark(lit, to);
        if edits.is_empty() {
            self.status_note = Some(format!("{} is already {}", lit.text, to.name()));
            return;
        }
        let what = match to {
            HintState::Free => "free (no hint)",
            HintState::Tunable => "tunable: drags prefer it",
            HintState::Rigid => "rigid: drags never move it",
        };
        self.apply_hint_edits(editor, line, &edits, format!("{} is now {what} (line {})", lit.text, line + 1));
    }

    /// The line-level toggle: the drag's candidate numbers on `line`, else
    /// its `let` initializer, all to the state after the first one's.
    pub(in crate::app) fn dm_toggle_line(&mut self, editor: usize, line: usize) {
        let ranges: Vec<(usize, usize)> = self
            .dm_choices
            .as_ref()
            .filter(|c| c.editor == editor)
            .map(|c| {
                c.marks
                    .iter()
                    .flatten()
                    .filter(|m| m.0 == line)
                    .map(|m| (m.1, m.2))
                    .collect()
            })
            .unwrap_or_default();
        let Some(view) = self.panes.get(editor).map(|p| &p.view) else { return };
        if line >= view.buffer.line_count() {
            return;
        }
        let text = view.buffer.line(line);
        match dm_hints::cycle_line(&text, &ranges) {
            Some((targets, to, edits)) if !edits.is_empty() => {
                let names: Vec<&str> = targets.iter().map(|l| l.text.as_str()).collect();
                self.apply_hint_edits(
                    editor,
                    line,
                    &edits,
                    format!("line {}: {} now {}", line + 1, names.join(", "), to.name()),
                );
            }
            _ => {
                self.status_note = Some(format!(
                    "nothing to mark on line {}: no candidate number of the last drag, and no `let` with a literal value",
                    line + 1
                ))
            }
        }
    }

    /// Splice hint edits into one line as one undo step, and carry the last
    /// drag's candidate spans across them.
    fn apply_hint_edits(&mut self, editor: usize, line: usize, edits: &[LineEdit], note: String) {
        let Some(pane) = self.panes.get_mut(editor) else { return };
        let before = pane.view.buffer.text();
        let cursor = pane.view.cursor;
        pane.view.buffer.begin_undo_group();
        let mut sorted: Vec<&LineEdit> = edits.iter().collect();
        sorted.sort_by_key(|e| Reverse((e.start, e.end)));
        for e in sorted {
            pane.view
                .edit(Point::new(line, e.start), Point::new(line, e.end), &e.text);
        }
        pane.view.buffer.end_undo_run();
        let moved = if cursor.line == line {
            Point::new(line, dm_hints::map_col(cursor.col, edits, true))
        } else {
            cursor
        };
        pane.view.cursor = pane.view.buffer.clamp(moved);
        pane.view.anchor = None;
        pane.view.desired_col = None;
        let after = pane.view.buffer.text();
        if let Some(c) = self.dm_choices.as_mut().filter(|c| c.editor == editor && c.written == before) {
            for m in c.marks.iter_mut().flatten() {
                if m.0 == line {
                    let (a, b) = dm_hints::map_range((m.1, m.2), edits);
                    m.1 = a;
                    m.2 = b;
                }
            }
            c.written = after;
            if !c.alternatives.is_empty() {
                c.hints_changed = true;
            }
        }
        self.dm_gen += 1;
        self.log_event("dm", note.clone());
        self.status_note = Some(note);
        self.needs_redraw = true;
    }

    /// `:DragHint [free|tunable|rigid|line]`: mark the number under the
    /// focused editor's cursor (no argument cycles it), or cycle its line.
    pub(in crate::app) fn dm_hint_command(&mut self, arg: &str) {
        let editor = self.focus;
        let Some(pane) = self.panes.get(editor).filter(|p| !p.is_panel()) else {
            self.status_error = Some("E: :DragHint works at an editor's cursor".to_string());
            return;
        };
        let at = pane.view.cursor;
        let text = pane.view.buffer.line(at.line);
        let arg = arg.trim();
        if arg == "line" {
            self.dm_toggle_line(editor, at.line);
            return;
        }
        let to = match arg {
            "" => None,
            s => match HintState::parse(s) {
                Some(h) => Some(h),
                None => {
                    self.status_error = Some(format!("E: :DragHint takes free, tunable, rigid or line, not {s}"));
                    return;
                }
            },
        };
        match dm_hints::literal_at(&text, at.col) {
            Some(lit) => self.dm_mark_literal(editor, at.line, &lit, to),
            None => self.status_error = Some("E: no number under the cursor".to_string()),
        }
        self.needs_redraw = true;
    }

    /// Rebuild the editors' drag-mode underlines and gutter markers when the
    /// mode, the drag answer or a buffer changed. Returns whether it did.
    pub(in crate::app) fn sync_dm_decor(&mut self) -> bool {
        let key = |app: &App| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (app.dm_mode, app.dm_panel_open, app.dm_gen).hash(&mut h);
            for p in app.panes.iter().filter(|p| !p.is_panel()) {
                p.view.buffer.revision().hash(&mut h);
            }
            h.finish()
        };
        if key(self) == self.dm_decor_key {
            return false;
        }
        // The answer only describes the buffer while it still says what the
        // drag wrote: a hand edit since closes the panel.
        let live = self.dm_choices.as_ref().is_some_and(|c| {
            self.panes
                .get(c.editor)
                .is_some_and(|p| p.view.buffer.text() == c.written)
        });
        if !live {
            self.dm_panel_open = false;
        }
        self.dm_decor_key = key(self);

        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        let traced: Vec<std::path::PathBuf> = self
            .panes
            .iter()
            .filter_map(|p| p.panel.as_ref())
            .filter(|p| p.traces_origins())
            .map(|p| super::panes::resolve_script(p.origin_script(), script_dir.as_deref()))
            .collect();

        // The drag's candidates, in the editor they belong to.
        let mut cand_marks: Vec<(usize, DmUnderline)> = Vec::new();
        if let Some(c) = self.dm_choices.as_ref().filter(|_| live && (self.dm_mode || self.dm_panel_open)) {
            let (listed, _, _) = Self::listed_candidates(c);
            for i in listed {
                let Some((line, s, e)) = c.marks.get(i).copied().flatten() else { continue };
                let state = self.cand_state(c, i);
                cand_marks.push((
                    c.editor,
                    DmUnderline {
                        line,
                        start: s,
                        end: e.max(s + 1),
                        color: state.color(),
                        strong: c.written_marks.get(i).copied().unwrap_or(false),
                        state: state.name(),
                    },
                ));
            }
        }

        let dm_mode = self.dm_mode;
        for (idx, pane) in self.panes.iter_mut().enumerate() {
            if pane.is_panel() {
                continue;
            }
            let paired = pane.file.as_ref().is_some_and(|f| {
                traced.contains(&super::panes::resolve_script(f, script_dir.as_deref()))
            });
            let mut underlines: Vec<DmUnderline> = cand_marks
                .iter()
                .filter(|(e, _)| *e == idx)
                .map(|(_, u)| u.clone())
                .collect();
            let mut gutter: Vec<DmGutterMark> = Vec::new();
            for u in &underlines {
                if !gutter.iter().any(|g| g.line == u.line) {
                    gutter.push(DmGutterMark {
                        line: u.line,
                        color: u.color,
                        filled: true,
                    });
                }
            }
            if dm_mode && paired {
                for line in 0..pane.view.buffer.line_count() {
                    let text = pane.view.buffer.line(line);
                    if !text.chars().any(|c| c.is_ascii_digit()) {
                        continue;
                    }
                    for lit in dm_hints::literals(&text).into_iter().filter(|l| l.hint.is_some()) {
                        let state = CandState::of_hint(lit.state());
                        if !underlines
                            .iter()
                            .any(|u| u.line == line && u.start < lit.end && lit.start < u.end)
                        {
                            underlines.push(DmUnderline {
                                line,
                                start: lit.start,
                                end: lit.end,
                                color: state.color(),
                                strong: false,
                                state: state.name(),
                            });
                        }
                        if !gutter.iter().any(|g| g.line == line) {
                            gutter.push(DmGutterMark {
                                line,
                                color: state.color(),
                                filled: true,
                            });
                        }
                    }
                    if !gutter.iter().any(|g| g.line == line) && !dm_hints::line_targets(&text, &[]).is_empty() {
                        gutter.push(DmGutterMark {
                            line,
                            color: CandState::Free.color(),
                            filled: false,
                        });
                    }
                }
            }
            pane.view.dm_underlines = underlines;
            pane.view.dm_gutter = (dm_mode && paired).then_some(gutter);
        }
        true
    }

    /// The drag-mode part of `/state`'s `dm`.
    pub(in crate::app) fn dm_mode_json(&self) -> serde_json::Value {
        use serde_json::json;
        let rect = |r: Rect| json!({"x": r.x, "y": r.y, "w": r.w, "h": r.h});
        let hover = self.dm_hover().map(|h| {
            json!({"pane": h.pane, "index": h.index, "rect": rect(h.rect), "label": h.label})
        });
        let overlay = self.dm_overlay().map(|o| {
            json!({
                "card": rect(o.card),
                "cells": o.cells.iter().filter(|c| !c.text.is_empty() || c.action.is_some()).map(|c| json!({
                    "text": c.text,
                    "action": c.action.as_ref().map(DmAction::label),
                    "rect": rect(c.rect),
                })).collect::<Vec<_>>(),
            })
        });
        let editor = self
            .dm_choices
            .as_ref()
            .map(|c| c.editor)
            .or_else(|| self.panes.iter().position(|p| p.view.dm_gutter.is_some()));
        let (marks, gutter) = editor
            .and_then(|e| self.panes.get(e))
            .map(|p| {
                (
                    p.view
                        .dm_underlines
                        .iter()
                        .map(|u| json!({"line": u.line, "start": u.start, "end": u.end, "state": u.state, "written": u.strong}))
                        .collect::<Vec<_>>(),
                    p.view
                        .dm_gutter
                        .as_ref()
                        .map(|g| g.iter().map(|m| json!({"line": m.line, "filled": m.filled})).collect::<Vec<_>>()),
                )
            })
            .unwrap_or_default();
        // Where the editor's text and drag-mode gutter are, so automation can
        // click a number or a marker by line and column.
        let geometry = editor.and_then(|e| self.panes.get(e).map(|p| (e, p))).map(|(e, p)| {
            let (cw, ch) = self.viewport.cell;
            let (ox, oy) = p.view.text_origin(p.rect, cw);
            json!({
                "pane": e,
                "text_x": ox,
                "text_y": oy,
                "line_h": ch,
                "col_w": cw,
                "gutter_x": p.view.dm_gutter.as_ref().map(|_| ox - 1.25 * cw),
                "top_line": p.view.scroll.top,
            })
        });
        json!({
            "editor": geometry,
            "mode": self.dm_mode,
            "panel_open": self.dm_panel_open,
            "hints_changed": self.dm_choices.as_ref().is_some_and(|c| c.hints_changed),
            "cursor": self.pointer_cursor().name(),
            "hover": hover,
            "overlay": overlay,
            "marks": marks,
            "gutter": gutter,
        })
    }
}

/// A quad cut to `clip` (dropped when nothing is left).
fn push_clipped(prims: &mut Vec<Primitive>, r: Rect, clip: Rect, color: Color) {
    let x0 = r.x.max(clip.x);
    let y0 = r.y.max(clip.y);
    let x1 = (r.x + r.w).min(clip.x + clip.w);
    let y1 = (r.y + r.h).min(clip.y + clip.h);
    if x1 > x0 && y1 > y0 {
        prims.push(Primitive::Quad {
            rect: Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
            color,
        });
    }
}

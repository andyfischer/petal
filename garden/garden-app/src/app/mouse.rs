//! Pointer input: hit-testing a pixel to a pane, click-and-drag selection
//! (with multi-click word/line granularity), and wheel scrolling. The
//! multi-click count is detected at the frontend boundary (see
//! [`ClickCounter`](super::ClickCounter)) and passed into [`App::mouse_down`].
//!
//! What a press *does* inside the hit pane is decided by [`classify_mouse_down`],
//! a pure function of the pane kind: a panel consumes the click itself, a GPP
//! process pane whose client opted in gets it forwarded as a `mouse`
//! notification, and everything else places the editor cursor / starts a drag.

use std::time::Duration;

use crate::editor_view::{EditorView, ScrollAxis};

use super::App;

/// An active left-button drag. Selection drags move the editor cursor; a
/// scrollbar drag scrolls the pane whose bar was grabbed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Drag {
    /// Text selection in the pane at this index.
    Text(usize),
    /// A scrollbar drag: the pane, which axis, and the grab offset from the
    /// pointer to the thumb's leading edge at the moment of the press.
    Scrollbar {
        pane: usize,
        axis: ScrollAxis,
        grab: f32,
    },
    /// A left-button press captured by a panel pane at this index: subsequent
    /// moves and the release route to it as `petal-ui` mouse events so the
    /// script sees a drag gesture and the `mouse_released` edge.
    Panel(usize),
    /// A selection drag inside a panel's `text_view` region: moves/release
    /// drive the region's embedded editor directly (native selection). The
    /// gesture is *also* teed to the script as `petal-ui` mouse events, so a
    /// selectable region never hides clicks from the app (row selection, hunk
    /// toggles, click-to-focus keep working over selectable text).
    PanelText { pane: usize, id: i64 },
    /// A **drag-to-edit** gesture on a traced canvas: the pointer is pulling a
    /// drawn shape, and each move rewrites the source that placed it. The
    /// gesture's own state lives in [`App::manip`](super::App::manip) — a drag
    /// carries a list of arguments, and this enum is `Copy`.
    Manipulate,
    /// A scrollbar drag on a panel `text_view` region's embedded editor.
    PanelTextScrollbar {
        pane: usize,
        id: i64,
        axis: ScrollAxis,
        grab: f32,
    },
}

/// An in-progress drag-to-edit gesture: which shape is being pulled, where the
/// pointer grabbed it, and what its position arguments were worth at that
/// moment.
///
/// The base values are captured once, at the press, and every frame's goal is
/// `base + total delta` — never `current + frame delta`. The difference matters
/// because each frame's edit rounds to a whole pixel: accumulating those
/// roundings would make a shape lag behind the pointer over a long drag.
pub(in crate::app) struct ManipDrag {
    /// The panel pane the shape is drawn on.
    pane: usize,
    /// The editor pane paired with it — where the edits are written.
    editor: usize,
    /// The grabbed shape's index in the frame's command list. Stable across the
    /// gesture's own recompiles, unlike the call's term id.
    index: usize,
    /// Pointer position at the press, and the fallback point for the
    /// jump-to-code that a click-without-drag falls back to.
    start: (f32, f32),
    /// `(argument index, value at press)` for the arguments that follow the
    /// pointer horizontally, and vertically.
    x_args: Vec<(usize, f64)>,
    y_args: Vec<(usize, f64)>,
    /// Whether the pointer has passed [`DRAG_SLOP`] — i.e. whether this is a
    /// drag at all yet, or still a click that hasn't been released.
    moved: bool,
}

/// How far the pointer must move before a Cmd-press becomes a drag rather than
/// a click. Small enough that a deliberate pull registers immediately, large
/// enough that the hand-shake in a click doesn't rewrite the file.
const DRAG_SLOP: f64 = 3.0;

/// Sub-cell wheel motion owed to *panel scripts*, which see the wheel as whole
/// `scroll_x()` / `scroll_y()` ticks.
///
/// Everything the host draws — editor panes and panels' `text_view` regions —
/// scrolls by the fraction directly. A script cannot: its scroll state is its
/// own (a selected row index, say), and half a tick has no meaning there. So
/// fractions accumulate here and are handed over a whole tick at a time. This
/// is what stops a slow trackpad gesture from being *dropped* over a script
/// pane: every pixel still counts toward the next tick instead of rounding to
/// zero on arrival.
#[derive(Clone, Copy, Default)]
pub(crate) struct ScrollTicks {
    x: f32,
    y: f32,
}

impl ScrollTicks {
    /// Add `dx`/`dy` cells of motion and take out the whole ticks that have
    /// accumulated, keeping the remainder for next time.
    fn take(&mut self, dx: f32, dy: f32) -> (i32, i32) {
        self.x += dx;
        self.y += dy;
        let (tx, ty) = (self.x.trunc(), self.y.trunc());
        self.x -= tx;
        self.y -= ty;
        (tx as i32, ty as i32)
    }
}

impl App {
    /// The toolbar action whose button contains `(x, y)`, if the pointer is on
    /// the Petal-IDE toolbar. Hit-tests the same [`ToolbarButton`](super::ToolbarButton)
    /// list [`build_toolbar`](App::build_toolbar) draws, so clicks always match.
    pub(in crate::app) fn toolbar_at(&self, x: f32, y: f32) -> Option<super::ToolbarAction> {
        self.toolbar_buttons().into_iter().find_map(|b| {
            (x >= b.rect.x && x < b.rect.x + b.rect.w && y >= b.rect.y && y < b.rect.y + b.rect.h)
                .then_some(b.action)
        })
    }

    fn pane_at(&self, x: f32, y: f32) -> Option<usize> {
        self.panes.iter().position(|p| {
            x >= p.rect.x && x < p.rect.x + p.rect.w && y >= p.rect.y && y < p.rect.y + p.rect.h
        })
    }

    /// **Direct manipulation, acted on:** Cmd/Ctrl-clicking a shape on a traced
    /// canvas puts the cursor on the `draw_*` call that drew it, scrolls it into
    /// view, and focuses the paired editor — so the code you were pointing at is
    /// the code you are now typing in. Go-to-definition, with a pixel as the
    /// symbol.
    ///
    /// Behind a modifier on purpose. Hovering is free to be implicit because it
    /// changes nothing, but a plain click is part of the panel input contract
    /// (`mouse_pressed()`), and an interactive sketch must not lose its clicks
    /// — or its keyboard focus — to the editor. Cmd-click is the idiom for
    /// "take me to the source" everywhere else, and no sketch sees it.
    ///
    /// Returns whether it consumed the press: `false` for a press that wasn't
    /// over a traced shape, or whose panel has no paired editor pane, which
    /// falls through to the ordinary click path.
    pub(in crate::app) fn jump_to_traced_code(&mut self, x: f32, y: f32) -> bool {
        let Some(idx) = self.pane_at(x, y) else {
            return false;
        };
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));

        let pane = &self.panes[idx];
        let Some(panel) = pane.panel.as_ref() else {
            return false;
        };
        // A drifted panel is showing a frame whose spans no longer describe the
        // buffer (see `PanelView::source_drifted`) — jumping there would land the
        // cursor on unrelated code, which is worse than not jumping.
        if !panel.traces_origins() || panel.source_drifted() {
            return false;
        }
        let Some(span) = panel.trace_at(pane.rect, x, y).and_then(|t| t.call) else {
            return false;
        };
        let path = super::panes::resolve_script(panel.origin_script(), script_dir.as_deref());

        // The editor showing that file — the same pairing rule the live binding
        // and the hover highlight use.
        let Some(target) = self.panes.iter().position(|p| {
            !p.is_panel()
                && !p.is_process()
                && p.file
                    .as_ref()
                    .is_some_and(|f| super::panes::resolve_script(f, script_dir.as_deref()) == path)
        }) else {
            return false;
        };

        self.focus = target;
        let cell = self.viewport.cell;
        let pane = &mut self.panes[target];
        crate::vim::goto_line(&mut pane.view, span.start_line);
        // Land on the call itself, not on the line's first non-blank: the column
        // is information the trace has and `goto_line` doesn't.
        pane.view.cursor.col = span
            .start_col
            .min(pane.view.buffer.line_len(span.start_line));
        let visible = EditorView::visible_lines(pane.rect, cell.1);
        let visible_cols = pane.view.visible_cols(pane.rect, cell.0);
        pane.view.ensure_cursor_visible(visible, visible_cols);
        self.log_event("mouse", format!("jump to code @ line {}", span.start_line));
        true
    }

    /// **Direct manipulation, written back:** begin a drag-to-edit gesture on
    /// the shape under `(x, y)`.
    ///
    /// The press only *arms* the gesture — nothing is written until the pointer
    /// actually moves ([`DRAG_SLOP`]), so a Cmd-click that doesn't move still
    /// falls through to [`jump_to_traced_code`](Self::jump_to_traced_code) on
    /// release. One modifier, two gestures, related the way click and drag are
    /// everywhere else: click to *go to* the code, drag to *change* it.
    ///
    /// What is captured here is the shape's **command index** and the values its
    /// position arguments have right now. The index survives the gesture's own
    /// edits (the sketch recompiles, but it draws the same shapes in the same
    /// order), while term ids do not — so each move re-resolves the call and
    /// states its goals as *press-time value + total pointer delta*, which is
    /// also what keeps a long drag from accumulating rounding drift.
    ///
    /// Returns whether it armed; `false` falls through to the ordinary press
    /// path (bare canvas, an untraced or drifted panel, a shape whose position
    /// isn't a pair of plain arguments, or no paired editor to write into).
    pub(in crate::app) fn begin_manipulation(&mut self, x: f32, y: f32) -> bool {
        let Some(idx) = self.pane_at(x, y) else {
            return false;
        };
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));

        let pane = &self.panes[idx];
        let Some(panel) = pane.panel.as_ref() else {
            return false;
        };
        // A drifted panel is showing a frame whose spans no longer describe the
        // buffer: an edit derived from it would land on unrelated text.
        if !panel.traces_origins() || panel.source_drifted() {
            return false;
        }
        let Some((index, handle)) = panel.drag_target_at(pane.rect, x, y) else {
            return false;
        };
        let path = super::panes::resolve_script(panel.origin_script(), script_dir.as_deref());
        let Some(editor) = self.editor_pane_for(&path) else {
            return false;
        };

        // One undo step for the whole gesture: a drag makes an edit per frame,
        // and undoing a drag means putting the shape back where it was, not
        // walking it backwards a pixel at a time.
        self.panes[editor].view.buffer.begin_undo_group();
        self.manip = Some(ManipDrag {
            pane: idx,
            editor,
            index,
            start: (x, y),
            x_args: handle.x_args,
            y_args: handle.y_args,
            moved: false,
        });
        self.drag = Some(Drag::Manipulate);
        true
    }

    /// The editor pane showing `path` — the pairing rule the live binding, the
    /// hover highlight and the jump all share.
    fn editor_pane_for(&self, path: &std::path::Path) -> Option<usize> {
        let script_dir = self
            .script
            .as_ref()
            .and_then(|s| s.path().parent().map(|p| p.to_path_buf()));
        self.panes.iter().position(|p| {
            !p.is_panel()
                && !p.is_process()
                && p.file
                    .as_ref()
                    .is_some_and(|f| super::panes::resolve_script(f, script_dir.as_deref()) == path)
        })
    }

    /// One frame of a drag-to-edit gesture: say where the grabbed shape should
    /// be now, and write the edits the runtime answers with into the paired
    /// editor's buffer.
    ///
    /// The goals are stated in *canvas* terms — "this argument should evaluate
    /// to 148" — never in text terms. Which literal moves to make that true is
    /// Petal's answer: the number in the call, the `let` behind it, or, when the
    /// argument is an expression, whichever of its leaves the sketch declared
    /// tunable with `config let`. Dragging a bar in a loop-drawn chart therefore
    /// edits the chart's data, because that is the only editable thing its
    /// height flows from.
    ///
    /// The edit goes into the *buffer*, not the file: the live binding
    /// recompiles the canvas from the buffer on the next frame, so the shape
    /// follows the pointer, and nothing touches disk until the user saves.
    fn drag_manipulate(&mut self, x: f32, y: f32) {
        let Some(m) = self.manip.as_ref() else {
            return;
        };
        let (dx, dy) = ((x - m.start.0) as f64, (y - m.start.1) as f64);
        if !m.moved && dx.abs() < DRAG_SLOP && dy.abs() < DRAG_SLOP {
            return;
        }
        let (pane, editor, index) = (m.pane, m.editor, m.index);

        // Re-resolve against the program running *now*: the previous frame's
        // edit recompiled it, so the call this shape came from has a new term
        // id. A sketch that has stopped compiling resolves to nothing, and the
        // drag simply produces no edit this frame rather than a wrong one.
        let Some(panel) = self.panes.get(pane).and_then(|p| p.panel.as_ref()) else {
            return;
        };
        if panel.source_drifted() {
            return;
        }
        if panel.drag_handle_at(index).is_none() {
            return;
        }
        // Only the axes that actually moved are stated. An axis with a zero
        // delta has no goal — asking for the value it already has would churn a
        // no-op edit into the buffer and, worse, would report "editable" for a
        // shape that only moves on one axis.
        let goals: Vec<(usize, f64)> = m
            .x_args
            .iter()
            .filter(|_| dx.round() != 0.0)
            .map(|&(i, base)| (i, base + dx))
            .chain(
                m.y_args
                    .iter()
                    .filter(|_| dy.round() != 0.0)
                    .map(|&(i, base)| (i, base + dy)),
            )
            .collect();
        if goals.is_empty() {
            return;
        }
        let outcome = panel.propose_drag_edits(index, &goals);
        if let Some(m) = self.manip.as_mut() {
            m.moved = true;
        }
        match outcome {
            garden_script::DragOutcome::Edits(rewrites) => {
                self.apply_rewrites(editor, &rewrites);
                self.needs_redraw = true;
            }
            // A refusal is reported, not swallowed: "the drag did nothing" is
            // otherwise indistinguishable from a broken feature.
            garden_script::DragOutcome::Refused(why) => {
                self.status_note = Some(why);
                self.needs_redraw = true;
            }
            // Stale is the normal state for one frame after the gesture's own
            // edit; the next move re-resolves.
            garden_script::DragOutcome::Stale => {}
        }
    }

    /// Splice a gesture's rewrites into an editor pane's buffer.
    ///
    /// Applied **back to front** so an earlier edit can't shift a later one's
    /// span, and deduplicated on the way: two goals can legitimately resolve to
    /// the same edit (both axes of a shape placed from one `config let`), which
    /// is one edit chosen twice, not a collision.
    fn apply_rewrites(&mut self, editor: usize, rewrites: &[garden_script::SourceRewrite]) {
        let mut ordered: Vec<&garden_script::SourceRewrite> = rewrites.iter().collect();
        ordered.sort_by_key(|r| std::cmp::Reverse((r.span.start_line, r.span.start_col)));
        ordered.dedup_by_key(|r| (r.span.start_line, r.span.start_col, r.new_text.clone()));

        let Some(pane) = self.panes.get_mut(editor) else {
            return;
        };
        for r in &ordered {
            let start = garden_core::Point::new(r.span.start_line, r.span.start_col);
            let end = garden_core::Point::new(r.span.end_line, r.span.end_col);
            pane.view.edit(start, end, &r.new_text);
        }
        // Keep the cursor on the number that moved: the code being rewritten is
        // the code you'd want to keep editing by hand after letting go.
        if let Some(first) = ordered.last() {
            pane.view.cursor = pane.view.buffer.clamp(garden_core::Point::new(
                first.span.start_line,
                first.span.start_col,
            ));
            pane.view.anchor = None;
            pane.view.desired_col = None;
        }
        // Say what moved. A `shared` binding is the case worth naming out loud:
        // the drag moved every other shape that reads it too.
        let shared = ordered.iter().find(|r| r.shared);
        self.status_note = Some(match shared {
            Some(r) => format!(
                "{} — shared, other shapes read it",
                r.description.replace('`', "")
            ),
            None => ordered
                .iter()
                .map(|r| r.description.replace('`', ""))
                .collect::<Vec<_>>()
                .join(", "),
        });
    }

    /// Left button pressed at `(x, y)`: focus the pane, place the cursor,
    /// and start a drag selection (shift extends the existing selection).
    /// `clicks` is the multi-click count from the frontend's [`ClickCounter`](super::ClickCounter)
    /// (or the debug `/mouse` body): 2 selects the word under the press, 3+
    /// the whole line, and the drag then extends word-/line-wise.
    ///
    /// `mods` carries the full modifier state because Cmd/Ctrl turns a press on
    /// a traced canvas into **jump-to-code** (see [`jump_to_traced_code`](Self::jump_to_traced_code)),
    /// the go-to-definition idiom applied to a drawn shape.
    pub fn mouse_down(&mut self, x: f32, y: f32, mods: super::Mods, clicks: u32) {
        let shift = mods.shift;
        self.mouse = (x, y);
        self.wake_panels();

        // Cmd/Ctrl-click on a shape: go to the code that drew it. Checked before
        // anything else consumes the press, and it consumes the press itself —
        // a plain click still belongs entirely to the script, so an interactive
        // sketch never has its clicks or its keyboard focus stolen.
        // Cmd/Ctrl on a shape arms direct manipulation: move the pointer and the
        // drag rewrites the code that placed it; release without moving and it
        // jumps to that code instead (see `begin_manipulation`). Checked before
        // anything else consumes the press, and it consumes the press itself —
        // a plain click still belongs entirely to the script, so an interactive
        // sketch never has its clicks or its keyboard focus stolen.
        if mods.cmd || mods.ctrl {
            if self.begin_manipulation(x, y) {
                self.needs_redraw = true;
                return;
            }
            if self.jump_to_traced_code(x, y) {
                self.needs_redraw = true;
                return;
            }
        }

        // A press on the Petal-IDE toolbar dispatches its button and consumes the
        // click (checked before pane/divider hit-testing — the band sits above
        // every pane).
        if let Some(action) = self.toolbar_at(x, y) {
            self.dispatch_toolbar(action);
            self.needs_redraw = true;
            return;
        }

        // A press on a split divider starts a resize drag. Checked before pane
        // hit-testing, since dividers sit in the inter-pane gap (and their grab
        // band overlaps the pane edges slightly). A press on a pane's own
        // scrollbar — which can sit right at the divider on a split's inner edge
        // — must still scroll, so the scrollbar wins that overlap.
        let divider = self
            .divider_at(x, y)
            .map(|d| (d.path.clone(), d.before, d.span, d.vertical));
        if let Some((path, before, span, vertical)) = divider {
            let on_scrollbar = self.pane_at(x, y).is_some_and(|idx| {
                !self.panes[idx].is_panel() && {
                    let rect = self.panes[idx].rect;
                    let cell = self.viewport.cell;
                    self.panes[idx]
                        .view
                        .scrollbar_hit(rect, cell, x, y)
                        .is_some()
                }
            });
            if !on_scrollbar {
                let start_px = if vertical { x } else { y };
                self.start_divider_drag(path, before, span, vertical, start_px);
                self.needs_redraw = true;
                return;
            }
        }

        let Some(idx) = self.pane_at(x, y) else {
            return;
        };
        self.focus = idx;
        self.log_event(
            "mouse",
            format!("click pane {idx} @ {x:.0},{y:.0} x{clicks}"),
        );

        // A press inside a panel's selectable `text_view` region drives that
        // region's embedded read-only editor (native selection + clipboard),
        // not the panel script. A press anywhere else in the panel hands
        // keyboard focus back to the script.
        if self.panes[idx].is_panel() {
            let pane_rect = self.panes[idx].rect;
            let cell = self.viewport.cell;
            let region = self.panes[idx]
                .panel
                .as_ref()
                .and_then(|p| p.text_view_at(pane_rect, x, y));
            if let Some(id) = region {
                // A press on the region's own scrollbar starts a scroll drag;
                // otherwise it starts a native text selection.
                let grabbed = self.panes[idx]
                    .panel
                    .as_mut()
                    .and_then(|p| p.region_scrollbar_grab(id, pane_rect, cell, x, y));
                if let Some((axis, grab)) = grabbed {
                    self.drag = Some(Drag::PanelTextScrollbar {
                        pane: idx,
                        id,
                        axis,
                        grab,
                    });
                    self.needs_redraw = true;
                    return;
                }
                if let Some(panel) = self.panes[idx].panel.as_mut() {
                    panel.region_press(id, pane_rect, cell, x, y, shift, clicks);
                    // Tee the press to the script as well: native selection is
                    // an affordance layered *on top of* the script's own click
                    // semantics, not a replacement for them — the script still
                    // sees `mouse_pressed()` and can select the clicked row,
                    // toggle a hunk, or move its focus ring.
                    let (lx, ly) = ((x - pane_rect.x) as i32, (y - pane_rect.y) as i32);
                    // The whole chord, not just shift: a script's alt-drag
                    // ("scale about the center") and cmd-click are as real as
                    // shift-click, and used to arrive with `mod_alt()` false.
                    panel.set_modifiers(mods);
                    panel.set_mouse(lx, ly);
                    panel.mouse_down_clicks(0, clicks);
                }
                self.drag = Some(Drag::PanelText { pane: idx, id });
                self.tick_panel_at(idx);
                self.needs_redraw = true;
                return;
            }
            if let Some(panel) = self.panes[idx].panel.as_mut() {
                panel.clear_focused_region();
            }
        }

        // A press on a scrollbar (editor or process pane — panels draw their
        // own) starts a scroll drag, taking precedence over text selection or
        // click forwarding.
        if !self.panes[idx].is_panel() {
            let cell = self.viewport.cell;
            let rect = self.panes[idx].rect;
            if let Some(hit) = self.panes[idx].view.scrollbar_hit(rect, cell, x, y) {
                self.panes[idx]
                    .view
                    .drag_scroll(hit.axis, hit.grab, rect, cell, x, y);
                self.drag = Some(Drag::Scrollbar {
                    pane: idx,
                    axis: hit.axis,
                    grab: hit.grab,
                });
                self.needs_redraw = true;
                return;
            }
        }

        let target = classify_mouse_down(
            self.panes[idx].is_panel(),
            self.panes[idx].process.as_ref().is_some_and(|p| p.mouse()),
        );
        match target {
            // A panel pane consumes the click itself: set its pane-local mouse
            // position, queue a left-button press edge, and tick it now. No editor
            // drag selection (the pane has no text buffer to select).
            MouseTarget::Panel => {
                let pane = &mut self.panes[idx];
                let (lx, ly) = ((x - pane.rect.x) as i32, (y - pane.rect.y) as i32);
                if let Some(panel) = pane.panel.as_mut() {
                    // The full chord reaches the script (see the `text_view`
                    // tee above): `mod_alt()`/`mod_cmd()` are how a panel says
                    // "ignore the snap grid" or "scale about the center".
                    panel.set_modifiers(mods);
                    // Sync the pointer before the press so the click chain and
                    // drag anchor start from the right spot.
                    panel.set_mouse(lx, ly);
                    // …and the click chain the host already counted, so
                    // `click_count()` can report a double-click.
                    panel.mouse_down_clicks(0, clicks);
                }
                // Capture the drag so moves/release route to this panel (drag
                // gesture + `mouse_released` edge).
                self.drag = Some(Drag::Panel(idx));
                self.tick_panel_at(idx);
                self.needs_redraw = true;
            }
            // A process pane whose client opted in gets the click forwarded as
            // a `mouse` notification carrying the content row it landed on
            // (scroll-adjusted) — instead of the passive-view cursor placement.
            // The host-side focus change above still happened, and scrolling
            // stays host-side; no drag selection starts (like a panel, the
            // content isn't the user's to select).
            MouseTarget::Process => {
                let cell = self.viewport.cell;
                let pane = &self.panes[idx];
                let p = pane.view.position_for_click(pane.rect, cell, x, y);
                let msgs = {
                    let Some(process) = self.panes[idx].process.as_mut() else {
                        return;
                    };
                    process.send_mouse(p.line, p.col, mouse_kind(clicks));
                    // Like a forwarded key: wait briefly so the client's re-render
                    // feels synchronous.
                    process.drain_for(Duration::from_millis(120))
                };
                self.apply_process_messages(idx, msgs);
                self.needs_redraw = true;
            }
            MouseTarget::Editor => {
                let cell = self.viewport.cell;
                let pane = &mut self.panes[idx];
                let p = pane.view.position_for_click(pane.rect, cell, x, y);
                pane.view.begin_drag_with_clicks(p, shift, clicks);
                self.drag = Some(Drag::Text(idx));
                self.needs_redraw = true;
            }
        }
    }

    /// Right button pressed at `(x, y)`. The right button is the *context*
    /// gesture: it opens menus, and nothing else. So unlike
    /// [`mouse_down`](Self::mouse_down) it places no cursor, starts no drag,
    /// grabs no scrollbar or divider, and — crucially — does not clear a
    /// panel's focused region, since right-clicking inside a diff to ask for a
    /// menu must not throw away the vim session you were in the middle of.
    ///
    /// Only panel panes act on it: a script sees the press as button 1 (the
    /// `petal-ui` numbering, `mouse_pressed(1)`). Everywhere else it is
    /// swallowed rather than falling through to a left-button behavior the user
    /// did not ask for.
    pub fn mouse_down_right(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        self.wake_panels();
        let Some(idx) = self.pane_at(x, y) else {
            return;
        };
        if !self.panes[idx].is_panel() {
            return;
        }
        self.focus = idx;
        self.log_event("mouse", format!("right-click pane {idx} @ {x:.0},{y:.0}"));
        let pane = &mut self.panes[idx];
        let (lx, ly) = ((x - pane.rect.x) as i32, (y - pane.rect.y) as i32);
        if let Some(panel) = pane.panel.as_mut() {
            panel.set_mouse(lx, ly);
            panel.mouse_down(1);
        }
        self.right_press = Some(idx);
        self.tick_panel_at(idx);
        self.needs_redraw = true;
    }

    /// Right button released: balance the press so the script sees the
    /// `mouse_released(1)` edge and does not think the button is still held.
    /// Delivered to the pane that took the press, wherever the pointer is now.
    pub fn mouse_up_right(&mut self) {
        let Some(idx) = self.right_press.take() else {
            return;
        };
        if let Some(panel) = self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
            panel.mouse_up(1);
        }
        self.tick_panel_at(idx);
        self.needs_redraw = true;
    }

    /// Mouse moved: track the position; while dragging, extend the selection
    /// in the pane where the drag started (auto-scrolling past its edges).
    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        self.wake_panels();
        // A divider drag resizes the split live (persisted on release).
        if self.divider_drag.is_some() {
            self.drag_divider_to(x, y);
            return;
        }
        let cell = self.viewport.cell;
        match self.drag {
            Some(Drag::Text(idx)) => {
                let Some(pane) = self.panes.get_mut(idx) else {
                    return;
                };
                let p = pane.view.position_for_click(pane.rect, cell, x, y);
                pane.view.drag_to(p);
                let vis_lines = EditorView::visible_lines(pane.rect, cell.1);
                let vis_cols = pane.view.visible_cols(pane.rect, cell.0);
                pane.view.ensure_cursor_visible(vis_lines, vis_cols);
                self.needs_redraw = true;
            }
            Some(Drag::Scrollbar { pane, axis, grab }) => {
                let Some(p) = self.panes.get_mut(pane) else {
                    return;
                };
                let rect = p.rect;
                p.view.drag_scroll(axis, grab, rect, cell, x, y);
                self.needs_redraw = true;
            }
            Some(Drag::Panel(idx)) => {
                if let Some(pane) = self.panes.get_mut(idx) {
                    let (lx, ly) = ((x - pane.rect.x) as i32, (y - pane.rect.y) as i32);
                    if let Some(panel) = pane.panel.as_mut() {
                        panel.set_mouse(lx, ly);
                    }
                }
                // Tick so `drag_active()` / `mouse_x/y()` update live under the drag.
                self.tick_panel_at(idx);
                self.needs_redraw = true;
            }
            Some(Drag::Manipulate) => self.drag_manipulate(x, y),
            // Not dragging: the pointer still moved *over* a panel, and the
            // script's `mouse_x()`/`mouse_y()` are a level it reads every frame.
            // Tick it now (as a press does) so hover is delivered on the frame
            // the move happened — otherwise the first move after launch lands
            // between ticks and is never seen by a `GET /state` right after it.
            None if self.divider_drag.is_none() => {
                if let Some(idx) = self.pane_at(x, y) {
                    if self.panes.get(idx).is_some_and(super::Pane::is_panel) {
                        self.tick_panel_at(idx);
                        self.needs_redraw = true;
                    }
                }
            }
            Some(Drag::PanelText { pane, id }) => {
                if let Some(p) = self.panes.get_mut(pane) {
                    let rect = p.rect;
                    if let Some(panel) = p.panel.as_mut() {
                        panel.region_drag_to(id, rect, cell, x, y);
                        // The press was teed to the script; keep its pointer in
                        // sync through the gesture (it holds the button down).
                        panel.set_mouse((x - rect.x) as i32, (y - rect.y) as i32);
                    }
                }
                self.tick_panel_at(pane);
                self.needs_redraw = true;
            }
            Some(Drag::PanelTextScrollbar {
                pane,
                id,
                axis,
                grab,
            }) => {
                if let Some(p) = self.panes.get_mut(pane) {
                    let rect = p.rect;
                    if let Some(panel) = p.panel.as_mut() {
                        panel.region_scrollbar_drag(id, rect, cell, axis, grab, x, y);
                    }
                }
                self.needs_redraw = true;
            }
            None => {}
        }
    }

    /// Left button released: finish a text-selection drag; a scrollbar drag
    /// just ends.
    pub fn mouse_up(&mut self) {
        // Finish a divider drag: persist the resized layout once, on release.
        if self.divider_drag.take().is_some() {
            self.end_divider_drag();
            self.needs_redraw = true;
            return;
        }
        if let Some(drag) = self.drag.take() {
            match drag {
                Drag::Text(idx) => {
                    if let Some(pane) = self.panes.get_mut(idx) {
                        pane.view.end_drag();
                    }
                }
                Drag::Panel(idx) => {
                    if let Some(panel) = self.panes.get_mut(idx).and_then(|p| p.panel.as_mut()) {
                        panel.mouse_up(0);
                    }
                    self.tick_panel_at(idx);
                }
                Drag::PanelText { pane, id } => {
                    if let Some(panel) = self.panes.get_mut(pane).and_then(|p| p.panel.as_mut()) {
                        panel.region_end_drag(id);
                        // Balance the teed press so the script sees the
                        // `mouse_released` edge and doesn't think the button
                        // is still held.
                        panel.mouse_up(0);
                    }
                    self.tick_panel_at(pane);
                }
                // A gesture that never moved was a Cmd-*click*: fall back to
                // jumping to the code, so the two live on one modifier without
                // either shadowing the other.
                Drag::Manipulate => {
                    if let Some(m) = self.manip.take() {
                        if let Some(pane) = self.panes.get_mut(m.editor) {
                            pane.view.buffer.end_undo_run();
                        }
                        if !m.moved {
                            self.jump_to_traced_code(m.start.0, m.start.1);
                        } else {
                            self.log_event("mouse", "drag-to-edit released".to_string());
                        }
                    }
                }
                Drag::PanelTextScrollbar { .. } => {}
                Drag::Scrollbar { .. } => {}
            }
            self.needs_redraw = true;
        }
    }

    /// Scroll the pane under the mouse (falling back to the focused pane)
    /// vertically by `lines` rows — fractional, so a trackpad's pixel deltas
    /// arrive intact and the pane scrolls smoothly rather than a row at a time.
    pub fn handle_scroll(&mut self, lines: f32) {
        self.wake_panels();
        let (x, y) = self.mouse;
        let cell_h = self.viewport.cell.1;
        let idx = self.pane_at(x, y).unwrap_or(self.focus);
        // A panel under the pointer gets the wheel as `scroll_y()` input; a normal
        // pane scrolls its text view.
        if self.panes.get(idx).is_some_and(|p| p.is_panel()) {
            // A selectable `text_view` region under the pointer scrolls its own
            // embedded editor (native scroll) — but only while its content
            // overflows its rect. A region with nothing to scroll can't act on
            // the wheel, so it falls through to the script as `scroll_y()`.
            let pane_rect = self.panes[idx].rect;
            let cell = self.viewport.cell;
            let region = self.panes[idx]
                .panel
                .as_ref()
                .and_then(|p| p.text_view_at(pane_rect, x, y));
            if let Some(id) = region {
                let consumed = self.panes[idx]
                    .panel
                    .as_mut()
                    .is_some_and(|panel| panel.region_scroll(id, pane_rect, cell, lines));
                if consumed {
                    self.needs_redraw = true;
                    return;
                }
            }
            let (_, ticks) = self.scroll_ticks.take(0.0, lines);
            if let Some(pane) = self.panes.get_mut(idx) {
                let (lx, ly) = ((x - pane.rect.x) as i32, (y - pane.rect.y) as i32);
                if let Some(panel) = pane.panel.as_mut() {
                    // Refresh the pane-local pointer first: a script's wheel
                    // handling hit-tests `mouse_x/y()`, and no move event
                    // precedes the wheel on some paths (the debug server's
                    // `scroll` op, a wheel before any motion).
                    panel.set_mouse(lx, ly);
                    if ticks != 0 {
                        panel.scroll(0, ticks);
                    }
                }
            }
            self.tick_panel_at(idx);
            self.needs_redraw = true;
            return;
        }
        if let Some(pane) = self.panes.get_mut(idx) {
            let visible = EditorView::visible_lines(pane.rect, cell_h);
            let visible_cols = pane.view.visible_cols(pane.rect, self.viewport.cell.0);
            pane.view.scroll_by(lines, visible, visible_cols);
            self.needs_redraw = true;
        }
    }

    /// Scroll the pane under the mouse horizontally by `cols` display columns,
    /// fractional like [`handle_scroll`](Self::handle_scroll).
    pub fn handle_scroll_h(&mut self, cols: f32) {
        self.wake_panels();
        let (x, y) = self.mouse;
        let idx = self.pane_at(x, y).unwrap_or(self.focus);
        // A panel under the pointer gets horizontal wheel as `scroll_x()` input.
        if self.panes.get(idx).is_some_and(|p| p.is_panel()) {
            // A selectable `text_view` region under the pointer scrolls its own
            // embedded editor horizontally — but only while a line is wider
            // than its rect; otherwise the wheel falls through to the script
            // as `scroll_x()`.
            let pane_rect = self.panes[idx].rect;
            let cell = self.viewport.cell;
            let region = self.panes[idx]
                .panel
                .as_ref()
                .and_then(|p| p.text_view_at(pane_rect, x, y));
            if let Some(id) = region {
                let consumed = self.panes[idx]
                    .panel
                    .as_mut()
                    .is_some_and(|panel| panel.region_scroll_h(id, pane_rect, cell, cols));
                if consumed {
                    self.needs_redraw = true;
                    return;
                }
            }
            let (ticks, _) = self.scroll_ticks.take(cols, 0.0);
            if let Some(pane) = self.panes.get_mut(idx) {
                let (lx, ly) = ((x - pane.rect.x) as i32, (y - pane.rect.y) as i32);
                if let Some(panel) = pane.panel.as_mut() {
                    // See handle_scroll: bind the pointer before the wheel edge.
                    panel.set_mouse(lx, ly);
                    if ticks != 0 {
                        panel.scroll(ticks, 0);
                    }
                }
            }
            self.tick_panel_at(idx);
            self.needs_redraw = true;
            return;
        }
        if let Some(pane) = self.panes.get_mut(idx) {
            pane.view.scroll_h_by(cols);
            self.needs_redraw = true;
        }
    }

    /// The split divider whose grab band contains `(x, y)`, if any. `None` when
    /// a divider drag is already in flight (so a stray hit-test can't restart it).
    pub(in crate::app) fn divider_at(&self, x: f32, y: f32) -> Option<&crate::layout::Divider> {
        if self.divider_drag.is_some() {
            return None;
        }
        self.dividers.iter().find(|d| d.rect.contains(x, y))
    }

    /// Begin resizing the split at `path`/`before`; snapshots the live layout as
    /// the drag baseline (see [`super::DividerDrag`]).
    fn start_divider_drag(
        &mut self,
        path: Vec<usize>,
        before: usize,
        span: f32,
        vertical: bool,
        start_px: f32,
    ) {
        let baseline = self.layout_from_panes();
        self.divider_drag = Some(super::DividerDrag {
            path,
            before,
            span,
            vertical,
            start_px,
            baseline,
        });
    }

    /// Resize the dragged split so its boundary tracks the pointer, live: the
    /// ratio delta is the total drag distance over the split span, applied to a
    /// fresh clone of the baseline (drift-free), then the panes are repositioned.
    fn drag_divider_to(&mut self, x: f32, y: f32) {
        let Some(drag) = self.divider_drag.as_ref() else {
            return;
        };
        let axis = if drag.vertical { x } else { y };
        let delta_frac = (axis - drag.start_px) / drag.span.max(1.0);
        let mut tree = drag.baseline.clone();
        let changed = crate::layout::resize_divider(&mut tree, &drag.path, drag.before, delta_frac);
        if changed {
            self.live_layout = Some(tree);
            self.reposition_panes();
            self.needs_redraw = true;
        }
    }

    /// Commit the resized layout on release: adopt and persist the live override,
    /// then drop it so [`layout`](super::App::layout) reads the tree normally.
    fn end_divider_drag(&mut self) {
        if let Some(tree) = self.live_layout.take() {
            self.apply_runtime_layout(tree);
        }
    }
}

/// Where a mouse press inside a pane is routed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MouseTarget {
    /// A panel pane consumes the click itself (script-side input).
    Panel,
    /// A GPP client that opted in gets the click as a `mouse` notification.
    Process,
    /// Host default: place the editor cursor / start a drag selection.
    Editor,
}

/// Decide where a press inside a pane goes, as a pure function of the pane
/// kind. `process_mouse` is whether the pane is process-backed **and** its
/// client opted into mouse forwarding (see [`gpp::InitializeResult::mouse`]);
/// a process pane that didn't opt in keeps today's editor-surface behavior,
/// so old clients are unaffected.
fn classify_mouse_down(is_panel: bool, process_mouse: bool) -> MouseTarget {
    if is_panel {
        MouseTarget::Panel
    } else if process_mouse {
        MouseTarget::Process
    } else {
        MouseTarget::Editor
    }
}

/// Map a frontend multi-click count to the GPP wire kind: the first press of
/// any sequence is a `click`, the second and later presses arrive as `double`
/// (a triple-click's third press is still "activate", like a file manager).
fn mouse_kind(clicks: u32) -> gpp::MouseKind {
    if clicks >= 2 {
        gpp::MouseKind::Double
    } else {
        gpp::MouseKind::Click
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel always consumes the click; an opted-in process pane gets it
    /// forwarded; every other pane (editor, or a process pane whose client
    /// never opted in — e.g. an older GPP client) keeps the editor behavior.
    #[test]
    fn classify_routes_by_pane_kind_and_opt_in() {
        assert_eq!(classify_mouse_down(true, false), MouseTarget::Panel);
        assert_eq!(classify_mouse_down(true, true), MouseTarget::Panel);
        assert_eq!(classify_mouse_down(false, true), MouseTarget::Process);
        assert_eq!(classify_mouse_down(false, false), MouseTarget::Editor);
    }

    #[test]
    fn click_counts_map_to_wire_kinds() {
        assert_eq!(mouse_kind(0), gpp::MouseKind::Click);
        assert_eq!(mouse_kind(1), gpp::MouseKind::Click);
        assert_eq!(mouse_kind(2), gpp::MouseKind::Double);
        assert_eq!(mouse_kind(3), gpp::MouseKind::Double);
    }

    /// Sub-tick motion is carried, not dropped: four tenths of a tick deliver
    /// nothing, and the fifth delivers the whole tick they add up to. Rounding
    /// each delta on arrival — what the frontend used to do — would have thrown
    /// all five away and left a slow trackpad gesture doing nothing at all over
    /// a script pane.
    #[test]
    fn sub_tick_scrolls_accumulate_into_whole_ticks() {
        let mut ticks = ScrollTicks::default();
        for _ in 0..4 {
            assert_eq!(ticks.take(0.0, 0.2), (0, 0));
        }
        assert_eq!(ticks.take(0.0, 0.2), (0, 1));
    }

    /// Reversing mid-gesture cancels out rather than each direction rounding
    /// up to a tick of its own.
    #[test]
    fn scroll_ticks_cancel_across_a_direction_change() {
        let mut ticks = ScrollTicks::default();
        assert_eq!(ticks.take(0.7, 0.7), (0, 0));
        assert_eq!(ticks.take(-0.7, -0.7), (0, 0));
        assert_eq!(ticks.take(1.0, -1.0), (1, -1));
    }
}

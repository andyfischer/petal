//! Direct manipulation v2 on a panel: a drag whose goal is solved by
//! re-running the frame speculatively (docs/dev/direct-manipulation-v2-plan.md
//! §4.4), so a shape placed through any number of function calls can be
//! dragged.
//!
//! The host-independent half (the probe, the session, the conversion of the
//! engine's answers to rewrites) is [`petal_ui::edit_mode`], shared with every
//! host that has an edit mode; this module is Garden's half: the checkpoint,
//! its speculative-run wrapper, and which command was grabbed.
//!
//! The pieces, in the order a gesture uses them:
//!
//! 1. **Checkpoint** ([`PanelHost::set_dm_checkpoint`]). While armed, every
//!    frame that runs first forks the stack as it stands with that frame's
//!    inputs bound (one heap fork per frame that runs; a gate-skipped frame
//!    costs nothing). A speculative run from it draws the frame on screen.
//! 2. **Press** ([`PanelHost::pin_dm_checkpoint`]): the drag takes that fork
//!    for itself, so the frames that run before the pointer moves cannot
//!    replace it.
//! 3. **First move** ([`PanelHost::begin_drag_v2`]): [`EditSession::open`]
//!    finds the grabbed command's emit in a speculative baseline and opens a
//!    session on it: discovery, screening and the Jacobian, the only
//!    expensive step, bounded by [`OPEN_RUNS`] and [`OPEN_WALL`].
//! 4. **Every move** ([`PanelHost::live_drag_v2`]): one linear solve
//!    against the cached Jacobian, spliced into the press-time source and
//!    hot-reloaded into the running program in place (its literals patched,
//!    its state kept), so the next frame is the real result. The drag stays
//!    open: its own live text is never stale. ([`PanelHost::preview_drag_v2`]
//!    is the same answer without the reload.)
//! 5. **Release** ([`PanelHost::finish_drag_v2`]): verify, round and rank, with
//!    a fresh budget of its own ([`FINISH_RUNS`], [`FINISH_WALL`]), from the
//!    press-time source (the program is patched back to it first); the
//!    session's linear answer is the warm start of the verification loop.
//!    The host then writes and reloads the alternative it shows.
//! 6. **Pick** ([`PanelHost::prefer_drag_alternative`]): the author chose
//!    another alternative; its literals become the panel's sticky choice, a
//!    soft preference every later drag ranks (and previews) first when it
//!    reaches the goal.
//!
//! Everything the engine reports is converted to plain types, so the app
//! layer still needs no Petal dependency.

use petal::dm::SpecHost;
use petal::env::Env;
use petal_ui::edit_mode::{EditSession, GoalTarget, LivePreview, SessionOptions};
use petal_ui::frame_core::FrameSpecHost;

pub use petal_ui::edit_mode::{DRAG_ALTERNATIVES, DRAG_ITERS, FINISH_RUNS, FINISH_WALL, OPEN_RUNS, OPEN_WALL};

use super::{swap_edit_view_edits, swap_edit_view_texts, PanelData, PanelHost};
use crate::panel_trace::{
    native_position_args, DragAlternative, DragFilter, DragOutcomeV2, DragProperty, DragSelector, DragStart,
    SourceRewrite,
};
use crate::query::{self, QueryProvider};

/// A v2 drag in progress.
pub(super) type V2Drag = EditSession;

/// Garden's speculative-run wrapper (plan R11): the frame core's providers
/// (host data, fonts) plus Garden's own channels (the query provider, the
/// edit-view maps and the panel store), installed around each speculative
/// run exactly as [`PanelHost::frame`] installs them around a live one —
/// otherwise a query-backed panel's baseline would see nothing loaded and
/// disagree with the frame on screen.
struct PanelSpecHost<'a> {
    inner: FrameSpecHost<'a>,
    query_provider: &'a mut Option<Box<dyn QueryProvider>>,
    edit_view_texts: &'a mut std::collections::HashMap<i64, String>,
    edit_view_edits: &'a mut std::collections::HashMap<i64, PanelData>,
    store: &'a mut Option<crate::panel_store::PanelStore>,
}

impl SpecHost for PanelSpecHost<'_> {
    fn with_run_env(&mut self, env: &mut Env, f: &mut dyn FnMut(&mut Env)) {
        let q = query::swap_query_provider(self.query_provider.take());
        let t = swap_edit_view_texts(std::mem::take(self.edit_view_texts));
        let e = swap_edit_view_edits(std::mem::take(self.edit_view_edits));
        let s = crate::panel_store::swap_store(self.store.take());
        self.inner.with_run_env(env, f);
        *self.store = crate::panel_store::swap_store(s);
        *self.edit_view_edits = swap_edit_view_edits(e);
        *self.edit_view_texts = swap_edit_view_texts(t);
        *self.query_provider = query::swap_query_provider(q);
    }
}

/// Split a host into its env and a speculative-run wrapper (a macro, not a
/// method, so the borrow checker sees the disjoint fields).
macro_rules! spec_parts {
    ($host:expr) => {{
        let h = $host;
        let (env, inner) = h.core.spec_parts();
        (
            env,
            PanelSpecHost {
                inner,
                query_provider: &mut h.query_provider,
                edit_view_texts: &mut h.edit_view_texts,
                edit_view_edits: &mut h.edit_view_edits,
                store: &mut h.store,
            },
        )
    }};
}

impl PanelHost {
    /// Keep a pre-run fork of the stack for direct manipulation v2: the
    /// checkpoint a drag re-runs the frame on screen from (see the module
    /// docs). Petal IDE turns it on with tracing. It costs one heap fork per
    /// frame that runs, and nothing when off.
    pub fn set_dm_checkpoint(&mut self, on: bool) {
        self.dm_checkpoint_on = on;
        if !on {
            if let Some(ck) = self.dm_checkpoint.take() {
                self.core.env.drop_fork(ck);
            }
        }
    }

    /// Forget the checkpoint (and a pinned press): a reload replaced the
    /// program it was forked under, so it no longer describes a frame of
    /// the running code. The next frame that runs takes a fresh one.
    pub(super) fn drop_dm_checkpoint(&mut self) {
        if let Some(ck) = self.dm_checkpoint.take() {
            self.core.env.drop_fork(ck);
        }
        self.drop_pinned();
    }

    /// Whether [`set_dm_checkpoint`](Self::set_dm_checkpoint) is on.
    pub fn dm_checkpoint_armed(&self) -> bool {
        self.dm_checkpoint_on
    }

    /// A drag was pressed: keep the current checkpoint for it, so the frames
    /// that run before the pointer moves do not replace it with a later one.
    /// [`begin_drag_v2`](Self::begin_drag_v2) uses it; a press that never
    /// becomes a drag is released by [`cancel_drag_v2`](Self::cancel_drag_v2).
    pub fn pin_dm_checkpoint(&mut self) {
        if let Some(old) = self.dm_pinned.take() {
            self.core.env.drop_fork(old);
        }
        self.dm_pinned = self.dm_checkpoint.take();
    }

    /// Whether a v2 drag is open.
    pub fn drag_v2_active(&self) -> bool {
        self.dm_drag.is_some()
    }

    /// Open a v2 drag on the command at `cmd_index` of the last frame: find
    /// its emit in a speculative baseline of that frame, and measure how each
    /// literal behind it moves it. `filter` is the client's say in which
    /// literals may move.
    ///
    /// `Err` is the outcome to report instead (`Refused` with the reason, or
    /// `Stale`). Any open drag is cancelled first.
    pub fn begin_drag_v2(&mut self, cmd_index: usize, filter: &DragFilter) -> Result<DragStart, DragOutcomeV2> {
        self.begin_adjust_v2(cmd_index, DragProperty::Position, filter)
    }

    /// [`begin_drag_v2`](Self::begin_drag_v2) for any [`DragProperty`] of the
    /// shape: its position, its size or its color. The gesture's moves and
    /// release then take one delta per axis of that property
    /// ([`preview_adjust_v2`](Self::preview_adjust_v2),
    /// [`finish_adjust_v2`](Self::finish_adjust_v2)).
    pub fn begin_adjust_v2(
        &mut self,
        cmd_index: usize,
        property: DragProperty,
        filter: &DragFilter,
    ) -> Result<DragStart, DragOutcomeV2> {
        self.cancel_open_session();
        let Some(cmd) = self.last_cmds.get(cmd_index).cloned() else {
            self.drop_pinned();
            return Err(DragOutcomeV2::Stale);
        };
        let position = native_position_args(&cmd);
        if property == DragProperty::Position && position.is_none() {
            self.drop_pinned();
            return Err(DragOutcomeV2::Refused {
                why: "this shape has no position to drag".to_string(),
                candidates: Vec::new(),
            });
        }
        // Which emit of the frame this command was. Several commands can come
        // from one call site (a loop), so it is the k-th emit with this
        // command's chain, k counted among the frame's commands; without
        // tracing it is the raw command index.
        let origin = self.frame_origins.get(cmd_index).cloned();
        let occurrence = origin
            .as_ref()
            .map(|o| self.frame_origins[..cmd_index].iter().filter(|p| *p == o).count());
        let raw_index = self.frame_raw_index.get(cmd_index).copied().unwrap_or(cmd_index);

        let (checkpoint, owned) = match self.dm_pinned.take().or_else(|| self.dm_checkpoint.take()) {
            Some(ck) => (ck, true),
            // No checkpoint: the live stack after the frame. Right for a
            // still sketch; an animated one re-runs one frame further on.
            None => (self.core.stack_id(), false),
        };
        let target = GoalTarget {
            property,
            position,
            origin: origin.map(|o| o.0),
            occurrence,
            raw_index,
        };
        let prefer = self.dm_prefer.clone();
        let result = {
            let (env, mut host) = spec_parts!(&mut *self);
            let opts = SessionOptions {
                prefer: prefer.clone(),
                ..SessionOptions::default()
            };
            EditSession::open(env, &mut host, checkpoint, &target, filter, &opts)
        };
        if owned {
            self.core.env.drop_fork(checkpoint);
        }
        let drag = result?;
        let start = drag.start();
        self.dm_drag = Some(drag);
        Ok(start)
    }

    /// What the open drag would write if the pointer were released `dx`,
    /// `dy` pixels from the press: the best candidate's linear answer, with
    /// no runs. Spans are in the source the drag started from. Empty with no
    /// drag open, or when nothing is predicted to move.
    pub fn preview_drag_v2(&self, dx: f64, dy: f64) -> Vec<SourceRewrite> {
        self.preview_adjust_v2(&[dx, dy])
    }

    /// [`preview_drag_v2`](Self::preview_drag_v2) for a gesture opened by
    /// [`begin_adjust_v2`](Self::begin_adjust_v2): `deltas` has one entry per
    /// axis of its property.
    pub fn preview_adjust_v2(&self, deltas: &[f64]) -> Vec<SourceRewrite> {
        match self.dm_drag.as_ref() {
            Some(drag) => drag.preview(&self.core.env, deltas),
            None => Vec::new(),
        }
    }

    /// Live apply: one move of the open drag `dx`, `dy` pixels from the
    /// press. The linear answer ([`preview_drag_v2`](Self::preview_drag_v2))
    /// is spliced into the press-time source and hot-reloaded into the
    /// running program in place, keeping its `state`, the drag and the
    /// checkpoint; the next frame draws the real result. Returns the whole
    /// text (for the editor) and its rewrites (spans in the press-time
    /// source). `Err` with no drag open, or when the text does not compile
    /// (the program is left as it was).
    pub fn live_drag_v2(&mut self, dx: f64, dy: f64) -> Result<LivePreview, String> {
        self.live_adjust_v2(&[dx, dy])
    }

    /// [`live_drag_v2`](Self::live_drag_v2) for a gesture opened by
    /// [`begin_adjust_v2`](Self::begin_adjust_v2).
    pub fn live_adjust_v2(&mut self, deltas: &[f64]) -> Result<LivePreview, String> {
        let drag = self.dm_drag.as_mut().ok_or("no drag is open")?;
        let live = drag.preview_live(&self.core.env, deltas)?;
        self.reload_source(&live.text)?;
        Ok(live)
    }

    /// The press-time source of the open drag, if one is open: what a host
    /// writes back when it cancels a live drag.
    pub fn drag_v2_source(&self) -> Option<&str> {
        self.dm_drag.as_ref().map(|d| d.source())
    }

    /// The text the open drag last wrote live ([`live_drag_v2`](Self::live_drag_v2)),
    /// if it has.
    pub fn drag_v2_live_text(&self) -> Option<&str> {
        self.dm_drag.as_ref().and_then(|d| d.live_text())
    }

    /// Whether `text` is the open drag's own (its press-time source or the
    /// text it last wrote live), so reloading it
    /// ([`reload_source`](Self::reload_source)) keeps the drag. False with
    /// no drag open.
    pub fn drag_v2_owns_text(&self, text: &str) -> bool {
        self.dm_drag.as_ref().is_some_and(|d| d.owns_text(text))
    }

    /// Release the open drag `dx`, `dy` pixels from the press: verify, round
    /// and rank. Closes the drag either way. After a live drag the running
    /// program is back at the press-time source: the host writes (and
    /// reloads) the alternative it shows, applied to that source.
    pub fn finish_drag_v2(&mut self, dx: f64, dy: f64) -> DragOutcomeV2 {
        self.finish_adjust_v2(&[dx, dy])
    }

    /// [`finish_drag_v2`](Self::finish_drag_v2) for a gesture opened by
    /// [`begin_adjust_v2`](Self::begin_adjust_v2).
    pub fn finish_adjust_v2(&mut self, deltas: &[f64]) -> DragOutcomeV2 {
        let Some(drag) = self.dm_drag.take() else {
            return DragOutcomeV2::Stale;
        };
        let pid = self.core.program_id();
        let (env, mut host) = spec_parts!(&mut *self);
        drag.finish(env, &mut host, pid, deltas)
    }

    /// Abandon the open drag (and a pinned press), dropping their forks. A
    /// live drag's edit is taken back: the program is reloaded at the
    /// press-time source ([`drag_v2_source`](Self::drag_v2_source)).
    pub fn cancel_drag_v2(&mut self) {
        self.cancel_open_session();
        self.drop_pinned();
    }

    /// Close the open drag's session. A live drag's edit is taken back
    /// first: the program is reloaded at the press-time source (the host
    /// writes that text back into its editor).
    fn cancel_open_session(&mut self) {
        let restore = self
            .dm_drag
            .as_ref()
            .filter(|d| d.live_text().is_some_and(|t| t != d.source()))
            .map(|d| d.source().to_string());
        if let Some(source) = restore {
            // A press-time source that no longer compiles cannot happen (it
            // was running); if it somehow does, the live text stays.
            let _ = self.reload_source(&source);
        }
        if let Some(drag) = self.dm_drag.take() {
            drag.cancel(&mut self.core.env);
        }
    }

    fn drop_pinned(&mut self) {
        if let Some(ck) = self.dm_pinned.take() {
            self.core.env.drop_fork(ck);
        }
    }

    /// The author picked `alt` of a release: remember its literals as the
    /// sticky choice, preferred by every later drag on this panel (soft:
    /// a drag it cannot move falls back to the usual ranking) until another
    /// pick or [`clear_drag_preference`](Self::clear_drag_preference). An
    /// alternative with no nameable literal leaves the choice as it was.
    /// The host writes and reloads `alt`'s text itself.
    pub fn prefer_drag_alternative(&mut self, alt: &DragAlternative) {
        let prefer = alt.preference();
        if !prefer.is_empty() {
            self.dm_prefer = prefer;
        }
    }

    /// The sticky choice: the literals later drags prefer.
    pub fn drag_preference(&self) -> &[DragSelector] {
        &self.dm_prefer
    }

    /// Set the sticky choice outright (empty clears it).
    pub fn set_drag_preference(&mut self, prefer: Vec<DragSelector>) {
        self.dm_prefer = prefer;
    }

    /// Forget the sticky choice.
    pub fn clear_drag_preference(&mut self) {
        self.dm_prefer.clear();
    }

    /// The whole v2 gesture in one call (plan §4.4's
    /// `propose_drag_edits_v2`): the command at `cmd_index` should move by
    /// (`dx`, `dy`). For automation and tests; an interactive drag uses
    /// [`begin_drag_v2`](Self::begin_drag_v2) and keeps the session across
    /// moves.
    pub fn propose_drag_edits_v2(&mut self, cmd_index: usize, dx: f64, dy: f64, filter: &DragFilter) -> DragOutcomeV2 {
        if let Err(outcome) = self.begin_drag_v2(cmd_index, filter) {
            return outcome;
        }
        self.finish_drag_v2(dx, dy)
    }
}

//! Direct manipulation v2 on a panel: a drag whose goal is solved by
//! re-running the frame speculatively (docs/dev/direct-manipulation-v2-plan.md
//! §4.4), so a shape placed through any number of function calls can be
//! dragged.
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
//! 3. **First move** ([`PanelHost::begin_drag_v2`]): one probe run finds the
//!    grabbed command's [`EmitKey`] in the speculative baseline, and a
//!    [`DmSession`] opens on it: discovery, screening and the Jacobian — the
//!    only expensive step, bounded by [`OPEN_RUNS`] and [`OPEN_WALL`].
//! 4. **Every move** ([`PanelHost::preview_drag_v2`]): one linear solve
//!    against the cached Jacobian. No runs, so it keeps up with the pointer.
//! 5. **Release** ([`PanelHost::finish_drag_v2`]): verify, round and rank, with
//!    a fresh budget of its own ([`FINISH_RUNS`], [`FINISH_WALL`]); the
//!    session's linear answer is the warm start of the verification loop.
//!
//! Everything the engine reports is converted to plain `garden-script`
//! types here, so the app layer still needs no Petal dependency.

use std::time::Duration;

use petal::dm_v2::spec::SpecSession;
use petal::dm_v2::{
    CandidateReport, DmGoal, DmRequest, DmSession, EmitKey, Observable, ParamChange, ParamFilter,
    ParamSelector, PathStep, SpecConfig, SpecHost,
};
use petal::env::Env;
use petal::program::Program;
use petal::source_map::{FileId, ENTRY_FILE};
use petal::stack::StackKey;
use petal_ui::frame_core::FrameSpecHost;

use super::{swap_edit_view_edits, swap_edit_view_texts, PanelData, PanelHost};
use crate::panel_trace::{
    native_position_args, CodeSpan, DragAlternative, DragCandidate, DragFilter, DragOutcomeV2,
    DragSelector, DragStart, SourceRewrite,
};
use crate::query::{self, QueryProvider};

/// Speculative runs opening a drag may take (screening and the Jacobian).
pub const OPEN_RUNS: usize = 96;
/// Wall-clock cap on opening a drag: the pause between the pointer passing
/// the drag slop and the drag following it.
pub const OPEN_WALL: Duration = Duration::from_millis(400);
/// Runs the release may take to verify, round and rank.
pub const FINISH_RUNS: usize = 48;
/// Wall-clock cap on the release.
pub const FINISH_WALL: Duration = Duration::from_millis(400);
/// Verification steps per candidate subset (the engine's default is 6; a
/// drag lands on a whole pixel, and a linear prediction from a Jacobian
/// measured a moment ago is usually within it).
pub const DRAG_ITERS: usize = 3;
/// Alternatives a release reports.
pub const DRAG_ALTERNATIVES: usize = 3;
/// The cap on one speculative run of the frame.
const RUN_WALL: Duration = Duration::from_millis(250);

/// A v2 drag in progress.
pub(super) struct V2Drag {
    session: DmSession,
    /// Goals `0..nx` are the horizontal ones, `nx..` the vertical ones.
    nx: usize,
    /// Each goal's baseline value.
    base: Vec<f64>,
    /// The entry file's text when the session opened: a release against
    /// any other text is stale.
    source: String,
    candidates: Vec<DragCandidate>,
}

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
    pub fn begin_drag_v2(
        &mut self,
        cmd_index: usize,
        filter: &DragFilter,
    ) -> Result<DragStart, DragOutcomeV2> {
        self.cancel_open_session();
        let Some(cmd) = self.last_cmds.get(cmd_index).cloned() else {
            self.drop_pinned();
            return Err(DragOutcomeV2::Stale);
        };
        let Some((xs, ys)) = native_position_args(&cmd) else {
            self.drop_pinned();
            return Err(DragOutcomeV2::Refused {
                why: "this shape has no position to drag".to_string(),
                candidates: Vec::new(),
            });
        };
        // Which emit of the frame this command was. Several commands can come
        // from one call site (a loop), so it is the k-th emit with this
        // command's chain, k counted among the frame's commands; without
        // tracing it is the raw command index.
        let origin = self.frame_origins.get(cmd_index).cloned();
        let occurrence = origin.as_ref().map(|o| {
            self.frame_origins[..cmd_index]
                .iter()
                .filter(|p| *p == o)
                .count()
        });
        let raw_index = self
            .frame_raw_index
            .get(cmd_index)
            .copied()
            .unwrap_or(cmd_index);

        let (checkpoint, owned) = match self.dm_pinned.take().or_else(|| self.dm_checkpoint.take()) {
            Some(ck) => (ck, true),
            // No checkpoint: the live stack after the frame. Right for a
            // still sketch; an animated one re-runs one frame further on.
            None => (self.core.stack_id(), false),
        };
        let result = self.open_session(checkpoint, cmd_index, &xs, &ys, origin, occurrence, raw_index, filter);
        if owned {
            self.core.env.drop_fork(checkpoint);
        }
        let drag = result?;
        let start = DragStart {
            base_x: drag.base[..drag.nx].to_vec(),
            base_y: drag.base[drag.nx..].to_vec(),
            candidates: drag.candidates.clone(),
            runs: drag.session.runs_used(),
        };
        self.dm_drag = Some(drag);
        Ok(start)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_session(
        &mut self,
        checkpoint: StackKey,
        cmd_index: usize,
        xs: &[usize],
        ys: &[usize],
        origin: Option<crate::panel_trace::DrawOrigin>,
        occurrence: Option<usize>,
        raw_index: usize,
        filter: &DragFilter,
    ) -> Result<V2Drag, DragOutcomeV2> {
        let _ = cmd_index;
        let pid = self.core.program_id();
        let draw = self
            .core
            .env
            .intern_symbol(petal_ui::draw::DRAW_COMMANDS_SYMBOL);
        let spec_cfg = SpecConfig {
            max_wall: RUN_WALL,
            ..SpecConfig::default()
        };
        let refused = |why: String| DragOutcomeV2::Refused {
            why,
            candidates: Vec::new(),
        };

        // The probe: one baseline to learn the emit's key and the raw values
        // of its position arguments.
        let (goals, base) = {
            let (env, mut host) = spec_parts!(&mut *self);
            let probe = SpecSession::begin(env, &mut host, checkpoint, &[], spec_cfg.clone(), None)
                .map_err(|e| refused(format!("could not re-run this frame: {e}")))?;
            let emits: Vec<_> = probe.baseline().channel(draw).collect();
            let found = match (&origin, occurrence) {
                (Some(o), Some(k)) => emits.iter().filter(|e| e.chain == o.0).nth(k).copied(),
                _ => None,
            }
            .or_else(|| emits.get(raw_index).copied());
            let key: Option<EmitKey> = found.map(|e| e.key);
            let axes = key.and_then(|key| position_observables(probe.baseline(), key, xs, ys));
            probe.end(env);
            match axes {
                Some(a) => a,
                None => {
                    return Err(refused(
                        "this shape is not in a re-run of the frame (it depends on input the re-run cannot see)"
                            .to_string(),
                    ))
                }
            }
        };
        let request = DmRequest {
            goals: goals
                .into_iter()
                .zip(&base)
                .map(|(o, &b)| DmGoal::to(o, b))
                .collect(),
            filter: param_filter(filter),
            max_alternatives: DRAG_ALTERNATIVES,
            max_runs: OPEN_RUNS,
            max_wall: OPEN_WALL,
            max_iters: DRAG_ITERS,
            spec: spec_cfg,
            ..DmRequest::default()
        };
        let source = self
            .core
            .env
            .get_program(pid)
            .and_then(|p| p.source_map.files.first())
            .map(|f| f.source.clone())
            .unwrap_or_default();
        let (env, mut host) = spec_parts!(&mut *self);
        let session = DmSession::open(env, &mut host, checkpoint, &request).map_err(|e| refused(e.to_string()))?;
        let nx = xs.len();
        let candidates = {
            let program = self.core.env.get_program(pid).ok_or(DragOutcomeV2::Stale)?;
            session
                .candidates()
                .iter()
                .map(|c| candidate(program, c, nx))
                .collect()
        };
        Ok(V2Drag {
            session,
            nx,
            base,
            source,
            candidates,
        })
    }

    /// The targets of an open drag moved by (`dx`, `dy`).
    fn drag_targets(drag: &V2Drag, dx: f64, dy: f64) -> Vec<f64> {
        drag.base
            .iter()
            .enumerate()
            .map(|(i, b)| b + if i < drag.nx { dx } else { dy })
            .collect()
    }

    /// What the open drag would write if the pointer were released `dx`,
    /// `dy` pixels from the press: the best candidate's linear answer, with
    /// no runs. Spans are in the source the drag started from. Empty with no
    /// drag open, or when nothing is predicted to move.
    pub fn preview_drag_v2(&self, dx: f64, dy: f64) -> Vec<SourceRewrite> {
        let Some(drag) = self.dm_drag.as_ref() else {
            return Vec::new();
        };
        let Some(program) = self.core.env.get_program(self.core.program_id()) else {
            return Vec::new();
        };
        let targets = Self::drag_targets(drag, dx, dy);
        drag.session
            .preview(&targets)
            .iter()
            .filter_map(|c| rewrite_of(program, c, None))
            .collect()
    }

    /// Release the open drag `dx`, `dy` pixels from the press: verify, round
    /// and rank. Closes the drag either way.
    pub fn finish_drag_v2(&mut self, dx: f64, dy: f64) -> DragOutcomeV2 {
        let Some(mut drag) = self.dm_drag.take() else {
            return DragOutcomeV2::Stale;
        };
        let pid = self.core.program_id();
        let now = self
            .core
            .env
            .get_program(pid)
            .and_then(|p| p.source_map.files.first())
            .map(|f| f.source.as_str());
        if now != Some(drag.source.as_str()) {
            drag.session.cancel(&mut self.core.env);
            return DragOutcomeV2::Stale;
        }
        let targets = Self::drag_targets(&drag, dx, dy);
        drag.session.renew_budget(FINISH_RUNS, FINISH_WALL);
        let candidates = drag.candidates;
        let (env, mut host) = spec_parts!(&mut *self);
        let response = match drag.session.finish(env, &mut host, &targets) {
            Ok(r) => r,
            Err(e) => {
                return DragOutcomeV2::Refused {
                    why: e.to_string(),
                    candidates,
                }
            }
        };
        let Some(program) = self.core.env.get_program(pid) else {
            return DragOutcomeV2::Stale;
        };
        let alternatives: Vec<DragAlternative> = response
            .alternatives
            .iter()
            .filter_map(|alt| {
                let rewrites: Vec<SourceRewrite> = alt
                    .edits
                    .iter()
                    .filter_map(|e| {
                        let change = alt.changes.iter().find(|c| c.span == e.edit.span && c.file == e.file)?;
                        let mut r = rewrite_of(program, change, Some(alt.collateral.items.len()))?;
                        r.new_text = e.edit.new_text.clone();
                        Some(r)
                    })
                    .collect();
                (!rewrites.is_empty()).then(|| DragAlternative {
                    rewrites,
                    verified: alt.verified,
                    moves_others: alt.collateral.items.len(),
                    stateful: alt.stateful,
                    config_only: alt.config_only,
                    score: alt.score,
                    description: alt.description.clone(),
                })
            })
            .collect();
        if alternatives.is_empty() {
            let why = response
                .diagnostics
                .warnings
                .first()
                .cloned()
                .unwrap_or_else(|| "nothing the shape depends on can move it there".to_string());
            return DragOutcomeV2::Refused { why, candidates };
        }
        DragOutcomeV2::Alternatives {
            alternatives,
            candidates,
        }
    }

    /// Abandon the open drag (and a pinned press), dropping their forks.
    pub fn cancel_drag_v2(&mut self) {
        self.cancel_open_session();
        self.drop_pinned();
    }

    fn cancel_open_session(&mut self) {
        if let Some(drag) = self.dm_drag.take() {
            drag.session.cancel(&mut self.core.env);
        }
    }

    fn drop_pinned(&mut self) {
        if let Some(ck) = self.dm_pinned.take() {
            self.core.env.drop_fork(ck);
        }
    }

    /// The whole v2 gesture in one call (plan §4.4's
    /// `propose_drag_edits_v2`): the command at `cmd_index` should move by
    /// (`dx`, `dy`). For automation and tests; an interactive drag uses
    /// [`begin_drag_v2`](Self::begin_drag_v2) and keeps the session across
    /// moves.
    pub fn propose_drag_edits_v2(
        &mut self,
        cmd_index: usize,
        dx: f64,
        dy: f64,
        filter: &DragFilter,
    ) -> DragOutcomeV2 {
        if let Err(outcome) = self.begin_drag_v2(cmd_index, filter) {
            return outcome;
        }
        self.finish_drag_v2(dx, dy)
    }
}

/// The observables of a command's position arguments, and their baseline
/// values: x goals first, then y. A number argument is read as it is; a
/// record argument (`draw_rect(rect, color)`) through its `x` / `y` field.
fn position_observables(
    base: &petal::dm_v2::SpecRun,
    key: EmitKey,
    xs: &[usize],
    ys: &[usize],
) -> Option<(Vec<Observable>, Vec<f64>)> {
    let at = |arg: usize, path: Vec<PathStep>| Observable::EmitArg { key, arg, path };
    let mut x_goals = Vec::new();
    let mut y_goals = Vec::new();
    for (&x, &y) in xs.iter().zip(ys) {
        let (ox, oy) = match (base.read(&at(x, vec![])), base.read(&at(y, vec![]))) {
            (Some(_), Some(_)) => (at(x, vec![]), at(y, vec![])),
            // One record carries both axes.
            (None, _) => (
                at(x, vec![PathStep::Field("x".into())]),
                at(x, vec![PathStep::Field("y".into())]),
            ),
            (Some(_), None) => return None,
        };
        let (vx, vy) = (base.read(&ox)?, base.read(&oy)?);
        x_goals.push((ox, vx));
        y_goals.push((oy, vy));
    }
    Some(x_goals.into_iter().chain(y_goals).unzip())
}

/// A [`DragFilter`] in the engine's terms.
fn param_filter(filter: &DragFilter) -> ParamFilter {
    let sel = |s: &DragSelector| match s {
        DragSelector::Variable(v) => ParamSelector::Variable(v.clone()),
        DragSelector::Line(l) => ParamSelector::Line(*l as u32 + 1),
        DragSelector::Function(f) => ParamSelector::Function(f.clone()),
        DragSelector::ConfigOnly => ParamSelector::ConfigOnly,
    };
    ParamFilter {
        allow: (!filter.allow.is_empty()).then(|| filter.allow.iter().map(sel).collect()),
        deny: filter.deny.iter().map(sel).collect(),
    }
}

/// Where a file of the program is, when it is not the sketch itself.
fn file_path(program: &Program, file: FileId) -> Option<std::path::PathBuf> {
    if file == ENTRY_FILE {
        return None;
    }
    program
        .source_map
        .files
        .get(file.0 as usize)
        .and_then(|f| f.origin.clone())
        .or_else(|| Some(std::path::PathBuf::from(format!("<file {}>", file.0))))
}

/// The text of `span` in its file (spans count characters).
fn span_text(program: &Program, span: &petal::source_map::SourceSpan) -> String {
    program
        .source_map
        .files
        .get(span.file.0 as usize)
        .map(|f| {
            f.source
                .chars()
                .skip(span.start.offset as usize)
                .take(span.end.offset.saturating_sub(span.start.offset) as usize)
                .collect()
        })
        .unwrap_or_default()
}

/// One engine candidate, for a picker.
fn candidate(program: &Program, c: &CandidateReport, nx: usize) -> DragCandidate {
    let p = &c.param;
    let variable = p
        .binding
        .as_ref()
        .map(|b| b.rsplit("::").next().unwrap_or(b).to_string());
    let function = p
        .function
        .and_then(|f| program.functions.get(f.0 as usize))
        .and_then(|d| d.name.clone())
        .map(|n| {
            let n = petal::program::base_fn_name(&n).to_string();
            n.rsplit("::").next().unwrap_or(&n).to_string()
        });
    let span = (p.file == ENTRY_FILE)
        .then(|| CodeSpan::from_petal(&p.span))
        .flatten();
    let selector = match (&variable, span) {
        (Some(v), _) => DragSelector::Variable(v.clone()),
        (None, Some(s)) => DragSelector::Line(s.start_line),
        (None, None) => match &function {
            Some(f) => DragSelector::Function(f.clone()),
            None => DragSelector::Line(p.span.start.line.saturating_sub(1) as usize),
        },
    };
    DragCandidate {
        label: variable.clone().unwrap_or_else(|| span_text(program, &p.span)),
        variable,
        function,
        span,
        file: file_path(program, p.file),
        value: p.value,
        config: p.is_knob(),
        tunable: matches!(p.hint, Some(petal::dm_v2::Hint::Tunable { .. })),
        rigid: matches!(p.hint, Some(petal::dm_v2::Hint::Rigid)),
        eligible: c.eligible.is_ok(),
        why_not: c.eligible.err().map(str::to_string),
        moves: c.column.as_ref().is_some_and(|col| col.iter().any(|v| *v != 0.0)),
        moves_x: c.column.as_ref().is_some_and(|col| col.iter().take(nx).any(|v| *v != 0.0)),
        moves_y: c.column.as_ref().is_some_and(|col| col.iter().skip(nx).any(|v| *v != 0.0)),
        selector,
    }
}

/// One changed literal as a rewrite. `others` is how many other shapes the
/// alternative moves, when it was measured.
fn rewrite_of(program: &Program, c: &ParamChange, others: Option<usize>) -> Option<SourceRewrite> {
    let span = CodeSpan::from_petal(&c.span)?;
    let line = c.span.start.line;
    let description = match &c.binding {
        Some(b) => format!("set `{b}` to {} (line {line})", c.new_text),
        None => match &c.function {
            Some(f) => format!("{} → {} in `{f}` (line {line})", c.old_text, c.new_text),
            None => format!("{} → {} (line {line})", c.old_text, c.new_text),
        },
    };
    Some(SourceRewrite {
        span,
        new_text: c.new_text.clone(),
        file: file_path(program, c.file),
        variable: c.binding.clone(),
        shared: others.is_some_and(|n| n > 0),
        config: c.config,
        description,
    })
}

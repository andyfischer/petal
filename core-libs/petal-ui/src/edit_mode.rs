//! Edit mode: drag a drawn item and get a source edit, in any host
//! (docs/tasks/differentiation-rewrite.md, Design §3).
//!
//! This is the host-independent half of Garden's drag mode, shared by every
//! host that draws: Garden's panels, `petal-sdl`, and (wasm) the web canvas.
//! It has no native-only dependency. A host brings three things:
//!
//! - a **checkpoint**: a fork of the stack as it stood before the frame on
//!   screen ran, with that frame's inputs bound, so a speculative re-run of it
//!   draws the frame on screen;
//! - a [`SpecHost`]: whatever it installs around a live frame (host data,
//!   fonts, its own channels), installed around each speculative run;
//! - the grabbed item: which emit of the frame it was ([`GoalTarget`]), from
//!   the emit trace the host already keeps.
//!
//! The pieces, in the order a gesture uses them:
//!
//! 1. **Selection to allow set** ([`DragFilter::allow_lines`]): the lines the
//!    author selected become `line:N` selectors, the request's allow set. With
//!    nothing selected the filter is empty, and a host with no source view
//!    runs the request under [`Policy::BestEffort`].
//! 2. **Open** ([`EditSession::open`]): one probe run finds the grabbed emit
//!    in a speculative baseline, and a [`DmSession`] opens on it: discovery,
//!    screening and the Jacobian, the only expensive step.
//! 3. **Every move** ([`EditSession::preview`]): one linear solve against the
//!    cached Jacobian, no runs.
//! 4. **Release** ([`EditSession::finish`]): verify, round and rank, with a
//!    fresh budget. The outcome is a list of alternatives (one under best
//!    effort) or a refusal with its reason.
//! 5. **Apply** ([`apply_rewrites_to`], [`DragAlternative::apply_to`]): the
//!    chosen alternative's rewrites spliced into the source text. The host
//!    writes the text (a buffer, a file) and its hot reload follows.
//!
//! Everything the engine reports is converted to the plain types here, so a
//! host's UI layer needs no `petal::dm` vocabulary.

use std::time::Duration;

use petal::dm::spec::SpecSession;
use petal::dm::{
    CandidateReport, DmGoal, DmRequest, DmSession, EmitKey, Observable, ParamChange, ParamFilter, ParamSelector,
    PathStep, Policy, SpecConfig, SpecHost, SpecRun,
};
use petal::env::Env;
use petal::execution_context::EmitSite;
use petal::program::{Program, ProgramId};
use petal::source_map::{ENTRY_FILE, FileId, SourceSpan};
use petal::stack::StackKey;

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
/// Verification steps for a color: a color is usually computed nonlinearly
/// (a hue wheel, a mix), and a picked color lands far from the one on
/// screen, where a linear prediction is a poor first guess.
pub const COLOR_ITERS: usize = 8;
/// Alternatives a release reports.
pub const DRAG_ALTERNATIVES: usize = 3;
/// The cap on one speculative run of the frame.
pub const RUN_WALL: Duration = Duration::from_millis(250);

// ── Plain vocabulary ─────────────────────────────────────────────────────

/// A source range, in 0-based line / 0-based column (character) coordinates,
/// the ones an editor uses. (Petal spans are 1-based on both axes; the
/// conversion happens once, here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl CodeSpan {
    /// Convert a Petal span. Returns `None` for the zero span: the placeholder
    /// a term with no real source position carries, which would otherwise
    /// highlight the start of line 1 for no reason.
    pub fn from_petal(span: &SourceSpan) -> Option<CodeSpan> {
        if span.start.line == 0 {
            return None;
        }
        Some(CodeSpan {
            start_line: span.start.line as usize - 1,
            start_col: span.start.column.saturating_sub(1) as usize,
            end_line: span.end.line.max(1) as usize - 1,
            end_col: span.end.column.saturating_sub(1) as usize,
        })
    }
}

/// One text replacement a drag resolved to: the runtime's answer to "this
/// shape should have been *there*". A host applies the text and re-traces; it
/// never re-resolves an id the edit has already made stale.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRewrite {
    /// The range to replace, in 0-based editor coordinates.
    pub span: CodeSpan,
    pub new_text: String,
    /// The file the range is in when it is not the sketch itself: an
    /// imported module (a v2 drag can solve through a helper in another
    /// file). `None` for the sketch.
    pub file: Option<std::path::PathBuf>,
    /// The binding this edits, or `None` for a literal written in the call.
    pub variable: Option<String>,
    /// Whether other code reads the edited binding: the edit moves more than
    /// the shape that was grabbed.
    pub shared: bool,
    /// The edited literal is a knob (`config let`, `tunable`).
    pub config: bool,
    /// Human-readable summary ("set `offset` to 12 (line 3)").
    pub description: String,
}

/// Which literals a drag may move: the client half of disambiguation. Empty
/// `allow` means the engine's defaults (every user literal the shape depends
/// on, knobs preferred); a non-empty one is exact.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragFilter {
    pub allow: Vec<DragSelector>,
    pub deny: Vec<DragSelector>,
}

impl DragFilter {
    /// Whether this is the default filter (no client choice).
    pub fn is_default(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// The selection-to-allow-set step of edit mode: the author selected
    /// these 0-based lines of the sketch, so only literals written on them
    /// may move. Duplicates count once; no lines is the default filter (the
    /// host then runs the request best effort).
    pub fn allow_lines(lines: impl IntoIterator<Item = usize>) -> DragFilter {
        let mut allow: Vec<DragSelector> = Vec::new();
        for l in lines {
            let s = DragSelector::Line(l);
            if !allow.contains(&s) {
                allow.push(s);
            }
        }
        DragFilter { allow, deny: Vec::new() }
    }

    /// [`allow_lines`](Self::allow_lines) for a selection running from
    /// `start` to `end` (0-based lines, either order, both included).
    pub fn allow_selection(start: usize, end: usize) -> DragFilter {
        let (a, b) = if start <= end { (start, end) } else { (end, start) };
        Self::allow_lines(a..=b)
    }

    /// The filter in the engine's terms.
    pub fn to_param_filter(&self) -> ParamFilter {
        let sel = |s: &DragSelector| match s {
            DragSelector::Variable(v) => ParamSelector::Variable(v.clone()),
            DragSelector::Line(l) => ParamSelector::Line(*l as u32 + 1),
            DragSelector::Function(f) => ParamSelector::Function(f.clone()),
            DragSelector::ConfigOnly => ParamSelector::ConfigOnly,
        };
        ParamFilter {
            allow: (!self.allow.is_empty()).then(|| self.allow.iter().map(sel).collect()),
            deny: self.deny.iter().map(sel).collect(),
        }
    }
}

/// One way to name literals for a [`DragFilter`].
#[derive(Debug, Clone, PartialEq)]
pub enum DragSelector {
    /// A binding by name (`spacing`, or `module.spacing`).
    Variable(String),
    /// Any literal on this 0-based line of the sketch's own file.
    Line(usize),
    /// Literals in this function's body.
    Function(String),
    /// `config let` and `tunable(...)` literals.
    ConfigOnly,
}

impl DragSelector {
    /// Parse the selector syntax: `name`, `line:N` (1-based, as a gutter
    /// shows it), `fn:NAME`, or `config` (alias `knob`: every `config let`
    /// and `tunable` number, one tier).
    pub fn parse(s: &str) -> Option<DragSelector> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if s == "config" || s == "knob" {
            return Some(DragSelector::ConfigOnly);
        }
        if let Some(n) = s.strip_prefix("line:") {
            let n: usize = n.trim().parse().ok()?;
            return (n >= 1).then(|| DragSelector::Line(n - 1));
        }
        if let Some(f) = s.strip_prefix("fn:") {
            let f = f.trim();
            return (!f.is_empty()).then(|| DragSelector::Function(f.to_string()));
        }
        s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            .then(|| DragSelector::Variable(s.to_string()))
    }

    /// The selector in the syntax [`parse`](Self::parse) reads.
    pub fn label(&self) -> String {
        match self {
            DragSelector::Variable(v) => v.clone(),
            DragSelector::Line(l) => format!("line:{}", l + 1),
            DragSelector::Function(f) => format!("fn:{f}"),
            DragSelector::ConfigOnly => "config".to_string(),
        }
    }
}

/// One literal the engine found behind the grabbed shape (the candidate
/// report): what a picker lists so the author can say "move this one".
#[derive(Debug, Clone, PartialEq)]
pub struct DragCandidate {
    /// What to call it: the binding's name, else the literal as written.
    pub label: String,
    /// The binding it defines, if any.
    pub variable: Option<String>,
    /// The function whose body it is written in, if any.
    pub function: Option<String>,
    /// The literal's own text range, in the sketch's file; `None` for a
    /// literal in an imported file.
    pub span: Option<CodeSpan>,
    /// Where it is, for display: `None` for the sketch itself.
    pub file: Option<std::path::PathBuf>,
    pub value: f64,
    /// A `config let` knob or a `tunable(...)` literal: preferred.
    pub config: bool,
    /// Wrapped in `tunable(...)` (also counted in `config`).
    pub tunable: bool,
    /// Wrapped in `rigid(...)`: never moved.
    pub rigid: bool,
    /// May the drag move it; `why_not` says why not.
    pub eligible: bool,
    pub why_not: Option<String>,
    /// Its measured effect on the goals, when it was measured: `false` for a
    /// literal that turned out not to move the shape at all.
    pub moves: bool,
    /// Whether it moves the shape horizontally / vertically.
    pub moves_x: bool,
    pub moves_y: bool,
    /// Whether it moves each axis of the gesture's [`DragProperty`] (for a
    /// position, `[moves_x, moves_y]`; for a color, r, g and b).
    pub moves_axes: Vec<bool>,
    /// The selector that names exactly this candidate, for re-asking.
    pub selector: DragSelector,
}

/// One way to put the shape where the pointer left it.
#[derive(Debug, Clone, PartialEq)]
pub struct DragAlternative {
    /// The edits; spans are in the source the drag *started* from.
    pub rewrites: Vec<SourceRewrite>,
    /// Re-run with the rendered text and found to hit the goals.
    pub verified: bool,
    /// Other shapes this moves, measured by the verification run.
    pub moves_others: usize,
    /// Some edited literal feeds back through `state` from earlier frames.
    pub stateful: bool,
    /// Every edited literal is a `config let` or `tunable`.
    pub config_only: bool,
    /// Lower is better.
    pub score: f64,
    /// "set `gap` to 14 (line 3); moves 4 other shapes".
    pub description: String,
}

impl DragAlternative {
    /// This alternative written into `source`, the sketch's text the drag
    /// started from (rewrites into other files are left out).
    pub fn apply_to(&self, source: &str) -> Result<String, String> {
        apply_rewrites_to(source, &self.rewrites)
    }

    /// Whether every rewrite lands in the sketch itself (none in an
    /// imported file), so [`apply_to`](Self::apply_to) is the whole edit.
    pub fn sketch_only(&self) -> bool {
        self.rewrites.iter().all(|r| r.file.is_none())
    }
}

/// What a drag resolved to on release.
#[derive(Debug, Clone, PartialEq)]
pub enum DragOutcomeV2 {
    /// Ranked, best first; the first is verified when any is. `candidates`
    /// is the report a picker shows. Under [`Policy::BestEffort`] there is
    /// exactly one.
    Alternatives {
        alternatives: Vec<DragAlternative>,
        candidates: Vec<DragCandidate>,
    },
    /// Nothing can move, and why.
    Refused { why: String, candidates: Vec<DragCandidate> },
    /// The session no longer matches the running program (it was reloaded
    /// under the drag), or no drag is open.
    Stale,
}

/// What a gesture changes about the grabbed shape. A drag moves its
/// position; the same engine answers "make it this big" and "make it this
/// color", by stating goals on the arguments that carry those instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragProperty {
    /// Where it is: axes x, y.
    Position,
    /// How big it is: a circle's radius (one axis), a rectangle's or an
    /// ellipse's width and height, a line's second endpoint, a text's size.
    Size,
    /// Its fill or stroke color: axes r, g, b.
    Color,
}

impl DragProperty {
    /// What it is called in a message.
    pub fn noun(self) -> &'static str {
        match self {
            DragProperty::Position => "position",
            DragProperty::Size => "size",
            DragProperty::Color => "color",
        }
    }
}

/// What [`EditSession::open`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct DragStart {
    /// The raw native arguments a horizontal / vertical drag moves, as the
    /// baseline run computed them (unrounded: `100.3`, where the drawn
    /// command says `100`).
    pub base_x: Vec<f64>,
    pub base_y: Vec<f64>,
    /// The baseline values per axis of the gesture's [`DragProperty`]: for a
    /// position `[base_x, base_y]`, for a size the extents (`[radius]`,
    /// `[w, h]`), for a color `[[r], [g], [b]]`.
    pub axes: Vec<Vec<f64>>,
    pub candidates: Vec<DragCandidate>,
    /// Speculative runs opening the session took.
    pub runs: usize,
}

/// Splice `rewrites` (spans in `source`'s 0-based line/char-column
/// coordinates) into `source`. Rewrites for other files (`file: Some`) are
/// skipped; overlapping or out-of-range ones are an error, and duplicates
/// (two goals choosing the same edit) count once.
pub fn apply_rewrites_to(source: &str, rewrites: &[SourceRewrite]) -> Result<String, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut line_starts = vec![0usize];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line_starts.push(i + 1);
        }
    }
    let offset = |line: usize, col: usize| -> Option<usize> {
        let start = *line_starts.get(line)?;
        let end = line_starts.get(line + 1).map_or(chars.len(), |n| n - 1);
        (start + col <= end).then_some(start + col)
    };
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for r in rewrites.iter().filter(|r| r.file.is_none()) {
        let a = offset(r.span.start_line, r.span.start_col).ok_or("rewrite out of range")?;
        let b = offset(r.span.end_line, r.span.end_col).ok_or("rewrite out of range")?;
        if !spans.iter().any(|&(x, y, t)| x == a && y == b && t == r.new_text) {
            spans.push((a, b, &r.new_text));
        }
    }
    spans.sort_by_key(|&(a, _, _)| std::cmp::Reverse(a));
    for w in spans.windows(2) {
        if w[1].1 > w[0].0 {
            return Err("overlapping rewrites".to_string());
        }
    }
    let mut out = chars;
    for (a, b, text) in spans {
        out.splice(a..b, text.chars());
    }
    Ok(out.into_iter().collect())
}

/// Where a shape's position lives in the raw arguments of the `petal-ui`
/// draw native named `native`: `(x arguments, y arguments)`. A record
/// argument (`draw_rect(rect, color)`) is read through its `x` / `y` fields.
/// `None` for a native no position pair describes (polygons, polylines).
pub fn native_position_args(native: &str) -> Option<(Vec<usize>, Vec<usize>)> {
    match native {
        "draw_rect" | "draw_rect_rounded" | "draw_rect_outline" | "draw_rect_rounded_outline"
        | "draw_rect_gradient" | "draw_rect_gradient_rounded" | "draw_circle" | "draw_circle_outline"
        | "draw_circle_gradient" | "draw_ellipse" | "draw_ellipse_outline" | "draw_shadow" | "fill_arc"
        | "fill_fan" => Some((vec![0], vec![1])),
        "draw_text" | "draw_image" => Some((vec![1], vec![2])),
        "draw_line" => Some((vec![0, 2], vec![1, 3])),
        _ => None,
    }
}

// ── The goal ─────────────────────────────────────────────────────────────

/// What a gesture is aimed at: the grabbed command's property and how to
/// find its emit in a re-run of the frame.
#[derive(Debug, Clone)]
pub struct GoalTarget {
    pub property: DragProperty,
    /// The command's position arguments, when the host knows them from the
    /// command's shape. `None` asks the probe run: the emitting native's
    /// [`native_position_args`].
    pub position: Option<(Vec<usize>, Vec<usize>)>,
    /// The call chain that drew it (from the emit trace), when traced.
    pub origin: Option<EmitSite>,
    /// Which emit with that chain it was: several commands can come from
    /// one call site (a loop).
    pub occurrence: Option<usize>,
    /// Its index among the frame's raw emits: the fallback when untraced.
    pub raw_index: usize,
}

impl GoalTarget {
    /// The target for command `index` of a traced frame whose emits came
    /// from `origins` (index-aligned with the frame's raw commands).
    pub fn at(origins: &[EmitSite], index: usize, property: DragProperty) -> GoalTarget {
        let origin = origins.get(index).filter(|o| !o.chain.is_empty()).cloned();
        let occurrence = origin
            .as_ref()
            .map(|o| origins[..index].iter().filter(|p| *p == o).count());
        GoalTarget {
            property,
            position: None,
            origin,
            occurrence,
            raw_index: index,
        }
    }
}

/// Options for an [`EditSession`]: budgets, and how the edit is chosen.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub open_runs: usize,
    pub open_wall: Duration,
    pub finish_runs: usize,
    pub finish_wall: Duration,
    pub run_wall: Duration,
    pub alternatives: usize,
    /// [`Policy::Ranked`] for a host that shows the alternatives;
    /// [`Policy::BestEffort`] for one that applies the single answer.
    pub policy: Policy,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            open_runs: OPEN_RUNS,
            open_wall: OPEN_WALL,
            finish_runs: FINISH_RUNS,
            finish_wall: FINISH_WALL,
            run_wall: RUN_WALL,
            alternatives: DRAG_ALTERNATIVES,
            policy: Policy::Ranked,
        }
    }
}

impl SessionOptions {
    /// The options of a host with no source view: one answer or an error.
    pub fn best_effort() -> SessionOptions {
        SessionOptions {
            policy: Policy::BestEffort,
            alternatives: 1,
            ..SessionOptions::default()
        }
    }
}

// ── The session ──────────────────────────────────────────────────────────

/// A drag in progress: an open [`DmSession`] plus what the gesture needs to
/// turn pointer deltas into goals and engine answers into rewrites.
pub struct EditSession {
    session: DmSession,
    /// The program the session was opened under.
    program: ProgramId,
    /// Each goal's axis: the index into the deltas a move is given (for a
    /// position 0 is horizontal and 1 vertical; see [`DragProperty`]).
    axis: Vec<usize>,
    /// How many axes the gesture has.
    n_axes: usize,
    /// Each goal's baseline value.
    base: Vec<f64>,
    property: DragProperty,
    /// The entry file's text when the session opened: a release against any
    /// other text is stale.
    source: String,
    candidates: Vec<DragCandidate>,
    finish_runs: usize,
    finish_wall: Duration,
}

impl EditSession {
    /// Open a drag on `target`, re-running the frame from `checkpoint` (the
    /// caller keeps ownership of the checkpoint and may drop it once this
    /// returns). `filter` is the author's say in which literals may move.
    ///
    /// `Err` is the outcome to report instead (`Refused` with the reason, or
    /// `Stale`).
    pub fn open(
        env: &mut Env,
        host: &mut dyn SpecHost,
        checkpoint: StackKey,
        target: &GoalTarget,
        filter: &DragFilter,
        opts: &SessionOptions,
    ) -> Result<EditSession, DragOutcomeV2> {
        let pid = env.stack_program(checkpoint).ok_or(DragOutcomeV2::Stale)?;
        let draw = env.intern_symbol(crate::draw::DRAW_COMMANDS_SYMBOL);
        let spec_cfg = SpecConfig {
            max_wall: opts.run_wall,
            ..SpecConfig::default()
        };
        let refused = |why: String| DragOutcomeV2::Refused {
            why,
            candidates: Vec::new(),
        };

        // The probe: one baseline to learn the emit's key, which native drew
        // it, and the raw values of the arguments the gesture moves.
        let (goals, base, axis) = {
            let probe = SpecSession::begin(env, host, checkpoint, &[], spec_cfg.clone(), None)
                .map_err(|e| refused(format!("could not re-run this frame: {e}")))?;
            let emits: Vec<_> = probe.baseline().channel(draw).collect();
            let found = match (&target.origin, target.occurrence) {
                (Some(o), Some(k)) => emits.iter().filter(|e| e.chain == *o).nth(k).copied(),
                _ => None,
            }
            .or_else(|| emits.get(target.raw_index).copied());
            let native = found
                .and_then(|e| e.native)
                .map(|id| env.native_table().get_name(id).to_string());
            let position = target
                .position
                .clone()
                .or_else(|| native.as_deref().and_then(native_position_args));
            let goals = found.and_then(|e| match (&position, target.property) {
                (Some((xs, ys)), DragProperty::Position) => position_observables(probe.baseline(), e.key, xs, ys),
                (None, DragProperty::Position) => None,
                _ => property_observables(probe.baseline(), e.key, native.as_deref()?, target.property),
            });
            let found = found.is_some();
            probe.end(env);
            match goals {
                Some(g) => g,
                None if found => {
                    return Err(refused(format!("this shape has no {} to change", target.property.noun())));
                }
                None => {
                    return Err(refused(
                        "this shape is not in a re-run of the frame (it depends on input the re-run cannot see)"
                            .to_string(),
                    ));
                }
            }
        };
        let n_axes = axis.iter().max().map_or(0, |m| m + 1);
        let request = DmRequest {
            goals: goals.into_iter().zip(&base).map(|(o, &b)| DmGoal::to(o, b)).collect(),
            filter: filter.to_param_filter(),
            max_alternatives: opts.alternatives,
            max_runs: opts.open_runs,
            max_wall: opts.open_wall,
            max_iters: if target.property == DragProperty::Color { COLOR_ITERS } else { DRAG_ITERS },
            spec: spec_cfg,
            policy: opts.policy,
            ..DmRequest::default()
        };
        let source = entry_source(env, pid).unwrap_or_default();
        let session = DmSession::open(env, host, checkpoint, &request).map_err(|e| refused(e.to_string()))?;
        let candidates = {
            let program = env.get_program(pid).ok_or(DragOutcomeV2::Stale)?;
            session.candidates().iter().map(|c| candidate(program, c, &axis)).collect()
        };
        Ok(EditSession {
            session,
            program: pid,
            axis,
            n_axes,
            base,
            property: target.property,
            source,
            candidates,
            finish_runs: opts.finish_runs,
            finish_wall: opts.finish_wall,
        })
    }

    /// What the open found: the baseline per axis and the candidates.
    pub fn start(&self) -> DragStart {
        let axes: Vec<Vec<f64>> = (0..self.n_axes).map(|k| self.on_axis(k)).collect();
        let pos = self.property == DragProperty::Position;
        DragStart {
            base_x: if pos { self.on_axis(0) } else { Vec::new() },
            base_y: if pos { self.on_axis(1) } else { Vec::new() },
            axes,
            candidates: self.candidates.clone(),
            runs: self.session.runs_used(),
        }
    }

    fn on_axis(&self, k: usize) -> Vec<f64> {
        self.base
            .iter()
            .zip(&self.axis)
            .filter(|(_, a)| **a == k)
            .map(|(b, _)| *b)
            .collect()
    }

    /// The literals the engine found behind the shape.
    pub fn candidates(&self) -> &[DragCandidate] {
        &self.candidates
    }

    /// The sketch text the session opened on.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The program the session opened under.
    pub fn program(&self) -> ProgramId {
        self.program
    }

    /// The targets moved by `deltas`, one per axis (a missing axis does not
    /// move).
    fn targets(&self, deltas: &[f64]) -> Vec<f64> {
        self.base
            .iter()
            .zip(&self.axis)
            .map(|(b, a)| b + deltas.get(*a).copied().unwrap_or(0.0))
            .collect()
    }

    /// What the drag would write if released at `deltas` (one per axis):
    /// the best candidate's linear answer, with no runs. Spans are in the
    /// source the drag started from. Empty when nothing is predicted to move
    /// or the program is gone.
    pub fn preview(&self, env: &Env, deltas: &[f64]) -> Vec<SourceRewrite> {
        let Some(program) = env.get_program(self.program) else {
            return Vec::new();
        };
        self.session
            .preview(&self.targets(deltas))
            .iter()
            .filter_map(|c| rewrite_of(program, c, None))
            .collect()
    }

    /// Release at `deltas`: verify, round and rank, with a fresh budget.
    /// `current` is the program running now: a release under any other
    /// sketch text than the one the session opened on is `Stale`.
    pub fn finish(
        mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        current: ProgramId,
        deltas: &[f64],
    ) -> DragOutcomeV2 {
        if entry_source(env, current).as_deref() != Some(self.source.as_str()) {
            self.session.cancel(env);
            return DragOutcomeV2::Stale;
        }
        let targets = self.targets(deltas);
        self.session.renew_budget(self.finish_runs, self.finish_wall);
        let candidates = self.candidates;
        let pid = self.program;
        let response = match self.session.finish(env, host, &targets) {
            Ok(r) => r,
            Err(e) => {
                return DragOutcomeV2::Refused {
                    why: e.to_string(),
                    candidates,
                };
            }
        };
        let Some(program) = env.get_program(pid) else {
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

    /// Abandon the drag, dropping the session's forks.
    pub fn cancel(self, env: &mut Env) {
        self.session.cancel(env);
    }
}

/// The text of `pid`'s entry file.
fn entry_source(env: &Env, pid: ProgramId) -> Option<String> {
    env.get_program(pid)
        .and_then(|p| p.source_map.files.first())
        .map(|f| f.source.clone())
}

/// A gesture's goals: each observable, its baseline value and its axis.
type Goals = (Vec<Observable>, Vec<f64>, Vec<usize>);

/// The observables of a command's position arguments, and their baseline
/// values: x goals first, then y. A number argument is read as it is; a
/// record argument (`draw_rect(rect, color)`) through its `x` / `y` field.
fn position_observables(base: &SpecRun, key: EmitKey, xs: &[usize], ys: &[usize]) -> Option<Goals> {
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
        x_goals.push((ox, vx, 0));
        y_goals.push((oy, vy, 1));
    }
    let mut goals: Goals = Default::default();
    for (o, v, a) in x_goals.into_iter().chain(y_goals) {
        goals.0.push(o);
        goals.1.push(v);
        goals.2.push(a);
    }
    Some(goals)
}

/// One way a native's raw arguments can carry a property: `(argument, field
/// path, axis)` per goal, where the path reads into a record argument.
type Layout = &'static [(usize, &'static [&'static str], usize)];

/// Where a size or a color lives in the raw arguments of the native named
/// `native`, as the layouts its overloads use, tried in order (the first one
/// the run's arguments fully answer is the call's).
///
/// A size is the shape's extent from its anchor: a circle's radius (one
/// axis), a rectangle's or an ellipse's width and height, a line's second
/// endpoint (so "resizing" a line drags its end), a text's font size. A
/// color is its `r`, `g`, `b` (axes 0, 1, 2).
fn property_layouts(native: &str, property: DragProperty) -> &'static [Layout] {
    const fn rgb(r: usize) -> [(usize, &'static [&'static str], usize); 3] {
        [(r, &[], 0), (r + 1, &[], 1), (r + 2, &[], 2)]
    }
    const RGB1: Layout = &rgb(1);
    const RGB3: Layout = &rgb(3);
    const RGB4: Layout = &rgb(4);
    const RGB5: Layout = &rgb(5);
    const RGB6: Layout = &rgb(6);
    const WH2: Layout = &[(2, &[], 0), (3, &[], 1)];
    match property {
        DragProperty::Position => &[],
        DragProperty::Size => match native {
            "draw_circle" | "draw_circle_outline" => &[&[(2, &[], 0)]],
            // `draw_rect` is a native itself, with a record overload.
            "draw_rect" => &[WH2, &[(0, &["w"], 0), (0, &["h"], 1)]],
            "draw_rect_rounded" | "draw_rect_outline" | "draw_rect_rounded_outline" | "draw_ellipse"
            | "draw_ellipse_outline" | "draw_line" => &[WH2],
            "draw_image" => &[&[(3, &[], 0), (4, &[], 1)]],
            "fill_arc" => &[&[(3, &[], 0)]],
            "draw_text" => &[&[(3, &[], 0)], &[(3, &["size"], 0)]],
            _ => &[],
        },
        DragProperty::Color => match native {
            "fill_poly" | "fill_polygon" => &[RGB1],
            "draw_circle" | "draw_circle_outline" | "fill_fan" => &[RGB3],
            // `(x, y, w, h, r, g, b)`, `(x, y, w, h, color)`, `(rect, color)`.
            "draw_rect" => &[
                RGB4,
                &[(4, &["r"], 0), (4, &["g"], 1), (4, &["b"], 2)],
                &[(1, &["r"], 0), (1, &["g"], 1), (1, &["b"], 2)],
            ],
            "draw_rect_outline" | "draw_line" | "draw_ellipse" | "draw_ellipse_outline" | "draw_text" => &[RGB4],
            "draw_rect_rounded" | "draw_rect_rounded_outline" => &[RGB5],
            "fill_arc" | "fill_triangle" => &[RGB6],
            _ => &[],
        },
    }
}

/// The observables of a command's size or color (see [`property_layouts`])
/// and their baseline values. `None` when the native has no such property,
/// or the run did not hand it numbers where any layout says (`draw_text`
/// with a style record carries no color arguments).
fn property_observables(base: &SpecRun, key: EmitKey, native: &str, property: DragProperty) -> Option<Goals> {
    property_layouts(native, property).iter().find_map(|layout| {
        let mut goals: Goals = Default::default();
        for &(arg, path, axis) in layout.iter() {
            let path = path.iter().map(|f| PathStep::Field((*f).into())).collect();
            let o = Observable::EmitArg { key, arg, path };
            goals.1.push(base.read(&o)?);
            goals.0.push(o);
            goals.2.push(axis);
        }
        Some(goals)
    })
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
fn span_text(program: &Program, span: &SourceSpan) -> String {
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

/// One engine candidate, for a picker. `axis` is each goal's axis.
fn candidate(program: &Program, c: &CandidateReport, axis: &[usize]) -> DragCandidate {
    let p = &c.param;
    let variable = p.binding.as_ref().map(|b| b.rsplit("::").next().unwrap_or(b).to_string());
    let function = p
        .function
        .and_then(|f| program.functions.get(f.0 as usize))
        .and_then(|d| d.name.clone())
        .map(|n| {
            let n = petal::program::base_fn_name(&n).to_string();
            n.rsplit("::").next().unwrap_or(&n).to_string()
        });
    let span = (p.file == ENTRY_FILE).then(|| CodeSpan::from_petal(&p.span)).flatten();
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
        tunable: matches!(p.hint, Some(petal::dm::Hint::Tunable { .. })),
        rigid: matches!(p.hint, Some(petal::dm::Hint::Rigid)),
        eligible: c.eligible.is_ok(),
        why_not: c.eligible.err().map(str::to_string),
        moves: c.column.as_ref().is_some_and(|col| col.iter().any(|v| *v != 0.0)),
        moves_x: moves_on(c, axis, 0),
        moves_y: moves_on(c, axis, 1),
        moves_axes: (0..axis.iter().max().map_or(0, |m| m + 1)).map(|k| moves_on(c, axis, k)).collect(),
        selector,
    }
}

/// Whether a candidate's measured column moves any goal on axis `k`.
fn moves_on(c: &CandidateReport, axis: &[usize], k: usize) -> bool {
    c.column
        .as_ref()
        .is_some_and(|col| col.iter().zip(axis).any(|(v, a)| *a == k && *v != 0.0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_becomes_line_selectors() {
        let f = DragFilter::allow_selection(4, 2);
        assert_eq!(
            f.allow,
            vec![DragSelector::Line(2), DragSelector::Line(3), DragSelector::Line(4)]
        );
        assert_eq!(f.allow.iter().map(DragSelector::label).collect::<Vec<_>>(), ["line:3", "line:4", "line:5"]);
        let p = f.to_param_filter();
        assert_eq!(
            p.allow,
            Some(vec![ParamSelector::Line(3), ParamSelector::Line(4), ParamSelector::Line(5)])
        );
        assert!(DragFilter::allow_lines([]).is_default());
        assert_eq!(DragFilter::allow_lines([1, 1]).allow.len(), 1);
    }

    #[test]
    fn rewrites_splice_back_to_front() {
        let r = |line, a, b, t: &str| SourceRewrite {
            span: CodeSpan { start_line: line, start_col: a, end_line: line, end_col: b },
            new_text: t.into(),
            file: None,
            variable: None,
            shared: false,
            config: false,
            description: String::new(),
        };
        let src = "let a = 10\nlet b = 20\n";
        assert_eq!(apply_rewrites_to(src, &[r(0, 8, 10, "12"), r(1, 8, 10, "7")]).unwrap(), "let a = 12\nlet b = 7\n");
        assert!(apply_rewrites_to(src, &[r(0, 8, 10, "1"), r(0, 9, 10, "2")]).is_err());
    }

    #[test]
    fn natives_name_their_position_arguments() {
        assert_eq!(native_position_args("draw_circle"), Some((vec![0], vec![1])));
        assert_eq!(native_position_args("draw_text"), Some((vec![1], vec![2])));
        assert_eq!(native_position_args("draw_line"), Some((vec![0, 2], vec![1, 3])));
        assert_eq!(native_position_args("fill_poly"), None);
    }
}

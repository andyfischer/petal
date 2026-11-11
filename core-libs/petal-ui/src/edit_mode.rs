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
//!    cached Jacobian, no runs. A live host calls
//!    [`EditSession::preview_live`] instead, which splices that answer into
//!    the press-time source and records it, and hot-reloads the running
//!    program with the text (keeping state and the session): the release
//!    accepts the session's own live text and is `Stale` only after an edit
//!    made elsewhere.
//! 4. **Release** ([`EditSession::finish`]): verify, round and rank, with a
//!    fresh budget. The outcome is a list of alternatives (one under best
//!    effort) or a refusal with its reason.
//! 5. **Apply** ([`apply_rewrites_to`], [`DragAlternative::apply_to`]): the
//!    chosen alternative's rewrites spliced into the source text. The host
//!    writes the text (a buffer, a file) and its hot reload follows.
//! 6. **Sticky choice** ([`DragAlternative::preference`],
//!    [`SessionOptions::prefer`]): the alternative the author picks names its
//!    literals; the host passes them to the next drag, whose preview and
//!    ranking then favor the same literals when that reaches the goals.
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
use petal::program::{Program, ProgramId, TermOp};
use petal::provenance::{self, CallSite};
use petal::source_map::{ENTRY_FILE, FileId, SourceSpan};
use petal::stack::StackKey;

use crate::draw::DrawCommand;

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
        let sel = DragSelector::to_param_selector;
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
    /// The literal written at this 0-based line and column (anywhere inside
    /// its text) of the sketch's own file: one inline literal, where
    /// [`Line`](Self::Line) names every literal on its line.
    At { line: usize, col: usize },
}

impl DragSelector {
    /// Parse the selector syntax: `name`, `line:N` (1-based, as a gutter
    /// shows it), `fn:NAME`, `at:LINE:COL` (1-based: the one literal written
    /// there), or `config` (alias `knob`: every `config let` and `tunable`
    /// number, one tier).
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
        if let Some(at) = s.strip_prefix("at:") {
            let (l, c) = at.split_once(':')?;
            let (l, c): (usize, usize) = (l.trim().parse().ok()?, c.trim().parse().ok()?);
            return (l >= 1 && c >= 1).then(|| DragSelector::At { line: l - 1, col: c - 1 });
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
            DragSelector::At { line, col } => format!("at:{}:{}", line + 1, col + 1),
        }
    }

    /// The selector in the engine's terms.
    pub fn to_param_selector(&self) -> ParamSelector {
        match self {
            DragSelector::Variable(v) => ParamSelector::Variable(v.clone()),
            DragSelector::Line(l) => ParamSelector::Line(*l as u32 + 1),
            DragSelector::Function(f) => ParamSelector::Function(f.clone()),
            DragSelector::ConfigOnly => ParamSelector::ConfigOnly,
            DragSelector::At { line, col } => {
                ParamSelector::At { file: None, line: *line as u32 + 1, column: *col as u32 + 1 }
            }
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
    /// Every edited literal is one the session's sticky preference names
    /// ([`SessionOptions::prefer`]): this alternative is the author's
    /// earlier choice made again.
    pub preferred: bool,
    /// Lower is better.
    pub score: f64,
    /// "set `gap` to 14 (line 3); moves 4 other shapes".
    pub description: String,
    /// The literals this alternative edits, named so that a later drag can
    /// find them again *after this alternative is written*: the binding's
    /// name, else (in the sketch) the literal's position in the edited text,
    /// else the function it is written in. What
    /// [`preference`](Self::preference) returns.
    pub selectors: Vec<DragSelector>,
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

    /// The sticky choice this alternative makes when the author picks it:
    /// pass it as [`SessionOptions::prefer`] to the next drag, so that drag
    /// moves the same literals when that verifies (a soft preference,
    /// never a filter). Empty when nothing it edits can be named.
    pub fn preference(&self) -> Vec<DragSelector> {
        self.selectors.clone()
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
    /// The sticky choice: literals the author picked on an earlier drag
    /// ([`DragAlternative::preference`]). A soft preference: the preview
    /// and the ranking favor an edit of only these literals when it reaches
    /// the goals, and fall back to the usual ranking when none does. Empty:
    /// no preference.
    pub prefer: Vec<DragSelector>,
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
            prefer: Vec::new(),
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
    /// The entry file's text when the session opened (`source0`): a release
    /// against any text but this one or [`live`](Self::live) is stale.
    source: String,
    /// The text the drag last wrote live ([`preview_live`](Self::preview_live),
    /// [`set_live_text`](Self::set_live_text)): the session's own edit, which
    /// the host reloads under the drag and which is therefore not stale.
    live: Option<String>,
    /// The literals' slot constants when the session opened: a live reload
    /// that recompiled the program (rather than patching its literals)
    /// leaves them behind, and the session no longer describes it.
    slots: Vec<(petal::program::TermId, petal::constant_table::ConstantId)>,
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
            preferred: opts.prefer.iter().map(DragSelector::to_param_selector).collect(),
            ..DmRequest::default()
        };
        let source = entry_source(env, pid).unwrap_or_default();
        let session = DmSession::open(env, host, checkpoint, &request).map_err(|e| refused(e.to_string()))?;
        let (candidates, slots) = {
            let program = env.get_program(pid).ok_or(DragOutcomeV2::Stale)?;
            let candidates = session.candidates().iter().map(|c| candidate(program, c, &axis)).collect();
            (candidates, slot_constants(program, session.candidates()))
        };
        Ok(EditSession {
            session,
            program: pid,
            axis,
            n_axes,
            base,
            property: target.property,
            source,
            live: None,
            slots,
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

    /// Live apply: the sketch text the drag would write if released at
    /// `deltas` ([`preview`](Self::preview) spliced into the source the drag
    /// started from), recorded as the session's live text so that a host
    /// may reload the running program with it under the drag (keeping its
    /// state) without the release turning `Stale`. Each move is computed
    /// from the press-time source and the total deltas, never from the last
    /// live text. With nothing predicted to move, the press-time source.
    pub fn preview_live(&mut self, env: &Env, deltas: &[f64]) -> Result<LivePreview, String> {
        let rewrites = self.preview(env, deltas);
        let text = apply_rewrites_to(&self.source, &rewrites)?;
        self.live = Some(text.clone());
        Ok(LivePreview { text, rewrites })
    }

    /// Record `text` as the session's live text: a host that writes the
    /// drag's edit under it by its own means (an alternative re-applied
    /// mid-gesture) says so here, so the release accepts it.
    pub fn set_live_text(&mut self, text: impl Into<String>) {
        self.live = Some(text.into());
    }

    /// The text the drag last wrote live, if any.
    pub fn live_text(&self) -> Option<&str> {
        self.live.as_deref()
    }

    /// Whether `text` is the session's own: the press-time source or the
    /// last live text. Anything else is an edit made elsewhere under the
    /// drag.
    pub fn owns_text(&self, text: &str) -> bool {
        text == self.source || self.live.as_deref() == Some(text)
    }

    /// Release at `deltas`: verify, round and rank, with a fresh budget,
    /// against the source the drag started from (rewrites are spans in it).
    /// `current` is the program running now: a release under any sketch
    /// text but the press-time one or the session's last live text
    /// ([`preview_live`](Self::preview_live)) is `Stale`.
    ///
    /// When the host reloaded the session's own program with the live text
    /// (an in-place hot reload), the program is first patched back to the
    /// press-time text, since the verification runs re-run the frame the
    /// drag started on; the host writes the chosen alternative (applied to
    /// [`source`](Self::source)) and reloads afterwards, as it would anyway.
    /// A live reload that had to recompile the program (rather than patch
    /// its literals) leaves the session describing a program that is gone:
    /// `Stale`.
    pub fn finish(
        mut self,
        env: &mut Env,
        host: &mut dyn SpecHost,
        current: ProgramId,
        deltas: &[f64],
    ) -> DragOutcomeV2 {
        let owned = entry_source(env, current).is_some_and(|t| self.owns_text(&t));
        if !owned || !self.restore_press_source(env) {
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
                    selectors: selectors_after(&alt.changes, &rewrites),
                    rewrites,
                    verified: alt.verified,
                    moves_others: alt.collateral.items.len(),
                    stateful: alt.stateful,
                    config_only: alt.config_only,
                    preferred: alt.preferred,
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

    /// Abandon the drag, dropping the session's forks. A host that wrote
    /// live text writes [`source`](Self::source) back itself.
    pub fn cancel(self, env: &mut Env) {
        self.session.cancel(env);
    }

    /// Put the session's program back at the press-time text when a live
    /// reload changed it in place. `false` when that cannot be done by
    /// patching literals, or the program was recompiled under the session.
    fn restore_press_source(&self, env: &mut Env) -> bool {
        let Some(now) = entry_source(env, self.program) else {
            return false;
        };
        if now == self.source {
            return self.slots_intact(env);
        }
        if self.live.as_deref() != Some(now.as_str()) || !self.slots_intact(env) {
            return false;
        }
        let Ok(stack) = env.create_stack(self.program) else {
            return false;
        };
        let change = env.diff_program(self.program, &self.source);
        let ok = env.apply_program_change(stack, &change).is_ok();
        env.drop_fork(stack);
        ok && entry_source(env, self.program).as_deref() == Some(self.source.as_str())
    }

    /// Whether the literals the session slotted are still the slots they
    /// were (a recompile rebuilds the constant table).
    fn slots_intact(&self, env: &Env) -> bool {
        let Some(program) = env.get_program(self.program) else {
            return false;
        };
        self.slots.iter().all(|&(t, c)| {
            matches!(program.terms.get(t.0 as usize).map(|t| &t.op), Some(TermOp::Constant(now)) if *now == c)
        })
    }
}

/// What [`EditSession::preview_live`] predicts: the whole sketch text to
/// reload under the drag, and the rewrites it is made of (spans in the
/// press-time source).
#[derive(Debug, Clone, PartialEq)]
pub struct LivePreview {
    pub text: String,
    pub rewrites: Vec<SourceRewrite>,
}

/// The slot constants of the candidates' literals, as the open left them.
fn slot_constants(
    program: &Program,
    candidates: &[CandidateReport],
) -> Vec<(petal::program::TermId, petal::constant_table::ConstantId)> {
    candidates
        .iter()
        .filter_map(|c| match program.terms.get(c.param.term.0 as usize).map(|t| &t.op) {
            Some(TermOp::Constant(k)) if program.constants.is_slot(*k) => Some((c.param.term, *k)),
            _ => None,
        })
        .collect()
}

/// Selectors naming the literals `changes` edit, as they will be found in
/// the text once `rewrites` (the same edits, in the sketch's coordinates)
/// are written: a binding by name; an inline literal of the sketch at its
/// start in the edited text (a rewrite earlier on the same line shifts it);
/// else the function it is written in.
fn selectors_after(changes: &[ParamChange], rewrites: &[SourceRewrite]) -> Vec<DragSelector> {
    let mut out: Vec<DragSelector> = Vec::new();
    for c in changes {
        let sel = if let Some(b) = &c.binding {
            Some(DragSelector::Variable(b.rsplit("::").next().unwrap_or(b).to_string()))
        } else if c.file == ENTRY_FILE
            && let Some(span) = CodeSpan::from_petal(&c.span)
        {
            let shift: isize = rewrites
                .iter()
                .filter(|r| r.file.is_none() && r.span.start_line == span.start_line && r.span.end_line == span.start_line)
                .filter(|r| r.span.end_col <= span.start_col)
                .map(|r| r.new_text.chars().count() as isize - (r.span.end_col - r.span.start_col) as isize)
                .sum();
            Some(DragSelector::At { line: span.start_line, col: (span.start_col as isize + shift).max(0) as usize })
        } else {
            c.function.clone().map(DragSelector::Function)
        };
        if let Some(s) = sel
            && !out.contains(&s)
        {
            out.push(s);
        }
    }
    out
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

// ── Naming the call that drew an item ────────────────────────────────────

/// The call in the sketch an emit came through: what a host shows when the
/// author clicks a drawn item, and the line it jumps a source view to.
#[derive(Debug, Clone, PartialEq)]
pub struct CallLabel {
    /// The function called (`draw_circle`, or a helper of the sketch's own).
    pub callee: String,
    /// The call's 1-based line in the sketch, as a gutter shows it.
    pub line: usize,
    /// That line's text, trimmed.
    pub text: String,
}

impl CallLabel {
    /// "`draw_circle` at line 7: draw_circle(x, 100, 20, ...)".
    pub fn label(&self) -> String {
        format!("`{}` at line {}: {}", self.callee, self.line, self.text)
    }
}

/// The frame of `origin`'s call chain in the sketch's own file (the
/// outermost call written there), resolved to its call site. `None` for an
/// emit with no traced chain or no source position.
pub fn describe_call(env: &Env, program_id: ProgramId, origin: &EmitSite) -> Option<CallLabel> {
    let program = env.get_program(program_id)?;
    let term = provenance::pick_frame(program, &origin.chain, ENTRY_FILE)?;
    let site = CallSite::resolve(program, term)?;
    let span = site.span?;
    let text = program
        .source_map
        .files
        .get(span.file.0 as usize)
        .and_then(|f| f.source.lines().nth(span.start.line.saturating_sub(1) as usize))
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Some(CallLabel {
        callee: site.callee.unwrap_or_else(|| "call".to_string()),
        line: span.start.line as usize,
        text,
    })
}

// ── Geometry over draw commands ──────────────────────────────────────────

/// Which command paints the point: the last one covering it, in paint order,
/// honoring clips and skipping offscreen targets (see the hit-test notes in
/// docs/features/direct-manipulation.md).
pub fn hit_test(cmds: &[DrawCommand], x: i32, y: i32) -> Option<usize> {
    let mut clip: Option<(i32, i32, i32, i32)> = None;
    let mut saved: Vec<Option<(i32, i32, i32, i32)>> = Vec::new();
    let mut target = 0u32;
    let mut hit = None;
    for (i, cmd) in cmds.iter().enumerate() {
        match cmd {
            DrawCommand::Clip { x: cx, y: cy, w, h, .. } => {
                clip = Some((*cx, *cy, *cx + *w as i32, *cy + *h as i32));
                continue;
            }
            DrawCommand::ClipNone => {
                clip = None;
                continue;
            }
            DrawCommand::ClipPush { x: cx, y: cy, w, h, .. } => {
                saved.push(clip);
                let want = (*cx, *cy, *cx + *w as i32, *cy + *h as i32);
                clip = Some(match clip {
                    Some(c) => (c.0.max(want.0), c.1.max(want.1), c.2.min(want.2), c.3.min(want.3)),
                    None => want,
                });
                continue;
            }
            DrawCommand::ClipPop => {
                clip = saved.pop().flatten();
                continue;
            }
            DrawCommand::SetTarget { id } => {
                target = *id;
                continue;
            }
            _ => {}
        }
        if target != 0 {
            continue;
        }
        if let Some((x0, y0, x1, y1)) = clip {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                continue;
            }
        }
        if contains(cmd, x, y) {
            hit = Some(i);
        }
    }
    hit
}

fn in_rect(rx: i32, ry: i32, w: u32, h: u32, x: i32, y: i32) -> bool {
    x >= rx && x < rx + w as i32 && y >= ry && y < ry + h as i32
}

fn near_segment(x1: i32, y1: i32, x2: i32, y2: i32, x: i32, y: i32, tol: f64) -> bool {
    let (ax, ay, bx, by, px, py) = (x1 as f64, y1 as f64, x2 as f64, y2 as f64, x as f64, y as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let (qx, qy) = (ax + t * dx, ay + t * dy);
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt() <= tol
}

fn in_polygon(points: &[(i32, i32)], x: i32, y: i32) -> bool {
    let mut inside = false;
    let n = points.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (x as f64, y as f64);
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (points[i].0 as f64, points[i].1 as f64);
        let (xj, yj) = (points[j].0 as f64, points[j].1 as f64);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether one command's painted geometry covers the point. A full-canvas
/// clear never matches, an outline is hit on its stroke, and lines get a
/// few pixels of tolerance.
fn contains(cmd: &DrawCommand, x: i32, y: i32) -> bool {
    match cmd {
        DrawCommand::Rect { x: rx, y: ry, w, h, .. }
        | DrawCommand::RectGradient { x: rx, y: ry, w, h, .. }
        | DrawCommand::Image { x: rx, y: ry, w, h, .. }
        | DrawCommand::DrawCanvas { x: rx, y: ry, w, h, .. } => in_rect(*rx, *ry, *w, *h, x, y),
        DrawCommand::RectOutline { x: rx, y: ry, w, h, width, .. } => {
            let t = (*width).max(1) as i32 + 2;
            in_rect(*rx - 1, *ry - 1, *w + 2, *h + 2, x, y)
                && !in_rect(*rx + t, *ry + t, w.saturating_sub(2 * t as u32), h.saturating_sub(2 * t as u32), x, y)
        }
        DrawCommand::Circle { cx, cy, radius, .. } | DrawCommand::CircleGradient { cx, cy, radius, .. } => {
            let (dx, dy) = ((x - cx) as i64, (y - cy) as i64);
            dx * dx + dy * dy <= (*radius as i64) * (*radius as i64)
        }
        DrawCommand::Text { text, x: tx, y: ty, size, .. } => {
            let w = (text.chars().count() as f64 * *size as f64 * 0.55) as u32;
            in_rect(*tx, *ty, w.max(4), (*size as u32).max(4), x, y)
        }
        DrawCommand::Line { x1, y1, x2, y2, width, .. } => {
            near_segment(*x1, *y1, *x2, *y2, x, y, 3.0 + *width as f64 / 2.0)
        }
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, .. } => in_polygon(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)], x, y),
        DrawCommand::Poly { points, .. } | DrawCommand::Polygon { points, .. } => in_polygon(points, x, y),
        _ => false,
    }
}

/// The axis-aligned box of a command.
pub fn bounds(cmd: &DrawCommand) -> Option<(i32, i32, u32, u32)> {
    match cmd {
        DrawCommand::Rect { x, y, w, h, .. }
        | DrawCommand::RectOutline { x, y, w, h, .. }
        | DrawCommand::RectGradient { x, y, w, h, .. }
        | DrawCommand::Image { x, y, w, h, .. }
        | DrawCommand::DrawCanvas { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
        DrawCommand::Circle { cx, cy, radius, .. } | DrawCommand::CircleGradient { cx, cy, radius, .. } => {
            Some((cx - radius, cy - radius, (*radius as u32) * 2, (*radius as u32) * 2))
        }
        DrawCommand::Text { text, x, y, size, .. } => {
            let w = (text.chars().count() as f64 * *size as f64 * 0.55) as u32;
            Some((*x, *y, w.max(1), *size as u32))
        }
        DrawCommand::Line { x1, y1, x2, y2, .. } => Some((
            *x1.min(x2),
            *y1.min(y2),
            (x1 - x2).unsigned_abs().max(1),
            (y1 - y2).unsigned_abs().max(1),
        )),
        DrawCommand::Triangle { x1, y1, x2, y2, x3, y3, .. } => {
            poly_bounds(&[(*x1, *y1), (*x2, *y2), (*x3, *y3)])
        }
        DrawCommand::Poly { points, .. } | DrawCommand::Polygon { points, .. } => poly_bounds(points),
        _ => None,
    }
}

fn poly_bounds(points: &[(i32, i32)]) -> Option<(i32, i32, u32, u32)> {
    let x0 = points.iter().map(|p| p.0).min()?;
    let y0 = points.iter().map(|p| p.1).min()?;
    let x1 = points.iter().map(|p| p.0).max()?;
    let y1 = points.iter().map(|p| p.1).max()?;
    Some((x0, y0, (x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32))
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

    // ── Live drags and sticky choices ────────────────────────────────────

    use petal::dm::NoHost;

    /// Two literals move the circle horizontally: `x` and `off`.
    const TWO_WAYS: &str = "import ui\nlet x = 100\nlet off = 0\ndraw_circle(x + off, 80, 20, 200, 100, 50)\n";

    /// An env running `src` for one frame: `(env, program, stack)`. The
    /// stack after the frame is the checkpoint (a still sketch).
    fn running(src: &str) -> (Env, ProgramId, StackKey) {
        let mut env = Env::new();
        crate::input::register_input(&mut env);
        crate::draw::register_draw(&mut env);
        crate::draw::register_canvas(&mut env);
        crate::register_prelude(&mut env);
        crate::provide_interface(&mut env);
        env.set_echo(false);
        let pid = env.load_program(src).unwrap();
        let stack = env.create_stack(pid).unwrap();
        frame(&mut env, stack);
        (env, pid, stack)
    }

    fn frame(env: &mut Env, stack: StackKey) {
        crate::draw::clear_draw_commands(env);
        env.reset_stack(stack).unwrap();
        env.run(stack).unwrap();
    }

    fn circle() -> GoalTarget {
        GoalTarget { property: DragProperty::Position, position: None, origin: None, occurrence: None, raw_index: 0 }
    }

    fn open(env: &mut Env, stack: StackKey, prefer: Vec<DragSelector>) -> EditSession {
        let opts = SessionOptions { prefer, ..SessionOptions::default() };
        EditSession::open(env, &mut NoHost, stack, &circle(), &DragFilter::default(), &opts)
            .unwrap_or_else(|o| panic!("open: {o:?}"))
    }

    fn alternatives(o: DragOutcomeV2) -> Vec<DragAlternative> {
        match o {
            DragOutcomeV2::Alternatives { alternatives, .. } => alternatives,
            o => panic!("expected alternatives: {o:?}"),
        }
    }

    /// The variables an alternative (or a preview) edits, sorted.
    fn vars(rewrites: &[SourceRewrite]) -> Vec<String> {
        let mut v: Vec<String> = rewrites.iter().filter_map(|r| r.variable.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn live_text_does_not_make_the_release_stale() {
        let (mut env, pid, stack) = running(TWO_WAYS);
        let mut s = open(&mut env, stack, Vec::new());
        // Each move reloads the running program in place with the live text.
        let mut last = String::new();
        for dx in [5.0, 12.0, 25.0] {
            let live = s.preview_live(&env, &[dx, 0.0]).unwrap();
            assert_ne!(live.text, TWO_WAYS, "a move writes an edit");
            assert!(!live.rewrites.is_empty());
            env.reload_program(stack, &live.text, None).unwrap();
            frame(&mut env, stack);
            last = live.text;
        }
        assert_eq!(s.live_text(), Some(last.as_str()));
        assert!(s.owns_text(TWO_WAYS) && s.owns_text(&last) && !s.owns_text("let x = 1\n"));
        let source0 = s.source().to_string();
        let alts = alternatives(s.finish(&mut env, &mut NoHost, pid, &[25.0, 0.0]));
        assert!(alts[0].verified, "{alts:?}");
        // Spans are in the press-time source, and the program was put back
        // there for the verification.
        let top = alts[0].apply_to(&source0).unwrap();
        assert!(top.contains("let x = 125") || top.contains("let off = 25"), "{top}");
        assert_eq!(entry_source(&env, pid).as_deref(), Some(TWO_WAYS));
        // The host then writes the choice.
        env.reload_program(stack, &top, None).unwrap();
        frame(&mut env, stack);
    }

    #[test]
    fn an_external_edit_under_a_live_drag_is_still_stale() {
        let (mut env, pid, stack) = running(TWO_WAYS);
        let mut s = open(&mut env, stack, Vec::new());
        let live = s.preview_live(&env, &[10.0, 0.0]).unwrap();
        env.reload_program(stack, &live.text, None).unwrap();
        // Someone else edits the sketch.
        let other = TWO_WAYS.replace("let off = 0", "let off = 3");
        env.reload_program(stack, &other, None).unwrap();
        assert_eq!(s.finish(&mut env, &mut NoHost, pid, &[10.0, 0.0]), DragOutcomeV2::Stale);
    }

    #[test]
    fn a_picked_alternative_is_preferred_on_the_next_drag() {
        let (mut env, pid, stack) = running(TWO_WAYS);
        let s = open(&mut env, stack, Vec::new());
        let first = alternatives(s.finish(&mut env, &mut NoHost, pid, &[25.0, 0.0]));
        assert!(first.len() >= 2, "both literals are ways to do it: {first:?}");
        assert!(first.iter().all(|a| !a.preferred), "no preference yet");
        let top = vars(&first[0].rewrites);
        // The author picks another verified alternative that moves one literal.
        let pick = first
            .iter()
            .find(|a| a.verified && a.rewrites.len() == 1 && vars(&a.rewrites) != top)
            .unwrap_or_else(|| panic!("a single-literal second choice: {first:?}"));
        let prefer = pick.preference();
        assert_eq!(prefer.len(), 1, "{prefer:?}");
        let picked = vars(&pick.rewrites);
        // Written, and the program reloaded with it.
        let text = pick.apply_to(TWO_WAYS).unwrap();
        env.reload_program(stack, &text, None).unwrap();
        frame(&mut env, stack);
        // The next drag of the same shape: the pick now ranks first.
        let s = open(&mut env, stack, prefer.clone());
        let preview = s.preview(&env, &[-10.0, 0.0]);
        let next = alternatives(s.finish(&mut env, &mut NoHost, pid, &[-10.0, 0.0]));
        assert!(next[0].preferred && next[0].verified, "{next:?}");
        assert_eq!(vars(&next[0].rewrites), picked, "{next:?}");
        assert_eq!(vars(&preview), picked, "the live preview prefers it too");
        // A preference that names nothing this drag can move changes
        // nothing.
        let s = open(&mut env, stack, vec![DragSelector::Variable("nowhere".into())]);
        let fallback = alternatives(s.finish(&mut env, &mut NoHost, pid, &[-10.0, 0.0]));
        assert!(fallback[0].verified && !fallback[0].preferred, "{fallback:?}");
        assert_eq!(vars(&fallback[0].rewrites), top, "the usual ranking: {fallback:?}");
    }

    #[test]
    fn the_preview_agrees_with_the_releases_first_choice() {
        for src in [
            TWO_WAYS,
            "import ui\nlet gap = 30\nfor i in range(0, 3) do\n  draw_circle(50 + i * gap, 80, 10, 200, 100, 50)\nend\n",
            "import ui\nconfig let left = 40\ndraw_circle(left + 60, 80, 20, 200, 100, 50)\n",
        ] {
            let (mut env, pid, stack) = running(src);
            let s = open(&mut env, stack, Vec::new());
            let preview = s.preview(&env, &[20.0, 0.0]);
            let alts = alternatives(s.finish(&mut env, &mut NoHost, pid, &[20.0, 0.0]));
            let spans = |r: &[SourceRewrite]| {
                let mut v: Vec<(usize, usize)> = r.iter().map(|r| (r.span.start_line, r.span.start_col)).collect();
                v.sort();
                v
            };
            assert_eq!(spans(&preview), spans(&alts[0].rewrites), "{src}: {preview:?} vs {alts:?}");
        }
    }

    #[test]
    fn an_inline_literal_is_named_where_the_edit_leaves_it() {
        let (mut env, pid, stack) = running("import ui\ndraw_circle(100, 80, 20, 200, 100, 50)\n");
        let s = open(&mut env, stack, Vec::new());
        let alts = alternatives(s.finish(&mut env, &mut NoHost, pid, &[905.0, 0.0]));
        assert_eq!(alts[0].preference(), vec![DragSelector::At { line: 1, col: 12 }], "{alts:?}");
        assert_eq!(DragSelector::parse("at:2:13"), Some(DragSelector::At { line: 1, col: 12 }));
        assert_eq!(DragSelector::At { line: 1, col: 12 }.label(), "at:2:13");
    }
}

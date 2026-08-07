//! Direct manipulation — turning a *goal* about an observed value into
//! concrete source edits.
//!
//! [`crate::provenance`] answers "which code produced this value, and where are
//! its arguments written?". This module answers the question a direct-
//! manipulation host asks next: **"this argument should have been X — what do I
//! edit to make that true?"**
//!
//! The request is goal-based, in the same spirit as
//! [`crate::goal_based_editing`]: the caller states the outcome ("argument 2 of
//! the call that emitted draw command 7 should be 55") and this module answers
//! with the edit — or with *several candidate edits* when several variables
//! feed the value, leaving the choice to the caller. A host narrows the
//! candidates by declaring variables **configurable** (prefer editing these) or
//! **static** (never edit these) via [`VarPolicy`]; with enough policy the
//! request resolves to a single edit.
//!
//! # What a proposal can do
//!
//! - **A literal in the call** (`draw_circle(120, …)`): one proposal, replacing
//!   the number in place.
//! - **A binding** (`let r = 30 … draw_circle(x, y, r)`): one proposal at the
//!   *definition*, flagged [`shared`](EditProposal::shared) when other code
//!   reads the same binding — the host should surface that before applying.
//! - **A computed argument** (`draw_circle(x + offset, …)`): the expression is
//!   walked and, for each literal-backed leaf, the arithmetic between the leaf
//!   and the argument is *inverted* using the values the run actually saw (from
//!   the [`TraceBuffer`](crate::trace::TraceBuffer)), yielding one proposal per
//!   leaf: "set `offset` to 12.5" and "set `x`'s literal to 30" are both ways
//!   to make `x + offset` equal 42.5, and both are returned.
//!
//! Inversion covers `+ - * / neg` chains — the shape of essentially every
//! position/size expression in a sketch. Anything else (a call, a field read,
//! `%`) ends the walk down that branch: proposing an edit there would be a
//! guess, and a wrong guess silently rewrites the user's code to mean something
//! else.
//!
//! Proposals carry spans plus replacement text and do **not** touch the file;
//! applying (and re-running, and re-tracing) is the host's move — with
//! [`apply_edits`] doing the mechanical splice for hosts that want it. See
//! `docs/direct-manipulation.md` for the full protocol.
//!
//! Two extensions ride on the same shapes: [`propose_edits_batch`] resolves a
//! *list* of goals consistently (a drag changes x and y in one gesture), and a
//! `config let` binding in the source acts as a default [`VarPolicy`] — config
//! bindings are the preferred edit targets, everything else is pinned — so a
//! bare drag on a script that declares its tuning knobs needs no policy
//! round-trip at all.

use std::collections::HashMap;

use crate::program::{Program, TermId, TermOp};
use crate::provenance::{self, ArgKind, CallSite};
use crate::source_map::SourceSpan;
use crate::static_value::StaticValue;
use crate::trace::TraceBuffer;

/// How a host classifies a variable for edit resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarPolicy {
    /// This variable is meant to be tuned: prefer proposals that edit it. When
    /// any proposal touches a configurable variable, non-configurable
    /// alternatives are dropped.
    Configurable,
    /// This variable must not change: proposals that edit it are discarded.
    Static,
}

/// One concrete text replacement: put `new_text` where `span` is.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceEdit {
    pub span: SourceSpan,
    pub new_text: String,
}

/// One way to satisfy a [`ManipulationGoal`]: a single replacement, plus the
/// context a host needs to choose between alternatives.
#[derive(Debug, Clone)]
pub struct EditProposal {
    /// The replacement to apply.
    pub edit: SourceEdit,
    /// The name of the binding this edits, when the literal lives behind one.
    /// `None` for a literal written directly in the call.
    pub variable: Option<String>,
    /// The literal term being rewritten (for callers that want to re-resolve).
    pub term: TermId,
    /// Whether other code also reads the edited binding, so this edit moves
    /// more than the value the goal named. Always `false` for a call-site
    /// literal.
    pub shared: bool,
    /// The edited binding was declared `config let` — the source itself names
    /// it as a tuning knob. Hosts can render these as sliders; when a program
    /// declares any config binding, [`propose_edits`] already prefers them
    /// (see the policy rules on that function).
    pub config: bool,
    /// Human-readable summary ("set `offset` to 12.5 (line 3)").
    pub description: String,
}

/// A goal about one argument of a traced call: after the edit, evaluating this
/// argument should yield `new_value`.
#[derive(Debug, Clone)]
pub struct ManipulationGoal {
    /// The call term — normally the frame [`provenance::pick_frame`] chose from
    /// an emit's chain.
    pub term: TermId,
    /// 0-based argument position (receiver of a method call not counted).
    pub arg_index: usize,
    /// The value the argument should evaluate to.
    pub new_value: StaticValue,
}

/// Why no proposals could be produced. Distinct from "zero proposals after
/// policy filtering", which is a legitimate empty `Vec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManipulationError {
    pub message: String,
}

impl std::fmt::Display for ManipulationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ManipulationError {}

fn err(message: impl Into<String>) -> ManipulationError {
    ManipulationError {
        message: message.into(),
    }
}

/// Propose source edits that satisfy `goal`, most direct first.
///
/// `trace` supplies the values the run actually computed, which is what lets a
/// computed argument be *solved* rather than refused: inverting `x + offset`
/// for `offset` needs the value `x` had. Pass the buffer from the traced run
/// that produced the emit; with `None`, only statically-known sibling values
/// can be used and fewer computed arguments resolve.
///
/// `policy` names variables the host has pinned. [`VarPolicy::Static`] removes
/// proposals; [`VarPolicy::Configurable`] makes its proposals win over
/// unpinned ones when both exist.
///
/// An empty result means every candidate was filtered or the argument doesn't
/// trace to editable text (e.g. it flows through a call); the error case is
/// reserved for a goal that can't be evaluated at all (stale term, bad index,
/// non-numeric target for an arithmetic solve).
pub fn propose_edits(
    program: &Program,
    goal: &ManipulationGoal,
    trace: Option<&TraceBuffer>,
    policy: &HashMap<String, VarPolicy>,
) -> Result<Vec<EditProposal>, ManipulationError> {
    let site = CallSite::resolve(program, goal.term)
        .ok_or_else(|| err(format!("term t{} is stale for this program", goal.term.0)))?;
    let arg = site.args.get(goal.arg_index).ok_or_else(|| {
        err(format!(
            "call has {} argument(s), no index {}",
            site.args.len(),
            goal.arg_index
        ))
    })?;

    let mut proposals = match arg.kind {
        // Directly editable: one proposal, at the literal (call site or
        // definition — `editable_span` already points at the right one).
        ArgKind::Literal | ArgKind::Binding => {
            let literal_term = arg.literal_term.unwrap_or(arg.term);
            let span = arg
                .editable_span(program)
                .ok_or_else(|| err("the argument has no source span to edit"))?;
            let info = (arg.kind == ArgKind::Binding)
                .then(|| binding_info(program, arg.term))
                .flatten();
            let config =
                info.as_ref().is_some_and(|i| i.1) || term_is_config(program, literal_term);
            vec![make_proposal(
                program,
                span,
                literal_term,
                info.map(|i| i.0),
                config,
                render_matching(arg.literal, &goal.new_value),
            )]
        }
        // Computed: solve the expression for each editable leaf.
        ArgKind::Computed => {
            let target = goal.new_value.as_f64().ok_or_else(|| {
                err("the argument is computed; solving it needs a numeric target value")
            })?;
            let mut out = Vec::new();
            solve(program, trace, arg.term, target, None, 0, &mut out);
            out
        }
    };

    apply_policy(program, &mut proposals, policy);
    Ok(proposals)
}

/// The policy filter shared by single and batch proposal calls: drop Static
/// proposals, and let Configurable ones displace the rest.
///
/// Explicit host policy (by variable name) always wins. When the program
/// declares any `config let` binding, undeclared variables get a *default*
/// from the source itself: config bindings are Configurable and every other
/// named binding is Static — so a bare drag on a script with declared tuning
/// knobs resolves to the knob with no dialog. Call-site literals carry no
/// variable and stay unpinned either way. A program with no config bindings
/// gets no defaults, which is the pre-`config` behavior.
fn apply_policy(
    program: &Program,
    proposals: &mut Vec<EditProposal>,
    policy: &HashMap<String, VarPolicy>,
) {
    let source_declares_knobs = program.terms.iter().any(|t| t.is_config);
    let policy_of = |p: &EditProposal| -> Option<VarPolicy> {
        let name = p.variable.as_deref()?;
        if let Some(&explicit) = policy.get(name) {
            return Some(explicit);
        }
        source_declares_knobs.then(|| {
            if p.config {
                VarPolicy::Configurable
            } else {
                VarPolicy::Static
            }
        })
    };
    proposals.retain(|p| policy_of(p) != Some(VarPolicy::Static));
    if proposals
        .iter()
        .any(|p| policy_of(p) == Some(VarPolicy::Configurable))
    {
        proposals.retain(|p| policy_of(p) == Some(VarPolicy::Configurable));
    }
}

/// Propose edits for several goals that must hold *together* — the multi-goal
/// form of [`propose_edits`], for gestures that change more than one value at
/// once (a drag moves x and y in one motion).
///
/// Each goal is resolved with the same trace and the same policy (one policy
/// round-trip covers the whole gesture), then the per-goal candidate lists are
/// filtered for **consistency**: a proposal survives only if one proposal per
/// goal can be chosen alongside it such that no two chosen edits collide.
/// Two edits collide when they touch overlapping source ranges with different
/// replacement text; identical edits (two goals both moving the same `config
/// let` to the same value) are compatible and deduplicated at apply time by
/// [`apply_edits`].
///
/// The result is index-aligned with `goals`. A goal whose argument doesn't
/// trace to editable text yields an empty list — that refusal stands on its
/// own and does not veto the other goals' proposals. A goal that can't be
/// evaluated at all (stale term, bad index) fails the whole batch, since the
/// caller's addressing is wrong, not just unlucky.
pub fn propose_edits_batch(
    program: &Program,
    goals: &[ManipulationGoal],
    trace: Option<&TraceBuffer>,
    policy: &HashMap<String, VarPolicy>,
) -> Result<Vec<Vec<EditProposal>>, ManipulationError> {
    if goals.is_empty() {
        return Err(err("a batch needs at least one goal"));
    }
    let mut per_goal: Vec<Vec<EditProposal>> = Vec::with_capacity(goals.len());
    for (i, goal) in goals.iter().enumerate() {
        let ps = propose_edits(program, goal, trace, policy)
            .map_err(|e| err(format!("goal {}: {}", i, e.message)))?;
        per_goal.push(ps);
    }

    // Consistency: keep a proposal only if a full, collision-free selection
    // (one proposal per non-empty goal) exists that includes it. Candidate
    // lists are a handful long and gestures carry two or three goals, so an
    // exhaustive search is the simple and honest check.
    let survives = |goal_idx: usize, prop_idx: usize| -> bool {
        let mut chosen: Vec<&EditProposal> = vec![&per_goal[goal_idx][prop_idx]];
        fn pick<'a>(
            per_goal: &'a [Vec<EditProposal>],
            skip: usize,
            next: usize,
            chosen: &mut Vec<&'a EditProposal>,
        ) -> bool {
            let Some(candidates) = per_goal.get(next) else {
                return true;
            };
            if next == skip || candidates.is_empty() {
                return pick(per_goal, skip, next + 1, chosen);
            }
            for c in candidates {
                if chosen.iter().all(|p| edits_compatible(&p.edit, &c.edit)) {
                    chosen.push(c);
                    if pick(per_goal, skip, next + 1, chosen) {
                        return true;
                    }
                    chosen.pop();
                }
            }
            false
        }
        pick(&per_goal, goal_idx, 0, &mut chosen)
    };

    let keep: Vec<Vec<bool>> = per_goal
        .iter()
        .enumerate()
        .map(|(gi, ps)| (0..ps.len()).map(|pi| survives(gi, pi)).collect())
        .collect();
    for (ps, keep) in per_goal.iter_mut().zip(keep) {
        let mut it = keep.into_iter();
        ps.retain(|_| it.next().unwrap());
    }
    Ok(per_goal)
}

/// Whether two edits can both apply: identical (same range, same text) or
/// touching disjoint ranges. Overlapping ranges with different text collide.
fn edits_compatible(a: &SourceEdit, b: &SourceEdit) -> bool {
    let (sa, sb) = (a.span, b.span);
    if sa.start.offset == sb.start.offset && sa.end.offset == sb.end.offset {
        return a.new_text == b.new_text;
    }
    sa.end.offset <= sb.start.offset || sb.end.offset <= sa.start.offset
}

/// Apply a set of chosen edits to `source` in one pass: duplicates (the same
/// edit chosen by two goals) collapse to one, and the survivors are spliced
/// back-to-front so earlier spans stay valid. Colliding edits are an error —
/// a batch filtered by [`propose_edits_batch`] never produces them, so hitting
/// this means the caller mixed proposals from different batches.
pub fn apply_edits(source: &str, edits: &[SourceEdit]) -> Result<String, ManipulationError> {
    let mut unique: Vec<&SourceEdit> = Vec::new();
    for e in edits {
        if unique.iter().any(|u| {
            u.span.start.offset == e.span.start.offset
                && u.span.end.offset == e.span.end.offset
                && u.new_text == e.new_text
        }) {
            continue;
        }
        if let Some(clash) = unique.iter().find(|u| !edits_compatible(u, e)) {
            return Err(err(format!(
                "edits collide: two different replacements for overlapping text ({:?} vs {:?})",
                clash.new_text, e.new_text
            )));
        }
        unique.push(e);
    }
    unique.sort_by(|a, b| b.span.start.offset.cmp(&a.span.start.offset));
    let mut out = source.to_string();
    for e in unique {
        out = crate::rewrite::splice(&out, e.span, &e.new_text);
    }
    Ok(out)
}

/// Build one proposal, computing `shared` from how many terms read the edited
/// definition. A call-site literal has exactly one reader (the call), so the
/// flag naturally stays `false` there.
fn make_proposal(
    program: &Program,
    span: SourceSpan,
    literal_term: TermId,
    variable: Option<String>,
    config: bool,
    new_text: String,
) -> EditProposal {
    let shared = variable.is_some() && reader_count(program, literal_term) > 1;
    let where_ = if span.start.line > 0 {
        format!(" (line {})", span.start.line)
    } else {
        String::new()
    };
    let description = match &variable {
        Some(name) => format!("set `{name}` to {new_text}{where_}"),
        None => format!("replace the literal with {new_text}{where_}"),
    };
    EditProposal {
        edit: SourceEdit { span, new_text },
        variable,
        term: literal_term,
        shared,
        config,
        description,
    }
}

/// How many terms use `id` as an input — the fan-out of a definition. More
/// than one reader means an edit at the definition moves other code too.
fn reader_count(program: &Program, id: TermId) -> usize {
    program
        .terms
        .iter()
        .filter(|t| t.inputs.contains(&id))
        .count()
}

/// The name the binding behind `arg` was declared with (walking the identity
/// chain a name reference compiles to and taking the first named term), plus
/// whether that declaration carried the `config` modifier.
fn binding_info(program: &Program, arg: TermId) -> Option<(String, bool)> {
    let mut cur = arg;
    for _ in 0..16 {
        let t = program.terms.get(cur.0 as usize)?;
        if let Some(name) = &t.name {
            return Some((
                name.rsplit("::").next().unwrap_or(name).to_string(),
                t.is_config,
            ));
        }
        cur = provenance::alias_target(program, cur)?;
    }
    None
}

/// Whether `id` is a term whose binding was declared `config let`.
fn term_is_config(program: &Program, id: TermId) -> bool {
    program
        .terms
        .get(id.0 as usize)
        .is_some_and(|t| t.is_config)
}

/// Render `new_value` the way the argument was already spelled: an integer slot
/// keeps `55` as `55`, a float slot renders `55` as `55.0` — so the diff is the
/// value change and nothing else.
fn render_matching(old: Option<provenance::Literal>, new_value: &StaticValue) -> String {
    match (old, new_value) {
        (Some(lit), StaticValue::Float(f)) if lit.is_int && f.fract() == 0.0 => {
            format!("{}", *f as i64)
        }
        (Some(lit), StaticValue::Int(n)) if !lit.is_int => format!("{:?}", *n as f64),
        _ => new_value.to_source(),
    }
}

/// Bound on the arithmetic-inversion recursion; expressions in real sketches
/// are a handful of nodes deep, the bound just keeps a malformed graph finite.
const MAX_SOLVE_DEPTH: usize = 32;

/// Walk the expression under `term`, and for every literal-backed leaf emit a
/// proposal that sets that leaf so the whole expression evaluates to `target`.
///
/// At each arithmetic node the walk recurses into one operand with the target
/// adjusted by the *current* value of the other — read from the trace when the
/// run recorded it, else from a static constant. A sibling whose value is
/// unknown ends that branch: without it there is nothing to invert against.
fn solve(
    program: &Program,
    trace: Option<&TraceBuffer>,
    term: TermId,
    target: f64,
    var: Option<(String, bool)>,
    depth: usize,
    out: &mut Vec<EditProposal>,
) {
    if depth > MAX_SOLVE_DEPTH {
        return;
    }

    let Some(t) = program.terms.get(term.0 as usize) else {
        return;
    };

    // Carry the innermost binding name seen on the way down — the definition's
    // constant often carries the name itself — so a proposal at the leaf can
    // say *which variable* it edits. The `config` flag rides along: it may sit
    // on a named intermediate (`config let n = 5 + 2`) rather than the leaf.
    let var_here = t
        .name
        .as_ref()
        .map(|n| (n.rsplit("::").next().unwrap_or(n).to_string(), t.is_config))
        .or(var);

    // A literal leaf — written here or reached through a binding chain.
    if let Some(lit) = provenance::literal_at(program, term) {
        if let Some(span) = program.source_map.get(term).copied() {
            let new_text = render_matching(
                Some(lit),
                &if target.fract() == 0.0 {
                    StaticValue::Int(target as i64)
                } else {
                    StaticValue::Float(target)
                },
            );
            let config = var_here.as_ref().is_some_and(|v| v.1) || t.is_config;
            out.push(make_proposal(
                program,
                span,
                term,
                var_here.map(|v| v.0),
                config,
                new_text,
            ));
        }
        return;
    }

    match t.op {
        TermOp::Copy | TermOp::StateInit => {
            if let Some(next) = t.inputs.first().copied() {
                solve(program, trace, next, target, var_here, depth + 1, out);
            }
        }
        TermOp::Neg => {
            if let Some(inner) = t.inputs.first().copied() {
                solve(program, trace, inner, -target, var_here, depth + 1, out);
            }
        }
        TermOp::Add | TermOp::Sub | TermOp::Mul | TermOp::Div => {
            let (Some(&a), Some(&b)) = (t.inputs.first(), t.inputs.get(1)) else {
                return;
            };
            let va = current_value(program, trace, a);
            let vb = current_value(program, trace, b);
            // Solve for the left operand using the right's current value…
            if let Some(vb) = vb
                && let Some(sub_target) = invert_left(t.op.clone(), target, vb)
            {
                solve(program, trace, a, sub_target, None, depth + 1, out);
            }
            // …and for the right operand using the left's.
            if let Some(va) = va
                && let Some(sub_target) = invert_right(t.op.clone(), target, va)
            {
                solve(program, trace, b, sub_target, None, depth + 1, out);
            }
        }
        // Anything else — a call, a comparison, a field read — is not
        // invertible without guessing. Stop; absence is the honest answer.
        _ => {}
    }
}

/// Given `a OP b = target` and `b`'s value, the value `a` must take.
fn invert_left(op: TermOp, target: f64, b: f64) -> Option<f64> {
    let v = match op {
        TermOp::Add => target - b,
        TermOp::Sub => target + b,
        TermOp::Mul => {
            if b == 0.0 {
                return None;
            }
            target / b
        }
        TermOp::Div => target * b,
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// Given `a OP b = target` and `a`'s value, the value `b` must take.
fn invert_right(op: TermOp, target: f64, a: f64) -> Option<f64> {
    let v = match op {
        TermOp::Add => target - a,
        TermOp::Sub => a - target,
        TermOp::Mul => {
            if a == 0.0 {
                return None;
            }
            target / a
        }
        TermOp::Div => {
            if target == 0.0 {
                return None;
            }
            a / target
        }
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// The numeric value `term` had in the traced run — from the trace's most
/// recent event for it, falling back to a statically-known constant. `None`
/// when neither knows, which ends a solve down that branch.
///
/// "Most recent" is a deliberate simplification: a term inside a loop takes
/// many values and this reads the last. The protocol handles that by re-running
/// and re-tracing after each applied edit, so a stale inversion shows up
/// immediately rather than compounding.
fn current_value(program: &Program, trace: Option<&TraceBuffer>, term: TermId) -> Option<f64> {
    if let Some(trace) = trace
        && let Some(event) = trace.last_for_term(term)
        && let Some(v) = event.result.as_f64()
    {
        return Some(v);
    }
    // Statically: a literal here, or through the alias chain.
    let mut cur = term;
    for _ in 0..16 {
        if let Some(lit) = provenance::literal_at(program, cur) {
            return Some(lit.value);
        }
        cur = provenance::alias_target(program, cur)?;
    }
    None
}

impl StaticValue {
    /// This value as an f64 when it is numeric — the form the arithmetic
    /// solver works in.
    fn as_f64(&self) -> Option<f64> {
        match self {
            StaticValue::Int(n) => Some(*n as f64),
            StaticValue::Float(f) => Some(*f),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;
    use crate::program::ProgramId;

    /// Compile and run `source` with tracing on, returning the env and program
    /// id — the state a host holds when it asks for proposals.
    fn run_traced(source: &str) -> (Env, ProgramId) {
        let mut env = Env::new();
        env.trace_mut().enable();
        let pid = env.load_program(source).expect("compiles");
        let sid = env.create_stack(pid).expect("stack");
        env.run(sid).expect("runs");
        (env, pid)
    }

    /// The first builtin-call term — the `print` these tests emit through.
    fn call_term(program: &Program) -> TermId {
        program
            .terms
            .iter()
            .find(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .expect("a builtin call")
            .id
    }

    fn propose(
        env: &Env,
        pid: ProgramId,
        arg_index: usize,
        new_value: StaticValue,
        policy: &HashMap<String, VarPolicy>,
    ) -> Vec<EditProposal> {
        let program = env.get_program(pid).unwrap();
        let goal = ManipulationGoal {
            term: call_term(program),
            arg_index,
            new_value,
        };
        propose_edits(program, &goal, Some(env.trace()), policy).expect("proposes")
    }

    /// Apply a single proposal to the source, for end-to-end assertions.
    fn apply(source: &str, p: &EditProposal) -> String {
        crate::rewrite::splice(source, p.edit.span, &p.edit.new_text)
    }

    #[test]
    fn a_call_site_literal_yields_one_direct_edit() {
        let src = "print(120)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(55), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert!(ps[0].variable.is_none());
        assert!(!ps[0].shared);
        assert_eq!(apply(src, &ps[0]), "print(55)\n");
    }

    #[test]
    fn a_binding_edit_lands_on_the_definition_and_reports_sharing() {
        let src = "let r = 30\nprint(r)\nprint(r)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(40), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("r"));
        assert!(ps[0].shared, "two prints read `r`");
        assert_eq!(apply(src, &ps[0]), "let r = 40\nprint(r)\nprint(r)\n");
    }

    #[test]
    fn a_computed_argument_yields_one_proposal_per_variable() {
        // x + offset = 30; the goal is 42.5. Either variable can move:
        // x -> 32.5 (offset stayed 10) or offset -> 22.5 (x stayed 20).
        let src = "let x = 20\nlet offset = 10\nprint(x + offset)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Float(42.5), &HashMap::new());
        let mut by_var: Vec<(Option<&str>, &str)> = ps
            .iter()
            .map(|p| (p.variable.as_deref(), p.edit.new_text.as_str()))
            .collect();
        by_var.sort();
        assert_eq!(by_var, vec![(Some("offset"), "22.5"), (Some("x"), "32.5")]);
    }

    #[test]
    fn policy_narrows_computed_candidates_to_one() {
        let src = "let x = 20\nlet offset = 10\nprint(x + offset)\n";
        let (env, pid) = run_traced(src);

        // Pinning `x` static leaves only `offset`.
        let policy = HashMap::from([("x".to_string(), VarPolicy::Static)]);
        let ps = propose(&env, pid, 0, StaticValue::Int(50), &policy);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("offset"));
        assert_eq!(
            apply(src, &ps[0]),
            "let x = 20\nlet offset = 30\nprint(x + offset)\n"
        );

        // Marking `offset` configurable prefers it over the unpinned `x`.
        let policy = HashMap::from([("offset".to_string(), VarPolicy::Configurable)]);
        let ps = propose(&env, pid, 0, StaticValue::Int(50), &policy);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("offset"));
    }

    #[test]
    fn multiplication_and_negation_invert() {
        // 2 * scale = 8 with scale = 4; goal 10 -> scale = 5 (and the literal
        // 2 -> 2.5). Neg: -(w) printed, goal -3 -> w = 3.
        let src = "let scale = 4\nprint(2 * scale)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(10), &HashMap::new());
        let texts: Vec<(Option<&str>, &str)> = ps
            .iter()
            .map(|p| (p.variable.as_deref(), p.edit.new_text.as_str()))
            .collect();
        assert!(texts.contains(&(Some("scale"), "5")), "got {texts:?}");
        assert!(texts.contains(&(None, "2.5")), "got {texts:?}");

        let src = "let w = 7\nprint(-w)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(-3), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("w"));
        assert_eq!(apply(src, &ps[0]), "let w = 3\nprint(-w)\n");
    }

    #[test]
    fn a_string_goal_rewrites_a_string_argument() {
        let src = "print(\"hello\")\n";
        let (env, pid) = run_traced(src);
        let ps = propose(
            &env,
            pid,
            0,
            StaticValue::Str("bye".into()),
            &HashMap::new(),
        );
        assert_eq!(ps.len(), 1);
        assert_eq!(apply(src, &ps[0]), "print(\"bye\")\n");
    }

    #[test]
    fn integer_spelling_is_preserved_for_whole_float_goals() {
        // A drag hands back floats; an arg written as `120` must come back as
        // `55`, not `55.0`.
        let src = "print(120)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Float(55.0), &HashMap::new());
        assert_eq!(ps[0].edit.new_text, "55");
    }

    #[test]
    fn an_uninvertible_expression_yields_no_proposals() {
        // The argument flows through a call; nothing is safely editable.
        let src = "fn f(n) n * 2 end\nlet x = 3\nprint(f(x))\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(10), &HashMap::new());
        assert!(ps.is_empty(), "got {ps:?}");
    }

    /// All builtin-call terms, in program order — for tests that address a
    /// specific call among several.
    fn call_terms(program: &Program) -> Vec<TermId> {
        program
            .terms
            .iter()
            .filter(|t| matches!(t.op, TermOp::BuiltinCall(_)))
            .map(|t| t.id)
            .collect()
    }

    #[test]
    fn a_config_binding_wins_with_no_policy_at_all() {
        // The whole point of `config let`: the source names its tuning knob,
        // so a bare goal resolves to one edit with no dialog.
        let src = "config let offset = 10\nlet x = 20\nprint(x + offset)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Float(42.5), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("offset"));
        assert!(ps[0].config);
        assert_eq!(
            apply(src, &ps[0]),
            "config let offset = 22.5\nlet x = 20\nprint(x + offset)\n"
        );
    }

    #[test]
    fn explicit_policy_overrides_config_defaults() {
        // The host pins the knob and frees `x`: its word beats the source's.
        let src = "config let offset = 10\nlet x = 20\nprint(x + offset)\n";
        let (env, pid) = run_traced(src);
        let policy = HashMap::from([
            ("offset".to_string(), VarPolicy::Static),
            ("x".to_string(), VarPolicy::Configurable),
        ]);
        let ps = propose(&env, pid, 0, StaticValue::Float(42.5), &policy);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("x"));
    }

    #[test]
    fn config_defaults_do_not_touch_call_site_literals() {
        // A program that declares a knob elsewhere: the direct literal in this
        // call carries no variable, so it stays editable.
        let src = "config let k = 1\nprint(120)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(55), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert!(ps[0].variable.is_none());
    }

    #[test]
    fn a_non_config_binding_is_pinned_once_the_source_declares_knobs() {
        // `r` feeds the argument, but the script declared `k` as the knob, so
        // a bare goal on `r + k` must not offer to move `r`.
        let src = "config let k = 1\nlet r = 30\nprint(r + k)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(40), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].variable.as_deref(), Some("k"));
        assert_eq!(
            apply(src, &ps[0]),
            "config let k = 10\nlet r = 30\nprint(r + k)\n"
        );
    }

    #[test]
    fn a_binding_argument_reports_its_config_flag() {
        let src = "config let r = 30\nprint(r)\n";
        let (env, pid) = run_traced(src);
        let ps = propose(&env, pid, 0, StaticValue::Int(40), &HashMap::new());
        assert_eq!(ps.len(), 1);
        assert!(ps[0].config);
        assert_eq!(apply(src, &ps[0]), "config let r = 40\nprint(r)\n");
    }

    #[test]
    fn a_batch_resolves_two_goals_against_one_policy() {
        // The x+y drag: one gesture, two arguments, one policy round-trip.
        let src = "let x = 20\nlet y = 30\nlet dx = 1\nlet dy = 2\nprint(x + dx, y + dy)\n";
        let (env, pid) = run_traced(src);
        let program = env.get_program(pid).unwrap();
        let term = call_term(program);
        let goals = vec![
            ManipulationGoal {
                term,
                arg_index: 0,
                new_value: StaticValue::Int(31),
            },
            ManipulationGoal {
                term,
                arg_index: 1,
                new_value: StaticValue::Int(42),
            },
        ];
        let policy = HashMap::from([
            ("x".to_string(), VarPolicy::Static),
            ("y".to_string(), VarPolicy::Static),
        ]);
        let per_goal = propose_edits_batch(program, &goals, Some(env.trace()), &policy).unwrap();
        assert_eq!(per_goal.len(), 2);
        assert_eq!(per_goal[0].len(), 1);
        assert_eq!(per_goal[0][0].variable.as_deref(), Some("dx"));
        assert_eq!(per_goal[1].len(), 1);
        assert_eq!(per_goal[1][0].variable.as_deref(), Some("dy"));

        let edits: Vec<SourceEdit> = per_goal.iter().map(|ps| ps[0].edit.clone()).collect();
        assert_eq!(
            apply_edits(src, &edits).unwrap(),
            "let x = 20\nlet y = 30\nlet dx = 11\nlet dy = 12\nprint(x + dx, y + dy)\n"
        );
    }

    #[test]
    fn inconsistent_batch_candidates_are_filtered() {
        // Goal 1 can only be met by moving `a` (to 15). Goal 0 could move `a`
        // (to 11) or the literal `1` — but `a` can't be both 11 and 15, so the
        // batch drops goal 0's `a` branch and keeps the literal.
        let src = "let a = 10\nprint(a + 1, a)\n";
        let (env, pid) = run_traced(src);
        let program = env.get_program(pid).unwrap();
        let term = call_term(program);
        let goals = vec![
            ManipulationGoal {
                term,
                arg_index: 0,
                new_value: StaticValue::Int(12),
            },
            ManipulationGoal {
                term,
                arg_index: 1,
                new_value: StaticValue::Int(15),
            },
        ];
        let per_goal =
            propose_edits_batch(program, &goals, Some(env.trace()), &HashMap::new()).unwrap();
        // Goal 0: only the call-site literal branch survives, retargeted so
        // `a + 1` still hits 12 pairing with... no — the literal keeps its own
        // solve (a stayed 10 in the traced run): 1 -> 2.
        assert_eq!(per_goal[0].len(), 1);
        assert!(per_goal[0][0].variable.is_none());
        assert_eq!(per_goal[0][0].edit.new_text, "2");
        // Goal 1: the binding edit stands.
        assert_eq!(per_goal[1].len(), 1);
        assert_eq!(per_goal[1][0].variable.as_deref(), Some("a"));
        assert_eq!(per_goal[1][0].edit.new_text, "15");
    }

    #[test]
    fn identical_edits_from_two_goals_are_compatible_and_dedupe() {
        // Both goals move the same binding to the same value — one edit.
        let src = "let a = 10\nprint(a, a)\n";
        let (env, pid) = run_traced(src);
        let program = env.get_program(pid).unwrap();
        let term = call_term(program);
        let goal = |arg_index| ManipulationGoal {
            term,
            arg_index,
            new_value: StaticValue::Int(25),
        };
        let per_goal = propose_edits_batch(
            program,
            &[goal(0), goal(1)],
            Some(env.trace()),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(per_goal[0].len(), 1);
        assert_eq!(per_goal[1].len(), 1);
        let edits: Vec<SourceEdit> = per_goal.iter().map(|ps| ps[0].edit.clone()).collect();
        assert_eq!(
            apply_edits(src, &edits).unwrap(),
            "let a = 25\nprint(a, a)\n"
        );
    }

    #[test]
    fn a_refused_goal_does_not_veto_the_rest_of_the_batch() {
        // Goal 1's argument flows through a call — no proposals, which is its
        // own honest answer; goal 0 keeps its edit.
        let src = "fn f(n) n * 2 end\nlet x = 3\nprint(120, f(x))\n";
        let (env, pid) = run_traced(src);
        let program = env.get_program(pid).unwrap();
        let term = *call_terms(program).last().expect("the print call");
        let goals = vec![
            ManipulationGoal {
                term,
                arg_index: 0,
                new_value: StaticValue::Int(55),
            },
            ManipulationGoal {
                term,
                arg_index: 1,
                new_value: StaticValue::Int(10),
            },
        ];
        let per_goal =
            propose_edits_batch(program, &goals, Some(env.trace()), &HashMap::new()).unwrap();
        assert_eq!(per_goal[0].len(), 1);
        assert!(per_goal[1].is_empty());
    }

    #[test]
    fn an_empty_batch_is_an_error() {
        let (env, pid) = run_traced("print(1)\n");
        let program = env.get_program(pid).unwrap();
        assert!(propose_edits_batch(program, &[], Some(env.trace()), &HashMap::new()).is_err());
    }

    #[test]
    fn apply_edits_splices_back_to_front_and_rejects_collisions() {
        let src = "let a = 1\nlet b = 2\n";
        let span = |start: u32, end: u32| {
            let mut s = SourceSpan::default();
            s.start.offset = start;
            s.end.offset = end;
            s
        };
        // "1" is at offset 8, "2" at offset 18.
        let e1 = SourceEdit {
            span: span(8, 9),
            new_text: "10".into(),
        };
        let e2 = SourceEdit {
            span: span(18, 19),
            new_text: "20".into(),
        };
        assert_eq!(
            apply_edits(src, &[e1.clone(), e2.clone()]).unwrap(),
            "let a = 10\nlet b = 20\n"
        );
        // Same edits in the other order: back-to-front splicing makes order
        // irrelevant.
        assert_eq!(
            apply_edits(src, &[e2.clone(), e1.clone()]).unwrap(),
            "let a = 10\nlet b = 20\n"
        );
        // Overlap with different text is a collision.
        let clash = SourceEdit {
            span: span(8, 9),
            new_text: "99".into(),
        };
        assert!(apply_edits(src, &[e1, clash]).is_err());
    }

    #[test]
    fn a_bad_arg_index_is_an_error_not_an_empty_answer() {
        let (env, pid) = run_traced("print(1)\n");
        let program = env.get_program(pid).unwrap();
        let goal = ManipulationGoal {
            term: call_term(program),
            arg_index: 5,
            new_value: StaticValue::Int(1),
        };
        assert!(propose_edits(program, &goal, None, &HashMap::new()).is_err());
    }

    #[test]
    fn a_stale_term_is_an_error() {
        let (env, pid) = run_traced("print(1)\n");
        let program = env.get_program(pid).unwrap();
        let goal = ManipulationGoal {
            term: TermId(u32::MAX),
            arg_index: 0,
            new_value: StaticValue::Int(1),
        };
        assert!(propose_edits(program, &goal, None, &HashMap::new()).is_err());
    }
}

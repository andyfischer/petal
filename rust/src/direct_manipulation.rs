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
//! applying (and re-running, and re-tracing) is the host's move. See
//! `docs/direct-manipulation.md` for the full protocol.

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
            let variable = (arg.kind == ArgKind::Binding)
                .then(|| binding_name(program, arg.term))
                .flatten();
            vec![make_proposal(
                program,
                span,
                literal_term,
                variable,
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

    // Policy: drop Static, and let Configurable proposals displace the rest.
    proposals.retain(|p| {
        p.variable
            .as_deref()
            .is_none_or(|v| policy.get(v) != Some(&VarPolicy::Static))
    });
    let any_configurable = proposals.iter().any(|p| {
        p.variable
            .as_deref()
            .is_some_and(|v| policy.get(v) == Some(&VarPolicy::Configurable))
    });
    if any_configurable {
        proposals.retain(|p| {
            p.variable
                .as_deref()
                .is_some_and(|v| policy.get(v) == Some(&VarPolicy::Configurable))
        });
    }

    Ok(proposals)
}

/// Build one proposal, computing `shared` from how many terms read the edited
/// definition. A call-site literal has exactly one reader (the call), so the
/// flag naturally stays `false` there.
fn make_proposal(
    program: &Program,
    span: SourceSpan,
    literal_term: TermId,
    variable: Option<String>,
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

/// The name the binding behind `arg` was declared with, walking the identity
/// chain a name reference compiles to and taking the first named term.
fn binding_name(program: &Program, arg: TermId) -> Option<String> {
    let mut cur = arg;
    for _ in 0..16 {
        let t = program.terms.get(cur.0 as usize)?;
        if let Some(name) = &t.name {
            return Some(name.rsplit("::").next().unwrap_or(name).to_string());
        }
        cur = provenance::alias_target(program, cur)?;
    }
    None
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
    var: Option<String>,
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
    // say *which variable* it edits.
    let var_here = t
        .name
        .as_ref()
        .map(|n| n.rsplit("::").next().unwrap_or(n).to_string())
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
            out.push(make_proposal(program, span, term, var_here, new_text));
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

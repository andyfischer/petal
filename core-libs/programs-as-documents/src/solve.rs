//! Step 4 of the ladder: a computed field, solved by direct manipulation.
//!
//! When a field is computed (`x={100 + i * 200}`), no literal holds its
//! value. [`solve_field`] asks Petal's direct-manipulation solver (DM v2,
//! best effort) which numbers of the document to change so the field
//! reaches its target. The goal names the element itself
//! ([`Observable::Element`]: the literal that built it and which of its
//! elements it is, from the run's element trace), so the document needs no
//! probe native and may hand its result over any way it likes. Only numbers
//! in the document's own file may move ([`ParamSelector::EntryFile`]).
//!
//! Other numeric fields can be held (move `x`, keep `y`). Best effort's
//! equal split counts the literals that reach only a held goal, so the
//! solve runs without holds first, and again holding them only when that
//! edit moved a held field.

use petal::dm::{
    DmError, DmGoal, DmRequest, NoHost, Observable, ParamFilter, ParamSelector, PathStep, Policy,
    apply_edits_multi, propose_edits_v2,
};
use petal::source_map::{FileId, SourceSpan};
use petal::static_value::StaticValue;

use crate::intent::field_of;
use crate::output_map::OutputNode;
use crate::run::{DocumentRun, Runner};

/// A source the solver found, not yet verified against the intent.
#[derive(Debug, Clone, PartialEq)]
pub struct Solved {
    pub source: String,
    /// The first literal it changed.
    pub span: SourceSpan,
    /// "set `200` on line 2 to `275`; …".
    pub description: String,
}

/// The number a static value is.
pub fn number(v: &StaticValue) -> Option<f64> {
    match v {
        StaticValue::Int(n) => Some(*n as f64),
        StaticValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn observable(term: petal::program::TermId, occurrence: u32, field: &str) -> Observable {
    Observable::Element {
        term,
        occurrence,
        path: field.split('.').map(|s| PathStep::Field(s.to_string())).collect(),
    }
}

/// Find an edit of the document's numbers that puts `field` (dotted) of
/// `node` at `target`, keeping each of `hold` (other numeric fields) where
/// it is. `allow_other_files` lets the solver move numbers in imported
/// modules (the edit is then refused, since only the entry file is
/// written). An unreachable target is an error that says why; nothing is
/// written.
pub fn solve_field(
    runner: &Runner,
    run: &DocumentRun,
    node: &OutputNode,
    field: &str,
    target: f64,
    hold: &[String],
    allow_other_files: bool,
) -> Result<Solved, String> {
    let tag = &node.tag;
    let origin = run
        .env
        .element_origin(node.element)
        .ok_or_else(|| format!("<{tag}> was not built by an element literal, so the solver cannot name it"))?
        .clone();
    let el = run.node_value(node).ok_or("the output has no literal form")?;
    let held: Vec<(String, f64)> = hold
        .iter()
        .filter(|h| h.as_str() != field)
        .filter_map(|h| field_of(&el, h).and_then(number).map(|v| (h.clone(), v)))
        .collect();
    let first = propose(runner, run, &origin, field, target, &[], allow_other_files)?;
    if held.is_empty() {
        return Ok(first);
    }
    // Did the first answer move a held field?
    let moved = match runner.run(&first.source) {
        Ok(after) => match after.map.find(&node.key).and_then(|n| after.node_value(n)) {
            Some(v) => held.iter().any(|(h, was)| field_of(&v, h).and_then(number).is_none_or(|x| (x - was).abs() > 1e-9)),
            None => true,
        },
        Err(_) => true,
    };
    if !moved {
        return Ok(first);
    }
    propose(runner, run, &origin, field, target, &held, allow_other_files)
}

fn propose(
    runner: &Runner,
    run: &DocumentRun,
    origin: &petal::execution_context::ElementOrigin,
    field: &str,
    target: f64,
    held: &[(String, f64)],
    allow_other_files: bool,
) -> Result<Solved, String> {
    let (mut env, _program, stack) = runner.prepare(&run.source)?;
    let mut goals = vec![DmGoal::to(observable(origin.term, origin.occurrence, field), target)];
    goals.extend(held.iter().map(|(h, v)| DmGoal::to(observable(origin.term, origin.occurrence, h), *v)));
    let mut holds = vec![false];
    holds.extend(held.iter().map(|_| true));
    let req = DmRequest {
        policy: Policy::BestEffort,
        holds,
        filter: if allow_other_files {
            ParamFilter { allow: Some(vec![ParamSelector::AllUserCode, ParamSelector::AllowPathDeps]), deny: Vec::new() }
        } else {
            ParamFilter { allow: Some(vec![ParamSelector::EntryFile]), deny: Vec::new() }
        },
        ..DmRequest::new(goals)
    };
    let what = format!("`{field}` to {}", fmt_num(target));
    let resp = propose_edits_v2(&mut env, &mut NoHost, stack, &req).map_err(|e| match e {
        DmError::NothingToMove { why } => format!("no number in this document moves {what} ({why})"),
        DmError::Unreachable { why } => format!("no edit of this document's numbers moves {what}: {why}"),
        other => format!("the solver could not move {what}: {other}"),
    })?;
    let best = resp.best_verified().ok_or_else(|| format!("no verified edit moves {what}"))?;
    if let Some(e) = best.edits.iter().find(|e| e.file != FileId(0)) {
        let name = e.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| format!("file {}", e.file.0));
        return Err(format!("the edit that moves {what} is in {name}, not in this document; edit it there by hand"));
    }
    let mut files = vec![(FileId(0), run.source.clone())];
    apply_edits_multi(&mut files, &best.edits).map_err(|e| e.to_string())?;
    let span = best.changes.first().map(|c| c.span).ok_or("the solver changed nothing")?;
    let description = best
        .changes
        .iter()
        .map(|c| {
            let old = petal::provenance::span_text(run.program(), &c.span).unwrap_or_else(|| c.old_text.clone());
            format!("set `{old}` on line {} to `{}`", c.span.start.line, c.new_text)
        })
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Solved { source: files.remove(0).1, span, description })
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

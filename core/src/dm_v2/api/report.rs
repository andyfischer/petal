//! The text an alternative is described by for people: goal labels,
//! collateral items, range notes.

use std::collections::HashMap;

use crate::dm_v2::jacobian::{ColumnQuality, Jacobian};
use crate::dm_v2::observe::{EmitKey, Observable, PathStep, path_text};
use crate::dm_v2::params::file_origins;
use crate::dm_v2::rank::{collateral_norm, weighted_collateral};
use crate::dm_v2::solve::GoalRow;
use crate::dm_v2::spec::SpecRun;
use crate::env::Env;

use super::emit_key_of;
use super::request::short;
use super::response::{CandidateReport, Collateral, CollateralItem, ParamChange};

/// What each baseline emit is, for collateral items: channel, native, line.
pub(super) fn emit_info(env: &Env, pid: crate::program::ProgramId, base: &SpecRun) -> HashMap<EmitKey, EmitInfo> {
    let program = env.get_program(pid);
    let origins = program.map(file_origins).unwrap_or_default();
    let mut per_channel: HashMap<crate::symbol::SymbolId, usize> = HashMap::new();
    base.emits
        .iter()
        .map(|e| {
            let nth = per_channel.entry(e.channel).or_insert(0);
            let emit = *nth;
            *nth += 1;
            let channel = env.symbol_name(e.channel).unwrap_or("?").to_string();
            let callee = e.native.map(|id| env.native_table().get_name(id).to_string());
            let line = program.and_then(|program| {
                e.chain.chain.iter().find_map(|t| {
                    let span = program.source_map.get(*t)?;
                    let user = origins.get(span.file.0 as usize).is_some_and(|o| *o != crate::dm_v2::CodeOrigin::Library);
                    user.then_some(span.start.line)
                })
            });
            (e.key, EmitInfo { channel, callee, line, emit })
        })
        .collect()
}

/// What [`emit_info`] knows about one baseline emit.
#[derive(Debug, Clone, Default)]
pub(super) struct EmitInfo {
    channel: String,
    callee: Option<String>,
    line: Option<u32>,
    /// Which emit on its channel.
    emit: usize,
}

/// The collateral of a verification run: rows grouped by emit.
pub(super) fn collateral_items(
    jac: &Jacobian,
    gr: usize,
    moves: &[Option<f64>],
    info: &HashMap<EmitKey, EmitInfo>,
    goal_moves: &[f64],
    weights: &[f64],
) -> Collateral {
    let mut items: Vec<CollateralItem> = Vec::new();
    for (k, m) in moves.iter().enumerate() {
        let moved = match m {
            Some(x) => x.abs() > 1e-9 * (1.0 + x.abs()) && x.abs() > 1e-9,
            None => true,
        };
        if !moved {
            continue;
        }
        let Some(key) = emit_key_of(&jac.rows[gr + k]) else { continue };
        let mag = m.map_or(f64::INFINITY, f64::abs);
        let (arg, path) = match &jac.rows[gr + k] {
            Observable::EmitArg { arg, path, .. } => (*arg, path.clone()),
            _ => (0, Vec::new()),
        };
        if let Some(it) = items.iter_mut().find(|i| i.key == key) {
            it.moved += 1;
            if mag > it.max_move {
                it.max_move = mag;
                it.arg = arg;
                it.path = path;
            }
            continue;
        }
        let EmitInfo { channel, callee, line, emit } =
            info.get(&key).cloned().unwrap_or_else(|| EmitInfo { channel: "?".into(), ..Default::default() });
        let weight = weights.get(k).copied().unwrap_or(1.0);
        items.push(CollateralItem { key, channel, callee, line, max_move: mag, emit, arg, path, moved: 1, weight });
    }
    let keys: Vec<Option<EmitKey>> = jac.rows[gr..].iter().map(emit_key_of).collect();
    let wc = weighted_collateral(&keys, moves, goal_moves, weights);
    Collateral { norm: collateral_norm(goal_moves, moves), shapes: wc.shapes, excess: wc.excess, items }
}

/// A range goal, for the description: `coins[0].8 ≤ 0.6 (was 1.4, now 0.5)`.
pub(super) struct RangeNote {
    pub(super) label: String,
    /// `≤ 0.6`.
    pub(super) text: String,
    pub(super) was: f64,
    pub(super) now: f64,
    /// It held at the baseline already.
    pub(super) held: bool,
}

/// How a goal reads in a sentence: `coins[0]`, `coins[0].8`, `shapes[2]
/// arg 1` (its channel, which emit on it, and the field).
/// Whether argument `arg` of a call to `callee` is the emitted value
/// itself (`push_output`'s second argument), so it reads without `arg N`.
pub(super) fn is_value_arg(callee: Option<&str>, arg: usize) -> bool {
    callee == Some("push_output") && arg == 1
}

pub(super) fn goal_label(o: &Observable, info: &HashMap<EmitKey, EmitInfo>) -> String {
    let key = emit_key_of(o);
    let head = key
        .and_then(|k| info.get(&k))
        .map_or_else(|| "goal".to_string(), |e| format!("{}[{}]", e.channel, e.emit));
    let steps = |path: &[PathStep]| if path.is_empty() { String::new() } else { format!(".{}", path_text(path)) };
    match o {
        Observable::EmitValue { path, .. } => format!("{head}{}", steps(path)),
        Observable::EmitArg { arg, path, .. } => {
            let value_arg = key.and_then(|k| info.get(&k)).is_some_and(|e| is_value_arg(e.callee.as_deref(), *arg));
            if value_arg { format!("{head}{}", steps(path)) } else { format!("{head} arg {arg}{}", steps(path)) }
        }
        Observable::Watch { path, .. } => format!("{head} (watched){}", steps(path)),
    }
}

/// "set `gap` to 14 (line 3); moves 4 other emits".
pub(super) fn describe(
    changes: &[ParamChange],
    coll: &Collateral,
    verified: bool,
    achieved: &[f64],
    goals: &[GoalRow],
    ranges: &[RangeNote],
) -> String {
    let parts: Vec<String> = changes
        .iter()
        .map(|c| {
            let what = match (&c.binding, &c.function) {
                (Some(b), _) => format!("set `{b}` to {}", c.new_text),
                (None, Some(f)) => format!("change `{}` to {} in `{f}`", c.old_text, c.new_text),
                (None, None) => format!("change `{}` to {}", c.old_text, c.new_text),
            };
            let line = c.span.start.line;
            if c.file.0 == 0 { format!("{what} (line {line})") } else { format!("{what} (file {}, line {line})", c.file.0) }
        })
        .collect();
    let mut s = parts.join(", ");
    let names = || {
        let mut l: Vec<String> = coll.items.iter().take(3).map(CollateralItem::label).collect();
        if coll.items.len() > 3 {
            l.push(format!("+{} more", coll.items.len() - 3));
        }
        l.join(", ")
    };
    match coll.items.len() {
        0 => s.push_str("; moves nothing else"),
        1 => s.push_str(&format!("; also moves 1 other emit ({})", names())),
        n => s.push_str(&format!("; also moves {n} other emits ({})", names())),
    }
    for r in ranges {
        let keeps = if r.held { "keeps " } else { "" };
        s.push_str(&format!("; {keeps}{} {} (was {}, now {})", r.label, r.text, short(r.was), short(r.now)));
    }
    if !verified {
        let worst = achieved.iter().zip(goals).map(|(a, g)| g.miss(*a)).fold(0.0, f64::max);
        s.push_str(&format!("; misses the goal by {worst:.3}"));
    }
    s
}

/// Why nothing can move the goals, from the candidate report.
pub(super) fn nothing_to_move(candidates: &[CandidateReport]) -> String {
    let mut reasons: Vec<String> = Vec::new();
    for c in candidates.iter().take(8) {
        let why = match (&c.eligible, &c.quality) {
            (Err(e), _) => (*e).to_string(),
            (Ok(()), Some(q)) if !q.usable() => format!("{q:?}"),
            (Ok(()), Some(ColumnQuality::Zero)) | (Ok(()), None) => "does not move the goals".into(),
            (Ok(()), Some(_)) => "not measured".into(),
        };
        reasons.push(format!("line {}: {why}", c.param.span.start.line));
    }
    if reasons.is_empty() {
        "no literal moves the goals".into()
    } else {
        format!("no editable literal moves the goals ({})", reasons.join("; "))
    }
}

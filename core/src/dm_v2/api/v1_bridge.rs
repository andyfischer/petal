//! The v1 fallback for a goal whose target is not a number.

use std::collections::HashMap;

use crate::direct_manipulation::{self as v1, ManipulationGoal, VarPolicy};
use crate::dm_v2::DmError;
use crate::dm_v2::observe::Observable;
use crate::dm_v2::params::ParamSelector;
use crate::dm_v2::rank::{ScoreParts, SubsetKind};
use crate::dm_v2::spec::SpecSession;
use crate::env::Env;
use crate::source_map::FileId;
use crate::static_value::StaticValue;

use super::request::{DmRequest, GoalTarget};
use super::response::{Alternative, Collateral, DmDiagnostics, DmResponse, Engine, FileEdit};

/// v1 for a request with a non-numeric goal: each goal is mapped to the
/// argument of the call that made its emit, and v1's proposals (not re-run)
/// become the alternatives.
pub(super) fn v1_fallback(env: &Env, spec: &SpecSession, req: &DmRequest) -> Result<DmResponse, DmError> {
    let program = env.get_program(spec.program()).ok_or(DmError::Spec(crate::dm_v2::SpecError::Stale))?;
    let mut goals: Vec<ManipulationGoal> = Vec::new();
    for (i, g) in req.goals.iter().enumerate() {
        let bad = || DmError::BadObservable(format!("goal {i}: v1 cannot address {:?}", g.observable));
        let (term, arg_index) = match &g.observable {
            Observable::EmitArg { key, arg, path } if path.is_empty() => {
                let e = spec.baseline().emit(key).ok_or_else(bad)?;
                (*e.chain.chain.first().ok_or_else(bad)?, *arg)
            }
            Observable::Watch { term, at, path } if path.is_empty() => {
                let e = spec.baseline().emit(at).ok_or_else(bad)?;
                e.chain
                    .chain
                    .iter()
                    .find_map(|&c| {
                        let t = program.terms.get(c.0 as usize)?;
                        let off = t.op.arg_offset()?;
                        let k = t.inputs.iter().skip(off).position(|x| x == term)?;
                        Some((c, k))
                    })
                    .ok_or_else(bad)?
            }
            _ => return Err(bad()),
        };
        let new_value = match &g.target {
            GoalTarget::Num(t) => {
                if t.fract() == 0.0 {
                    StaticValue::Int(*t as i64)
                } else {
                    StaticValue::Float(*t)
                }
            }
            GoalTarget::Value(v) => v.clone(),
            GoalTarget::Range { .. } => {
                return Err(DmError::BadObservable(format!(
                    "goal {i}: a range goal and a non-numeric goal cannot be asked together"
                )));
            }
        };
        goals.push(ManipulationGoal { term, arg_index, new_value });
    }
    let mut policy: HashMap<String, VarPolicy> = HashMap::new();
    for s in req.filter.allow.iter().flatten() {
        if let ParamSelector::Variable(n) = s {
            policy.insert(n.clone(), VarPolicy::Configurable);
        }
    }
    for s in &req.filter.deny {
        if let ParamSelector::Variable(n) = s {
            policy.insert(n.clone(), VarPolicy::Static);
        }
    }
    let per_goal = v1::propose_edits_batch(program, &goals, None, &policy)
        .map_err(|e| DmError::BadObservable(e.message))?;
    let origin = |f: FileId| {
        program.source_map.files.get(f.0 as usize).and_then(|f| f.origin.clone())
    };
    let to_alt = |props: Vec<&v1::EditProposal>| -> Alternative {
        let edits: Vec<FileEdit> = props
            .iter()
            .map(|p| FileEdit { file: p.edit.span.file, path: origin(p.edit.span.file), edit: p.edit.clone() })
            .collect();
        Alternative {
            edits,
            changes: Vec::new(),
            achieved: Vec::new(),
            predicted: Vec::new(),
            misses: Vec::new(),
            max_error: f64::NAN,
            verified: false,
            collateral: Collateral::default(),
            stateful: false,
            config_only: props.iter().all(|p| p.config),
            regime_changed: false,
            rng_order_changed: false,
            rng_difference: None,
            score: 0.0,
            score_parts: ScoreParts::default(),
            kind: SubsetKind::Tuple,
            iterations: 0,
            engine: Engine::V1,
            description: props.iter().map(|p| p.description.clone()).collect::<Vec<_>>().join(", "),
            rejections: Vec::new(),
        }
    };
    let alternatives: Vec<Alternative> = if per_goal.len() == 1 {
        per_goal[0].iter().take(req.max_alternatives.max(1)).map(|p| to_alt(vec![p])).collect()
    } else {
        let firsts: Vec<&v1::EditProposal> = per_goal.iter().filter_map(|ps| ps.first()).collect();
        if firsts.is_empty() { Vec::new() } else { vec![to_alt(firsts)] }
    };
    Ok(DmResponse {
        alternatives,
        candidates: Vec::new(),
        diagnostics: DmDiagnostics { engine: Some(Engine::V1), ..Default::default() },
        explain: None,
    })
}

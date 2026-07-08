//! Call-resolution helpers for the bytecode VM.
//!
//! Resolving a callable `Value` to a concrete `ClosureId` (including overload
//! selection by argument count) and building an overload-set value are pure over
//! `(&Program, closures, overload_sets)`, so they live here rather than inline
//! in the [`Vm`](super::bytecode::Vm). Frame construction
//! ([`VmFrame`](super::bytecode::VmFrame)) stays in the VM.

use crate::backend::RuntimeClosure;
use crate::program::{base_fn_name, ClosureId, OverloadEntry, OverloadSetId, Program};
use crate::value::Value;

/// Resolve a callable to a `ClosureId`, selecting an overload by `arg_count`.
pub fn resolve_callable(
    program: &Program,
    closures: &[RuntimeClosure],
    overload_sets: &[Vec<OverloadEntry>],
    callable: Value,
    arg_count: usize,
) -> Result<ClosureId, String> {
    match callable {
        Value::Closure(id) => Ok(id),
        Value::OverloadSet(set_id) => {
            resolve_overload(program, closures, &overload_sets[set_id.0 as usize], arg_count)
        }
        _ => Err(format!("Expected a function, got {}", callable.type_name())),
    }
}

/// Resolve an overload set to the closure whose arity matches `arg_count`.
pub fn resolve_overload(
    program: &Program,
    closures: &[RuntimeClosure],
    entries: &[OverloadEntry],
    arg_count: usize,
) -> Result<ClosureId, String> {
    for entry in entries {
        if entry.arity == arg_count {
            return Ok(entry.closure_id);
        }
    }
    // Derive the base function name from the first entry's internal name
    // (e.g. "foo#2" → "foo") for the error message.
    let base_name = entries
        .first()
        .and_then(|e| {
            let func = &program.functions[closures[e.closure_id.0 as usize].function_id.0 as usize];
            func.name.as_deref().map(|n| base_fn_name(n).to_string())
        })
        .unwrap_or_else(|| "<anonymous>".to_string());
    let arities: Vec<String> = entries.iter().map(|e| e.arity.to_string()).collect();
    Err(format!(
        "{}() expects {} arguments, got {}",
        base_name,
        arities.join(" or "),
        arg_count,
    ))
}

/// Build an overload-set value from per-arity closures, patching each closure's
/// self-recursion capture (which was Nil at `MakeClosure` time because the set
/// did not exist yet). Registers the new set and returns its `Value`.
pub fn make_overload_set(
    program: &Program,
    closures: &mut [RuntimeClosure],
    overload_sets: &mut Vec<Vec<OverloadEntry>>,
    inputs: &[Value],
) -> Value {
    let mut entries = Vec::with_capacity(inputs.len());
    for &input in inputs {
        if let Value::Closure(cid) = input {
            let func = &program.functions[closures[cid.0 as usize].function_id.0 as usize];
            entries.push(OverloadEntry {
                arity: func.params.len(),
                closure_id: cid,
            });
        }
    }
    let set_id = OverloadSetId(overload_sets.len() as u32);
    let overload_val = Value::OverloadSet(set_id);

    // Derive the base name from an internal name (e.g. "count#1" → "count"),
    // then patch every capture of that name to the overload set value.
    let base_name = entries.first().and_then(|e| {
        let func = &program.functions[closures[e.closure_id.0 as usize].function_id.0 as usize];
        func.name.as_deref().map(|n| base_fn_name(n).to_string())
    });
    if let Some(ref base) = base_name {
        for entry in &entries {
            let closure = &mut closures[entry.closure_id.0 as usize];
            let func = &program.functions[closure.function_id.0 as usize];
            let cap_names = func.capture_names.clone();
            for (i, cap_name) in cap_names.iter().enumerate() {
                if cap_name == base {
                    closure.captures[i] = overload_val;
                }
            }
        }
    }

    overload_sets.push(entries);
    overload_val
}

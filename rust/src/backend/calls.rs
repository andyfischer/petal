//! Call-resolution helpers for the bytecode VM.
//!
//! Resolving a callable `Value` to a concrete `ClosureId` (including overload
//! selection by argument count) and building an overload-set value are pure over
//! `(&Program, &ClosureTable)`, so they live here rather than inline
//! in the [`Vm`](super::bytecode::Vm). Frame construction
//! ([`VmFrame`](super::bytecode::VmFrame)) stays in the VM.

use crate::closure_table::ClosureTable;
use crate::program::{ClosureId, OverloadEntry, Program, base_fn_name};
use crate::value::Value;
use smallvec::SmallVec;

/// Resolve a callable to a `ClosureId`, selecting an overload by `arg_count`.
pub fn resolve_callable(
    program: &Program,
    closures: &ClosureTable,
    callable: Value,
    arg_count: usize,
) -> Result<ClosureId, String> {
    match callable {
        Value::Closure(id) => Ok(id),
        Value::OverloadSet(set_id) => {
            resolve_overload(program, closures, closures.set(set_id), arg_count)
        }
        _ => Err(format!("Expected a function, got {}", callable.type_name())),
    }
}

/// Resolve an overload set to the closure whose arity matches `arg_count`.
pub fn resolve_overload(
    program: &Program,
    closures: &ClosureTable,
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
            let func = &program.functions[closures.closure(e.closure_id).function_id.0 as usize];
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
    closures: &mut ClosureTable,
    inputs: &[Value],
) -> Value {
    let mut entries = Vec::with_capacity(inputs.len());
    for &input in inputs {
        if let Value::Closure(cid) = input {
            let func = &program.functions[closures.closure(cid).function_id.0 as usize];
            entries.push(OverloadEntry {
                arity: func.params.len(),
                closure_id: cid,
            });
        }
    }
    // Registered before the self-capture patch below, so the id the patch
    // writes into each closure is the one the set actually has.
    let set_id = closures.alloc_set(entries.clone());
    let overload_val = Value::OverloadSet(set_id);

    // Derive the base name from an internal name (e.g. "count#1" → "count"),
    // then patch every capture of that name to the overload set value.
    let base_name = entries.first().and_then(|e| {
        let func = &program.functions[closures.closure(e.closure_id).function_id.0 as usize];
        func.name.as_deref().map(|n| base_fn_name(n).to_string())
    });
    if let Some(ref base) = base_name {
        for entry in &entries {
            let closure = closures.closure_mut(entry.closure_id);
            let func = &program.functions[closure.function_id.0 as usize];
            let cap_names = func.capture_names.clone();
            for (i, cap_name) in cap_names.iter().enumerate() {
                // Only the *unresolved* self-capture is patched. A hoisted
                // overload captures its own name as a cell, which the
                // declaration writes the finished set into — patching that
                // would replace the cell with the set and turn every read of
                // it into a `cell_read` on a function.
                if cap_name == base && matches!(closure.captures[i], Value::Nil) {
                    closure.captures[i] = overload_val;
                }
            }
        }
    }

    overload_val
}

/// Permute `args` into the callee's parameter order, given the written name of
/// each argument (`None` = positional).
///
/// `names` is parallel to `args`; the parser guarantees every positional
/// argument precedes every named one, so the positional prefix fills slots
/// `0..k` in order and each named argument then claims the slot its name picks
/// out. A method's receiver arrives as a leading positional argument, so it
/// owns `params[0]` and a named argument that repeats it is reported as a
/// double-bind rather than silently overwriting it.
///
/// Only called when at least one argument is named — the all-positional path
/// never builds this vector. `args.len() == params.len()` is already checked by
/// the caller's arity error, which is why an unfilled slot here can only mean a
/// name landed on a slot some other argument already took.
pub fn bind_named_args(
    fn_name: &str,
    params: &[String],
    args: &[Value],
    names: &[Option<&str>],
) -> Result<SmallVec<[Value; 8]>, String> {
    // An overload variant is named `box#1` internally; every message below
    // names the function as the source wrote it, like `resolve_overload`.
    let fn_name = base_fn_name(fn_name);
    let mut slots: SmallVec<[Option<Value>; 8]> = smallvec::smallvec![None; params.len()];
    let mut next_positional = 0usize;
    for (i, &arg) in args.iter().enumerate() {
        let slot = match names.get(i).copied().flatten() {
            None => {
                let slot = next_positional;
                next_positional += 1;
                slot
            }
            Some(name) => match params.iter().position(|p| p == name) {
                Some(slot) => slot,
                None => return Err(format!("{fn_name}() has no parameter named '{name}'")),
            },
        };
        match slots.get_mut(slot) {
            Some(cell) if cell.is_none() => *cell = Some(arg),
            Some(_) => {
                return Err(format!(
                    "{}() got multiple values for parameter '{}'",
                    fn_name, params[slot]
                ));
            }
            // Unreachable while the arity check runs first, but hand-written
            // bytecode reaches here without it.
            None => {
                return Err(format!(
                    "{}() expects {} arguments, got {}",
                    fn_name,
                    params.len(),
                    args.len()
                ));
            }
        }
    }
    let mut bound: SmallVec<[Value; 8]> = SmallVec::with_capacity(params.len());
    for (slot, cell) in slots.into_iter().enumerate() {
        match cell {
            Some(v) => bound.push(v),
            None => {
                return Err(format!(
                    "{}() is missing a value for parameter '{}'",
                    fn_name, params[slot]
                ));
            }
        }
    }
    Ok(bound)
}

//! Call-resolution helpers for the bytecode VM.
//!
//! Resolving a callable `Value` to a concrete `ClosureId` (including overload
//! selection by argument count and names) and building an overload-set value are pure over
//! `(&Program, &ClosureTable)`, so they live here rather than inline
//! in the [`Vm`](super::bytecode::Vm). Frame construction
//! ([`VmFrame`](super::bytecode::VmFrame)) stays in the VM.

use crate::closure_table::ClosureTable;
use crate::program::{ClosureId, FunctionDef, OverloadEntry, Program, base_fn_name};
use crate::value::Value;
use smallvec::SmallVec;

/// Resolve a callable to a `ClosureId`, selecting an overload for a call that
/// writes `arg_count` arguments, named as `names` says (empty when none is).
pub fn resolve_callable(
    program: &Program,
    closures: &ClosureTable,
    callable: Value,
    arg_count: usize,
    names: &[Option<&str>],
) -> Result<ClosureId, String> {
    match callable {
        Value::Closure(id) => Ok(id),
        Value::OverloadSet(set_id) => {
            resolve_overload(program, closures, closures.set(set_id), arg_count, names)
        }
        _ => Err(format!("Expected a function, got {}", callable.type_name())),
    }
}

/// Whether `func` can run a call that writes `arg_count` arguments, the
/// trailing ones named as `names` says (empty = all positional): no more
/// arguments than parameters, every name one of its parameters and not one a
/// positional argument or an earlier name already filled, and every parameter
/// left unfilled one that has a default.
///
/// The one definition of "this variant accepts this call" — overload
/// resolution asks it of each variant, and it never allocates.
pub fn accepts_call(func: &FunctionDef, arg_count: usize, names: &[Option<&str>]) -> bool {
    let params = &func.params;
    if arg_count > params.len() {
        return false;
    }
    let required = func.required_params();
    if names.is_empty() {
        return arg_count >= required;
    }
    // Positional arguments precede named ones (the parser enforces it).
    let positional = names.iter().take_while(|n| n.is_none()).count();
    let named = &names[positional..];
    for (i, name) in named.iter().enumerate() {
        let Some(name) = name else { return false };
        match params.iter().position(|p| p == name) {
            Some(slot) if slot >= positional => {}
            _ => return false,
        }
        if named[..i].contains(&Some(*name)) {
            return false;
        }
    }
    (positional..required).all(|slot| named.contains(&Some(params[slot].as_str())))
}

/// Whether a call of `arg_count` arguments, the named ones spelt `names`,
/// *skips* a parameter of `params`: a name lands past the slots the call's own
/// argument count reaches, so some earlier parameter is left to its default.
/// Such a call cannot be written positionally against that variant — the name
/// is doing work there — which is what lets it outrank the exact-arity variant
/// in [`resolve_overload`].
pub fn skips_a_parameter<'a, S: AsRef<str>>(
    params: &[S],
    arg_count: usize,
    names: impl IntoIterator<Item = &'a str>,
) -> bool {
    names.into_iter().any(|name| {
        params
            .iter()
            .position(|p| p.as_ref() == name)
            .is_some_and(|slot| slot >= arg_count)
    })
}

/// Resolve an overload set to the variant a call selects.
///
/// 1. A variant *accepts* the call per [`accepts_call`].
/// 2. An accepting variant whose parameter count equals the number of
///    arguments written wins. A call that names nothing and matches an arity
///    exactly always lands here, which is the whole of the rule as it stood
///    before defaults — so such a call resolves as it always has.
///    The one thing that outranks it: a single other accepting variant in
///    which a written name skips a parameter ([`skips_a_parameter`]). In the
///    exact-arity variant that name sits where a positional argument would
///    have gone anyway; in the other it is the only way to write the call, so
///    that is the variant the name was written for.
/// 3. Otherwise the call must be accepted by exactly one variant. None is the
///    arity error; more than one is reported as ambiguous rather than settled
///    by declaration order.
pub fn resolve_overload(
    program: &Program,
    closures: &ClosureTable,
    entries: &[OverloadEntry],
    arg_count: usize,
    names: &[Option<&str>],
) -> Result<ClosureId, String> {
    if names.is_empty() {
        for entry in entries {
            if entry.arity == arg_count {
                return Ok(entry.closure_id);
            }
        }
    }
    let func_of = |e: &OverloadEntry| {
        &program.functions[closures.closure(e.closure_id).function_id.0 as usize]
    };
    let mut accepting = entries
        .iter()
        .filter(|e| accepts_call(func_of(e), arg_count, names));
    let first = accepting.next();
    let second = accepting.next();
    match (first, second) {
        (Some(only), None) => return Ok(only.closure_id),
        (Some(a), Some(b)) => {
            // Two or more accept. Arities are distinct within a set, so at most
            // one of them is the exact-arity variant.
            let all: SmallVec<[&OverloadEntry; 4]> = [a, b].into_iter().chain(accepting).collect();
            let skips = |e: &&&OverloadEntry| {
                skips_a_parameter(&func_of(e).params, arg_count, names.iter().flatten().copied())
            };
            let mut skipping = all.iter().filter(skips);
            let (skip, more) = (skipping.next(), skipping.next());
            if let Some(exact) = all.iter().find(|e| e.arity == arg_count) {
                match (skip, more) {
                    (None, _) => return Ok(exact.closure_id),
                    (Some(only), None) => return Ok(only.closure_id),
                    // Two variants the name skips into: nothing picks one.
                    _ => {}
                }
            }
            let base = overload_base_name(program, closures, entries);
            let all: Vec<String> = entries
                .iter()
                .filter(|e| accepts_call(func_of(e), arg_count, names))
                .map(|e| describe_variant(&base, func_of(e)))
                .collect();
            return Err(format!(
                "{base}() is ambiguous: {} {} accept this call — pass or name \
                 another argument to pick one",
                all.join(" and "),
                if all.len() == 2 { "both" } else { "all" },
            ));
        }
        (None, _) => {}
    }
    // Nothing accepts. When a variant has exactly this many parameters the
    // call was aimed at it, and binding its arguments says precisely which
    // name is wrong — a better message than any summary written here.
    if let Some(exact) = entries.iter().find(|e| e.arity == arg_count) {
        return Ok(exact.closure_id);
    }
    let base_name = overload_base_name(program, closures, entries);
    let in_range = entries
        .iter()
        .any(|e| (func_of(e).required_params()..=e.arity).contains(&arg_count));
    if in_range {
        // The count fits a variant, so it is the names that fit none.
        let variants: Vec<String> = entries
            .iter()
            .map(|e| describe_variant(&base_name, func_of(e)))
            .collect();
        let written: Vec<String> = names
            .iter()
            .flatten()
            .map(|n| format!("'{n}'"))
            .collect();
        return Err(format!(
            "{base_name}() has no variant that accepts {arg_count} argument{} with {} named {} (variants: {})",
            if arg_count == 1 { "" } else { "s" },
            if written.len() == 1 { "one" } else { "some" },
            written.join(", "),
            variants.join(", "),
        ));
    }
    let arities = arity_ranges(
        entries
            .iter()
            .map(|e| (func_of(e).required_params(), e.arity)),
    );
    Err(format!(
        "{}() expects {} arguments, got {}",
        base_name,
        arities.join(" or "),
        arg_count,
    ))
}

/// The source name of an overload set, from its first variant's internal name
/// (e.g. "foo#2" → "foo").
fn overload_base_name(
    program: &Program,
    closures: &ClosureTable,
    entries: &[OverloadEntry],
) -> String {
    entries
        .first()
        .and_then(|e| {
            let func = &program.functions[closures.closure(e.closure_id).function_id.0 as usize];
            func.name.as_deref().map(|n| base_fn_name(n).to_string())
        })
        .unwrap_or_else(|| "<anonymous>".to_string())
}

/// How many arguments a function takes, as an error message spells it: `2`,
/// or `1-3` when the trailing parameters have defaults.
pub fn arity_range(required: usize, total: usize) -> String {
    if required == total {
        total.to_string()
    } else {
        format!("{required}-{total}")
    }
}

/// The argument counts a set of variants takes between them, each spelled by
/// [`arity_range`], in declaration order. Ranges that share a count are
/// reported as the one range they cover, so variants whose defaults stretch
/// over each other's counts read `3-9` rather than `3 or 3-5 or 5-7 or 7-9`.
/// Ranges that merely sit side by side stay apart (`1 or 2-3`).
pub fn arity_ranges(ranges: impl IntoIterator<Item = (usize, usize)>) -> Vec<String> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (mut lo, mut hi) in ranges {
        // Fold in every range already kept that this one touches; the result
        // takes the place of the first of them.
        let mut at = None;
        let mut i = 0;
        while i < merged.len() {
            let (l, h) = merged[i];
            if l <= hi && lo <= h {
                lo = lo.min(l);
                hi = hi.max(h);
                merged.remove(i);
                at = Some(at.map_or(i, |a: usize| a.min(i)));
                // An earlier range may only now overlap the widened one.
                i = 0;
            } else {
                i += 1;
            }
        }
        let at = at.map_or(merged.len(), |a| a.min(merged.len()));
        merged.insert(at, (lo, hi));
    }
    merged
        .into_iter()
        .map(|(lo, hi)| arity_range(lo, hi))
        .collect()
}

/// `f(a, b = …)` — one variant, as a message lists it.
fn describe_variant(base: &str, func: &FunctionDef) -> String {
    let required = func.required_params();
    let params: Vec<String> = func
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i < required {
                p.clone()
            } else {
                format!("{p} = …")
            }
        })
        .collect();
    format!("{base}({})", params.join(", "))
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

/// Lay a call's arguments out in the order the callee's frame takes them,
/// given the written name of each argument (`None` = positional).
///
/// `names` is parallel to `args`, or empty when no argument is named; the
/// parser guarantees every positional argument precedes every named one, so
/// the positional prefix fills slots `0..k` in order and each named argument
/// then claims the slot its name picks out. A method's receiver arrives as a
/// leading positional argument, so it owns `params[0]` and a named argument
/// that repeats it is reported as a double-bind rather than silently
/// overwriting it.
///
/// The last `optional` parameters have default values. One left unfilled is
/// not an error: its slot gets a placeholder `nil`, and the result carries
/// `optional` extra trailing values — a `Bool` per optional parameter saying
/// whether the call supplied it — which is what the callee's prologue tests
/// before evaluating a default (see `FunctionDef::optional_params`). The
/// default itself is *not* evaluated here; it is code in the callee.
///
/// Only called when an argument is named or the callee has defaults — the
/// all-positional call of a function without them never builds this vector.
/// The caller has already rejected a call with too many arguments, which is
/// why an out-of-range slot here cannot happen outside hand-written bytecode.
pub fn bind_named_args(
    fn_name: &str,
    params: &[String],
    optional: usize,
    args: &[Value],
    names: &[Option<&str>],
) -> Result<SmallVec<[Value; 8]>, String> {
    // An overload variant is named `box#1` internally; every message below
    // names the function as the source wrote it, like `resolve_overload`.
    let fn_name = base_fn_name(fn_name);
    let required = params.len().saturating_sub(optional);
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
                    arity_range(required, params.len()),
                    args.len()
                ));
            }
        }
    }
    let mut bound: SmallVec<[Value; 8]> = SmallVec::with_capacity(params.len() + optional);
    for (slot, cell) in slots.iter().enumerate() {
        match cell {
            Some(v) => bound.push(*v),
            None if slot >= required => bound.push(Value::Nil),
            None => {
                return Err(format!(
                    "{}() is missing a value for parameter '{}'",
                    fn_name, params[slot]
                ));
            }
        }
    }
    for cell in &slots[required..] {
        bound.push(Value::Bool(cell.is_some()));
    }
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::arity_ranges;

    #[test]
    fn arity_ranges_merge_only_where_they_overlap() {
        // Distinct counts, and ranges that merely touch, are listed apart.
        assert_eq!(arity_ranges([(2, 2), (3, 3)]), ["2", "3"]);
        assert_eq!(arity_ranges([(1, 1), (2, 3)]), ["1", "2-3"]);
        assert_eq!(arity_ranges([(4, 4), (2, 2)]), ["4", "2"]);
        // A range that covers another's counts absorbs it, in the place of
        // the first of them.
        assert_eq!(
            arity_ranges([(3, 3), (4, 4), (4, 5), (6, 7)]),
            ["3", "4-5", "6-7"]
        );
        assert_eq!(arity_ranges([(2, 2), (2, 3)]), ["2-3"]);
        // A chain of overlaps collapses to its span.
        assert_eq!(
            arity_ranges([(7, 9), (6, 6), (5, 7), (3, 3), (4, 4), (3, 5)]),
            ["3-9"]
        );
    }
}

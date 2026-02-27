//! Builtins - Built-in function implementations registered via native FFI.

use crate::heap::Heap;
use crate::native_fn::{NativeFnTable, PetalState};
use crate::value::{self, Value};


/// Register all built-in functions into the native function table.
/// Must be called once at startup before any programs are loaded.
pub fn register_builtins(table: &mut NativeFnTable) {
    // Order matters — these must be registered in the same order as the old
    // BuiltinTable so that phantom term indices stay consistent.
    table.register("print", native_print);
    table.register("range", native_range);
    table.register("len", native_len);
    table.register("push", native_push);
    table.register("str", native_str);
    table.register("abs", native_abs);
    table.register("sqrt", native_sqrt);
    table.register("floor", native_floor);
    table.register("ceil", native_ceil);
    table.register("float", native_float);
    table.register("int", native_int);
    table.register("random", native_random);
    table.register("type", native_type);
    table.register("append", native_append);
    table.register("pop", native_pop);
    table.register("keys", native_keys);
    table.register("values", native_values);
    table.register("contains", native_contains);
    table.register("min", native_min);
    table.register("max", native_max);
    table.register("round", native_round);
    table.register("dual", native_dual);
    table.register("value_of", native_value_of);
    table.register("deriv_of", native_deriv_of);

    // Higher-order builtins: registered so the compiler sees them, but
    // dispatched as evaluator intrinsics at runtime.
    let map_id = table.register("map", native_intrinsic_placeholder);
    let filter_id = table.register("filter", native_intrinsic_placeholder);
    let reduce_id = table.register("reduce", native_intrinsic_placeholder);

    table.intrinsic_map = Some(map_id);
    table.intrinsic_filter = Some(filter_id);
    table.intrinsic_reduce = Some(reduce_id);
}

// ---------------------------------------------------------------------------
// Placeholder for higher-order builtins (should never be called directly)
// ---------------------------------------------------------------------------

fn native_intrinsic_placeholder(_state: &mut PetalState) -> Result<u32, String> {
    Err("This function requires evaluator context and should be dispatched as an intrinsic".into())
}

// ---------------------------------------------------------------------------
// Native function implementations
// ---------------------------------------------------------------------------

fn native_print(state: &mut PetalState) -> Result<u32, String> {
    let parts: Vec<String> = (1..=state.arg_count())
        .map(|i| {
            let v = state.get_value(i).unwrap();
            value::value_to_display_string(&v, state.heap())
        })
        .collect();
    let line = parts.join(" ");
    state.print(line);
    state.push_nil();
    Ok(1)
}

fn native_range(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("range() expects 2 arguments".into());
    }
    let start = state.get_int(1)?;
    let end = state.get_int(2)?;
    let items: Vec<Value> = (start..end).map(Value::Int).collect();
    state.push_list(items);
    Ok(1)
}

fn native_len(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("len() expects 1 argument".into());
    }
    let v = state.get_value(1)?;
    match v {
        Value::List(id) => {
            state.push_int(state.heap().list_len(id) as i64);
            Ok(1)
        }
        Value::String(id) => {
            state.push_int(state.heap().get_string(id).len() as i64);
            Ok(1)
        }
        _ => Err(format!("Cannot get length of {}", v.type_name())),
    }
}

fn native_push(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("push() expects 2 arguments".into());
    }
    let list = state.get_value(1)?;
    let val = state.get_value(2)?;
    match list {
        Value::List(id) => {
            state.heap_mut().get_list_mut(id).push(val);
            state.push_nil();
            Ok(1)
        }
        _ => Err("push() expects a list as first argument".into()),
    }
}

fn native_str(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("str() expects 1 argument".into());
    }
    let v = state.get_value(1)?;
    let s = value::value_to_display_string(&v, state.heap());
    state.push_string(s);
    Ok(1)
}

fn native_abs(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("abs() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n.abs()); Ok(1) }
        Value::Float(f) => { state.push_float(f.abs()); Ok(1) }
        Value::Dual { value, derivative } => {
            // d/dx |x| = sign(x) * dx
            let sign = if value > 0.0 { 1.0 } else if value < 0.0 { -1.0 } else { 0.0 };
            state.push_value(Value::Dual { value: value.abs(), derivative: sign * derivative });
            Ok(1)
        }
        _ => Err("abs() expects a number".into()),
    }
}

fn native_sqrt(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("sqrt() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx sqrt(x) = 1 / (2 * sqrt(x))
            let sqrt_val = value.sqrt();
            let d = if sqrt_val == 0.0 { 0.0 } else { derivative / (2.0 * sqrt_val) };
            state.push_value(Value::Dual { value: sqrt_val, derivative: d });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.sqrt());
            Ok(1)
        }
    }
}

fn native_floor(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("floor() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.floor()); Ok(1) }
        Value::Dual { value, .. } => {
            // floor is a step function: derivative is 0 almost everywhere
            state.push_value(Value::Dual { value: value.floor(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("floor() expects a number".into()),
    }
}

fn native_ceil(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("ceil() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.ceil()); Ok(1) }
        Value::Dual { value, .. } => {
            state.push_value(Value::Dual { value: value.ceil(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("ceil() expects a number".into()),
    }
}

fn native_float(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("float() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            state.push_value(Value::Dual { value, derivative });
            Ok(1)
        }
        _ => {
            let f = state.get_float(1)?;
            state.push_float(f);
            Ok(1)
        }
    }
}

fn native_int(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("int() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_int(f as i64); Ok(1) }
        Value::String(id) => {
            let s = state.heap().get_string(id).to_string();
            match s.parse::<i64>() {
                Ok(n) => { state.push_int(n); Ok(1) }
                Err(_) => Err(format!("Cannot convert '{}' to int", s)),
            }
        }
        v => Err(format!("Cannot convert {} to int", v.type_name())),
    }
}

fn native_random(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("random() expects 2 arguments".into());
    }
    let min = state.get_float(1)?;
    let max = state.get_float(2)?;
    let pseudo = ((std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as f64)
        / 4_294_967_295.0)
        * (max - min)
        + min;
    state.push_float(pseudo);
    Ok(1)
}

fn native_type(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("type() expects 1 argument".into());
    }
    let v = state.get_value(1)?;
    state.push_string(v.type_name().to_string());
    Ok(1)
}

fn native_append(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("append() expects 2 arguments".into());
    }
    let list = state.get_value(1)?;
    let val = state.get_value(2)?;
    match list {
        Value::List(id) => {
            state.heap_mut().get_list_mut(id).push(val);
            state.push_nil();
            Ok(1)
        }
        _ => Err("append() expects a list".into()),
    }
}

fn native_pop(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("pop() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::List(id) => {
            let v = state.heap_mut().get_list_mut(id).pop().unwrap_or(Value::Nil);
            state.push_value(v);
            Ok(1)
        }
        _ => Err("pop() expects a list".into()),
    }
}

fn native_keys(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("keys() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Map(id) => {
            let key_strings: Vec<String> = state.heap().get_map(id).keys().cloned().collect();
            let keys: Vec<Value> = key_strings
                .into_iter()
                .map(|k| {
                    let sid = state.heap_mut().alloc_string(k);
                    Value::String(sid)
                })
                .collect();
            state.push_list(keys);
            Ok(1)
        }
        _ => Err("keys() expects a record".into()),
    }
}

fn native_values(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("values() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Map(id) => {
            let vals: Vec<Value> = state.heap().get_map(id).values().copied().collect();
            state.push_list(vals);
            Ok(1)
        }
        _ => Err("values() expects a record".into()),
    }
}

fn native_contains(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("contains() expects 2 arguments".into());
    }
    let container = state.get_value(1)?;
    let needle = state.get_value(2)?;
    match container {
        Value::List(id) => {
            let elems = state.heap().get_list(id);
            let found = elems
                .iter()
                .any(|v| value::values_equal(v, &needle, state.heap()));
            state.push_bool(found);
            Ok(1)
        }
        Value::String(id) => match needle {
            Value::String(sub_id) => {
                let s = state.heap().get_string(id).to_string();
                let sub = state.heap().get_string(sub_id).to_string();
                state.push_bool(s.contains(&sub));
                Ok(1)
            }
            _ => Err("contains() on string expects a string".into()),
        },
        _ => Err("contains() expects a list or string".into()),
    }
}

fn native_min(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("min() expects 2 arguments".into());
    }
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Greater => { state.push_value(b); Ok(1) }
    }
}

fn native_max(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("max() expects 2 arguments".into());
    }
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Less => { state.push_value(b); Ok(1) }
    }
}

fn native_round(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("round() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Int(n) => { state.push_int(n); Ok(1) }
        Value::Float(f) => { state.push_float(f.round()); Ok(1) }
        Value::Dual { value, .. } => {
            state.push_value(Value::Dual { value: value.round(), derivative: 0.0 });
            Ok(1)
        }
        _ => Err("round() expects a number".into()),
    }
}

// ---------------------------------------------------------------------------
// Automatic differentiation (dual numbers)
// ---------------------------------------------------------------------------

fn native_dual(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 2 {
        return Err("dual() expects 2 arguments (value, derivative)".into());
    }
    let value = match state.get_value(1)? {
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
        _ => return Err("dual() value must be a number".into()),
    };
    let derivative = match state.get_value(2)? {
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
        _ => return Err("dual() derivative must be a number".into()),
    };
    state.push_value(Value::Dual { value, derivative });
    Ok(1)
}

fn native_value_of(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("value_of() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Dual { value, .. } => { state.push_float(value); Ok(1) }
        Value::Int(n) => { state.push_float(n as f64); Ok(1) }
        Value::Float(f) => { state.push_float(f); Ok(1) }
        _ => Err("value_of() expects a number or dual".into()),
    }
}

fn native_deriv_of(state: &mut PetalState) -> Result<u32, String> {
    if state.arg_count() != 1 {
        return Err("deriv_of() expects 1 argument".into());
    }
    match state.get_value(1)? {
        Value::Dual { derivative, .. } => { state.push_float(derivative); Ok(1) }
        Value::Int(_) | Value::Float(_) => { state.push_float(0.0); Ok(1) }
        _ => Err("deriv_of() expects a number or dual".into()),
    }
}

// ---------------------------------------------------------------------------
// Utility (used by eval.rs for sorting, etc.)
// ---------------------------------------------------------------------------

pub fn compare_values(
    a: &Value,
    b: &Value,
    heap: &Heap,
) -> Result<std::cmp::Ordering, String> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Ok(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => {
            Ok(a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Int(a), Value::Float(b)) => {
            Ok((*a as f64)
                .partial_cmp(b)
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::Float(a), Value::Int(b)) => {
            Ok(a.partial_cmp(&(*b as f64))
                .unwrap_or(std::cmp::Ordering::Equal))
        }
        (Value::String(a), Value::String(b)) => {
            Ok(heap.get_string(*a).cmp(heap.get_string(*b)))
        }
        // Dual comparisons use primal value only
        _ if a.as_f64().is_some() && b.as_f64().is_some() => {
            let af = a.as_f64().unwrap();
            let bf = b.as_f64().unwrap();
            Ok(af.partial_cmp(&bf).unwrap_or(std::cmp::Ordering::Equal))
        }
        _ => Err(format!(
            "Cannot compare {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

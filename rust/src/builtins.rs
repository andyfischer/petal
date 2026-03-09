//! Builtins - Built-in function implementations registered via native FFI.

use crate::heap::Heap;
use crate::native_fn::{NativeFnTable, PetalCxt};
use crate::value::{self, Value};

/// Validate that a native function received exactly `$n` arguments.
macro_rules! require_args {
    ($state:expr, $n:expr, $name:expr) => {
        if $state.arg_count() != $n {
            return Err(format!(
                "{}() expects {} argument{}",
                $name,
                $n,
                if $n == 1 { "" } else { "s" }
            ));
        }
    };
}


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
    table.register("sort", native_sort);
    table.register("reverse", native_reverse);
    table.register("join", native_join);
    table.register("split", native_split);
    table.register("enumerate", native_enumerate);
    table.register("zip", native_zip);
    table.register("slice", native_slice);
    table.register("flat", native_flat);
    table.register("includes", native_contains); // JS-style alias for contains
    table.register("sin", native_sin);
    table.register("cos", native_cos);
    table.register("tan", native_tan);
    table.register("atan2", native_atan2);
    table.register("pi", native_pi);

    // --- Creative coding math builtins ---
    table.register("clamp", native_clamp);
    table.register("lerp", native_lerp);
    table.register("map_range", native_map_range);
    table.register("distance", native_distance);
    table.register("mag", native_mag);
    table.register("pow", native_pow);
    table.register("sign", native_sign);
    table.register("fract", native_fract);
    table.register("smoothstep", native_smoothstep);
    table.register("radians", native_radians);
    table.register("degrees", native_degrees);
    table.register("exp", native_exp);
    table.register("log", native_log);

    // --- Noise ---
    table.register("noise", native_noise);
    table.register("noise_seed", native_noise_seed);

    // --- Randomness ---
    table.register("random_int", native_random_int);
    table.register("choose", native_choose);

    // --- Color ---
    table.register("hsv", native_hsv);
    table.register("hsl", native_hsl);
    table.register("color_lerp", native_color_lerp);

    // --- Vec2 ---
    table.register("vec2", native_vec2);
    table.register("normalize", native_normalize);
    table.register("dot", native_dot);
    table.register("limit", native_limit);

    // Higher-order builtins: registered so the compiler sees them, but
    // dispatched as evaluator intrinsics at runtime.
    let map_id = table.register("map", native_intrinsic_placeholder);
    let filter_id = table.register("filter", native_intrinsic_placeholder);
    let reduce_id = table.register("reduce", native_intrinsic_placeholder);
    let for_each_id = table.register("forEach", native_intrinsic_placeholder);

    table.intrinsic_map = Some(map_id);
    table.intrinsic_filter = Some(filter_id);
    table.intrinsic_reduce = Some(reduce_id);
    table.intrinsic_for_each = Some(for_each_id);
}

// ---------------------------------------------------------------------------
// Placeholder for higher-order builtins (should never be called directly)
// ---------------------------------------------------------------------------

fn native_intrinsic_placeholder(_state: &mut PetalCxt) -> Result<u32, String> {
    Err("This function requires evaluator context and should be dispatched as an intrinsic".into())
}

// ---------------------------------------------------------------------------
// Native function implementations
// ---------------------------------------------------------------------------

fn native_print(state: &mut PetalCxt) -> Result<u32, String> {
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

fn native_range(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    let (start, end) = match argc {
        1 => {
            let end = state.get_int(1)?;
            (0, end)
        }
        2 => {
            let start = state.get_int(1)?;
            let end = state.get_int(2)?;
            (start, end)
        }
        _ => {
            return Err("range() expects 1 or 2 arguments".to_string());
        }
    };
    let items: Vec<Value> = (start..end).map(Value::Int).collect();
    state.push_list(items);
    Ok(1)
}

fn native_len(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "len");
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

fn native_push(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "push");
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

fn native_str(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "str");
    let v = state.get_value(1)?;
    let s = value::value_to_display_string(&v, state.heap());
    state.push_string(s);
    Ok(1)
}

fn native_abs(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "abs");
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

fn native_sqrt(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "sqrt");
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

fn native_floor(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "floor");
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

fn native_ceil(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "ceil");
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

fn native_float(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "float");
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

fn native_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "int");
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

fn native_random(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "random");
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

fn native_type(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "type");
    let v = state.get_value(1)?;
    state.push_string(v.type_name().to_string());
    Ok(1)
}

fn native_append(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "append");
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

fn native_pop(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "pop");
    match state.get_value(1)? {
        Value::List(id) => {
            let v = state.heap_mut().get_list_mut(id).pop().unwrap_or(Value::Nil);
            state.push_value(v);
            Ok(1)
        }
        _ => Err("pop() expects a list".into()),
    }
}

fn native_keys(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "keys");
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

fn native_values(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "values");
    match state.get_value(1)? {
        Value::Map(id) => {
            let vals: Vec<Value> = state.heap().get_map(id).values().copied().collect();
            state.push_list(vals);
            Ok(1)
        }
        _ => Err("values() expects a record".into()),
    }
}

fn native_contains(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "contains");
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

fn native_min(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "min");
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Less | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Greater => { state.push_value(b); Ok(1) }
    }
}

fn native_max(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "max");
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match compare_values(&a, &b, state.heap())? {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => { state.push_value(a); Ok(1) }
        std::cmp::Ordering::Less => { state.push_value(b); Ok(1) }
    }
}

fn native_round(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "round");
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
// List & string manipulation
// ---------------------------------------------------------------------------

/// Sort key extracted from Values so sorting doesn't need heap access.
#[derive(PartialEq, PartialOrd)]
enum SortKey {
    Num(f64),
    Str(String),
    Other,
}

impl Eq for SortKey {}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

fn native_sort(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "sort");
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            // Build sort keys: extract string content and numeric values up front
            // so the sort closure doesn't need heap access.
            let mut keyed: Vec<(SortKey, Value)> = items.into_iter()
                .map(|v| {
                    let key = match v {
                        Value::Int(n) => SortKey::Num(n as f64),
                        Value::Float(f) => SortKey::Num(f),
                        Value::Dual { value, .. } => SortKey::Num(value),
                        Value::String(sid) => SortKey::Str(state.heap().get_string(sid).to_string()),
                        _ => SortKey::Other,
                    };
                    (key, v)
                })
                .collect();
            keyed.sort_by(|(a, _), (b, _)| a.cmp(b));
            let sorted: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
            state.push_list(sorted);
            Ok(1)
        }
        _ => Err("sort() expects a list".into()),
    }
}

fn native_reverse(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "reverse");
    match state.get_value(1)? {
        Value::List(id) => {
            let mut items: Vec<Value> = state.heap().get_list(id).to_vec();
            items.reverse();
            state.push_list(items);
            Ok(1)
        }
        Value::String(id) => {
            let s: String = state.heap().get_string(id).chars().rev().collect();
            state.push_string(s);
            Ok(1)
        }
        _ => Err("reverse() expects a list or string".into()),
    }
}

fn native_join(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "join");
    let list = state.get_value(1)?;
    let sep = state.get_value(2)?;
    match (list, sep) {
        (Value::List(list_id), Value::String(sep_id)) => {
            let separator = state.heap().get_string(sep_id).to_string();
            let elements = state.heap().get_list(list_id);
            let parts: Vec<String> = elements.iter()
                .map(|v| value::value_to_display_string(v, state.heap()))
                .collect();
            let result = parts.join(&separator);
            state.push_string(result);
            Ok(1)
        }
        _ => Err("join() expects (list, string)".into()),
    }
}

fn native_split(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "split");
    let s = state.get_value(1)?;
    let sep = state.get_value(2)?;
    match (s, sep) {
        (Value::String(s_id), Value::String(sep_id)) => {
            let string = state.heap().get_string(s_id).to_string();
            let separator = state.heap().get_string(sep_id).to_string();
            let parts: Vec<Value> = string.split(&separator)
                .map(|part| {
                    let id = state.heap_mut().alloc_string(part.to_string());
                    Value::String(id)
                })
                .collect();
            state.push_list(parts);
            Ok(1)
        }
        _ => Err("split() expects (string, string)".into()),
    }
}

fn native_enumerate(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "enumerate");
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            let pairs: Vec<Value> = items.into_iter().enumerate()
                .map(|(i, v)| {
                    let pair = vec![Value::Int(i as i64), v];
                    let pair_id = state.heap_mut().alloc_list(pair);
                    Value::List(pair_id)
                })
                .collect();
            state.push_list(pairs);
            Ok(1)
        }
        _ => Err("enumerate() expects a list".into()),
    }
}

fn native_zip(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "zip");
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    match (a, b) {
        (Value::List(a_id), Value::List(b_id)) => {
            let a_items = state.heap().get_list(a_id).to_vec();
            let b_items = state.heap().get_list(b_id).to_vec();
            let pairs: Vec<Value> = a_items.into_iter().zip(b_items)
                .map(|(x, y)| {
                    let pair = vec![x, y];
                    let pair_id = state.heap_mut().alloc_list(pair);
                    Value::List(pair_id)
                })
                .collect();
            state.push_list(pairs);
            Ok(1)
        }
        _ => Err("zip() expects two lists".into()),
    }
}

fn native_slice(state: &mut PetalCxt) -> Result<u32, String> {
    if !(2..=3).contains(&state.arg_count()) {
        return Err("slice() expects 2-3 arguments".into());
    }
    let list = state.get_value(1)?;
    let start = state.get_int(2)?;
    match list {
        Value::List(id) => {
            let items = state.heap().get_list(id);
            let len = items.len() as i64;
            let start_idx = if start < 0 { (len + start).max(0) as usize } else { start.min(len) as usize };
            let end_idx = if state.arg_count() == 3 {
                let end = state.get_int(3)?;
                if end < 0 { (len + end).max(0) as usize } else { end.min(len) as usize }
            } else {
                len as usize
            };
            let sliced = if start_idx <= end_idx {
                items[start_idx..end_idx].to_vec()
            } else {
                Vec::new()
            };
            state.push_list(sliced);
            Ok(1)
        }
        Value::String(id) => {
            let s = state.heap().get_string(id);
            let len = s.len() as i64;
            let start_idx = if start < 0 { (len + start).max(0) as usize } else { start.min(len) as usize };
            let end_idx = if state.arg_count() == 3 {
                let end = state.get_int(3)?;
                if end < 0 { (len + end).max(0) as usize } else { end.min(len) as usize }
            } else {
                len as usize
            };
            let sliced = if start_idx <= end_idx {
                s[start_idx..end_idx].to_string()
            } else {
                String::new()
            };
            state.push_string(sliced);
            Ok(1)
        }
        _ => Err("slice() expects a list or string".into()),
    }
}

fn native_flat(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "flat");
    match state.get_value(1)? {
        Value::List(id) => {
            let items = state.heap().get_list(id).to_vec();
            let mut result = Vec::new();
            for item in items {
                match item {
                    Value::List(inner_id) => {
                        let inner = state.heap().get_list(inner_id).to_vec();
                        result.extend(inner);
                    }
                    _ => result.push(item),
                }
            }
            state.push_list(result);
            Ok(1)
        }
        _ => Err("flat() expects a list".into()),
    }
}

// ---------------------------------------------------------------------------
// Trigonometry
// ---------------------------------------------------------------------------

fn native_sin(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "sin");
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx sin(x) = cos(x) * dx
            state.push_value(Value::Dual { value: value.sin(), derivative: value.cos() * derivative });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.sin());
            Ok(1)
        }
    }
}

fn native_cos(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "cos");
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx cos(x) = -sin(x) * dx
            state.push_value(Value::Dual { value: value.cos(), derivative: -value.sin() * derivative });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.cos());
            Ok(1)
        }
    }
}

fn native_tan(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "tan");
    match state.get_value(1)? {
        Value::Dual { value, derivative } => {
            // d/dx tan(x) = sec^2(x) * dx = dx / cos^2(x)
            let cos_val = value.cos();
            state.push_value(Value::Dual { value: value.tan(), derivative: derivative / (cos_val * cos_val) });
            Ok(1)
        }
        _ => {
            let n = state.get_float(1)?;
            state.push_float(n.tan());
            Ok(1)
        }
    }
}

fn native_atan2(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "atan2");
    let y = state.get_float(1)?;
    let x = state.get_float(2)?;
    state.push_float(y.atan2(x));
    Ok(1)
}

fn native_pi(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 0, "pi");
    state.push_float(std::f64::consts::PI);
    Ok(1)
}

// ---------------------------------------------------------------------------
// Creative coding math
// ---------------------------------------------------------------------------

fn native_clamp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "clamp");
    let v = state.get_float(1)?;
    let lo = state.get_float(2)?;
    let hi = state.get_float(3)?;
    state.push_float(v.max(lo).min(hi));
    Ok(1)
}

fn native_lerp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "lerp");
    let a = state.get_float(1)?;
    let b = state.get_float(2)?;
    let t = state.get_float(3)?;
    state.push_float(a + (b - a) * t);
    Ok(1)
}

fn native_map_range(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 5, "map_range");
    let v = state.get_float(1)?;
    let in_lo = state.get_float(2)?;
    let in_hi = state.get_float(3)?;
    let out_lo = state.get_float(4)?;
    let out_hi = state.get_float(5)?;
    let t = if (in_hi - in_lo).abs() < f64::EPSILON {
        0.0
    } else {
        (v - in_lo) / (in_hi - in_lo)
    };
    state.push_float(out_lo + (out_hi - out_lo) * t);
    Ok(1)
}

fn native_distance(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    match argc {
        2 => {
            // distance(vec2, vec2)
            let a = state.get_value(1)?;
            let b = state.get_value(2)?;
            match (a, b) {
                (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
                    let dx = bx - ax;
                    let dy = by - ay;
                    state.push_float((dx * dx + dy * dy).sqrt());
                    Ok(1)
                }
                _ => Err("distance(a, b) expects two vec2 values".into()),
            }
        }
        4 => {
            let x1 = state.get_float(1)?;
            let y1 = state.get_float(2)?;
            let x2 = state.get_float(3)?;
            let y2 = state.get_float(4)?;
            let dx = x2 - x1;
            let dy = y2 - y1;
            state.push_float((dx * dx + dy * dy).sqrt());
            Ok(1)
        }
        _ => Err("distance() expects 2 (vec2, vec2) or 4 (x1, y1, x2, y2) arguments".into()),
    }
}

fn native_mag(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    match argc {
        1 => {
            // mag(vec2)
            match state.get_value(1)? {
                Value::Vec2(x, y) => {
                    state.push_float((x * x + y * y).sqrt());
                    Ok(1)
                }
                _ => {
                    let x = state.get_float(1)?;
                    state.push_float(x.abs());
                    Ok(1)
                }
            }
        }
        2 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            state.push_float((x * x + y * y).sqrt());
            Ok(1)
        }
        3 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            let z = state.get_float(3)?;
            state.push_float((x * x + y * y + z * z).sqrt());
            Ok(1)
        }
        _ => Err("mag() expects 1-3 arguments".into()),
    }
}

fn native_pow(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "pow");
    let base = state.get_float(1)?;
    let exp = state.get_float(2)?;
    state.push_float(base.powf(exp));
    Ok(1)
}

fn native_sign(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "sign");
    match state.get_value(1)? {
        Value::Int(n) => {
            state.push_int(if n > 0 { 1 } else if n < 0 { -1 } else { 0 });
            Ok(1)
        }
        Value::Float(f) => {
            state.push_float(if f > 0.0 { 1.0 } else if f < 0.0 { -1.0 } else { 0.0 });
            Ok(1)
        }
        _ => Err("sign() expects a number".into()),
    }
}

fn native_fract(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "fract");
    let f = state.get_float(1)?;
    state.push_float(f - f.floor());
    Ok(1)
}

fn native_smoothstep(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "smoothstep");
    let edge0 = state.get_float(1)?;
    let edge1 = state.get_float(2)?;
    let x = state.get_float(3)?;
    let t = ((x - edge0) / (edge1 - edge0)).max(0.0).min(1.0);
    state.push_float(t * t * (3.0 - 2.0 * t));
    Ok(1)
}

fn native_radians(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "radians");
    let deg = state.get_float(1)?;
    state.push_float(deg.to_radians());
    Ok(1)
}

fn native_degrees(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "degrees");
    let rad = state.get_float(1)?;
    state.push_float(rad.to_degrees());
    Ok(1)
}

fn native_exp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "exp");
    let x = state.get_float(1)?;
    state.push_float(x.exp());
    Ok(1)
}

fn native_log(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "log");
    let x = state.get_float(1)?;
    state.push_float(x.ln());
    Ok(1)
}

// ---------------------------------------------------------------------------
// Perlin noise (simplex-like implementation)
// ---------------------------------------------------------------------------

/// Global noise seed, set via noise_seed().
static NOISE_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Permutation table for noise, derived from seed.
fn noise_perm(seed: u64) -> [u8; 512] {
    let mut perm = [0u8; 512];
    let mut p = [0u8; 256];
    for i in 0..256 {
        p[i] = i as u8;
    }
    // Fisher-Yates shuffle with seed
    let mut rng = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..256).rev() {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = (rng >> 33) as usize % (i + 1);
        p.swap(i, j);
    }
    for i in 0..512 {
        perm[i] = p[i & 255];
    }
    perm
}

fn grad1(hash: u8, x: f64) -> f64 {
    if hash & 1 == 0 { x } else { -x }
}

fn grad2(hash: u8, x: f64, y: f64) -> f64 {
    let h = hash & 3;
    match h {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        _ => -x - y,
    }
}

fn grad3(hash: u8, x: f64, y: f64, z: f64) -> f64 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 { y } else if h == 12 || h == 14 { x } else { z };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn perlin_1d(x: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let xf = x - x.floor();
    let u = fade(xf);
    let a = perm[xi as usize];
    let b = perm[(xi + 1) as usize & 255];
    let g0 = grad1(a, xf);
    let g1 = grad1(b, xf - 1.0);
    g0 + u * (g1 - g0)
}

fn perlin_2d(x: f64, y: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let yi = y.floor() as i32 & 255;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let u = fade(xf);
    let v = fade(yf);

    let aa = perm[perm[xi as usize] as usize + yi as usize] as usize;
    let ab = perm[perm[xi as usize] as usize + (yi + 1) as usize & 255] as usize;
    let ba = perm[perm[(xi + 1) as usize & 255] as usize + yi as usize] as usize;
    let bb = perm[perm[(xi + 1) as usize & 255] as usize + (yi + 1) as usize & 255] as usize;

    let x1 = grad2(perm[aa], xf, yf);
    let x2 = grad2(perm[ba], xf - 1.0, yf);
    let y1 = grad2(perm[ab], xf, yf - 1.0);
    let y2 = grad2(perm[bb], xf - 1.0, yf - 1.0);

    let lerp_x1 = x1 + u * (x2 - x1);
    let lerp_x2 = y1 + u * (y2 - y1);
    lerp_x1 + v * (lerp_x2 - lerp_x1)
}

fn perlin_3d(x: f64, y: f64, z: f64, perm: &[u8; 512]) -> f64 {
    let xi = x.floor() as i32 & 255;
    let yi = y.floor() as i32 & 255;
    let zi = z.floor() as i32 & 255;
    let xf = x - x.floor();
    let yf = y - y.floor();
    let zf = z - z.floor();
    let u = fade(xf);
    let v = fade(yf);
    let w = fade(zf);

    let a  = perm[xi as usize] as usize + yi as usize;
    let aa = perm[a & 255] as usize + zi as usize;
    let ab = perm[(a + 1) & 255] as usize + zi as usize;
    let b  = perm[((xi + 1) & 255) as usize] as usize + yi as usize;
    let ba = perm[b & 255] as usize + zi as usize;
    let bb = perm[(b + 1) & 255] as usize + zi as usize;

    let l1 = grad3(perm[aa & 511], xf, yf, zf);
    let l2 = grad3(perm[(ba) & 511], xf - 1.0, yf, zf);
    let l3 = grad3(perm[(ab) & 511], xf, yf - 1.0, zf);
    let l4 = grad3(perm[(bb) & 511], xf - 1.0, yf - 1.0, zf);
    let l5 = grad3(perm[(aa + 1) & 511], xf, yf, zf - 1.0);
    let l6 = grad3(perm[(ba + 1) & 511], xf - 1.0, yf, zf - 1.0);
    let l7 = grad3(perm[(ab + 1) & 511], xf, yf - 1.0, zf - 1.0);
    let l8 = grad3(perm[(bb + 1) & 511], xf - 1.0, yf - 1.0, zf - 1.0);

    let x1 = l1 + u * (l2 - l1);
    let x2 = l3 + u * (l4 - l3);
    let x3 = l5 + u * (l6 - l5);
    let x4 = l7 + u * (l8 - l7);
    let y1 = x1 + v * (x2 - x1);
    let y2 = x3 + v * (x4 - x3);
    y1 + w * (y2 - y1)
}

fn native_noise(state: &mut PetalCxt) -> Result<u32, String> {
    let argc = state.arg_count();
    let seed = NOISE_SEED.load(std::sync::atomic::Ordering::Relaxed);
    let perm = noise_perm(seed);
    match argc {
        1 => {
            let x = state.get_float(1)?;
            state.push_float(perlin_1d(x, &perm));
            Ok(1)
        }
        2 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            state.push_float(perlin_2d(x, y, &perm));
            Ok(1)
        }
        3 => {
            let x = state.get_float(1)?;
            let y = state.get_float(2)?;
            let z = state.get_float(3)?;
            state.push_float(perlin_3d(x, y, z, &perm));
            Ok(1)
        }
        _ => Err("noise() expects 1-3 arguments".into()),
    }
}

fn native_noise_seed(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "noise_seed");
    let seed = state.get_int(1)? as u64;
    NOISE_SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
    state.push_nil();
    Ok(1)
}

// ---------------------------------------------------------------------------
// Extended randomness
// ---------------------------------------------------------------------------

fn native_random_int(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "random_int");
    let min = state.get_int(1)?;
    let max = state.get_int(2)?;
    if min >= max {
        state.push_int(min);
        return Ok(1);
    }
    let pseudo = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let range = (max - min) as u64;
    let val = min + (pseudo as u64 % range) as i64;
    state.push_int(val);
    Ok(1)
}

fn native_choose(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "choose");
    match state.get_value(1)? {
        Value::List(id) => {
            let list = state.heap().get_list(id);
            if list.is_empty() {
                state.push_nil();
            } else {
                let pseudo = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos();
                let idx = pseudo as usize % list.len();
                let val = list[idx];
                state.push_value(val);
            }
            Ok(1)
        }
        _ => Err("choose() expects a list".into()),
    }
}

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

/// HSV to RGB conversion. h: 0-360, s: 0-1, v: 0-1. Returns (r, g, b) 0-255.
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// HSL to RGB conversion. h: 0-360, s: 0-1, l: 0-1. Returns (r, g, b) 0-255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

fn push_color_map(state: &mut PetalCxt, r: f64, g: f64, b: f64) {
    let mut map = indexmap::IndexMap::new();
    map.insert("r".to_string(), Value::Int(r.round() as i64));
    map.insert("g".to_string(), Value::Int(g.round() as i64));
    map.insert("b".to_string(), Value::Int(b.round() as i64));
    let map_id = state.heap_mut().alloc_map(map);
    state.push_value(Value::Map(map_id));
}

fn native_hsv(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "hsv");
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let v = state.get_float(3)?;
    let (r, g, b) = hsv_to_rgb(h, s, v);
    push_color_map(state, r, g, b);
    Ok(1)
}

fn native_hsl(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "hsl");
    let h = state.get_float(1)?;
    let s = state.get_float(2)?;
    let l = state.get_float(3)?;
    let (r, g, b) = hsl_to_rgb(h, s, l);
    push_color_map(state, r, g, b);
    Ok(1)
}

fn native_color_lerp(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 3, "color_lerp");
    let c1 = state.get_value(1)?;
    let c2 = state.get_value(2)?;
    let t = state.get_float(3)?;
    match (c1, c2) {
        (Value::Map(id1), Value::Map(id2)) => {
            let m1 = state.heap().get_map(id1);
            let r1 = m1.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let g1 = m1.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b1 = m1.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let m2 = state.heap().get_map(id2);
            let r2 = m2.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let g2 = m2.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b2 = m2.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let r = r1 + (r2 - r1) * t;
            let g = g1 + (g2 - g1) * t;
            let b = b1 + (b2 - b1) * t;
            push_color_map(state, r, g, b);
            Ok(1)
        }
        _ => Err("color_lerp() expects two color records {r, g, b}".into()),
    }
}

// ---------------------------------------------------------------------------
// Vec2
// ---------------------------------------------------------------------------

fn native_vec2(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "vec2");
    let x = state.get_float(1)?;
    let y = state.get_float(2)?;
    state.push_value(Value::Vec2(x, y));
    Ok(1)
}

fn native_normalize(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "normalize");
    match state.get_value(1)? {
        Value::Vec2(x, y) => {
            let m = (x * x + y * y).sqrt();
            if m < f64::EPSILON {
                state.push_value(Value::Vec2(0.0, 0.0));
            } else {
                state.push_value(Value::Vec2(x / m, y / m));
            }
            Ok(1)
        }
        _ => Err("normalize() expects a vec2".into()),
    }
}

fn native_dot(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "dot");
    match (state.get_value(1)?, state.get_value(2)?) {
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            state.push_float(ax * bx + ay * by);
            Ok(1)
        }
        _ => Err("dot() expects two vec2 values".into()),
    }
}

fn native_limit(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "limit");
    match state.get_value(1)? {
        Value::Vec2(x, y) => {
            let max_mag = state.get_float(2)?;
            let m = (x * x + y * y).sqrt();
            if m > max_mag && m > f64::EPSILON {
                let scale = max_mag / m;
                state.push_value(Value::Vec2(x * scale, y * scale));
            } else {
                state.push_value(Value::Vec2(x, y));
            }
            Ok(1)
        }
        _ => Err("limit() expects a vec2 as first argument".into()),
    }
}

// ---------------------------------------------------------------------------
// Automatic differentiation (dual numbers)
// ---------------------------------------------------------------------------

fn native_dual(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 2, "dual");
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

fn native_value_of(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "value_of");
    match state.get_value(1)? {
        Value::Dual { value, .. } => { state.push_float(value); Ok(1) }
        Value::Int(n) => { state.push_float(n as f64); Ok(1) }
        Value::Float(f) => { state.push_float(f); Ok(1) }
        _ => Err("value_of() expects a number or dual".into()),
    }
}

fn native_deriv_of(state: &mut PetalCxt) -> Result<u32, String> {
    require_args!(state, 1, "deriv_of");
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

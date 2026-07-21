//! Shared primitive operation handlers used by *both* execution backends.
//!
//! These are the pure, value-producing bodies of the arithmetic, comparison,
//! logical, container-allocation, and field/index ops. Factoring them here —
//! rather than duplicating the logic in each engine — is the parity lever from
//! the bytecode plan: the graph [`Evaluator`](super::graph::Evaluator) and the
//! bytecode `Vm` call the same functions, so their arithmetic / allocation /
//! access semantics cannot drift.
//!
//! Each function is pure over `(&Program, &mut Heap, payload, inputs)` and
//! returns `Result<Value, String>` (or a bare `Value` when it cannot fail). The
//! caller stores the result into a register and is responsible for error
//! annotation (source snippet / stack trace).

use indexmap::IndexMap;

use crate::constant_table::{ConstantId, ConstantValue};
use crate::heap::Heap;
use crate::program::{MapSpreadEntry, Program, TermOp};
use crate::value::{self, Value};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Materialize a compile-time constant into a runtime `Value`, interning string
/// constants on the heap.
pub fn constant_to_value(program: &Program, heap: &mut Heap, cid: ConstantId) -> Value {
    match program.constants.get(cid) {
        ConstantValue::Nil => Value::Nil,
        ConstantValue::Bool(b) => Value::Bool(*b),
        ConstantValue::Int(n) => Value::Int(*n),
        ConstantValue::Float(bits) => Value::Float(f64::from_bits(*bits)),
        ConstantValue::String(s) => Value::String(heap.alloc_string(s.clone())),
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

/// Arithmetic on Int/Float pairs, with dual-number (forward-mode AD) and vec2
/// operands delegated to their own handlers. `op` must be one of
/// `Add`/`Sub`/`Mul`/`Div`/`Mod`.
pub fn arithmetic(op: &TermOp, a: Value, b: Value, heap: &mut Heap) -> Result<Value, String> {
    // Pending is strict-absorbing: any Pending operand short-circuits the whole
    // op to that Pending (leftmost wins). Checked before the div-by-zero guard
    // so `pending / 0` yields the Pending, not a spurious error. Mirrors the
    // Value::Dual precedent below, but absorbs rather than threading a payload.
    if let Some(p) = leftmost_pending(a, b) {
        return Ok(p);
    }
    // Guard integer/float division by zero up front (dual/vec2 handlers do
    // their own checks, so this only fires for scalar operands).
    if matches!(op, TermOp::Div) {
        match b {
            Value::Int(0) => return Err("Division by zero".into()),
            Value::Float(f) if f == 0.0 => return Err("Division by zero".into()),
            _ => {}
        }
    }
    match (a, b) {
        (Value::Dual { .. }, _) | (_, Value::Dual { .. }) => dual_arith(op, a, b),
        (Value::Vec2(..), _) | (_, Value::Vec2(..)) => vec2_arith(op, a, b),
        (Value::List(..), _) | (_, Value::List(..)) => list_scalar_arith(op, a, b, heap),
        (Value::Int(x), Value::Int(y)) => int_arith(op, x, y).map(Value::Int),
        (Value::Float(x), Value::Float(y)) => Ok(Value::Float(float_arith(op, x, y))),
        (Value::Int(x), Value::Float(y)) => Ok(Value::Float(float_arith(op, x as f64, y))),
        (Value::Float(x), Value::Int(y)) => Ok(Value::Float(float_arith(op, x, y as f64))),
        // `+` on two strings is the mistake every JS/Python user makes first;
        // point them at `++` / interpolation instead of a vague type error.
        (Value::String(_), Value::String(_)) if matches!(op, TermOp::Add) => Err(
            "Cannot add string and string — use ++ to concatenate strings, \
             or string interpolation: \"{a}{b}\""
                .into(),
        ),
        _ => Err(format!(
            "Cannot {} {} and {}",
            binop_verb(op),
            a.type_name(),
            b.type_name()
        )),
    }
}

/// The leftmost `Value::Pending` among two operands, if either is Pending.
///
/// This is the single strict-absorption primitive shared by every binary
/// operator: a Pending operand makes the whole operation Pending, and when both
/// operands are Pending the *leftmost* one wins (the design's rule). `Value` is
/// `Copy`, so the returned Pending is the same thin `PendingId` handle.
fn leftmost_pending(a: Value, b: Value) -> Option<Value> {
    match (a, b) {
        (p @ Value::Pending(_), _) | (_, p @ Value::Pending(_)) => Some(p),
        _ => None,
    }
}

/// Forward-mode AD arithmetic for Dual numbers.
fn dual_arith(op: &TermOp, a: Value, b: Value) -> Result<Value, String> {
    let (Some(a_val), Some(b_val)) = (a.as_f64(), b.as_f64()) else {
        return Err(format!(
            "Cannot perform arithmetic on {} and {}",
            a.type_name(),
            b.type_name()
        ));
    };
    let a_deriv = a.derivative();
    let b_deriv = b.derivative();

    let (value, derivative) = match op {
        TermOp::Add => (a_val + b_val, a_deriv + b_deriv),
        TermOp::Sub => (a_val - b_val, a_deriv - b_deriv),
        TermOp::Mul => (a_val * b_val, a_deriv * b_val + a_val * b_deriv),
        TermOp::Div => {
            if b_val == 0.0 {
                return Err("Division by zero".into());
            }
            (
                a_val / b_val,
                (a_deriv * b_val - a_val * b_deriv) / (b_val * b_val),
            )
        }
        // Mod derivative: d(a%b)/da = 1, d(a%b)/db is complex; approximate.
        TermOp::Mod => (a_val % b_val, a_deriv),
        _ => return Err("Unsupported dual operation".into()),
    };

    Ok(Value::Dual { value, derivative })
}

/// Vec2 arithmetic: component-wise between vectors, scalar broadcast otherwise.
fn vec2_arith(op: &TermOp, a: Value, b: Value) -> Result<Value, String> {
    let val = match (a, b) {
        // vec2 op vec2
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => match op {
            TermOp::Add => Value::Vec2(ax + bx, ay + by),
            TermOp::Sub => Value::Vec2(ax - bx, ay - by),
            TermOp::Mul => Value::Vec2(ax * bx, ay * by),
            TermOp::Div => {
                if bx == 0.0 || by == 0.0 {
                    return Err("Division by zero in vec2".into());
                }
                Value::Vec2(ax / bx, ay / by)
            }
            _ => return Err("Unsupported vec2 operation".into()),
        },
        // vec2 op scalar
        (Value::Vec2(x, y), other) => {
            let s = match other.as_f64() {
                Some(v) => v,
                None => {
                    return Err(format!(
                        "Cannot perform arithmetic on vec2 and {}",
                        other.type_name()
                    ));
                }
            };
            match op {
                TermOp::Mul => Value::Vec2(x * s, y * s),
                TermOp::Div => {
                    if s == 0.0 {
                        return Err("Division by zero".into());
                    }
                    Value::Vec2(x / s, y / s)
                }
                TermOp::Add => Value::Vec2(x + s, y + s),
                TermOp::Sub => Value::Vec2(x - s, y - s),
                _ => return Err("Unsupported vec2 operation".into()),
            }
        }
        // scalar op vec2
        (other, Value::Vec2(x, y)) => {
            let s = match other.as_f64() {
                Some(v) => v,
                None => {
                    return Err(format!(
                        "Cannot perform arithmetic on {} and vec2",
                        other.type_name()
                    ));
                }
            };
            match op {
                TermOp::Mul => Value::Vec2(s * x, s * y),
                TermOp::Add => Value::Vec2(s + x, s + y),
                TermOp::Sub => Value::Vec2(s - x, s - y),
                _ => return Err("Unsupported vec2 operation".into()),
            }
        }
        _ => return Err("Unsupported vec2 operation".into()),
    };
    Ok(val)
}

/// Broadcast a numeric scalar across a list element-wise: `[a, b, c] op s` or
/// `s op [a, b, c]`. This is the initial linear-algebra support — only a list
/// against a numeric scalar (Int/Float), for `+`/`-`/`*`/`/`. List-against-list
/// and other array operations are deliberately unsupported (they fall through to
/// the "Cannot ..." error, since neither operand is a numeric scalar).
///
/// Each element is combined via `arithmetic` itself, so Int/Float promotion and
/// per-element division-by-zero behave exactly as they do for scalar operands.
fn list_scalar_arith(op: &TermOp, a: Value, b: Value, heap: &mut Heap) -> Result<Value, String> {
    let is_scalar = |v: &Value| matches!(v, Value::Int(_) | Value::Float(_));
    let (list_id, scalar, list_on_left) = match (a, b) {
        (Value::List(id), s) if is_scalar(&s) => (id, s, true),
        (s, Value::List(id)) if is_scalar(&s) => (id, s, false),
        _ => {
            return Err(format!(
                "Cannot {} {} and {}",
                binop_verb(op),
                a.type_name(),
                b.type_name()
            ));
        }
    };
    let elems = heap.get_list(list_id).to_vec();
    let mut out = Vec::with_capacity(elems.len());
    for el in elems {
        if !is_scalar(&el) {
            return Err(format!(
                "Cannot {} a list containing {} by a scalar",
                binop_verb(op),
                el.type_name()
            ));
        }
        let result = if list_on_left {
            arithmetic(op, el, scalar, heap)?
        } else {
            arithmetic(op, scalar, el, heap)?
        };
        out.push(result);
    }
    Ok(Value::List(heap.alloc_list(out)))
}

/// Integer arithmetic with checked operators. A raw `+`/`*`/`%` would panic on
/// overflow or a zero divisor, which in WASM is an `unreachable` trap that
/// poisons the whole module; a clean `Err` surfaces a normal runtime error.
fn int_arith(op: &TermOp, a: i64, b: i64) -> Result<i64, String> {
    let result = match op {
        TermOp::Add => a.checked_add(b),
        TermOp::Sub => a.checked_sub(b),
        TermOp::Mul => a.checked_mul(b),
        // checked_div / checked_rem return None for a zero divisor and for the
        // i64::MIN / -1 overflow case.
        TermOp::Div => a.checked_div(b),
        TermOp::Mod => a.checked_rem(b),
        _ => unreachable!("non-arithmetic op in arithmetic()"),
    };
    result.ok_or_else(|| {
        if b == 0 && matches!(op, TermOp::Div | TermOp::Mod) {
            "Division by zero".to_string()
        } else {
            format!("Integer overflow when trying to {}", binop_verb(op))
        }
    })
}

fn float_arith(op: &TermOp, a: f64, b: f64) -> f64 {
    match op {
        TermOp::Add => a + b,
        TermOp::Sub => a - b,
        TermOp::Mul => a * b,
        TermOp::Div => a / b,
        TermOp::Mod => a % b,
        _ => unreachable!("non-arithmetic op in arithmetic()"),
    }
}

/// Human-readable verb for a binary op, used in error messages so the message
/// says "Cannot add Int and String" rather than the vague "Cannot perform
/// arithmetic on Int and String".
fn binop_verb(op: &TermOp) -> &'static str {
    match op {
        TermOp::Add => "add",
        TermOp::Sub => "subtract",
        TermOp::Mul => "multiply",
        TermOp::Div => "divide",
        TermOp::Mod => "take the modulus of",
        _ => "perform arithmetic on",
    }
}

// ---------------------------------------------------------------------------
// Comparison / logical
// ---------------------------------------------------------------------------

/// `==` with Pending absorption. A Pending operand makes the comparison itself
/// Pending — never `Bool` — because a comparison against an unresolved value is
/// *unknown*, not false (the SQL-NULL footgun the design explicitly forbids).
/// On resolved operands it is ordinary structural equality.
pub fn eq(a: Value, b: Value, heap: &Heap) -> Value {
    match leftmost_pending(a, b) {
        Some(p) => p,
        None => Value::Bool(value::values_equal(&a, &b, heap)),
    }
}

/// `!=` with the same Pending absorption as [`eq`]; the negation only applies to
/// resolved operands, so a Pending stays Pending rather than flipping to a Bool.
pub fn ne(a: Value, b: Value, heap: &Heap) -> Value {
    match leftmost_pending(a, b) {
        Some(p) => p,
        None => Value::Bool(!value::values_equal(&a, &b, heap)),
    }
}

/// Lt / Le / Gt / Ge via the shared value-ordering in `builtins`.
pub fn comparison(op: &TermOp, a: Value, b: Value, heap: &Heap) -> Result<Value, String> {
    use std::cmp::Ordering;
    // A Pending operand absorbs: the ordering is unknown, so the result is that
    // Pending — never a Bool. Same footgun-avoidance as `eq`/`ne`.
    if let Some(p) = leftmost_pending(a, b) {
        return Ok(p);
    }
    let ord = value::compare_values(&a, &b, heap)?;
    let result = match op {
        TermOp::Lt => ord == Ordering::Less,
        TermOp::Le => ord != Ordering::Greater,
        TermOp::Gt => ord == Ordering::Greater,
        TermOp::Ge => ord != Ordering::Less,
        _ => unreachable!("comparison() called for non-comparison op"),
    };
    Ok(Value::Bool(result))
}

/// Logical `!`: negate truthiness. A Pending operand absorbs (the negation is
/// unknown), yielding the same Pending rather than a Bool.
pub fn not(v: Value) -> Value {
    if let Value::Pending(_) = v {
        return v;
    }
    Value::Bool(!v.is_truthy())
}

/// Unary negation for numbers, dual numbers, and vec2.
pub fn negate(v: Value) -> Result<Value, String> {
    match v {
        // Pending absorbs: negating an unresolved value stays that Pending.
        p @ Value::Pending(_) => Ok(p),
        Value::Int(n) => Ok(Value::Int(-n)),
        Value::Float(f) => Ok(Value::Float(-f)),
        Value::Dual { value, derivative } => Ok(Value::Dual {
            value: -value,
            derivative: -derivative,
        }),
        Value::Vec2(x, y) => Ok(Value::Vec2(-x, -y)),
        other => Err(format!("Cannot negate {}", other.type_name())),
    }
}

/// `++`: list concatenation, or string concatenation with display conversion
/// for non-string operands.
pub fn concat(a: Value, b: Value, heap: &mut Heap) -> Result<Value, String> {
    // Pending absorbs: any Pending part makes the whole result Pending (the
    // string never materializes). String interpolation lowers to `++`, so this
    // one guard covers interpolation too.
    if let Some(p) = leftmost_pending(a, b) {
        return Ok(p);
    }
    match (a, b) {
        (Value::List(x), Value::List(y)) => {
            let mut combined = heap.get_list(x).to_vec();
            combined.extend_from_slice(heap.get_list(y));
            Ok(Value::List(heap.alloc_list(combined)))
        }
        _ => {
            let l = value::value_to_display_string(&a, heap);
            let r = value::value_to_display_string(&b, heap);
            Ok(Value::String(heap.alloc_string(format!("{}{}", l, r))))
        }
    }
}

// ---------------------------------------------------------------------------
// Container allocation
// ---------------------------------------------------------------------------

/// `[a, b, ...]`.
pub fn alloc_list(heap: &mut Heap, elems: &[Value]) -> Value {
    Value::List(heap.alloc_list(elems.to_vec()))
}

/// `{ field: val, ... }` — `fields[i]` is keyed to `inputs[i]`.
pub fn alloc_map(
    program: &Program,
    heap: &mut Heap,
    fields: &[ConstantId],
    inputs: &[Value],
) -> Result<Value, String> {
    let mut map = IndexMap::new();
    for (i, field_cid) in fields.iter().enumerate() {
        if let Some(key) = program.get_string_constant(*field_cid) {
            let val = inputs.get(i).copied().unwrap_or(Value::Nil);
            map.insert(key.to_string(), val);
        }
    }
    Ok(Value::Map(heap.alloc_map(map)))
}

/// `{ ...spread, field: val }` — entries are applied in order.
pub fn alloc_map_spread(
    program: &Program,
    heap: &mut Heap,
    entries: &[MapSpreadEntry],
    inputs: &[Value],
) -> Result<Value, String> {
    let mut map = IndexMap::new();
    for entry in entries {
        match entry {
            MapSpreadEntry::Spread(idx) => {
                let src = inputs.get(*idx).copied().unwrap_or(Value::Nil);
                match src {
                    Value::Map(src_id) => {
                        let pairs: Vec<(String, Value)> = heap
                            .get_map(src_id)
                            .iter()
                            .map(|(k, v)| (k.clone(), *v))
                            .collect();
                        for (k, v) in pairs {
                            map.insert(k, v);
                        }
                    }
                    Value::Nil => {} // Spreading nil is a no-op
                    _ => {
                        return Err(format!(
                            "Cannot spread {} into record (expected record)",
                            src.type_name()
                        ));
                    }
                }
            }
            MapSpreadEntry::Named(cid, idx) => {
                if let Some(key) = program.get_string_constant(*cid) {
                    let val = inputs.get(*idx).copied().unwrap_or(Value::Nil);
                    map.insert(key.to_string(), val);
                }
            }
        }
    }
    Ok(Value::Map(heap.alloc_map(map)))
}

/// JSX-like element `<tag prop=... >children</tag>`. `inputs` is
/// `[props..., children...]`, split at `prop_keys.len()`.
pub fn alloc_element(
    program: &Program,
    heap: &mut Heap,
    tag: ConstantId,
    prop_keys: &[ConstantId],
    inputs: &[Value],
) -> Result<Value, String> {
    let tag_str = match program.get_string_constant(tag) {
        Some(s) => s.to_string(),
        None => return Err("AllocElement: invalid tag".into()),
    };
    let tag_id = heap.alloc_string(tag_str);

    let mut map = IndexMap::new();
    for (i, key_cid) in prop_keys.iter().enumerate() {
        if let Some(key) = program.get_string_constant(*key_cid) {
            let val = inputs.get(i).copied().unwrap_or(Value::Nil);
            map.insert(key.to_string(), val);
        }
    }
    let props_id = heap.alloc_map(map);
    let children_id = heap.alloc_list(inputs[prop_keys.len()..].to_vec());
    Ok(Value::Element(heap.alloc_element(
        tag_id,
        props_id,
        children_id,
    )))
}

/// `Variant(fields...)` — an enum variant carrying a name tag and field list.
pub fn make_enum_variant(
    program: &Program,
    heap: &mut Heap,
    name_cid: ConstantId,
    inputs: &[Value],
) -> Result<Value, String> {
    let name_str = match program.get_string_constant(name_cid) {
        Some(s) => s.to_string(),
        None => return Err("MakeEnumVariant: invalid name".into()),
    };
    let tag = heap.alloc_string(name_str);
    let data = heap.alloc_list(inputs.to_vec());
    Ok(Value::EnumVariant { tag, data })
}

// ---------------------------------------------------------------------------
// Field / index access
// ---------------------------------------------------------------------------

/// Field access on records, elements, lists/strings (`.length`), and vec2.
pub fn get_field(
    program: &Program,
    heap: &Heap,
    field_cid: ConstantId,
    obj: Value,
) -> Result<Value, String> {
    // Pending base absorbs: `pending.name` is the same Pending, regardless of
    // which field is requested (the object isn't there yet to have fields).
    if let Value::Pending(_) = obj {
        return Ok(obj);
    }
    let field_name = match program.get_string_constant(field_cid) {
        Some(s) => s,
        None => return Err("GetField: invalid field name".into()),
    };
    let val = match obj {
        Value::Map(map_id) => match heap.get_map(map_id).get(field_name).copied() {
            Some(v) => v,
            None => return Err(format!("No field '{}' on record", field_name)),
        },
        Value::Element(elem_id) => match field_name {
            "tag" => Value::String(heap.get_element_tag(elem_id)),
            "props" => Value::Map(heap.get_element_props(elem_id)),
            "children" => Value::List(heap.get_element_children(elem_id)),
            _ => return Err(format!("No field '{}' on element", field_name)),
        },
        Value::List(list_id) if field_name == "length" => Value::Int(heap.list_len(list_id) as i64),
        Value::String(str_id) if field_name == "length" => {
            Value::Int(heap.get_string(str_id).len() as i64)
        }
        Value::Vec2(x, y) => match field_name {
            "x" => Value::Float(x),
            "y" => Value::Float(y),
            _ => {
                return Err(format!(
                    "No field '{}' on vec2 (available: x, y)",
                    field_name
                ));
            }
        },
        _ => {
            return Err(format!(
                "Cannot access field '{}' on {}",
                field_name,
                obj.type_name()
            ));
        }
    };
    Ok(val)
}

/// `obj.field = val` — value semantics: produces a *new* record.
pub fn set_field(
    program: &Program,
    heap: &mut Heap,
    field_cid: ConstantId,
    obj: Value,
    val: Value,
) -> Result<Value, String> {
    set_field_impl(program, heap, field_cid, obj, val, false)
}

/// In-place `obj.field = val`: mutates `obj`'s backing map and reuses its id.
/// Sound only when the bytecode escape analysis has proven `obj` unique and
/// non-escaping (see `backend/bytecode/escape.rs`); the VM emits this via
/// `Inst::SetFieldInPlace` only when that gate holds (never with opts off).
pub fn set_field_in_place(
    program: &Program,
    heap: &mut Heap,
    field_cid: ConstantId,
    obj: Value,
    val: Value,
) -> Result<Value, String> {
    set_field_impl(program, heap, field_cid, obj, val, true)
}

fn set_field_impl(
    program: &Program,
    heap: &mut Heap,
    field_cid: ConstantId,
    obj: Value,
    val: Value,
    in_place: bool,
) -> Result<Value, String> {
    match obj {
        Value::Map(map_id) => {
            let field_name = match program.get_string_constant(field_cid) {
                Some(s) => s.to_string(),
                None => return Err("SetField: invalid field name".into()),
            };
            let new_id = if in_place {
                heap.map_set_in_place(map_id, field_name, val)
            } else {
                heap.map_set(map_id, field_name, val)
            };
            Ok(Value::Map(new_id))
        }
        _ => Err(format!("Cannot set field on {}", obj.type_name())),
    }
}

/// `obj[idx]` on lists (negative indices count from the end), f64 arrays, and
/// records (string key).
pub fn get_index(heap: &Heap, obj: Value, idx: Value) -> Result<Value, String> {
    // Pending base absorbs: `pending[i]` is the same Pending. (A resolved
    // collection *containing* a Pending is element-wise — a later chunk — so
    // only the base is handled here, not a Pending index.)
    if let Value::Pending(_) = obj {
        return Ok(obj);
    }
    match (obj, idx) {
        (Value::List(list_id), Value::Int(i)) => {
            let list = heap.get_list(list_id);
            let index = if i < 0 {
                (list.len() as i64 + i) as usize
            } else {
                i as usize
            };
            match list.get(index).copied() {
                Some(v) => Ok(v),
                None => Err(format!(
                    "Index {} out of bounds (len {})",
                    i,
                    heap.list_len(list_id)
                )),
            }
        }
        (Value::F64Array(arr_id), Value::Int(i)) => {
            let data = heap.get_f64_array(arr_id);
            if i < 0 || i as usize >= data.len() {
                Err(format!("Index {} out of bounds (len {})", i, data.len()))
            } else {
                Ok(Value::Float(data[i as usize]))
            }
        }
        (Value::Map(map_id), Value::String(key_id)) => {
            let key = heap.get_string(key_id).to_string();
            match heap.get_map(map_id).get(&key).copied() {
                Some(v) => Ok(v),
                None => Err(format!("No key '{}' on record", key)),
            }
        }
        _ => Err(format!(
            "Cannot index {} with {}",
            obj.type_name(),
            idx.type_name()
        )),
    }
}

/// `obj[idx] = val` — value semantics: produces a *new* container.
pub fn set_index(heap: &mut Heap, obj: Value, idx: Value, val: Value) -> Result<Value, String> {
    set_index_impl(heap, obj, idx, val, false)
}

/// In-place `obj[idx] = val`: mutates `obj`'s backing store and reuses its id.
/// Sound only under the escape-analysis gate; the VM emits this via
/// `Inst::SetIndexInPlace` only when that gate holds (never with opts off).
pub fn set_index_in_place(
    heap: &mut Heap,
    obj: Value,
    idx: Value,
    val: Value,
) -> Result<Value, String> {
    set_index_impl(heap, obj, idx, val, true)
}

fn set_index_impl(
    heap: &mut Heap,
    obj: Value,
    idx: Value,
    val: Value,
    in_place: bool,
) -> Result<Value, String> {
    // A Pending in key/index position is a HARD error, never absorbed: a key
    // that silently vanished would corrupt the container's structure. This is
    // the enumerated map-key hard-error position from the design.
    if let Value::Pending(_) = idx {
        return Err("Cannot use a pending value as a map key / index".into());
    }
    match (obj, idx) {
        (Value::List(list_id), Value::Int(i)) => {
            let len = heap.list_len(list_id);
            // Negative indices count from the end, symmetric with get_index —
            // required so a negative index at a non-leaf level of a nested
            // assignment (`grid[-1][0] = v`) rebuilds the slot it read from.
            let index = if i < 0 { len as i64 + i } else { i };
            if index >= 0 && (index as usize) < len {
                let new_id = if in_place {
                    heap.list_set_in_place(list_id, index as usize, val)
                } else {
                    heap.list_set(list_id, index as usize, val)
                };
                Ok(Value::List(new_id))
            } else {
                Err(format!("Index {} out of bounds (len {})", i, len))
            }
        }
        (Value::F64Array(arr_id), Value::Int(i)) => {
            let v = match val {
                Value::Float(f) => f,
                Value::Int(n) => n as f64,
                other => {
                    return Err(format!(
                        "Cannot assign {} into f64_array",
                        other.type_name()
                    ));
                }
            };
            if i >= 0 && (i as usize) < heap.f64_array_len(arr_id) {
                let new_id = if in_place {
                    heap.f64_array_set_in_place(arr_id, i as usize, v)
                } else {
                    heap.f64_array_set(arr_id, i as usize, v)
                };
                Ok(Value::F64Array(new_id))
            } else {
                Err(format!(
                    "Index {} out of bounds (len {})",
                    i,
                    heap.f64_array_len(arr_id)
                ))
            }
        }
        _ => Err(format!(
            "Cannot index-assign {} with {}",
            obj.type_name(),
            idx.type_name()
        )),
    }
}

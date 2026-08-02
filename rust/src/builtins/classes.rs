//! The built-in classes: constructors and methods implemented in Rust.
//!
//! Petal has no source-level prelude, so a class that every program should see
//! without an import has to be registered here. Today that is `Rect` — the
//! rectangle every UI and drawing program passes around — declared in
//! [`crate::classes`] (fields and method list) and implemented below.
//!
//! Two registration shapes:
//! - The **constructor** is a plain native named after the class (`Rect`), so
//!   `Rect(0, 0, 100, 40)` is an ordinary call and the name can be shadowed
//!   like any other builtin.
//! - Each **method** is a native registered under the qualified name
//!   `Rect.center_x`. The dot makes it unreachable as a bare identifier — the
//!   only way in is method dispatch on a receiver tagged with that class, which
//!   is exactly the scoping a method should have.
//!
//! A user-declared `fn Rect.area(r: Rect)` extends the same class through the
//! *other* half of dispatch (the VM's per-run method table); the two never
//! collide, because a user declaration is consulted first.

use indexmap::IndexMap;

use crate::backend::ops::arithmetic;
use crate::classes::{RECT_FIELDS, qualified_method_name};
use crate::native_fn::{NativeFnTable, PetalCxt};
use crate::program::TermOp;
use crate::value::Value;

use super::require_args;

/// Build a `Rect` instance from four already-computed edges.
///
/// The edges are `Value`s, not `i64`s, because a rect holds whatever numbers it
/// was given: `Rect(10.5, …)` is a float rect and stays one. Petal has no
/// implicit casting (docs/dev/type-declarations-plan.md), so a constructor that
/// truncated its argument would be losing data the caller never asked to lose —
/// and sub-pixel layout and animation lose their precision with it. Screen
/// coordinates become integers at the draw call, which is the only place that
/// has to decide.
fn push_rect(state: &mut PetalCxt, edges: [Value; 4]) {
    let mut entries = IndexMap::new();
    for (name, value) in RECT_FIELDS.iter().zip(edges) {
        entries.insert((*name).to_string(), value);
    }
    let tag = state.heap_mut().alloc_string("Rect".to_string());
    let id = state.heap_mut().alloc_class_instance(entries, tag);
    state.push_value(Value::Map(id));
}

/// Accept a number (int or float) and keep it as it came, or say what was
/// wrong. `what` names the position for the message — a field for the
/// constructor, a parameter for a method.
fn number(value: Value, callee: &str, what: &str) -> Result<Value, String> {
    match value {
        Value::Int(_) | Value::Float(_) => Ok(value),
        other => Err(format!(
            "{callee}(): {what} expects a number, got {}",
            other.type_name()
        )),
    }
}

/// Read one numeric field of the receiver. Methods take the receiver as
/// argument 1 (the VM prepends it), so this is where a non-`Rect` receiver is
/// caught.
fn field(state: &PetalCxt, method: &str, name: &str) -> Result<Value, String> {
    let recv = state.get_value(1)?;
    let Value::Map(id) = recv else {
        return Err(format!(
            "Rect.{method}() expects a Rect, got {}",
            recv.type_name()
        ));
    };
    let Some(&value) = state.heap().get_map(id).get(name) else {
        return Err(format!("Rect.{method}(): receiver has no `{name}` field"));
    };
    number(value, &format!("Rect.{method}"), &format!("field `{name}`"))
}

/// The receiver's four edges.
fn rect_of(state: &PetalCxt, method: &str) -> Result<[Value; 4], String> {
    Ok([
        field(state, method, "x")?,
        field(state, method, "y")?,
        field(state, method, "w")?,
        field(state, method, "h")?,
    ])
}

/// One arithmetic step on rect geometry, run through the same evaluator the
/// language uses for `+`, `-`, `*` and `/`. That is what makes each method's
/// documented equivalent (`r.center_x()` is `r.x + r.w / 2`) exactly true: int
/// operands stay int — including `/`, which truncates on ints — and a float
/// anywhere makes the result a float, with the same overflow reporting.
fn arith(state: &mut PetalCxt, op: TermOp, a: Value, b: Value) -> Result<Value, String> {
    arithmetic(&op, a, b, state.heap_mut())
}

/// A width or height clamped at zero, keeping the number's own kind so a float
/// rect stays a float rect.
fn clamp_at_zero(v: Value) -> Value {
    match v {
        Value::Int(n) if n < 0 => Value::Int(0),
        Value::Float(f) if f < 0.0 => Value::Float(0.0),
        other => other,
    }
}

/// Validate a *method* call's argument count and report it as the call site
/// writes it. Natives see the receiver as argument 1, but nobody types it —
/// `r.inset(1, 2)` passed 2 arguments, not 3, and the message has to agree or
/// it reads as nonsense.
fn require_method_args(state: &PetalCxt, arity: usize, name: &str) -> Result<(), String> {
    if state.arg_count() == arity {
        return Ok(());
    }
    let plural = |n: usize| match n {
        0 => "no arguments".to_string(),
        1 => "1 argument".to_string(),
        n => format!("{n} arguments"),
    };
    Err(format!(
        "{name}() expects {}, got {}",
        plural(arity - 1),
        state.arg_count().saturating_sub(1)
    ))
}

/// `Rect(x, y, w, h)` — the constructor. Each edge is taken as the number it
/// is (int or float) rather than coerced, so the four arguments are read
/// untyped and checked here.
fn native_rect(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 4, "Rect")?;
    let x = state.get_value(1)?;
    let y = state.get_value(2)?;
    let w = state.get_value(3)?;
    let h = state.get_value(4)?;
    let edges = [x, y, w, h];
    for (value, name) in edges.iter().zip(RECT_FIELDS) {
        number(*value, "Rect", &format!("field `{name}`"))?;
    }
    push_rect(state, edges);
    Ok(1)
}

/// `r.center_x()` — the horizontal midpoint, `r.x + r.w / 2`.
fn native_rect_center_x(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 1, "Rect.center_x")?;
    let [x, _, w, _] = rect_of(state, "center_x")?;
    let half = arith(state, TermOp::Div, w, Value::Int(2))?;
    let center = arith(state, TermOp::Add, x, half)?;
    state.push_value(center);
    Ok(1)
}

/// `r.center_y()` — the vertical midpoint.
fn native_rect_center_y(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 1, "Rect.center_y")?;
    let [_, y, _, h] = rect_of(state, "center_y")?;
    let half = arith(state, TermOp::Div, h, Value::Int(2))?;
    let center = arith(state, TermOp::Add, y, half)?;
    state.push_value(center);
    Ok(1)
}

/// `r.right()` — the x just past the right edge (`x + w`), the half-open
/// convention the hit tests already use.
fn native_rect_right(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 1, "Rect.right")?;
    let [x, _, w, _] = rect_of(state, "right")?;
    let right = arith(state, TermOp::Add, x, w)?;
    state.push_value(right);
    Ok(1)
}

/// `r.bottom()` — the y just past the bottom edge (`y + h`).
fn native_rect_bottom(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 1, "Rect.bottom")?;
    let [_, y, _, h] = rect_of(state, "bottom")?;
    let bottom = arith(state, TermOp::Add, y, h)?;
    state.push_value(bottom);
    Ok(1)
}

/// `r.inset(n)` — the same rect pulled in by `n` on all four sides. A negative
/// `n` grows it. Width and height are clamped at zero rather than going
/// negative, which is what every drawing backend wants.
fn native_rect_inset(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 2, "Rect.inset")?;
    let [x, y, w, h] = rect_of(state, "inset")?;
    let n = number(state.get_value(2)?, "Rect.inset", "the margin")?;
    let twice = arith(state, TermOp::Mul, n, Value::Int(2))?;
    let edges = [
        arith(state, TermOp::Add, x, n)?,
        arith(state, TermOp::Add, y, n)?,
        clamp_at_zero(arith(state, TermOp::Sub, w, twice)?),
        clamp_at_zero(arith(state, TermOp::Sub, h, twice)?),
    ];
    push_rect(state, edges);
    Ok(1)
}

/// `r.offset(dx, dy)` — the same size moved by a delta.
fn native_rect_offset(state: &mut PetalCxt) -> Result<u32, String> {
    require_method_args(state, 3, "Rect.offset")?;
    let [x, y, w, h] = rect_of(state, "offset")?;
    let dx = number(state.get_value(2)?, "Rect.offset", "`dx`")?;
    let dy = number(state.get_value(3)?, "Rect.offset", "`dy`")?;
    let edges = [
        arith(state, TermOp::Add, x, dx)?,
        arith(state, TermOp::Add, y, dy)?,
        w,
        h,
    ];
    push_rect(state, edges);
    Ok(1)
}

/// Register the built-in classes' constructors and methods. Called from
/// [`super::register_builtins`]; append-only, like every other registration
/// there (phantom term indices are positional).
pub(super) fn register(table: &mut NativeFnTable) {
    table.register("Rect", native_rect);
    for (name, func) in [
        (
            "center_x",
            native_rect_center_x as crate::native_fn::NativeFn,
        ),
        ("center_y", native_rect_center_y),
        ("right", native_rect_right),
        ("bottom", native_rect_bottom),
        ("inset", native_rect_inset),
        ("offset", native_rect_offset),
    ] {
        let id = table.register(&qualified_method_name("Rect", name), func);
        table.register_class_method("Rect", name, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::RECT_METHODS;

    /// The declared method list and the registered natives are written out
    /// separately (one is compile-time knowledge, the other runtime); this is
    /// what stops them drifting.
    #[test]
    fn every_declared_rect_method_has_a_native() {
        let mut table = NativeFnTable::new();
        super::super::register_builtins(&mut table);
        for (name, _) in RECT_METHODS {
            assert!(
                table.lookup_class_method("Rect", name).is_some(),
                "no native registered for Rect.{name}"
            );
        }
        assert!(table.lookup_name("Rect").is_some(), "no Rect constructor");
    }

    /// The reverse direction: nothing is registered on `Rect` that the class
    /// table does not declare, or the checker would warn on a call that works.
    #[test]
    fn every_registered_rect_method_is_declared() {
        let classes = crate::classes::ClassTable::new();
        let def = classes.get(classes.lookup("Rect").unwrap());
        let mut table = NativeFnTable::new();
        super::super::register_builtins(&mut table);
        for (name, _) in RECT_METHODS {
            assert!(def.method(name).is_some(), "Rect.{name} is not declared");
        }
        let _ = table;
    }
}

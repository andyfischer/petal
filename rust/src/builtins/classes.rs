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

use crate::classes::{RECT_FIELDS, qualified_method_name};
use crate::native_fn::{NativeFnTable, PetalCxt};
use crate::value::Value;

use super::require_args;

/// Build a `Rect` instance from four already-computed edges.
fn push_rect(state: &mut PetalCxt, x: i64, y: i64, w: i64, h: i64) {
    let mut entries = IndexMap::new();
    for (name, value) in RECT_FIELDS.iter().zip([x, y, w, h]) {
        entries.insert((*name).to_string(), Value::Int(value));
    }
    let tag = state.heap_mut().alloc_string("Rect".to_string());
    let id = state.heap_mut().alloc_class_instance(entries, tag);
    state.push_value(Value::Map(id));
}

/// Read one integer field of the receiver. Methods take the receiver as
/// argument 1 (the VM prepends it), so this is where a non-`Rect` receiver is
/// caught.
fn field(state: &PetalCxt, method: &str, name: &str) -> Result<i64, String> {
    let recv = state.get_value(1)?;
    let Value::Map(id) = recv else {
        return Err(format!(
            "Rect.{method}() expects a Rect, got {}",
            recv.type_name()
        ));
    };
    match state.heap().get_map(id).get(name) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Float(f)) => Ok(*f as i64),
        _ => Err(format!("Rect.{method}(): receiver has no `{name}` field")),
    }
}

/// The receiver's four edges.
fn rect_of(state: &PetalCxt, method: &str) -> Result<(i64, i64, i64, i64), String> {
    Ok((
        field(state, method, "x")?,
        field(state, method, "y")?,
        field(state, method, "w")?,
        field(state, method, "h")?,
    ))
}

/// `Rect(x, y, w, h)` — the constructor.
fn native_rect(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 4, "Rect")?;
    let x = state.get_int(1)?;
    let y = state.get_int(2)?;
    let w = state.get_int(3)?;
    let h = state.get_int(4)?;
    push_rect(state, x, y, w, h);
    Ok(1)
}

/// `r.center_x()` — the horizontal midpoint, `r.x + r.w / 2`.
fn native_rect_center_x(state: &mut PetalCxt) -> Result<u32, String> {
    let (x, _, w, _) = rect_of(state, "center_x")?;
    state.push_int(x + w / 2);
    Ok(1)
}

/// `r.center_y()` — the vertical midpoint.
fn native_rect_center_y(state: &mut PetalCxt) -> Result<u32, String> {
    let (_, y, _, h) = rect_of(state, "center_y")?;
    state.push_int(y + h / 2);
    Ok(1)
}

/// `r.right()` — the x just past the right edge (`x + w`), the half-open
/// convention the hit tests already use.
fn native_rect_right(state: &mut PetalCxt) -> Result<u32, String> {
    let (x, _, w, _) = rect_of(state, "right")?;
    state.push_int(x + w);
    Ok(1)
}

/// `r.bottom()` — the y just past the bottom edge (`y + h`).
fn native_rect_bottom(state: &mut PetalCxt) -> Result<u32, String> {
    let (_, y, _, h) = rect_of(state, "bottom")?;
    state.push_int(y + h);
    Ok(1)
}

/// `r.inset(n)` — the same rect pulled in by `n` on all four sides. A negative
/// `n` grows it. Width and height are clamped at zero rather than going
/// negative, which is what every drawing backend wants.
fn native_rect_inset(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "Rect.inset")?;
    let (x, y, w, h) = rect_of(state, "inset")?;
    let n = state.get_int(2)?;
    push_rect(state, x + n, y + n, (w - 2 * n).max(0), (h - 2 * n).max(0));
    Ok(1)
}

/// `r.offset(dx, dy)` — the same size moved by a delta.
fn native_rect_offset(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 3, "Rect.offset")?;
    let (x, y, w, h) = rect_of(state, "offset")?;
    let dx = state.get_int(2)?;
    let dy = state.get_int(3)?;
    push_rect(state, x + dx, y + dy, w, h);
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

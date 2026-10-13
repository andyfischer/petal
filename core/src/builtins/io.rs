//! I/O and type-name builtins: print, str, type, assert, assert_eq, and the
//! recorded expectations expect, expect_eq, expect_near.

use crate::native_fn::PetalCxt;
use crate::value;
use crate::value::Value;

pub(super) fn native_print(state: &mut PetalCxt) -> Result<u32, String> {
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

pub(super) fn native_str(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "str")?;
    let v = state.get_value(1)?;
    let s = value::value_to_display_string(&v, state.heap());
    state.push_string(s);
    Ok(1)
}

pub(super) fn native_type(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 1, "type")?;
    let v = state.get_value(1)?;
    let name = type_name_id(state.heap_mut(), v);
    state.push_value(Value::String(name));
    Ok(1)
}

/// What `type(v)` answers, as an interned string. Shared with the VM, which
/// answers `type` without a native call (`Inst::Intrinsic`).
pub(crate) fn type_name_id(heap: &mut crate::heap::Heap, v: Value) -> crate::heap::StringId {
    // A class instance reports its class rather than the underlying `record`:
    // the class name is the more specific true answer, and it is what a
    // `type(x) == "Rect"` test wants. Untagged records still say "record".
    match v {
        Value::Map(id) => match heap.map_class(id) {
            Some(class) => class,
            None => heap.intern_str(v.type_name()),
        },
        // A variant reports its enum, as a class instance reports its class.
        Value::EnumVariant { tag, .. } => {
            match crate::program::split_enum_tag(heap.get_string(tag)).0 {
                Some(name) => {
                    let name = name.to_string();
                    heap.alloc_string(name)
                }
                None => heap.intern_str(v.type_name()),
            }
        }
        _ => heap.intern_str(v.type_name()),
    }
}

pub(super) fn native_assert(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if n != 1 && n != 2 {
        return Err("assert() expects 1 or 2 arguments".into());
    }
    let cond = state.get_value(1)?;
    let ok = match cond {
        crate::value::Value::Bool(b) => b,
        crate::value::Value::Nil => false,
        _ => true,
    };
    if !ok {
        return Err(if n == 2 {
            let m = state.get_value(2)?;
            format!(
                "assertion failed: {}",
                value::value_to_display_string(&m, state.heap())
            )
        } else {
            "assertion failed".to_string()
        });
    }
    state.push_nil();
    Ok(1)
}

/// The optional trailing message argument of an expectation, rendered.
fn expect_note(state: &PetalCxt, index: usize) -> Result<Option<String>, String> {
    if state.arg_count() < index {
        return Ok(None);
    }
    let v = state.get_value(index)?;
    Ok(match v {
        Value::Nil => None,
        _ => Some(value::value_to_display_string(&v, state.heap())),
    })
}

/// A difference to about six significant digits: `0.1`, not
/// `0.09999999999999998`.
fn short_float(d: f64) -> String {
    if d == 0.0 || !d.is_finite() {
        return format!("{d}");
    }
    let magnitude = d.abs().log10().floor() as i32;
    if !(-5..=15).contains(&magnitude) {
        return format!("{d:.5e}");
    }
    let decimals = (5 - magnitude).max(0) as usize;
    let text = format!("{d:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

fn with_note(message: String, note: &Option<String>) -> String {
    match note {
        Some(note) => format!("{message} ({note})"),
        None => message,
    }
}

/// `expect(condition, message?)` — record a failure when `condition` is
/// false or nil, and carry on. Answers whether it held.
pub(super) fn native_expect(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if n != 1 && n != 2 {
        return Err(format!("expect() expects 1 or 2 arguments, got {n}"));
    }
    let ok = !matches!(state.get_value(1)?, Value::Bool(false) | Value::Nil);
    if ok {
        state.pass_expectation();
    } else {
        let note = expect_note(state, 2)?;
        let message = match &note {
            Some(note) => note.clone(),
            None => "expected a true value".to_string(),
        };
        let actual = value::value_to_debug_string(&state.get_value(1)?, state.heap());
        state.fail_expectation_with("expect", message, Some(actual), None, None, note);
    }
    state.push_bool(ok);
    Ok(1)
}

/// `expect_eq(actual, expected, message?)` — record a failure when the two
/// are not `==`, with both values, and carry on.
pub(super) fn native_expect_eq(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if n != 2 && n != 3 {
        return Err(format!("expect_eq() expects 2 or 3 arguments, got {n}"));
    }
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    let ok = value::values_equal(&a, &b, state.heap());
    if ok {
        state.pass_expectation();
    } else {
        let note = expect_note(state, 3)?;
        let actual = value::value_to_debug_string(&a, state.heap());
        let expected = value::value_to_debug_string(&b, state.heap());
        let message = with_note(format!("expected {expected}, got {actual}"), &note);
        state.fail_expectation_with(
            "expect_eq",
            message,
            Some(actual),
            Some(expected),
            None,
            note,
        );
    }
    state.push_bool(ok);
    Ok(1)
}

/// `expect_near(actual, expected, tolerance, message?)` — record a failure
/// when two numbers differ by more than `tolerance`, and carry on. A value
/// that is not a number, and a NaN, never is near anything.
pub(super) fn native_expect_near(state: &mut PetalCxt) -> Result<u32, String> {
    let n = state.arg_count();
    if n != 3 && n != 4 {
        return Err(format!("expect_near() expects 3 or 4 arguments, got {n}"));
    }
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    let eps = state.get_value(3)?;
    let number = |v: &Value| match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    let Some(eps_f) = number(&eps).filter(|e| *e >= 0.0) else {
        return Err(format!(
            "expect_near() expects a tolerance of zero or more as its third argument, got {}",
            value::value_to_debug_string(&eps, state.heap())
        ));
    };
    let diff = match (number(&a), number(&b)) {
        (Some(x), Some(y)) => Some((x - y).abs()),
        _ => None,
    };
    // An exact match is near at any tolerance, infinities included.
    let ok = match diff {
        Some(d) => d <= eps_f || number(&a) == number(&b),
        None => false,
    };
    if ok {
        state.pass_expectation();
    } else {
        let note = expect_note(state, 4)?;
        let actual = value::value_to_debug_string(&a, state.heap());
        let expected = value::value_to_debug_string(&b, state.heap());
        let tolerance = value::value_to_debug_string(&eps, state.heap());
        let by = match diff {
            Some(d) if !d.is_nan() => format!(" (off by {})", short_float(d)),
            Some(_) => String::new(),
            None => " (not a number)".to_string(),
        };
        let message = with_note(
            format!("expected {expected} within {tolerance}, got {actual}{by}"),
            &note,
        );
        state.fail_expectation_with(
            "expect_near",
            message,
            Some(actual),
            Some(expected),
            Some(tolerance),
            note,
        );
    }
    state.push_bool(ok);
    Ok(1)
}

pub(super) fn native_assert_eq(state: &mut PetalCxt) -> Result<u32, String> {
    super::require_args(state, 2, "assert_eq")?;
    let a = state.get_value(1)?;
    let b = state.get_value(2)?;
    if !value::values_equal(&a, &b, state.heap()) {
        let a_str = value::value_to_display_string(&a, state.heap());
        let b_str = value::value_to_display_string(&b, state.heap());
        return Err(format!(
            "assertion failed: assert_eq: left={} right={}",
            a_str, b_str
        ));
    }
    state.push_nil();
    Ok(1)
}

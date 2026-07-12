//! Automatic differentiation (dual numbers): dual, value_of, deriv_of.

use crate::native_fn::PetalCxt;
use crate::value::Value;

use super::require_args;

pub(super) fn native_dual(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 2, "dual")?;
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

pub(super) fn native_value_of(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "value_of")?;
    match state.get_value(1)? {
        Value::Dual { value, .. } => {
            state.push_float(value);
            Ok(1)
        }
        Value::Int(n) => {
            state.push_float(n as f64);
            Ok(1)
        }
        Value::Float(f) => {
            state.push_float(f);
            Ok(1)
        }
        _ => Err("value_of() expects a number or dual".into()),
    }
}

pub(super) fn native_deriv_of(state: &mut PetalCxt) -> Result<u32, String> {
    require_args(state, 1, "deriv_of")?;
    match state.get_value(1)? {
        Value::Dual { derivative, .. } => {
            state.push_float(derivative);
            Ok(1)
        }
        Value::Int(_) | Value::Float(_) => {
            state.push_float(0.0);
            Ok(1)
        }
        _ => Err("deriv_of() expects a number or dual".into()),
    }
}

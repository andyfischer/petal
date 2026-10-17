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

// ── Derivative rules for the multi-argument builtins ───────────────────────
//
// A builtin that can be handed a dual either carries the derivative through
// (the rules below, and `unary_float_dual` in `math.rs`), hands back one of
// its arguments unchanged (`min`, `max`), or refuses it: `PetalCxt::get_float`
// errors on a dual, so a builtin with no rule never drops a derivative
// silently.

/// A number as a (value, derivative) pair: the arithmetic the rules are
/// written in. A plain number has derivative `0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct D {
    pub v: f64,
    pub d: f64,
}

impl D {
    pub fn konst(v: f64) -> D {
        D { v, d: 0.0 }
    }
    pub fn add(self, o: D) -> D {
        D { v: self.v + o.v, d: self.d + o.d }
    }
    pub fn sub(self, o: D) -> D {
        D { v: self.v - o.v, d: self.d - o.d }
    }
    pub fn mul(self, o: D) -> D {
        D { v: self.v * o.v, d: self.d * o.v + self.v * o.d }
    }
    pub fn div(self, o: D) -> D {
        D { v: self.v / o.v, d: (self.d * o.v - self.v * o.d) / (o.v * o.v) }
    }
    /// `f(self)` with chain-rule multiplier `df` (the derivative of `f` at
    /// `self.v`).
    pub fn chain(self, v: f64, df: f64) -> D {
        D { v, d: df * self.d }
    }
    pub fn sqrt(self) -> D {
        let s = self.v.sqrt();
        self.chain(s, if s == 0.0 { 0.0 } else { 0.5 / s })
    }
}

/// Argument `index` as a [`D`] when it is a number or a dual, and whether it
/// was a dual. Anything else is the error `get_float` gives for a non-number.
pub(super) fn arg_d(state: &PetalCxt, index: usize) -> Result<(D, bool), String> {
    match state.get_value(index)? {
        Value::Dual { value, derivative } => Ok((D { v: value, d: derivative }, true)),
        Value::Int(n) => Ok((D::konst(n as f64), false)),
        Value::Float(f) => Ok((D::konst(f), false)),
        other => Err(format!(
            "Expected float at arg {index}, got {}",
            other.type_name()
        )),
    }
}

/// Arguments `1..=N` as [`D`]s, and whether any of them was a dual.
pub(super) fn args_d<const N: usize>(state: &PetalCxt) -> Result<([D; N], bool), String> {
    let mut out = [D::konst(0.0); N];
    let mut any = false;
    for (i, slot) in out.iter_mut().enumerate() {
        let (d, is_dual) = arg_d(state, i + 1)?;
        *slot = d;
        any |= is_dual;
    }
    Ok((out, any))
}

/// Push `r`: as a dual when any argument was one, else as its plain float.
pub(super) fn push_d(state: &mut PetalCxt, r: D, as_dual: bool) {
    if as_dual {
        state.push_value(Value::Dual { value: r.v, derivative: r.d });
    } else {
        state.push_float(r.v);
    }
}

//! Parameter names for the petal-ui natives, so a script can pass their
//! arguments by name: `clip(x: 0, y: 0, w: 320, h: 200)`,
//! `mouse_pressed(button: 0)`.
//!
//! The names themselves are `petal::typecheck::globals::PETAL_UI_NATIVE_PARAMS`.
//! They live in the core crate because `petal check` must answer for these
//! natives without ever registering them; this module is what makes that table
//! the one this host actually binds against. [`register`] is how every native
//! in this crate is registered, and it declares the native's parameters from
//! the table in the same step, so the two cannot drift apart.

use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeFn, NativeFnId};
use petal::typecheck::globals::PETAL_UI_NATIVE_PARAMS;

/// `Env::register_native`, plus the native's declared parameters when the
/// table lists it. A native the table leaves out is registered all the same,
/// and takes its arguments by position only.
pub(crate) fn register(
    env: &mut Env,
    name: &str,
    func: NativeFn,
    effects: NativeEffects,
) -> NativeFnId {
    let id = env.register_native(name, func, effects);
    if let Some((_, specs)) = PETAL_UI_NATIVE_PARAMS.iter().find(|(n, _)| *n == name) {
        for spec in *specs {
            env.declare_native_params(id, spec)
                .unwrap_or_else(|e| panic!("PETAL_UI_NATIVE_PARAMS[{name}]: {e}"));
        }
    }
    id
}

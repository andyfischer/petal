//! Parameter names for the petal-ui natives, so a script can pass their
//! arguments by name: `clip(x: 0, y: 0, w: 320, h: 200)`,
//! `mouse_pressed(button: 0)`.
//!
//! The names are written once, in the `native fn` declarations of the `ui`
//! module (`prelude/ui.ptl`) — the interface `petal check` reads without ever
//! registering these natives. This module is what makes those declarations
//! the ones this host actually binds against: [`register`] is how every
//! native in this crate is registered, and it gives the native the parameter
//! lists its declaration has in the same step, so the two cannot drift apart.

use std::collections::HashMap;
use std::sync::OnceLock;

use petal::env::Env;
use petal::native_decl::NativeDecls;
use petal::native_fn::{NativeEffects, NativeFn, NativeFnId, NativeSignature};

/// The `native fn` declarations of the `ui` module.
pub fn interface() -> &'static NativeDecls {
    static DECLS: OnceLock<NativeDecls> = OnceLock::new();
    DECLS.get_or_init(|| {
        NativeDecls::parse(crate::prelude_source())
            .unwrap_or_else(|e| panic!("prelude/ui.ptl: {e}"))
    })
}

/// Host native → the parameter lists `ui.ptl` declares for it.
fn declared() -> &'static HashMap<String, Vec<NativeSignature>> {
    static BY_HOST: OnceLock<HashMap<String, Vec<NativeSignature>>> = OnceLock::new();
    BY_HOST.get_or_init(|| interface().signatures_by_host())
}

/// `Env::register_native`, plus the native's declared parameters when the
/// `ui` module declares it. A native the module leaves out is registered all
/// the same, and takes its arguments by position only.
pub(crate) fn register(
    env: &mut Env,
    name: &str,
    func: NativeFn,
    effects: NativeEffects,
) -> NativeFnId {
    let id = env.register_native(name, func, effects);
    if let Some(signatures) = declared().get(name) {
        env.set_native_signatures(id, signatures.clone());
    }
    id
}

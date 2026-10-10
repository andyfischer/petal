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

/// The `native fn` declarations of the `ui` package (`ui` and `ui/input`).
pub fn interface() -> &'static NativeDecls {
    static DECLS: OnceLock<NativeDecls> = OnceLock::new();
    DECLS.get_or_init(|| {
        let mut decls = NativeDecls::parse(crate::input_source())
            .unwrap_or_else(|e| panic!("prelude/input.ptl: {e}"));
        decls.absorb(
            &NativeDecls::parse(crate::prelude_source())
                .unwrap_or_else(|e| panic!("prelude/ui.ptl: {e}")),
        );
        decls
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

/// A declaration of every petal-ui native under its own host name, one per
/// declared form, all on one line (so a script appended after `; ` keeps its
/// own line numbers): `native fn clip(x, y, w, h); native fn …`.
///
/// For tests that call the raw natives. A script reaches a native only
/// through a declaration, and `import ui` binds the prelude's overloads
/// over most of the draw names — this is how to get at what is underneath.
pub fn raw_declarations() -> String {
    let mut by_host: Vec<_> = declared().iter().collect();
    by_host.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::new();
    for (name, sigs) in by_host {
        for sig in sigs {
            let params: Vec<String> = sig
                .params()
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if i < sig.required() {
                        p.to_string()
                    } else {
                        format!("{p}?")
                    }
                })
                .collect();
            out.push(format!("native fn {name}({})", params.join(", ")));
        }
    }
    out.join("; ")
}

/// `source` made loadable in `env` by the shortest route: behind `import ui`
/// when `env` has the `ui` package registered, else behind
/// [`raw_declarations`]. Either prefix shares the script's first line, so
/// line numbers in errors are the script's own.
pub fn script_for(env: &Env, source: &str) -> String {
    if env.packages().iter().any(|p| p.name == crate::MODULE_NAME) {
        format!("import ui; {source}")
    } else {
        format!("{}; {source}", raw_declarations())
    }
}

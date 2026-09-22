//! Build script: locate the petal-ui prelude, when this crate is built inside
//! the Petal monorepo, so `petal check` can resolve the `ui` module a panel
//! script imports implicitly (see `typecheck::globals::ui_prelude_source`).
//! Built anywhere else (a vendored copy without `petal-ui/`), the cfg is left
//! off and `check` falls back to accepting the prelude's names unresolved.

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(petal_ui_prelude)");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let prelude = Path::new(&manifest).join("../petal-ui/prelude/ui.ptl");
    if let Ok(path) = prelude.canonicalize() {
        println!("cargo::rerun-if-changed={}", path.display());
        println!("cargo::rustc-cfg=petal_ui_prelude");
        println!("cargo::rustc-env=PETAL_UI_PRELUDE_PATH={}", path.display());
    } else {
        println!("cargo::rerun-if-changed=build.rs");
    }
}

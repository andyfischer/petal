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
    // The packages Garden registers for its panels (bloom, text_layout) live
    // here; `petal check --host garden` puts it on the module path so a panel
    // script's `import text_layout` resolves the way it does inside Garden.
    println!("cargo::rustc-check-cfg=cfg(petal_libs_dir)");
    let libs = Path::new(&manifest).join("../petal-libs");
    if let Ok(path) = libs.canonicalize() {
        println!("cargo::rustc-cfg=petal_libs_dir");
        println!("cargo::rustc-env=PETAL_LIBS_DIR={}", path.display());
    }
}

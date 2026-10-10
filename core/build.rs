//! Build script: locate the petal-ui prelude, when this crate is built inside
//! the Petal monorepo, so `petal check` can resolve the `ui` module a panel
//! script imports implicitly (see `typecheck::globals::ui_prelude_source`).
//! Built anywhere else (a vendored copy without `core-libs/petal-ui/`), the cfg is left
//! off and `check` falls back to accepting the prelude's names unresolved.

use std::path::Path;

/// The packages compiled into the petal binary: `(name, directory relative to
/// this crate)`. Each is a `petal.toml` package of Petal source and
/// `native fn` declarations; `petal::bundled` serves them. A directory that
/// is missing fails the build — a binary that silently lacks `core` would
/// compile every script against an empty set of builtins.
const BUNDLED_PACKAGES: &[(&str, &str)] = &[("core", "packages/core")];

/// Every file under `dir` a package is made of (`petal.toml`, `*.ptl`),
/// as `/`-joined paths relative to `dir`, sorted.
fn package_files(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading bundled package directory {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if path.is_dir() {
            package_files(&path, &rel_path, out);
        } else if name == "petal.toml" || name.ends_with(".ptl") {
            out.push(rel_path);
        }
    }
}

/// Write `$OUT_DIR/bundled_packages.rs`: the table `petal::bundled` includes.
fn embed_bundled_packages(manifest: &str) {
    let mut code = String::from("&[\n");
    for (name, rel) in BUNDLED_PACKAGES {
        let dir = Path::new(manifest).join(rel);
        let dir = dir.canonicalize().unwrap_or_else(|e| {
            panic!(
                "bundled package `{name}`: its source directory {} is missing ({e}). \
                 The petal binary embeds this package; restore the directory.",
                dir.display()
            )
        });
        if !dir.join("petal.toml").is_file() {
            panic!(
                "bundled package `{name}`: {} has no petal.toml",
                dir.display()
            );
        }
        println!("cargo::rerun-if-changed={}", dir.display());
        let mut files = Vec::new();
        package_files(&dir, "", &mut files);
        if !files.iter().any(|f| f.ends_with(".ptl")) {
            panic!(
                "bundled package `{name}`: {} holds no .ptl modules",
                dir.display()
            );
        }
        code.push_str(&format!("    BundledPackage {{ name: {name:?}, files: &[\n"));
        for file in files {
            let abs = dir.join(&file);
            println!("cargo::rerun-if-changed={}", abs.display());
            code.push_str(&format!(
                "        ({file:?}, include_str!({:?})),\n",
                abs.display().to_string()
            ));
        }
        code.push_str("    ] },\n");
    }
    code.push_str("]\n");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(Path::new(&out).join("bundled_packages.rs"), code)
        .expect("writing bundled_packages.rs");
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(petal_ui_prelude)");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    embed_bundled_packages(&manifest);
    let prelude = Path::new(&manifest).join("../core-libs/petal-ui/prelude/ui.ptl");
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
    let libs = Path::new(&manifest).join("../core-runtime");
    if let Ok(path) = libs.canonicalize() {
        println!("cargo::rustc-cfg=petal_libs_dir");
        println!("cargo::rustc-env=PETAL_LIBS_DIR={}", path.display());
    }
}

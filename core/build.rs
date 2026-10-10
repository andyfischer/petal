//! Build script: embed the bundled packages (`petal::bundled`) in the binary.
//!
//! A bundled package is Petal source and `native fn` declarations only. Each
//! one is read from its home in the repository; a missing directory fails the
//! build rather than producing a binary that silently lacks a package.

use std::path::Path;

/// The packages compiled into the petal binary: `(name, directory relative to
/// this crate)`. Each is a `petal.toml` package of Petal source and
/// `native fn` declarations; `petal::bundled` serves them. A directory that
/// is missing fails the build — a binary that silently lacks `core` would
/// compile every script against an empty set of builtins.
const BUNDLED_PACKAGES: &[(&str, &str)] = &[
    ("core", "packages/core"),
    // The domain builtins: natives every Env registers, in scope only where
    // imported (`petal::bundled::BUILTIN_PACKAGES`).
    ("noise", "packages/noise"),
    ("color", "packages/color"),
    ("vec2", "packages/vec2"),
    ("autodiff", "packages/autodiff"),
    ("json", "packages/json"),
    ("io", "packages/io"),
    ("creative", "packages/creative"),
    ("ui", "../core-libs/petal-ui/prelude"),
    ("bloom", "../core-runtime/bloom"),
    ("text_layout", "../core-runtime/text-layout"),
    ("glsl", "../core-runtime/glsl"),
];

/// Directories of a package that are not part of what it exports: its tests
/// and examples are loose scripts beside the modules.
const SKIPPED_DIRS: &[&str] = &["tests", "examples", "docs", "target"];

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
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }
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
    println!("cargo::rerun-if-changed=build.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    embed_bundled_packages(&manifest);
}

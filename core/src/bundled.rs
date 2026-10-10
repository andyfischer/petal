//! The packages compiled into the petal binary.
//!
//! A bundled package is an ordinary `petal.toml` package — Petal source and
//! `native fn` declarations — whose files `core/build.rs` embeds, so they are
//! there with no filesystem (wasm) and wherever the binary is copied. The
//! build fails if a bundled package's directory is missing.
//!
//! `core` holds the declarations of the builtins every program can call
//! without an import (`core/packages/core`); they are bound in every file's
//! global scope, see [`crate::native_decl::core_decls`]. The domain builtins
//! ([`BUILTIN_PACKAGES`]: `noise`, `color`, …) are registered in every Env
//! too but are in scope only where imported. The rest (`ui`, `bloom`,
//! `text_layout`, `glsl`) are first-party libraries.

/// One embedded package: its name and every file it is made of.
#[derive(Debug)]
pub struct BundledPackage {
    /// The `[package] name`.
    pub name: &'static str,
    /// `(path relative to the package root, contents)`, sorted by path:
    /// `petal.toml` and each `*.ptl` under it.
    pub files: &'static [(&'static str, &'static str)],
}

impl BundledPackage {
    /// The package's `petal.toml`.
    pub fn manifest(&self) -> &'static str {
        self.files
            .iter()
            .find(|(path, _)| *path == crate::package::MANIFEST_FILE)
            .map(|(_, text)| *text)
            .unwrap_or("")
    }

    /// The package's Petal sources: `(path relative to the package root,
    /// source)` for each `*.ptl`, sorted by path.
    pub fn sources(&self) -> impl Iterator<Item = (&'static str, &'static str)> {
        self.files
            .iter()
            .copied()
            .filter(|(path, _)| path.ends_with(".ptl"))
    }
}

/// Every bundled package, in the order `core/build.rs` lists them.
pub static BUNDLED_PACKAGES: &[BundledPackage] =
    include!(concat!(env!("OUT_DIR"), "/bundled_packages.rs"));

/// The bundled package called `name`.
pub fn package(name: &str) -> Option<&'static BundledPackage> {
    BUNDLED_PACKAGES.iter().find(|p| p.name == name)
}

/// The name of the package that declares the always-in-scope builtins.
pub const CORE_PACKAGE: &str = "core";

/// The bundled packages that declare the *domain* builtins: natives the
/// runtime registers in every Env ([`crate::builtins::register_builtins`]),
/// but which are in scope only in a file that imports the package
/// (`import noise`). One per group of `core/src/builtins/`; `io` is the
/// host-I/O set of `output.rs`, `creative` the creative-coding set.
pub const BUILTIN_PACKAGES: &[&str] = &[
    "noise", "color", "vec2", "autodiff", "json", "io", "creative",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_bundled_with_its_manifest_and_modules() {
        let core = package(CORE_PACKAGE).expect("core is bundled");
        assert!(core.manifest().contains("name = \"core\""));
        let modules: Vec<&str> = core.sources().map(|(path, _)| path).collect();
        assert!(modules.contains(&"src/math.ptl"), "{modules:?}");
        assert!(modules.contains(&"src/collections.ptl"), "{modules:?}");
        assert!(package("nope").is_none());
    }
}

//! The `text_layout` placement library, embedded.
//!
//! text_layout (`petal-libs/text-layout/`) turns the host's font metrics into
//! the `y` a run should be drawn at: cap-height centring, wrapping, elision,
//! caret hit-testing. Like [`crate::bloom`] it is *source*, and Garden makes it
//! available the same way and for the same reason — a panel-mode GPP drawer
//! arrives as pushed source with no directory of its own, so an in-memory
//! module is the only kind every panel can reach.
//!
//! It is registered unconditionally rather than on demand because **bloom
//! imports it**: every bloom label goes through `draw_text_line`, so a Garden
//! that registered bloom alone would fail to load it.
//!
//! The directory is `text-layout` and the package is `text_layout`: a package
//! name is the first segment of an import path, so it has to be spellable as
//! an identifier.

use petal::env::Env;

/// The package name, which is also the module a bare `import text_layout`
/// finds.
pub const PACKAGE: &str = "text_layout";

/// Every module under its *package-relative* name. `text_layout` itself is the
/// facade, `src/text_layout.ptl`.
///
/// `include_str!` means cargo rebuilds Garden when a `.ptl` here changes, so a
/// library edit cannot go stale in a built binary.
pub const MODULES: &[(&str, &str)] = &[
    (
        "place",
        include_str!("../../../petal-libs/text-layout/src/place.ptl"),
    ),
    (
        "fit",
        include_str!("../../../petal-libs/text-layout/src/fit.ptl"),
    ),
    (
        "block",
        include_str!("../../../petal-libs/text-layout/src/block.ptl"),
    ),
    (
        "text_layout",
        include_str!("../../../petal-libs/text-layout/src/text_layout.ptl"),
    ),
];

/// Make `import text_layout` and `import text_layout/fit` work in this env.
///
/// The only way this can fail is a package name that is not an identifier, and
/// the name is a constant here, so the result is unwrapped rather than pushed
/// onto every caller.
pub fn register(env: &mut Env) {
    env.register_package(PACKAGE, MODULES.iter().copied())
        .expect("`text_layout` is a valid package name");
}

/// The importable module names, for a host that wants to report what a panel
/// may import: `text_layout`, `text_layout/fit`, …
pub fn module_names() -> Vec<String> {
    MODULES
        .iter()
        .map(|(name, _)| {
            if *name == PACKAGE {
                PACKAGE.to_string()
            } else {
                format!("{PACKAGE}/{name}")
            }
        })
        .collect()
}

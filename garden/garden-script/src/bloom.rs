//! The `bloom` component library, embedded.
//!
//! bloom (`petal-libs/bloom/`) is a component library written entirely in
//! Petal — buttons, menus, controls, overlays, and the animation core under
//! them. It is *source*, so a host makes it available the way it makes any
//! Petal module available: by registering it.
//!
//! Garden registers it in memory rather than putting its directory on the
//! module path, because a panel's source does not always come from a file it
//! sits beside. A panel-mode GPP app pushes its drawer over a socket
//! ([`PanelHost::from_source`](crate::panel::PanelHost::from_source)), and a
//! module path would not help that script find anything. An in-memory module
//! is reachable from every panel however its source arrived.
//!
//! It is registered as a *package* — one [`Env::register_package`] call whose
//! name (`bloom`) is the one in `petal-libs/bloom/petal.toml`. That is what
//! makes `import bloom/menu` resolve, and what makes `import bloom` find the
//! facade module named like its package. Before packages this was nine
//! `register_module` calls whose flat names the library had to prefix by hand.
//!
//! The package is registered but *not* implicitly imported: a script says
//! `import bloom` (or `import bloom: button, dropdown`) and pays nothing
//! otherwise. That is deliberate — `ui` is the host's own surface and belongs
//! in every script's scope; bloom is a library, and a library that silently
//! occupies a hundred names would collide with the panels that already define
//! `button` or `switch` of their own.

use petal::env::Env;

/// The package name, which is also the module a bare `import bloom` finds.
pub const PACKAGE: &str = "bloom";

/// Every bloom module under its *package-relative* name, in dependency order
/// (the order is documentation only — `import` resolves by name, not by
/// registration order). `bloom` itself is the facade, `src/bloom.ptl`.
///
/// `include_str!` means cargo rebuilds Garden when a `.ptl` here changes, so a
/// library edit cannot go stale in a built binary.
pub const MODULES: &[(&str, &str)] = &[
    (
        "motion",
        include_str!("../../../petal-libs/bloom/src/motion.ptl"),
    ),
    (
        "theme",
        include_str!("../../../petal-libs/bloom/src/theme.ptl"),
    ),
    ("icon", include_str!("../../../petal-libs/bloom/src/icon.ptl")),
    (
        "interact",
        include_str!("../../../petal-libs/bloom/src/interact.ptl"),
    ),
    (
        "button",
        include_str!("../../../petal-libs/bloom/src/button.ptl"),
    ),
    (
        "controls",
        include_str!("../../../petal-libs/bloom/src/controls.ptl"),
    ),
    ("menu", include_str!("../../../petal-libs/bloom/src/menu.ptl")),
    (
        "overlay",
        include_str!("../../../petal-libs/bloom/src/overlay.ptl"),
    ),
    (
        "bloom",
        include_str!("../../../petal-libs/bloom/src/bloom.ptl"),
    ),
];

/// Make `import bloom` and `import bloom/menu` work in this env.
///
/// The only way this can fail is a package name that is not an identifier, and
/// the name is a constant here, so the result is unwrapped rather than pushed
/// onto every caller.
pub fn register(env: &mut Env) {
    env.register_package(PACKAGE, MODULES.iter().copied())
        .expect("`bloom` is a valid package name");
}

/// The importable module names, for a host that wants to report what a panel
/// may import: `bloom`, `bloom/menu`, …
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

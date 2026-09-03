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
//! The modules are registered but *not* implicitly imported: a script says
//! `import bloom` (or `import bloom: button, dropdown`) and pays nothing
//! otherwise. That is deliberate — `ui` is the host's own surface and belongs
//! in every script's scope; bloom is a library, and a library that silently
//! occupies a hundred names would collide with the panels that already define
//! `button` or `switch` of their own.

use petal::env::Env;

/// Every bloom module, in dependency order (the order is documentation only —
/// `import` resolves by name, not by registration order).
///
/// `include_str!` means cargo rebuilds Garden when a `.ptl` here changes, so a
/// library edit cannot go stale in a built binary.
pub const MODULES: &[(&str, &str)] = &[
    (
        "bloom_motion",
        include_str!("../../../petal-libs/bloom/src/bloom_motion.ptl"),
    ),
    (
        "bloom_theme",
        include_str!("../../../petal-libs/bloom/src/bloom_theme.ptl"),
    ),
    (
        "bloom_icon",
        include_str!("../../../petal-libs/bloom/src/bloom_icon.ptl"),
    ),
    (
        "bloom_interact",
        include_str!("../../../petal-libs/bloom/src/bloom_interact.ptl"),
    ),
    (
        "bloom_button",
        include_str!("../../../petal-libs/bloom/src/bloom_button.ptl"),
    ),
    (
        "bloom_controls",
        include_str!("../../../petal-libs/bloom/src/bloom_controls.ptl"),
    ),
    (
        "bloom_menu",
        include_str!("../../../petal-libs/bloom/src/bloom_menu.ptl"),
    ),
    (
        "bloom_overlay",
        include_str!("../../../petal-libs/bloom/src/bloom_overlay.ptl"),
    ),
    (
        "bloom",
        include_str!("../../../petal-libs/bloom/src/bloom.ptl"),
    ),
];

/// Make `import bloom` work in this env.
pub fn register(env: &mut Env) {
    for (name, source) in MODULES {
        env.register_module(name, source);
    }
}

/// The module names, for a host that wants to report what a panel may import.
pub fn module_names() -> Vec<&'static str> {
    MODULES.iter().map(|(name, _)| *name).collect()
}

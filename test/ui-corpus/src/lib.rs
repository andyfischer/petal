//! This package is its tests (`tests/`). See the `test/ui-corpus/` section of
//! `test/README.md`.
//!
//! They test core features (`petal::static_cache`, the frame gate in
//! `petal::run_deps`, `petal::memo`, `petal::effect_audit`, `petal::dm`)
//! over the panel corpus, driven through `petal_ui_headless::harness::Headless`
//! because a panel needs a UI host to run. Tests of `petal-ui` itself stay in
//! `core-libs/petal-ui/tests/`.
//!
//! The one thing here is [`panel`], how every test loads an app: much of the
//! corpus is Garden panels (`import garden`, often through an example's
//! `layout.ptl`), so each runs with Garden's panel stubs registered.

use std::path::{Path, PathBuf};

use petal_ui_headless::harness::Headless;

/// Load `path` at `width`×`height` with extra module directories
/// `module_paths`, with Garden's panel stubs
/// (`garden_script::headless::register_panel_stubs`) registered so a panel
/// that does `import garden` loads. The stubs are inert for a script that
/// never calls them.
pub fn panel(
    path: &Path,
    width: i32,
    height: i32,
    module_paths: &[PathBuf],
) -> Result<Headless, String> {
    Headless::from_file_with_setup(
        path,
        width,
        height,
        module_paths,
        garden_script::headless::register_panel_stubs,
    )
}

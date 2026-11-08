//! No library: this package is its tests (`tests/`). See the `test/ui-corpus/`
//! section of `test/README.md`.
//!
//! They test core features (`petal::static_cache`, the frame gate in
//! `petal::run_deps`, `petal::memo`, `petal::effect_audit`, `petal::dm`)
//! over the panel corpus, driven through `petal_ui::harness::Headless`
//! because a panel needs a UI host to run. Tests of `petal-ui` itself stay in
//! `core-libs/petal-ui/tests/`.

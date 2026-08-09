//! The `garden` binary: a thin wrapper over the `garden-app` library crate.
//! All argument parsing, layout resolution, and frontend dispatch live in
//! [`garden_app::run`] (`src/lib.rs`).

fn main() {
    garden_app::run();
}

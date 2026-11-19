//! `petal-ui-run`: the generic headless trace driver, petal-ui's natives only.
//! See [`petal_ui_headless::run_cli`].

fn main() {
    petal_ui_headless::run_cli::main("petal-ui-run", &mut petal_ui_headless::NoHost)
}

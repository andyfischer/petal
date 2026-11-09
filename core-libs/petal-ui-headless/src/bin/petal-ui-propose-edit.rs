//! `petal-ui-propose-edit`: `petal propose-edit` for scripts that `import ui`,
//! petal-ui's natives only. See [`petal_ui_headless::propose_edit_cli`].

fn main() {
    petal_ui_headless::propose_edit_cli::main("petal-ui-propose-edit", &mut petal_ui_headless::NoHost)
}

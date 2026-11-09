//! `garden-panel-run`: `petal-ui-run` with Garden's panel stubs. See the
//! crate docs and `petal_ui_headless::run_cli`.

fn main() {
    petal_ui_headless::run_cli::main("garden-panel-run", &mut garden_panel_run::GardenPanelHost)
}

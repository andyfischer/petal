//! `garden-panel-propose-edit`: `petal-ui-propose-edit` with Garden's panel
//! stubs. See the crate docs and `petal_ui_headless::propose_edit_cli`.

fn main() {
    petal_ui_headless::propose_edit_cli::main(
        "garden-panel-propose-edit",
        &mut garden_panel_run::GardenPanelHost,
    )
}

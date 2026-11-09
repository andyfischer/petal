//! `garden-panel-run` and `garden-panel-propose-edit`: petal-ui-headless's
//! `petal-ui-run` and `petal-ui-propose-edit`, with Garden's panel stubs
//! ([`garden_script::headless`]) registered before the script loads, so a
//! panel drawer — anything that does `import garden`, which most of
//! `examples/` does through its `layout.ptl` — runs headlessly. The stubs are
//! inert for a script that never calls them, so these run every `import ui`
//! script exactly as the generic CLIs do; the tools (`tools/verify.ts`, the
//! MCP server, `tools/oracle-external.ts`) use them for everything.
//!
//! The one flag beyond the generic CLI's is `--query-fixtures q.json`: ready
//! answers for the stub `query(kind, arg)`, so a drawer renders its loaded
//! path instead of its spinner (format in
//! [`garden_script::headless::parse_query_fixtures`]).

use std::path::Path;

use petal::env::Env;
use petal_ui_headless::CliHost;

/// The Garden panel host for the CLIs.
#[derive(Default)]
pub struct GardenPanelHost;

impl CliHost for GardenPanelHost {
    fn setup(&self, env: &mut Env) {
        garden_script::headless::register_panel_stubs(env);
    }

    fn flag(
        &mut self,
        flag: &str,
        value: &mut dyn FnMut() -> Result<String, String>,
    ) -> Result<bool, String> {
        match flag {
            "--query-fixtures" => {
                let path = value()?;
                let table = query_fixtures(Path::new(&path))?;
                // The stubs read the table on this thread, which is the one
                // the CLI runs its frames on.
                garden_script::headless::set_query_fixtures(table);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn usage(&self) -> &str {
        "[--query-fixtures q.json]"
    }
}

/// Read a `--query-fixtures` file; errors are prefixed with its path.
fn query_fixtures(
    path: &Path,
) -> Result<Vec<((String, String), serde_json::Value)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
    garden_script::headless::parse_query_fixtures(&json)
        .map_err(|e| format!("{}: {e}", path.display()))
}

//! `gpp-test-app` — a panel-mode GPP app that puts a Garden pane into a chosen
//! situation on demand, so panel behavior (chiefly the **error card**) can be
//! reproduced for a screenshot or an integration test without needing a real
//! failing app like a broken `:PR`.
//!
//! The **first launch arg selects the mode**; each mode pushes a different
//! colocated `.ptl` drawer:
//!
//! - `ok` (default)             — a healthy panel that just paints.
//! - `runtime-error`            — a classic drawer crash: `len(nil)` raises
//!   "Cannot get length of nil [line N, column M]" mid-frame, so the host
//!   shows the error card over a live frame.
//! - `runtime-error-long`       — a runtime error with a long message, to
//!   exercise the card's word-wrapping.
//! - `query-error`              — the panel stays healthy but a `query(...)`
//!   fails; the drawer surfaces it via `error_of` (the soft/async error path).
//!
//! Launch it as the whole layout, e.g.
//! `garden --subprocess gpp-test-app runtime-error`. It is a fixture, not a
//! tool, so it has no bare `garden gpp-test-app` subcommand. See `docs/gpp.md`.

use petal_query::gpp::{self, PanelUi};
use petal_query::{Provider, Reply};

const OK: &str = include_str!("ok.ptl");
const RUNTIME_ERROR: &str = include_str!("runtime_error.ptl");
const RUNTIME_ERROR_LONG: &str = include_str!("runtime_error_long.ptl");
const QUERY_ERROR: &str = include_str!("query_error.ptl");
const SAVE: &str = include_str!("save.ptl");

/// Resolve the launch args to (a human label, the drawer to push). Hyphen and
/// underscore spellings both work; an unknown or absent mode is the healthy one.
fn select_mode(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "runtime-error" | "runtime_error" | "error" => ("runtime-error", RUNTIME_ERROR),
        "runtime-error-long" | "runtime_error_long" | "error-long" | "long" => {
            ("runtime-error-long", RUNTIME_ERROR_LONG)
        }
        "query-error" | "query_error" | "query" => ("query-error", QUERY_ERROR),
        "save" | "edit" => ("save", SAVE),
        _ => ("ok", OK),
    }
}

fn main() {
    // The first non-flag arg is the mode (`--subprocess gpp-test-app <mode>`).
    let mode = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .unwrap_or_default();
    let (_, script) = select_mode(&mode);

    // A single always-failing `boom` query backs the `query-error` mode; the
    // other modes never call it, so it is harmless to always register.
    //
    // A `save` mutation backs the `save` mode (the editable-panel write-back
    // path): it writes the payload's `text` to the file named by
    // `GPP_TEST_SAVE_PATH` (so a test can assert the round-trip) and replies with
    // a status string the host surfaces. Harmless to always register.
    let provider = Provider::stateless()
        .query("boom", |_state: &mut (), _ctx| {
            Reply::error("boom: the upstream query failed (simulated by gpp-test-app)")
        })
        .on_mutation("save", |_state: &mut (), ctx| {
            let text = ctx.arg["text"].as_str().unwrap_or_default();
            match std::env::var("GPP_TEST_SAVE_PATH") {
                Ok(path) => match std::fs::write(&path, text) {
                    Ok(()) => Reply::json(format!("wrote {} bytes", text.len())),
                    Err(e) => Reply::error(format!("write failed: {e}")),
                },
                Err(_) => Reply::error("GPP_TEST_SAVE_PATH not set"),
            }
        });

    if let Err(err) = gpp::serve(provider, PanelUi::new("gpp-test-app", script)) {
        eprintln!("gpp-test-app: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_to_scripts_and_default_is_ok() {
        assert_eq!(select_mode("runtime-error").0, "runtime-error");
        assert_eq!(select_mode("runtime_error").0, "runtime-error");
        assert_eq!(select_mode("runtime-error-long").0, "runtime-error-long");
        assert_eq!(select_mode("query-error").0, "query-error");
        // Unknown / empty falls back to the healthy panel.
        assert_eq!(select_mode("").0, "ok");
        assert_eq!(select_mode("nonsense").0, "ok");
    }
}

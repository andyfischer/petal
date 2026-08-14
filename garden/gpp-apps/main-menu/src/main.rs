//! `main-menu` — Garden's start screen, as a panel-mode GPP app.
//!
//! It pushes the colocated `main_menu.ptl` drawer and answers its single query
//! kind from the shared state database (see [`main_menu`] for the schema-drift
//! caveat and the read-only contract):
//!
//! - `query("recents", "")` → `{ projects, files, prs }`, newest-first.
//!
//! The screen's *actions* are not this app's business at all: a row click calls
//! the host-handled `mutate("open_path" | "open_project" | "open_pr" |
//! "open_file_dialog", …)`, which `App::host_mutation` intercepts before any
//! forwarding reaches here. That is deliberate — only the host can open a file
//! into a pane, and it also means the menu works identically whether it runs as
//! this subprocess or as an in-process `panel(...)`.

use std::path::PathBuf;
use std::time::Duration;

use main_menu::{recents, state_db_path};
use petal_query::gpp::{self, PanelUi};
use petal_query::{CachePolicy, Provider, Reply};

/// The start-screen drawer, embedded from this crate. The host compiles and
/// runs it in-process; this app only answers its `query(...)` requests.
const UI_SCRIPT: &str = include_str!("main_menu.ptl");

fn main() {
    // Per-run state is the directory the pane was launched in — not read by the
    // `recents` answer (the database is user-global, not per-project), but kept
    // because it is what a "current project" affordance would be resolved from.
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        // The lists only change when the user opens something — which leaves this
        // screen — so a couple of seconds of freshness is plenty. The stale window
        // means a re-poll never blanks a list that is already on screen.
        .query("recents", |_cwd: &mut PathBuf, _ctx| {
            Reply::json(recents(&state_db_path())).cache(
                CachePolicy::max_age(Duration::from_secs(2))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        });

    if let Err(err) = gpp::serve(provider, PanelUi::new("main-menu", UI_SCRIPT)) {
        eprintln!("main-menu: {err}");
        std::process::exit(1);
    }
}

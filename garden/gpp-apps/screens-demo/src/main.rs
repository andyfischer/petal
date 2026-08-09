//! `screens-demo` — the worked example of GPP Phase 4: browser-style navigation
//! across a **subprocess** panel's screens.
//!
//! It declares two navigable screens with [`PanelUi::screen`] and answers no
//! queries at all — the point is purely the navigation path. The home screen
//! (`home.ptl`) is pushed at startup; from it, pressing `n` calls
//! `navigate("detail.ptl")`, which the host serves from this app over the
//! built-in `navigate` mutation. `Ctrl+[`/`Ctrl+]` (or `:back`/`:forward`) walk
//! the host-owned history stack; the detail screen also drives them from the
//! script via `navigate_back()`.
//!
//! Launch from a layout: `layout(process("/abs/path/target/debug/screens-demo"))`.

use petal_query::gpp::{self, PanelUi};
use petal_query::Provider;

const HOME: &str = include_str!("home.ptl");
const DETAIL: &str = include_str!("detail.ptl");

fn main() {
    // No state, no queries — a stateless provider that only carries screens.
    let provider = Provider::stateless();
    let ui = PanelUi::new("screens-demo", HOME).screen("detail.ptl", DETAIL);
    if let Err(err) = gpp::serve(provider, ui) {
        eprintln!("screens-demo: {err}");
        std::process::exit(1);
    }
}

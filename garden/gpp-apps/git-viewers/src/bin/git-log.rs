//! `git-log` — a panel-mode GPP app reimplementing Garden's built-in `:Git`
//! history browser as a subprocess-backed pane.
//!
//! It pushes the **production drawer** (`git_panel.ptl`, colocated in this crate)
//! the host runs in-process for `:Git`, then answers that drawer's two query kinds
//! by shelling out to `git`:
//!
//! - `query("log", "")`      → the commit history + repo/branch/dirty header;
//! - `query("commit", arg)`  → one commit's (or the working tree's) numbered diff,
//!   with `@full:` for full-context and `@worktree` for uncommitted changes.
//!
//! It is a worked example of the [`petal_query::App`] provider API: declare a
//! handler per query kind, stamp each answer with a [`CachePolicy`], and
//! [`serve`](petal_query::App::serve). The per-run state is the repo path,
//! resolved from the handshake (first launch arg, else the pane cwd). Launch it
//! from a layout with `process("/abs/path/git-log", ["/repo/dir"])`.

use std::path::PathBuf;
use std::time::Duration;

use git_viewers::{git_commit, git_log, WORKTREE_ARG};
use petal_query::gpp::{self, PanelUi};
use petal_query::{CachePolicy, Provider, Reply};

/// The `:Git` history drawer, embedded from this crate. The host compiles and
/// runs it in-process; the app only answers its `query(...)` requests.
const UI_SCRIPT: &str = include_str!("../git_panel.ptl");

fn main() {
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        // The history list + repo/branch/dirty header changes whenever the repo
        // does (a new commit, a checkout). Refresh every few seconds, serving the
        // last list while the refresh runs so the pane never flashes a spinner.
        .query("log", |repo: &mut PathBuf, _ctx| {
            Reply::from(git_log(repo)).cache(
                CachePolicy::max_age(Duration::from_secs(3))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        })
        // A commit addressed by hash is immutable — cache it forever. The
        // working-tree diff (`@worktree`, and `@full:@worktree`) is live, so it
        // is never cached.
        .query("commit", |repo: &mut PathBuf, ctx| {
            let policy = if ctx.arg.contains(WORKTREE_ARG) {
                CachePolicy::no_store()
            } else {
                CachePolicy::immutable()
            };
            Reply::value(git_commit(repo, ctx.arg)).cache(policy)
        });
    let result = gpp::serve(provider, PanelUi::new("git-log", UI_SCRIPT));

    if let Err(err) = result {
        eprintln!("git-log: {err}");
        std::process::exit(1);
    }
}

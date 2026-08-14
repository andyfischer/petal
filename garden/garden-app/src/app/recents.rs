//! The [`App`] side of the recents lists ([`crate::recents`]): the handle the
//! frontends attach, and the recording helpers the open paths call.
//!
//! Every helper here is best-effort. A recents write is bookkeeping for a
//! later menu screen, so a failing (or absent) state database must leave the
//! open itself untouched — failures are warned about once per call site's
//! `eprintln!`, never surfaced as a user-visible error.

use std::path::Path;

use crate::recents::{repo_identity, Recents};

use super::App;

impl App {
    /// Attach the recents lists, called by each frontend right after
    /// [`App::new`](super::App::new). `None` disables recording — the case in
    /// unit tests and when the state database is unavailable.
    pub fn set_recents(&mut self, recents: Option<Recents>) {
        self.recents = recents;
    }

    /// Record a file the user opened (and, in [`Recents::record_file`], the
    /// project it belongs to). Paths that do not name a file on disk are
    /// skipped: `:e newfile` is a buffer the user may never save, and a
    /// directory argument is a browse, recorded by
    /// [`record_project_opened`](App::record_project_opened) instead.
    pub(in crate::app) fn record_file_opened(&mut self, path: &str) {
        let path = Path::new(path);
        if !path.is_file() {
            return;
        }
        if let Some(recents) = self.recents.as_ref() {
            if let Err(err) = recents.record_file(path) {
                eprintln!("garden: {err}");
            }
        }
    }

    /// Record a directory the user opened as a project. A directory inside a
    /// repo records the repo root rather than the directory itself, so
    /// opening `src/` and opening the checkout are one entry; a directory
    /// outside any repo is recorded as its own project, since that is still
    /// the root the user chose to work in.
    pub(in crate::app) fn record_project_opened(&mut self, dir: &str) {
        let dir = Path::new(dir);
        if !dir.is_dir() {
            return;
        }
        let root = crate::recents::project_root(dir).unwrap_or_else(|| dir.to_path_buf());
        if let Some(recents) = self.recents.as_ref() {
            if let Err(err) = recents.record_project(&root) {
                eprintln!("garden: {err}");
            }
        }
    }

    /// Record a PR review opened over `dir`. The title is left empty — filling
    /// it needs `gh`, which is slow and may not be authenticated, so a later
    /// step fills it in from the review pane once the client has the data.
    pub(in crate::app) fn record_pr_opened(&mut self, dir: &str, number: i64) {
        let dir = Path::new(dir);
        let Some(recents) = self.recents.as_ref() else {
            return;
        };
        let repo = repo_identity(dir);
        if let Err(err) = recents.record_pr(&repo, number, "", Some(dir)) {
            eprintln!("garden: {err}");
        }
    }
}

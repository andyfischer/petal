//! Petal scripts as application documents.
//!
//! An application's file is a Petal script; running it produces what the
//! user sees and edits, and saving writes the script back. This crate is
//! the write path for such documents (the design is in
//! `docs/aspirational-ideas/programs-as-documents.md`):
//!
//! ```text
//! gesture ─► intent ─► resolve ─► source edit ─► verify ─► commit ─► save
//! ```
//!
//! - [`DocumentSession`]: the source text as the single source of truth,
//!   with undo and redo, keyed coalescing, gestures, save and
//!   `poll_disk` ([`session`]).
//! - [`Runner`] / [`DocumentRun`]: run a source with the host's vocabulary
//!   registered and element tracing on, and read its result ([`run`]).
//! - [`OutputMap`]: for each element of the result, its construction site,
//!   call path, fan-out, per-field provenance and a stable [`OutputKey`]
//!   ([`output_map`]).
//! - [`Intent`] / [`resolve`]: an edit stated against the output, resolved
//!   to a verified source edit, a choice, or a reason ([`intent`]), by the
//!   resolution ladder: direct literal, single site, a shared site's scope
//!   ([`scope`]: the shared literal, the data entry, a host override), a
//!   direct-manipulation solve ([`solve`]), and with consent writing the
//!   output out as source ([`concretize`]).
//! - [`editable_literal`]: whether a top-level binding is a literal the
//!   application can write into ([`editable`]).
//!
//! [`Document`] puts a session, a runner and the latest run together.
//!
//! The crate uses only petal's public API, and petal is an optional
//! dependency of a host: an embedder that does not edit documents leaves
//! this crate out.

pub mod concretize;
pub mod editable;
pub mod intent;
pub mod output_map;
pub mod run;
pub mod scope;
pub mod session;
pub mod solve;
pub mod splice;

pub use editable::{NotEditable, editable_literal};
pub use concretize::{Concretized, concretize};
pub use intent::{
    Choice, Intent, Resolution, ResolveOptions, ResolvedEdit, Step, Verified, concretize_and_apply, resolve, verify,
};
pub use scope::{DataEntry, OverrideEdit, OverrideHook, OverrideRequest, Scope, ScopePolicy, UseSite, data_entry};
pub use solve::{Solved, solve_field};
pub use output_map::{
    Field, FieldProvenance, LiteralSource, OutputKey, OutputMap, OutputNode, PathSeg, Site, WithinLiteral,
};
pub use run::{DocumentRun, OutputSource, Runner};
pub use session::DocumentSession;

/// An open document: its session, how to run it, and its latest run.
pub struct Document {
    pub session: DocumentSession,
    pub runner: Runner,
    /// The run of the current source, once [`run`](Self::run) made it.
    last: Option<(String, Result<DocumentRun, String>)>,
    pub options: ResolveOptions,
}

impl Document {
    pub fn new(session: DocumentSession, runner: Runner) -> Document {
        Document { session, runner, last: None, options: ResolveOptions::default() }
    }

    /// Open the file at `runner`'s path.
    pub fn open(runner: Runner) -> Result<Document, String> {
        Ok(Document::new(DocumentSession::read(runner.path())?, runner))
    }

    /// The run of the current source, running it if it changed since the
    /// last run.
    pub fn run(&mut self) -> Result<&DocumentRun, &str> {
        let stale = self.last.as_ref().is_none_or(|(s, _)| s != self.session.source());
        if stale {
            let source = self.session.source().to_string();
            let run = self.runner.run(&source);
            self.last = Some((source, run));
        }
        match &self.last.as_ref().expect("just set").1 {
            Ok(r) => Ok(r),
            Err(e) => Err(e),
        }
    }

    /// Resolve `intent` against the current source and, when it resolves
    /// to an edit, commit it as one undo step (coalesced by `key`). Returns
    /// the resolution either way; on anything but [`Resolution::Edit`] the
    /// source is unchanged.
    pub fn apply(&mut self, intent: &Intent, key: Option<&str>) -> Resolution {
        let options = self.options.clone();
        let runner = self.runner.clone();
        let res = match self.run() {
            Ok(run) => resolve(&runner, run, intent, &options),
            Err(e) => return Resolution::Refused { reason: format!("the document does not run: {e}") },
        };
        if let Resolution::Edit(edit) = &res {
            self.session.commit(edit.source.clone(), key);
        }
        res
    }

    /// Commit an edit the host chose (a [`Choice`]'s), as one undo step.
    pub fn commit(&mut self, edit: &ResolvedEdit, key: Option<&str>) -> bool {
        self.session.commit(edit.source.clone(), key)
    }
}

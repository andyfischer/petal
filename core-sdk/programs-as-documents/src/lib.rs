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
//! - [`Runner`] / [`DocumentRun`]: run a source with the host's packages
//!   registered and element tracing on, and read its output, usually the
//!   arguments of the [`OutputCall`] it ends in ([`run`]).
//!   [`Runner::evaluate`] / [`Evaluated`]: the output alone, untraced, as
//!   plain JSON, and [`Props`] to read an element's attributes from it
//!   ([`props`]).
//! - [`OutputMap`]: for each element of the result, its construction site,
//!   call path, fan-out, per-field provenance and a stable [`OutputKey`]
//!   ([`output_map`]).
//! - [`Intent`] / [`resolve`]: an edit stated against the output, resolved
//!   to a verified source edit, a choice, or a reason ([`intent`]), by the
//!   resolution ladder: direct literal, single site, a shared site's scope
//!   ([`scope`]: the shared literal, the data entry, a host override), a
//!   direct-manipulation solve ([`solve`]), and with consent writing the
//!   output out as source ([`concretize`]).
//! - [`StructureIntent`] / [`resolve_structure`]: structural edits by
//!   outcome (reorder, move between parents, insert, remove, duplicate),
//!   resolved to the entries of the document's list and element literals or
//!   to the data list a `map`/`for` reads ([`structure`], [`entries`]).
//! - [`copy`] / [`paste`]: an output element as source text with its free
//!   references, pasted into the same or another document with each
//!   reference copied as source, as a value, or rebound ([`clipboard`]).
//! - [`editable_literal`]: whether a top-level binding is a literal the
//!   application can write into ([`editable`]).
//!
//! [`Document`] puts a session, a runner and the latest run together.
//!
//! The crate uses only petal's public API, and petal is an optional
//! dependency of a host: an embedder that does not edit documents leaves
//! this crate out.

pub mod clipboard;
pub mod concretize;
pub mod editable;
pub mod entries;
pub mod intent;
pub mod output_check;
pub mod output_map;
pub mod props;
pub mod run;
pub mod scope;
pub mod session;
pub mod solve;
pub mod splice;
pub mod structure;
pub mod text;

pub use editable::{NotEditable, editable_literal};
pub use concretize::{ConcretizeOptions, Concretized, concretize, concretize_with, tidied};
pub use intent::{
    Choice, Intent, Resolution, ResolveOptions, ResolvedEdit, Step, Verified, concretize_and_apply, resolve, source_hash,
    text_of, verify,
};
pub use petal::static_expr::StaticExpr;
pub use scope::{DataEntry, OverrideEdit, OverrideHook, OverrideRequest, Scope, ScopePolicy, UseSite, data_entry, data_entry_from};
pub use solve::{Solved, solve_field};
pub use output_map::{
    Field, FieldProvenance, LiteralSource, OutputKey, OutputMap, OutputNode, PathSeg, Site, WithinLiteral,
};
pub use output_check::check_output_calls;
pub use run::{DocumentRun, Evaluated, OutputCall, OutputSource, Runner, register_output_native};
pub use props::Props;
pub use session::DocumentSession;
pub use structure::{
    EntrySource, Parent, Place, SiblingEntry, StructureIntent, StructureOptions, Template, parent_of,
    resolve_structure, sibling_entries, siblings,
};
pub use clipboard::{Clip, ClipBinding, ClipSource, FreeRef, PasteOptions, PasteRef, PreparedPaste, RefKind, RefUse, copy, copy_text, paste, prepare_paste};
pub use entries::{Container, ContainerKind};

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

    /// Resolve a structural intent ([`StructureIntent`]) against the
    /// current source and commit it as one undo step when it resolves to an
    /// edit.
    pub fn apply_structure(&mut self, intent: &StructureIntent, opts: &StructureOptions, key: Option<&str>) -> Resolution {
        let runner = self.runner.clone();
        let res = match self.run() {
            Ok(run) => resolve_structure(&runner, run, intent, opts),
            Err(e) => return Resolution::Refused { reason: format!("the document does not run: {e}") },
        };
        if let Resolution::Edit(edit) = &res {
            self.session.commit(edit.source.clone(), key);
        }
        res
    }

    /// Paste `clip` at `at` and commit it as one undo step when it
    /// resolves.
    pub fn paste(&mut self, clip: &Clip, at: Place, opts: &PasteOptions) -> Resolution {
        let runner = self.runner.clone();
        let res = match self.run() {
            Ok(run) => paste(&runner, run, clip, at, opts),
            Err(e) => return Resolution::Refused { reason: format!("the document does not run: {e}") },
        };
        if let Resolution::Edit(edit) = &res {
            self.session.commit(edit.source.clone(), None);
        }
        res
    }

    /// Copy output `key` of the current run as source.
    pub fn copy(&mut self, key: &OutputKey) -> Result<Clip, String> {
        let run = self.run().map_err(|e| format!("the document does not run: {e}"))?;
        copy(run, key)
    }

    /// Commit an edit the host chose (a [`Choice`]'s), as one undo step.
    /// `Ok(false)` when it changes nothing.
    ///
    /// An edit resolved against another source than the current one (the
    /// document changed while a choice was open: another edit, an undo, a
    /// reload) is refused, since committing it would undo that change; its
    /// [`ResolvedEdit::base`] says which source it was resolved against.
    /// Resolve the intent again instead.
    pub fn commit(&mut self, edit: &ResolvedEdit, key: Option<&str>) -> Result<bool, String> {
        if let Some(base) = edit.base
            && base != source_hash(self.session.source())
        {
            return Err(format!(
                "the document changed since this edit was worked out ({}); make the change again",
                edit.description
            ));
        }
        Ok(self.session.commit(edit.source.clone(), key))
    }
}

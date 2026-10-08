//! `petal apply-change` — refactors that change what the program does.
//!
//! The other two rewriting commands promise the program stays the same:
//! `petal lint --fix` normalizes spelling behind an IR-equality proof, and
//! `petal suggest --apply` adds annotations behind one. An *operation* here
//! makes no such promise. It is a change the author has decided on — "this
//! binding should be a mutable cell" — carried out everywhere it has to be,
//! consistently, across every file it reaches. What it does promise:
//!
//! - **It names what it touches.** An operation takes an explicit target
//!   ([`target`]) and edits only the mentions that resolve to it.
//! - **It refuses rather than guesses.** A use it has no faithful rewrite for
//!   stops the whole change, with the lines that caused it.
//! - **The result compiles** ([`compile_gate`]). Every file it would write,
//!   and every `--from` entry point, is compiled with the edits in place
//!   before any of them is written. Nothing is written unless all of them
//!   pass. (A file that was already broken may stay broken in the same ways,
//!   and gain no new one.)
//!
//! Edits are planned as [`Splice`]s over the original text and applied with
//! [`crate::rewrite::apply_splices`], the way `lint --fix` plans its own, so
//! everything an operation does not touch — comments, layout — survives. A
//! file that was `petal fmt`-clean before is formatted again afterwards; one
//! that was not is left exactly as edited.
//!
//! # Adding an operation
//!
//! An operation is a module with a `plan` function that returns a [`Plan`],
//! a row in [`OPERATIONS`], and an arm in [`plan`]. It reads the target file,
//! decides its edits per file as [`Edited`] values, and hands them to
//! [`finish`], which applies, formats and compile-gates them. Finding the
//! files that import a binding is [`importers`].
//!
//! The one operation so far is [`convert_to_var`].

pub mod convert_to_var;
pub mod importers;
pub mod target;

use std::path::{Path, PathBuf};

use crate::rewrite::{Splice, apply_splices};
use crate::suggest::HostEnv;
use crate::typecheck::globals::HostProfile;

pub use importers::ptl_files_under;

/// One operation, as `petal apply-change` lists it.
pub struct OperationInfo {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
}

pub const CONVERT_TO_VAR: &str = "convert-to-var";

/// Every operation.
pub const OPERATIONS: &[OperationInfo] = &[OperationInfo {
    name: CONVERT_TO_VAR,
    summary: "turn a `let` into a `var`, or a `state` into a `state var`",
    usage: "petal apply-change convert-to-var <file> --target <path> [--from <file>]... [--dry-run]",
}];

pub fn operation(name: &str) -> Option<&'static OperationInfo> {
    OPERATIONS.iter().find(|op| op.name == name)
}

/// Which operation to run, with its own arguments.
pub enum Request {
    /// [`convert_to_var`]: `target` is a [`target`] path.
    ConvertToVar { target: String },
}

/// What every operation is told.
#[derive(Default)]
pub struct ChangeOptions {
    /// Module search directories, as `-I` gives them.
    pub include_dirs: Vec<PathBuf>,
    /// Entry files of the programs that use the target file. Their import
    /// graphs are where importers are looked for, and each is compiled by the
    /// gate. Empty means "scan the target's project directory instead"; see
    /// [`importers`].
    pub from: Vec<PathBuf>,
    /// The host the scripts are written for, as `petal check --host` names it.
    pub host: HostProfile,
}

impl ChangeOptions {
    /// An `Env` set up the way `petal check` and `petal suggest` set theirs
    /// up, so the gate agrees with them on what compiles.
    fn host_env(&self) -> HostEnv {
        HostEnv::new(&self.include_dirs, self.host)
    }
}

/// One file's planned edits, before they are applied.
pub(crate) struct Edited {
    pub path: PathBuf,
    pub before: String,
    pub splices: Vec<Splice>,
    pub detail: String,
}

/// One file an operation changes.
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    /// What changed, in a phrase (`3 writes -> set, 2 reads -> get`).
    pub detail: String,
}

/// One file found to import the target binding.
#[derive(Debug, Clone)]
pub struct ImporterNote {
    pub path: PathBuf,
    /// The import that reaches the binding, as written.
    pub form: String,
    /// What was done there, or why nothing had to be.
    pub detail: String,
}

/// Where importers were looked for, and what was found.
#[derive(Debug, Clone)]
pub struct ImporterReport {
    pub how: String,
    pub importers: Vec<ImporterNote>,
}

/// A change, worked out and proven, not yet written.
#[derive(Debug)]
pub struct Plan {
    pub operation: &'static str,
    /// The change in one line.
    pub summary: String,
    /// Every file that changes, the target file first.
    pub files: Vec<FileChange>,
    /// The importers considered. `None` when the target is not visible to
    /// other files, so none were looked for.
    pub importers: Option<ImporterReport>,
    /// Things worth saying that did not stop the change.
    pub notes: Vec<String>,
}

impl Plan {
    /// The text `path` will hold, if this plan changes it.
    pub fn after(&self, path: &Path) -> Option<&str> {
        let path = crate::module::canonical_path(path);
        self.files
            .iter()
            .find(|f| crate::module::canonical_path(&f.path) == path)
            .map(|f| f.after.as_str())
    }

    /// Write every changed file. If one write fails, the files already
    /// written are put back, so the change lands whole or not at all.
    pub fn write(&self) -> Result<(), String> {
        for (i, file) in self.files.iter().enumerate() {
            if let Err(e) = std::fs::write(&file.path, &file.after) {
                for done in &self.files[..i] {
                    let _ = std::fs::write(&done.path, &done.before);
                }
                return Err(format!(
                    "writing {}: {e} (no file was changed)",
                    file.path.display()
                ));
            }
        }
        Ok(())
    }
}

/// Work out `request` against `file`. Nothing is written; see [`Plan::write`].
/// An `Err` is a refusal or a failure, worded for the user.
pub fn plan(file: &Path, request: &Request, opts: &ChangeOptions) -> Result<Plan, String> {
    match request {
        Request::ConvertToVar { target } => convert_to_var::plan(file, target, opts),
    }
}

/// Apply each file's splices, keep a formatted file formatted, and prove the
/// result ([`compile_gate`]). Returns the files that change, and anything the
/// gate wants said.
pub(crate) fn finish(
    edited: Vec<Edited>,
    opts: &ChangeOptions,
) -> Result<(Vec<FileChange>, Vec<String>), String> {
    let mut files = Vec::new();
    for mut e in edited {
        e.splices.sort_by_key(|s| s.start);
        let chars: Vec<char> = e.before.chars().collect();
        let mut after = apply_splices(&chars, &e.splices);
        // Inserting a keyword can push a line past what `fmt` allows; a file
        // that was a `fmt` fixed point stays one.
        if crate::fmt::format_source(&e.before).is_ok_and(|f| f == e.before) {
            after = crate::fmt::format_source(&after).map_err(|err| {
                format!("{}: the result does not format: {err}", e.path.display())
            })?;
        }
        if after != e.before {
            files.push(FileChange {
                path: e.path,
                before: e.before,
                after,
                detail: e.detail,
            });
        }
    }
    let notes = compile_gate(&files, opts)?;
    Ok((files, notes))
}

/// The gate: compile every changed file, and every `--from` entry, as
/// `petal check` would, with the changed files' new text standing in for
/// what is on disk. Each must compile.
///
/// One exception, because the commonest reason to run an operation is a file
/// that does *not* compile yet: a file that failed before the change may
/// still fail after it, provided it fails in the compiler proper (so its
/// imports resolved and every diagnostic was collected) and every error it
/// has now is one it had before. A file with three bindings to convert gets
/// there one conversion at a time. Such a file is reported in the returned
/// notes; anything else is a refusal.
fn compile_gate(files: &[FileChange], opts: &ChangeOptions) -> Result<Vec<String>, String> {
    use crate::error::{LoadError, Phase};
    use crate::module::canonical_path;

    let mut entries: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    for from in &opts.from {
        if !entries
            .iter()
            .any(|e| canonical_path(e) == canonical_path(from))
        {
            entries.push(from.clone());
        }
    }
    let compile = |entry: &Path, edited: bool| -> Result<(), LoadError> {
        let mut host = opts.host_env();
        let mut source = None;
        for f in files {
            if edited {
                host.env.override_file_source(&f.path, &f.after);
            }
            if canonical_path(&f.path) == canonical_path(entry) {
                source = Some(if edited { &f.after } else { &f.before }.clone());
            }
        }
        let source = match source {
            Some(s) => s,
            None => std::fs::read_to_string(entry)
                .map_err(|e| LoadError::message(Phase::Module, e.to_string()))?,
        };
        host.env
            .compile_program_diag(crate::program::ProgramId(0), &source, Some(entry))
            .map(|_| ())
    };
    // An error's identity across the edit: its text and file. Positions move.
    let keys = |e: &LoadError| -> Vec<(Option<String>, String)> {
        e.items
            .iter()
            .map(|i| (i.file.clone(), i.message.clone()))
            .collect()
    };

    let mut notes = Vec::new();
    for entry in &entries {
        let Err(after) = compile(entry, true) else {
            continue;
        };
        let refuse = |why: &str| {
            Err(format!(
                "refusing to write: {} would not compile after the change.\n{after}\n{why}",
                entry.display()
            ))
        };
        let Err(before) = compile(entry, false) else {
            return refuse("It compiled before the change, so the rewrite is at fault.");
        };
        let mut had = keys(&before);
        let nothing_new = before.phase == Phase::Compile
            && after.phase == Phase::Compile
            && keys(&after).into_iter().all(|k| {
                had.iter()
                    .position(|h| *h == k)
                    .map(|at| had.swap_remove(at))
                    .is_some()
            });
        if !nothing_new {
            return refuse(
                "It did not compile before the change either, and this is not simply an error \
                 it already had. If its imports or host natives are not found here, pass \
                 `-I <dir>` or `--host <name>`.",
            );
        }
        notes.push(format!(
            "{} still does not compile, for {} that {} there before the change: {}",
            entry.display(),
            count(after.items.len(), "reason"),
            if after.items.len() == 1 {
                "was"
            } else {
                "were"
            },
            after.items[0].message
        ));
    }
    Ok(notes)
}

/// `1 write`, `3 writes`.
pub(crate) fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate's reason to exist: a rewrite that breaks a file that used to
    /// compile is never handed back as a plan.
    #[test]
    fn the_gate_refuses_a_rewrite_that_breaks_a_compiling_file() {
        let dir = std::env::temp_dir().join(format!("petal-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.ptl");
        let before = "let x = 1\nx = 2\nprint(x)\n";
        std::fs::write(&path, before).unwrap();
        let broken = FileChange {
            path: path.clone(),
            before: before.to_string(),
            // `var` without the matching `set`.
            after: "var x = 1\nx = 2\nprint(x)\n".to_string(),
            detail: String::new(),
        };
        let opts = ChangeOptions {
            host: HostProfile::Core,
            ..ChangeOptions::default()
        };
        let err = compile_gate(&[broken], &opts).expect_err("must refuse");
        assert!(err.contains("use `set x = ...` to write it"), "{err}");
        assert!(err.contains("the rewrite is at fault"), "{err}");

        let fine = FileChange {
            path,
            before: before.to_string(),
            after: "var x = 1\nset x = 2\nprint(x)\n".to_string(),
            detail: String::new(),
        };
        assert_eq!(compile_gate(&[fine], &opts), Ok(Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Hot reload that does only the work an edit calls for.
//!
//! [`Env::reload_program`] is the entry point a host calls when a source file
//! changed. It compares every source file the running program was compiled
//! from with the text that file holds now ([`crate::source_diff`]) and takes
//! the cheapest path that is still exact:
//!
//! | The edit                                   | What happens                                   |
//! |--------------------------------------------|------------------------------------------------|
//! | nothing, or whitespace / comments / layout | source positions move; nothing else is touched |
//! | literal values (`0.35` to `0.4`)           | the constants are written in place             |
//! | anything else                              | recompile, then [`Env::transfer_state`]        |
//!
//! The first two never compile and never lower. The contract for all three is
//! the same: afterwards the program, and the stack, are what a full recompile
//! of the new source followed by `transfer_state` would have left — same
//! terms, same constants by value, same spans, same warnings, same state.
//! `core/tests/hot_reload.rs` holds that to a corpus of edits.
//!
//! # Late binding
//!
//! A literal compiles to a `Constant` term, lowered to one `LoadConst`
//! instruction that reads the program's constant table *each time it runs*.
//! Nothing downstream bakes the value in: the optimizer passes look at
//! instruction kinds and registers, never at constant values. So changing a
//! literal's value is: give its term a constant-table slot of its own
//! ([`ConstantTable::alloc_slot`](crate::constant_table::ConstantTable::alloc_slot))
//! the first time, point the one instruction at it, and from then on write
//! the slot.
//!
//! What a run *derived* from the old value is dropped the same way a full
//! reload drops it — closures (which captured it), the captured function
//! table, memo records (a function body that loads the constant is an input
//! no record lists), and the frame gate's verdict — by running the same
//! [`transfer_stack_state`](crate::transfer_state::transfer_stack_state) a
//! full reload runs. `state` is kept, exactly as there: a `state` slot that
//! was initialized from the old value keeps its value under both.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::*;
use crate::ast::Literal;
use crate::backend::bytecode::Inst;
use crate::constant_table::{ConstantId, ConstantValue};
use crate::program::{TermId, TermOp};
use crate::source_diff::{FileDiff, SourceChange, diff_source};
use crate::source_map::{FileId, SourceSpan};

/// How a reload was carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// No source file's text changed. Nothing was touched, beyond leaving
    /// the stack ready to run from the top (as every reload does).
    Unchanged,
    /// Only whitespace, comments or layout changed. The program's recorded
    /// source text and positions were updated; nothing was recompiled, no
    /// state, closure or memo record was dropped. The stack is left ready to
    /// run from the top.
    Relocated,
    /// Only literal values changed. They were written into the running
    /// program; nothing was recompiled.
    Patched,
    /// The program was recompiled and swapped in with `transfer_state`.
    Recompiled,
}

impl ReloadOutcome {
    /// `unchanged`, `relocated`, `patched` or `recompiled`.
    pub fn label(self) -> &'static str {
        match self {
            ReloadOutcome::Unchanged => "unchanged",
            ReloadOutcome::Relocated => "relocated",
            ReloadOutcome::Patched => "patched",
            ReloadOutcome::Recompiled => "recompiled",
        }
    }
}

/// One source file of a program, and how its text changed.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// The file's index in the program's file table (0 is the entry file).
    pub file: FileId,
    /// Its display name (`config.ptl`; empty for an entry file with no table).
    pub name: String,
    /// Where it was read from, when it came from disk.
    pub origin: Option<std::path::PathBuf>,
    /// The text the file holds now.
    pub new_source: String,
    pub diff: FileDiff,
}

/// What changed across every source file of a loaded program: the input of
/// [`Env::apply_program_change`], made by [`Env::diff_program`].
#[derive(Debug, Clone)]
pub struct ProgramChange {
    /// The files whose text changed. Files that are byte-identical are not
    /// listed.
    pub files: Vec<FileChange>,
    /// Why no incremental path exists regardless of the diffs: a source file
    /// that can no longer be read, say.
    pub blocker: Option<String>,
}

impl ProgramChange {
    /// The change as one classification: the broadest of the files'.
    pub fn summary(&self) -> SourceChange {
        if let Some(why) = &self.blocker {
            return SourceChange::Full(why.clone());
        }
        let mut values = Vec::new();
        let mut constructs = Vec::new();
        for f in &self.files {
            match &f.diff.change {
                SourceChange::None => {}
                SourceChange::Values(v) => values.extend(v.iter().cloned()),
                SourceChange::Constructs(c) => constructs.extend(c.iter().cloned()),
                SourceChange::Full(why) => return SourceChange::Full(why.clone()),
            }
        }
        if !constructs.is_empty() {
            SourceChange::Constructs(constructs)
        } else if !values.is_empty() {
            SourceChange::Values(values)
        } else {
            SourceChange::None
        }
    }

    /// Whether [`Env::apply_program_change`] can take it without recompiling.
    pub fn is_incremental(&self) -> bool {
        self.blocker.is_none() && self.files.iter().all(|f| f.diff.change.is_incremental())
    }
}

/// What a reload did.
#[derive(Debug, Clone)]
pub struct ReloadReport {
    pub outcome: ReloadOutcome,
    /// The classification of the edit (see [`ProgramChange::summary`]).
    pub change: SourceChange,
    /// The files whose text changed, by display name.
    pub changed_files: Vec<String>,
    /// State entries kept and dropped, as [`Env::transfer_state`] counts
    /// them. An incremental reload drops none.
    pub state_preserved: usize,
    pub state_dropped: usize,
    /// When the edit was incremental by classification but the program was
    /// recompiled anyway, why.
    pub fallback: Option<String>,
}

/// Why [`Env::set_config_value`] did not set a value. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSetError {
    /// No such binding, path or source file.
    NotFound(String),
    /// The path is malformed or ambiguous, or the edit makes no sense there.
    Invalid(String),
    /// The value is settable, but not without recompiling: write the file
    /// and reload.
    NeedsReload(String),
}

impl std::fmt::Display for ConfigSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigSetError::NotFound(m) | ConfigSetError::Invalid(m) => f.write_str(m),
            ConfigSetError::NeedsReload(m) => {
                write!(f, "{m}; write the file and reload instead")
            }
        }
    }
}

impl std::error::Error for ConfigSetError {}

fn constant_of(lit: &Literal) -> ConstantValue {
    match lit {
        Literal::Nil => ConstantValue::Nil,
        Literal::Bool(b) => ConstantValue::Bool(*b),
        Literal::Int(n) => ConstantValue::Int(*n),
        Literal::Float(f) => ConstantValue::from_f64(*f),
        Literal::String(s) => ConstantValue::String(s.clone()),
    }
}

/// Everything one incremental apply will write, computed before any of it is
/// written so a change that turns out not to be applicable leaves the program
/// untouched.
#[derive(Default)]
struct Patch {
    spans: Vec<(TermId, SourceSpan)>,
    warnings: Vec<(usize, crate::diagnostic::Diagnostic)>,
    layout_deps: Vec<(usize, crate::diagnostic::LayoutDep)>,
    /// (file index, new text).
    sources: Vec<(usize, String)>,
    constants: Vec<(TermId, ConstantValue)>,
    /// (index into `Program.functions`, where that function is written now).
    fn_spans: Vec<(usize, SourceSpan)>,
}

impl Env {
    /// Compare every source file `program_id` was compiled from with what
    /// that file holds now. The entry file's current text is
    /// `new_entry_source`; module files are read from where they were loaded
    /// (disk, an [`override_file_source`](Self::override_file_source), or the
    /// in-memory registration).
    ///
    /// Read-only: this is the question "what kind of edit was that?", for a
    /// host that wants to report it or decide for itself.
    pub fn diff_program(&self, program_id: ProgramId, new_entry_source: &str) -> ProgramChange {
        let mut change = ProgramChange {
            files: Vec::new(),
            blocker: None,
        };
        let Some(program) = self.programs.get(&program_id) else {
            change.blocker = Some("Program not found".to_string());
            return change;
        };
        let mut consider = |file: usize, name: &str, origin: Option<&Path>, old: &str, new: String| {
            if old == new {
                return;
            }
            let diff = diff_source(old, &new, FileId(file as u16));
            change.files.push(FileChange {
                file: FileId(file as u16),
                name: name.to_string(),
                origin: origin.map(Path::to_path_buf),
                new_source: new,
                diff,
            });
        };
        if program.source_map.files.is_empty() {
            consider(0, "", None, &program.source, new_entry_source.to_string());
            return change;
        }
        let mut unreadable = None;
        for (i, f) in program.source_map.files.iter().enumerate() {
            let new = if i == 0 {
                Some(new_entry_source.to_string())
            } else {
                self.modules.current_source(&f.name, f.origin.as_deref())
            };
            match new {
                Some(new) => consider(i, &f.name, f.origin.as_deref(), &f.source, new),
                None => {
                    unreadable.get_or_insert_with(|| {
                        format!("source file `{}` can no longer be read", f.name)
                    });
                }
            }
        }
        change.blocker = unreadable;
        change
    }

    /// Bring the program `stack_id` runs up to date with its source files,
    /// doing only the work the edit calls for (see the module docs). The
    /// replacement for the recompile-and-`transfer_state` pair a hot-reloading
    /// host used to write:
    ///
    /// ```no_run
    /// # let mut env = petal::env::Env::new();
    /// # let pid = env.load_program("").unwrap();
    /// # let stack = env.create_stack(pid).unwrap();
    /// # let path = std::path::Path::new("game.ptl");
    /// let source = std::fs::read_to_string(path).unwrap();
    /// match env.reload_program(stack, &source, Some(path)) {
    ///     Ok(report) => println!("reload: {}", report.outcome.label()),
    ///     Err(e) => eprintln!("{e}"), // the old program keeps running
    /// }
    /// ```
    ///
    /// `new_entry_source` is the entry file's text and `origin` its path
    /// (imports resolve next to it), as for
    /// [`compile_program_diag`](Self::compile_program_diag). On a compile
    /// error the old program stays loaded and untouched.
    pub fn reload_program(
        &mut self,
        stack_id: StackKey,
        new_entry_source: &str,
        origin: Option<&Path>,
    ) -> Result<ReloadReport, crate::error::LoadError> {
        let program_id = self
            .stacks
            .get(&stack_id)
            .map(|s| s.program_id)
            .ok_or_else(|| {
                crate::error::LoadError::message(crate::error::Phase::Module, "Stack not found")
            })?;
        let change = self.diff_program(program_id, new_entry_source);
        let fallback = match self.apply_program_change(stack_id, &change) {
            Ok(report) => return Ok(report),
            Err(why) => why,
        };
        let summary = change.summary();
        let program = self.compile_program_diag(program_id, new_entry_source, origin)?;
        let result = self.transfer_state(stack_id, program).map_err(|e| {
            crate::error::LoadError::message(crate::error::Phase::Module, e)
        })?;
        Ok(ReloadReport {
            outcome: ReloadOutcome::Recompiled,
            // An edit that classified as incremental and still had to be
            // recompiled says why; one that was never incremental needs no
            // excuse.
            fallback: summary.is_incremental().then_some(fallback),
            change: summary,
            changed_files: change.files.iter().map(|f| f.name.clone()).collect(),
            state_preserved: result.state_preserved,
            state_dropped: result.state_dropped,
        })
    }

    /// Apply a change that needs no recompile ([`ProgramChange::is_incremental`]):
    /// move source positions, and write changed literal values into the
    /// running program.
    ///
    /// All or nothing. `Err` says why the change cannot be applied this way —
    /// it is not incremental, or the running program does not have the shape
    /// the diff assumes — and leaves the program and the stack exactly as
    /// they were; the caller recompiles (which is what
    /// [`reload_program`](Self::reload_program) does).
    pub fn apply_program_change(
        &mut self,
        stack_id: StackKey,
        change: &ProgramChange,
    ) -> Result<ReloadReport, String> {
        let program_id = self
            .stacks
            .get(&stack_id)
            .map(|s| s.program_id)
            .ok_or("Stack not found")?;
        if let Some(why) = &change.blocker {
            return Err(why.clone());
        }
        if let Some(f) = change.files.iter().find(|f| !f.diff.change.is_incremental()) {
            return Err(match &f.diff.change {
                SourceChange::Full(why) => why.clone(),
                _ => "the edit changes the program's structure".to_string(),
            });
        }
        let state_count = self.stacks.get(&stack_id).map_or(0, |s| s.state.len());
        let summary = change.summary();
        let changed_files: Vec<String> = change.files.iter().map(|f| f.name.clone()).collect();
        if change.files.is_empty() {
            self.rewind(stack_id);
            return Ok(ReloadReport {
                outcome: ReloadOutcome::Unchanged,
                change: summary,
                changed_files,
                state_preserved: state_count,
                state_dropped: 0,
                fallback: None,
            });
        }

        let program = self.programs.get(&program_id).ok_or("Program not found")?;
        let patch = plan_patch(program, change)?;
        let patched = !patch.constants.is_empty();

        // Nothing below can fail.
        let program = self.programs.get_mut(&program_id).expect("program found above");
        for (tid, span) in patch.spans {
            program.source_map.add(tid, span);
        }
        for (i, span) in patch.fn_spans {
            program.functions[i].span = Some(span);
        }
        // What `fn_info` / `fn_ast` cached was read from the old text.
        program.introspect.clear();
        for (i, warning) in patch.warnings {
            program.warnings[i] = warning;
        }
        for (i, dep) in patch.layout_deps {
            program.layout_deps[i] = dep;
        }
        for (file, text) in patch.sources {
            if file == 0 {
                program.source = text.clone();
            }
            if let Some(f) = program.source_map.files.get_mut(file) {
                f.source = text;
            }
        }
        let mut retarget: Vec<(TermId, ConstantId, ConstantId)> = Vec::new();
        for (tid, value) in patch.constants {
            let term = &mut program.terms[tid.0 as usize];
            let TermOp::Constant(old) = term.op else {
                unreachable!("plan_patch only lists constant terms");
            };
            if program.constants.is_slot(old) {
                program.constants.set_slot(old, value);
            } else {
                let slot = program.constants.alloc_slot(value);
                term.op = TermOp::Constant(slot);
                retarget.push((tid, old, slot));
            }
        }
        if !retarget.is_empty()
            && let Some((_, bc)) = self.bytecode.get_mut(&program_id)
            && !retarget_constants(bc, &retarget)
        {
            // The lowering is not the shape this expects of it: drop it, and
            // the next run lowers the patched program from scratch.
            self.bytecode.remove(&program_id);
        }

        if !patched {
            self.rewind(stack_id);
            return Ok(ReloadReport {
                outcome: ReloadOutcome::Relocated,
                change: summary,
                changed_files,
                state_preserved: state_count,
                state_dropped: 0,
                fallback: None,
            });
        }

        // The stack-side half of a reload, unchanged: whatever the last run
        // derived from the old values goes, `state` stays.
        let state_keys: HashSet<StateKey> = self.programs[&program_id]
            .state_terms()
            .map(|(k, _)| k)
            .collect();
        self.clear_closures();
        for (key, stack) in self.stacks.iter_mut() {
            if stack.program_id == program_id && *key != stack_id {
                // Another stack on the same program: its memo records and its
                // gate verdict are as stale as this one's.
                stack.memo.clear();
                stack.run_deps.force();
            }
        }
        let stack = self.stacks.get_mut(&stack_id).expect("stack found above");
        let result = crate::transfer_state::transfer_stack_state(stack, &state_keys);
        Ok(ReloadReport {
            outcome: ReloadOutcome::Patched,
            change: summary,
            changed_files,
            state_preserved: result.state_preserved,
            state_dropped: result.state_dropped,
            fallback: None,
        })
    }

    /// Leave the stack ready to run from the top, which is how every reload
    /// leaves it: a host may `run` straight after reloading, without a
    /// `reset_stack` in between, and must get a run. This rewinds execution
    /// only; `state`, closures, memo records and the frame gate's verdict are
    /// untouched.
    fn rewind(&mut self, stack_id: StackKey) {
        if let Some(stack) = self.stacks.get_mut(&stack_id) {
            stack.reset_execution();
        }
    }

    /// Set one value of a top-level binding *now*, without the file changing
    /// on disk: the live half of a drag. `path` is a binding path
    /// (`SPEED`, `POST.effects[2].amount`) and `value` what it should read.
    ///
    /// The edit is made to the text the running program holds for that file —
    /// by [`literal_edit::set_path`](crate::literal_edit::set_path), the same
    /// edit a host makes to the file when the drag ends — and applied only if
    /// it needs no recompile. So afterwards the program is exactly what a
    /// reload of that edited file would give, and when the host does write
    /// the file (with the same edit, or simply
    /// [`program_source`](Self::program_source)) the reload that follows
    /// finds nothing to do.
    ///
    /// `file` names the source file holding the binding, as a path. `None`
    /// searches the program's files and requires exactly one to bind the
    /// name at its top level.
    ///
    /// An error leaves everything as it was. [`ConfigSetError::NeedsReload`]
    /// means the value cannot be set this way: it has a different type or
    /// shape from the one written (`10` to `10.5`, a list that grew). Write
    /// the file and reload for those.
    pub fn set_config_value(
        &mut self,
        stack_id: StackKey,
        file: Option<&Path>,
        path: &str,
        value: &crate::static_value::StaticValue,
    ) -> Result<ReloadReport, ConfigSetError> {
        use crate::literal_edit::EditErrorKind;
        let program_id = self
            .stacks
            .get(&stack_id)
            .map(|s| s.program_id)
            .ok_or_else(|| ConfigSetError::NotFound("Stack not found".to_string()))?;
        let (index, name, origin, old) = self.config_file(program_id, file, path)?;
        let new = crate::literal_edit::set_path(&old, path, value).map_err(|e| match e.kind {
            EditErrorKind::NotFound => ConfigSetError::NotFound(e.message),
            EditErrorKind::Invalid | EditErrorKind::Parse => ConfigSetError::Invalid(e.message),
        })?;
        let files = if new == old {
            Vec::new()
        } else {
            let diff = diff_source(&old, &new, FileId(index as u16));
            if !diff.change.is_incremental() {
                return Err(ConfigSetError::NeedsReload(format!(
                    "setting `{path}` changes more than a value (a different type or shape)"
                )));
            }
            vec![FileChange {
                file: FileId(index as u16),
                name,
                origin,
                new_source: new,
                diff,
            }]
        };
        self.apply_program_change(
            stack_id,
            &ProgramChange {
                files,
                blocker: None,
            },
        )
        .map_err(ConfigSetError::NeedsReload)
    }

    /// The text the running program holds for one of its source files: what
    /// it was compiled from, plus every value set since with
    /// [`set_config_value`](Self::set_config_value). `file` is the file's
    /// path; `None` is the entry file.
    pub fn program_source(&self, program_id: ProgramId, file: Option<&Path>) -> Option<&str> {
        let program = self.programs.get(&program_id)?;
        match file {
            None => Some(&program.source),
            Some(path) => {
                let want = crate::module::canonical_path(path);
                program
                    .source_map
                    .files
                    .iter()
                    .find(|f| {
                        f.origin
                            .as_deref()
                            .is_some_and(|o| crate::module::canonical_path(o) == want)
                    })
                    .map(|f| f.source.as_str())
            }
        }
    }

    /// Which source file `path`'s binding lives in: (file index, display
    /// name, origin, current text).
    #[allow(clippy::type_complexity)]
    fn config_file(
        &self,
        program_id: ProgramId,
        file: Option<&Path>,
        path: &str,
    ) -> Result<(usize, String, Option<std::path::PathBuf>, String), ConfigSetError> {
        let program = self
            .programs
            .get(&program_id)
            .ok_or_else(|| ConfigSetError::NotFound("Program not found".to_string()))?;
        let (binding, _) = crate::rewrite::parse_binding_path(path).ok_or_else(|| {
            ConfigSetError::Invalid(format!(
                "`{path}` is not a binding path (name, then .field or [index] steps)"
            ))
        })?;
        if program.source_map.files.is_empty() {
            return Ok((0, String::new(), None, program.source.clone()));
        }
        if let Some(path) = file {
            let want = crate::module::canonical_path(path);
            return program
                .source_map
                .files
                .iter()
                .enumerate()
                .find(|(_, f)| {
                    f.origin
                        .as_deref()
                        .is_some_and(|o| crate::module::canonical_path(o) == want)
                })
                .map(|(i, f)| (i, f.name.clone(), f.origin.clone(), f.source.clone()))
                .ok_or_else(|| {
                    ConfigSetError::NotFound(format!(
                        "`{}` is not a source file of this program",
                        path.display()
                    ))
                });
        }
        let mut found = Vec::new();
        for (i, f) in program.source_map.files.iter().enumerate() {
            if binds_at_top_level(&f.source, &binding) {
                found.push(i);
            }
        }
        match found[..] {
            [i] => {
                let f = &program.source_map.files[i];
                Ok((i, f.name.clone(), f.origin.clone(), f.source.clone()))
            }
            [] => Err(ConfigSetError::NotFound(format!(
                "no source file of this program binds `{binding}` at its top level"
            ))),
            _ => Err(ConfigSetError::Invalid(format!(
                "`{binding}` is bound at the top level of more than one source file ({}); \
                 name the file",
                found
                    .iter()
                    .map(|&i| program.source_map.files[i].name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

/// Whether `source` binds `name` at its top level (`let name = …`, or a bare
/// `name = …`), the test [`crate::rewrite::find_binding`] makes. Asked of
/// every file of a program on each `set_config_value` that names no file, so
/// it avoids parsing where it can: a file that never spells the name, or
/// never writes it after `let`/`var` or before `=`, is ruled out from its text and
/// tokens alone.
fn binds_at_top_level(source: &str, name: &str) -> bool {
    use crate::lexer::{Lexer, Token};
    if !source.contains(name) {
        return false;
    }
    let mut lexer = Lexer::new(source);
    if lexer.tokenize().is_err() {
        return false;
    }
    let is_name = |t: &Token| matches!(t, Token::Ident(n) if n == name);
    let may_bind = lexer.tokens.windows(2).any(|w| {
        (matches!(w[0], Token::Let | Token::Var) && is_name(&w[1]))
            || (is_name(&w[0]) && matches!(w[1], Token::Assign))
    });
    if !may_bind {
        return false;
    }
    let mut parser = crate::parse::Parser::new(lexer.tokens.clone(), lexer.token_spans.clone());
    parser
        .parse_program()
        .is_ok_and(|stmts| crate::rewrite::find_binding(&stmts, name).is_some())
}

/// Work out everything an incremental change writes, or why it cannot be
/// applied to this program.
fn plan_patch(program: &Program, change: &ProgramChange) -> Result<Patch, String> {
    let mut patch = Patch::default();
    let by_file: HashMap<u16, &FileChange> = change.files.iter().map(|f| (f.file.0, f)).collect();
    for f in &change.files {
        patch.sources.push((f.file.0 as usize, f.new_source.clone()));
    }

    // The literals to find, as (file, span) -> the constant terms written at
    // that span.
    let mut wanted: HashMap<(u16, u32, u32), Vec<TermId>> = HashMap::new();
    for f in &change.files {
        if let SourceChange::Values(values) = &f.diff.change {
            for v in values {
                wanted
                    .entry((f.file.0, v.old_span.start.offset, v.old_span.end.offset))
                    .or_default();
            }
        }
    }

    for (tid, span) in program.source_map.iter() {
        let Some(f) = by_file.get(&span.file.0) else {
            continue;
        };
        if !wanted.is_empty()
            && let Some(terms) = wanted.get_mut(&(span.file.0, span.start.offset, span.end.offset))
            && matches!(program.terms[tid.0 as usize].op, TermOp::Constant(_))
        {
            terms.push(tid);
        }
        let moved = f.diff.map_span(*span).ok_or_else(|| {
            format!(
                "a source position in `{}` (line {}) has no counterpart in the new text",
                f.name, span.start.line
            )
        })?;
        if moved != *span {
            patch.spans.push((tid, moved));
        }
    }

    for (i, def) in program.functions.iter().enumerate() {
        let Some(span) = def.span else { continue };
        let Some(f) = by_file.get(&span.file.0) else {
            continue;
        };
        let moved = f.diff.map_span(span).ok_or_else(|| {
            format!(
                "a function in `{}` (line {}) has no counterpart in the new text",
                f.name, span.start.line
            )
        })?;
        if moved != span {
            patch.fn_spans.push((i, moved));
        }
    }

    for (i, w) in program.warnings.iter().enumerate() {
        let cited_file = w.cited.as_ref().map(|c| c.span.file.0);
        if !by_file.contains_key(&w.span.file.0)
            && !cited_file.is_some_and(|f| by_file.contains_key(&f))
        {
            continue;
        }
        // A diagnostic is about the code its span covers. One that covers a
        // changed literal could read differently after the edit, and only the
        // compiler can say how.
        if let Some(f) = by_file.get(&w.span.file.0)
            && let SourceChange::Values(values) = &f.diff.change
            && values.iter().any(|v| {
                w.span.start.offset <= v.old_span.start.offset
                    && v.old_span.end.offset <= w.span.end.offset
            })
        {
            return Err("a changed value is inside code the compiler warned about".to_string());
        }
        // Spans of a file that did not change stay where they are.
        let moved = w
            .relocated(|span| match by_file.get(&span.file.0) {
                Some(f) => f.diff.map_span(span),
                None => Some(span),
            })
            .ok_or("a diagnostic's position has no counterpart in the new text")?;
        if moved != *w {
            patch.warnings.push((i, moved));
        }
    }

    // A check that compared the layout of two places must read the same at
    // their new positions, or the edit added or removed a warning.
    for (i, dep) in program.layout_deps.iter().enumerate() {
        let Some(f) = by_file.get(&dep.prev.file.0) else {
            continue;
        };
        let moved = (|| {
            Some(crate::diagnostic::LayoutDep {
                prev: f.diff.map_span(dep.prev)?,
                next: f.diff.map_span(dep.next)?,
            })
        })()
        .ok_or("a position a layout check compared has no counterpart in the new text")?;
        if moved.reading() != dep.reading() {
            return Err(format!(
                "the layout change at line {} of `{}` changes what the compiler warns about",
                moved.next.start.line, f.name
            ));
        }
        if moved != *dep {
            patch.layout_deps.push((i, moved));
        }
    }

    for f in &change.files {
        let SourceChange::Values(values) = &f.diff.change else {
            continue;
        };
        for v in values {
            let terms = wanted
                .get_mut(&(f.file.0, v.old_span.start.offset, v.old_span.end.offset))
                .expect("every value change was registered above");
            // Terms are numbered in the order they were compiled, which for
            // the literals of one span (a color's components) is source order.
            terms.sort_by_key(|t| t.0);
            if terms.len() != v.span_count as usize {
                return Err(format!(
                    "the literal at line {} of `{}` is compiled to {} constant(s), expected {}",
                    v.old_span.start.line,
                    f.name,
                    terms.len(),
                    v.span_count
                ));
            }
            let tid = terms[v.ordinal as usize];
            let TermOp::Constant(cid) = program.terms[tid.0 as usize].op else {
                unreachable!("only constant terms were collected");
            };
            if *program.constants.get(cid) != constant_of(&v.old) {
                return Err(format!(
                    "the literal at line {} of `{}` does not hold the value its source gives",
                    v.old_span.start.line, f.name
                ));
            }
            patch.constants.push((tid, constant_of(&v.new)));
        }
    }
    Ok(patch)
}

/// Point each listed term's `LoadConst` at its new constant. Returns false
/// when an instruction lowered from one of the terms is not the `LoadConst`
/// of the old constant it should be (the caller then discards the lowering).
/// A term with no instruction at all is fine: its load was dead and removed.
fn retarget_constants(
    bc: &mut BytecodeProgram,
    retarget: &[(TermId, ConstantId, ConstantId)],
) -> bool {
    let by_term: HashMap<TermId, (ConstantId, ConstantId)> =
        retarget.iter().map(|&(t, old, new)| (t, (old, new))).collect();
    for f in std::iter::once(&mut bc.root).chain(bc.fns.iter_mut()) {
        for (inst, origin) in f.code.iter_mut().zip(&f.origins) {
            let Some((old, new)) = origin.and_then(|t| by_term.get(&t)) else {
                continue;
            };
            match inst {
                Inst::LoadConst { k, .. } if k == old => *k = *new,
                _ => return false,
            }
        }
    }
    true
}

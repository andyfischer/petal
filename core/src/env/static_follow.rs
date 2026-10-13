//! Following the diff: which static values a reload may keep.
//!
//! The static-value cache ([`crate::static_cache`]) keeps top-level values
//! that depend on no dynamic data. A reload used to empty all of them. This
//! module decides, from what the edit touched, which ones it cannot have
//! changed, and puts those back
//! (design: `docs/proposals/static-values.md`, section 4, stage 2; work
//! order: `docs/tasks/static-value-optimization.md`, S2).
//!
//! # The rule
//!
//! Everything is asked in terms of *where things are written*:
//!
//! 1. [`crate::source_diff`] says which top-level statements of each file an
//!    edit changed, added or removed, with their spans in the old and the
//!    new text. For a literal-only edit it says which statement each literal
//!    is in.
//! 2. A **declaration is changed** when a term that binds it is written
//!    inside one of those spans. Asking by position and not by the diff's
//!    `(kind, name)` is what makes a class with its methods, an overloaded
//!    function and a destructuring `let` come out right without a rule each.
//! 3. A slot is **kept** when all of these hold, and is rebuilt otherwise:
//!    - none of the terms that write its register is inside a changed span
//!      of the old program;
//!    - none of the declarations it depends on (the classifier's set,
//!      closed transitively, its own declaration included) is changed in
//!      the old program;
//!    - the new lowering has a slot for a declaration of the same
//!      module-qualified name, and that slot depends on the same *names*;
//!    - none of those is changed in the new program.
//!
//! The third condition is what makes this sound without hashing anything:
//! an edit that changes what a name resolves to (a new import, a shadowing
//! declaration, an added overload or method) changes the set of names, or
//! adds a statement that binds one of them.
//!
//! Everything is rebuilt, as before, when the diff cannot say what changed
//! (the new text was not classifiable, statements were reordered, a file
//! could not be read), when the program's file table changed (an import
//! added or removed), and when the reload was asked not to follow
//! ([`Env::set_static_follow`]), which is the behaviour this is tested
//! against.
//!
//! A literal-only edit recompiles nothing, so the old lowering and its
//! dependency sets are still the program's: slots are matched by position
//! and only the old-program conditions apply.
//!
//! # What it does not do
//!
//! - A value with no declaration of its own (a table written inside a call)
//!   has no name to be matched by across a recompile, and is rebuilt by
//!   every reload that recompiles. A literal-only edit keeps it.
//! - An enum's constant data is always rebuilt: the reload empties the
//!   stack's enum table and only the run can publish into it.
//! - Other stacks of the same program start empty, as before.
//! - A rebuild is put off ([`Env::set_static_debounce`]) run by run, not
//!   value by value: a cheap value that shares a run with a slow one and
//!   depends on the dragged literal waits with it. The literal's own slot
//!   does not wait: it is given the new value directly.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::reload::ProgramChange;
use super::*;
use crate::program::{StateKey, TermId};
use crate::source_diff::{ConstructEdit, SourceChange};
use crate::source_map::SourceSpan;
use crate::static_cache::{FollowInfo, Seed, StaticDebounce, StaticLayout};

/// A `state` cell that kept its value across a reload although something
/// its initializer read was changed by the edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleSeed {
    /// The cell (`grid`, `lib::cache`).
    pub name: String,
    /// The declaration its initializer reads that changed. `None` when it
    /// is the cell's own declaration that was edited.
    pub read: Option<String>,
}

/// What a reload did to the static values of the stack it reloaded: the
/// part of a [`ReloadReport`](super::ReloadReport) about constants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticReload {
    /// Slots that still hold the value they held before the reload.
    pub kept: usize,
    /// Slots the next run fills: the ones the edit reached, and new ones.
    pub rebuilt: usize,
    /// The declarations among `rebuilt`, by module-qualified name.
    pub rebuilt_names: Vec<String>,
    /// Declarations whose value the edit changed but which keep the old one
    /// until the drag pauses, because their last build was slow
    /// ([`Env::set_static_debounce`]).
    pub deferred: Vec<String>,
    /// `state` cells that kept their value although their initializer read
    /// something the edit changed.
    pub stale_seeds: Vec<StaleSeed>,
    /// Why every value was rebuilt, when the reload could not follow the
    /// edit.
    pub cleared: Option<String>,
}

impl StaticReload {
    /// Nothing to say: no static value and no stale seed.
    pub fn is_empty(&self) -> bool {
        self.kept == 0 && self.rebuilt == 0 && self.deferred.is_empty() && self.stale_seeds.is_empty()
    }

    /// The report as a host prints it, one line per thing worth saying:
    ///
    /// ```text
    /// constants: 3 rebuilt (TILES, GRID, lib::PALETTE), 41 kept
    /// stale seed: state grid was initialized from TILES, which changed; it keeps its value
    /// ```
    ///
    /// Empty when the reload changed no constant and left no stale seed.
    pub fn lines(&self) -> Vec<String> {
        const SHOWN: usize = 6;
        let list = |names: &[String]| {
            let mut s = names.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
            if names.len() > SHOWN {
                s.push_str(&format!(", and {} more", names.len() - SHOWN));
            }
            s
        };
        let mut out = Vec::new();
        if self.rebuilt > 0 || !self.deferred.is_empty() {
            let mut line = format!("constants: {} rebuilt", self.rebuilt);
            if !self.rebuilt_names.is_empty() {
                line.push_str(&format!(" ({})", list(&self.rebuilt_names)));
            }
            line.push_str(&format!(", {} kept", self.kept));
            if !self.deferred.is_empty() {
                line.push_str(&format!(
                    ", {} waiting for the drag to pause ({})",
                    self.deferred.len(),
                    list(&self.deferred)
                ));
            }
            if let Some(why) = &self.cleared {
                line.push_str(&format!(" (all of them: {why})"));
            }
            out.push(line);
        }
        for seed in self.stale_seeds.iter().take(SHOWN) {
            out.push(match &seed.read {
                Some(read) => format!(
                    "stale seed: state {} was initialized from {read}, which changed; it keeps its value",
                    seed.name
                ),
                None => format!(
                    "stale seed: the declaration of state {} changed; it keeps its value",
                    seed.name
                ),
            });
        }
        if self.stale_seeds.len() > SHOWN {
            out.push(format!("stale seeds: and {} more", self.stale_seeds.len() - SHOWN));
        }
        out
    }

    /// The report as JSON: `{kept, rebuilt, rebuilt_names, deferred,
    /// stale_seeds: [{name, read}], cleared}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kept": self.kept,
            "rebuilt": self.rebuilt,
            "rebuilt_names": self.rebuilt_names,
            "deferred": self.deferred,
            "stale_seeds": self.stale_seeds.iter().map(|s| serde_json::json!({
                "name": s.name,
                "read": s.read,
            })).collect::<Vec<_>>(),
            "cleared": self.cleared,
        })
    }
}

/// One slot of the table a reload is about to empty.
struct OldSlot {
    key: Option<String>,
    value: Option<Value>,
    /// The edit reached it, as far as the old program can tell.
    dirty: bool,
    /// The names it depends on, sorted; `None` when unknown.
    deps: Option<Vec<String>>,
    build: Duration,
    enum_data: bool,
}

/// What [`Env::begin_static_follow`] learned from the old program and the
/// stack's table, before the reload replaced one and emptied the other.
pub(super) struct Followed {
    layout: u64,
    slots: Vec<OldSlot>,
    timings: Vec<u64>,
    /// Names of the declarations the edit changed in the old program.
    changed: HashSet<String>,
    /// The old program's `state` declarations: name, key, what the
    /// initializer depends on.
    states: Vec<(String, StateKey, Vec<String>)>,
    /// Why nothing may be kept.
    cleared: Option<String>,
    /// The file table of the old program: (display name, origin).
    files: Vec<(String, Option<std::path::PathBuf>)>,
}

/// Whether `span` starts inside one of `changed` (spans of the same text).
fn inside(span: &SourceSpan, changed: &[SourceSpan]) -> bool {
    changed.iter().any(|c| {
        c.file == span.file
            && c.start.offset <= span.start.offset
            && span.start.offset < c.end.offset.max(c.start.offset + 1)
    })
}

/// The declarations of `follow` written inside `changed`, by index. A
/// declaration that cannot be placed (no term of it has a position) counts
/// as changed whenever its file has a change at all.
fn changed_decls(program: &Program, follow: &FollowInfo, changed: &[SourceSpan]) -> Vec<bool> {
    if changed.is_empty() {
        return vec![false; follow.decls.len()];
    }
    let files: HashSet<u16> = changed.iter().map(|c| c.file.0).collect();
    follow
        .decls
        .iter()
        .map(|d| {
            let mut placed = false;
            for t in &d.defs {
                if let Some(span) = program.source_map.get(*t).filter(|s| s.start.line > 0) {
                    placed = true;
                    if inside(span, changed) {
                        return true;
                    }
                }
            }
            !placed && files.contains(&d.file.0)
        })
        .collect()
}

/// Whether any term in `terms` is written inside `changed`; a slot none of
/// whose terms has a position is taken to be.
fn written_inside(program: &Program, terms: &[TermId], changed: &[SourceSpan]) -> bool {
    if changed.is_empty() {
        return false;
    }
    let mut placed = false;
    for t in terms {
        if let Some(span) = program.source_map.get(*t).filter(|s| s.start.line > 0) {
            placed = true;
            if inside(span, changed) {
                return true;
            }
        }
    }
    !placed
}

/// The spans an edit touched: in the old text, and in the new one. `Err`
/// says why the diff cannot be followed at all.
fn changed_spans(change: &ProgramChange) -> Result<(Vec<SourceSpan>, Vec<SourceSpan>), String> {
    if let Some(why) = &change.blocker {
        return Err(why.clone());
    }
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for f in &change.files {
        if f.diff.reordered {
            return Err(format!("top-level statements of `{}` were reordered", f.name));
        }
        match &f.diff.change {
            SourceChange::None => {}
            SourceChange::Values(values) => {
                // The statement is in both texts; the old side says which
                // declarations it holds, and they have the same names in
                // the new one.
                old.extend(values.iter().map(|v| v.construct_span));
            }
            SourceChange::Constructs(constructs) => {
                for c in constructs {
                    old.extend(c.old_span);
                    if c.edit != ConstructEdit::Removed {
                        new.push(c.span);
                    }
                }
            }
            SourceChange::Full(why) => return Err(why.clone()),
        }
    }
    Ok((old, new))
}

fn file_table(program: &Program) -> Vec<(String, Option<std::path::PathBuf>)> {
    program
        .source_map
        .files
        .iter()
        .map(|f| (f.name.clone(), f.origin.clone()))
        .collect()
}

fn names_of(follow: &FollowInfo, deps: &[u32]) -> Vec<String> {
    let mut out: Vec<String> = deps
        .iter()
        .filter_map(|d| follow.decls.get(*d as usize))
        .map(|d| d.name.clone())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

impl Env {
    /// Whether a reload keeps the static values an edit cannot have changed
    /// (the default), or rebuilds every one of them. Off is the behaviour
    /// the cache is checked against: both must leave the same values.
    pub fn set_static_follow(&mut self, on: bool) {
        self.static_follow = on;
    }

    /// See [`set_static_follow`](Self::set_static_follow).
    pub fn static_follow(&self) -> bool {
        self.static_follow
    }

    /// How a slow rebuild is put off while
    /// [`set_config_value`](Self::set_config_value) is driven continuously
    /// (a drag): a slot whose last build took at least
    /// [`threshold`](StaticDebounce::threshold) keeps its old value for
    /// every tick after the first, and is rebuilt by the first frame that
    /// starts [`quiet`](StaticDebounce::quiet) after the last tick, or by
    /// the next reload. `None` rebuilds on every tick. The default is
    /// [`StaticDebounce::default`].
    ///
    /// While a rebuild is put off the program runs with the old value of
    /// that one declaration and the new value of the literal everywhere
    /// else; the reload report names it
    /// ([`StaticReload::deferred`]).
    pub fn set_static_debounce(&mut self, debounce: Option<StaticDebounce>) {
        self.static_debounce = debounce;
    }

    /// See [`set_static_debounce`](Self::set_static_debounce).
    pub fn static_debounce(&self) -> Option<StaticDebounce> {
        self.static_debounce
    }

    /// Rebuild now what a drag put off on `stack_id`, without waiting for
    /// it to pause: the next run builds those values. Returns how many
    /// there were.
    pub fn flush_static_rebuilds(&mut self, stack_id: StackKey) -> usize {
        let Some(stack) = self.stacks.get_mut(&stack_id) else { return 0 };
        if stack.statics.deferred().is_empty() {
            return 0;
        }
        let layout = self
            .bytecode
            .get(&stack.program_id)
            .map(|(_, bc)| &bc.statics)
            .filter(|l| stack.statics.stamp().is_some_and(|(id, _)| id == l.id));
        let n = match layout {
            Some(layout) => stack.statics.flush_deferred(layout),
            // The lowering the values belong to is gone; so are they, at
            // the next run.
            None => {
                let n = stack.statics.deferred().len();
                stack.statics.clear();
                n
            }
        };
        stack.run_deps.force();
        n
    }

    /// Whether a drag left rebuilds waiting on `stack_id` and has paused
    /// long enough for them to happen.
    pub(super) fn static_rebuilds_due(&self, stack_id: StackKey) -> bool {
        let Some(stack) = self.stacks.get(&stack_id) else { return false };
        if stack.statics.deferred().is_empty() {
            return false;
        }
        let quiet = self.static_debounce.map_or(Duration::ZERO, |d| d.quiet);
        stack.statics.drag_paused(quiet)
    }

    /// The first half of following an edit: read, from the program and the
    /// table as they are *before* the reload touches either, what each slot
    /// holds and whether the edit reached it. `None` when there is nothing
    /// to follow (the cache is off, or the program has not been lowered
    /// with it).
    pub(super) fn begin_static_follow(
        &self,
        stack_id: StackKey,
        change: &ProgramChange,
    ) -> Option<Followed> {
        if !self.static_follow {
            return None;
        }
        let stack = self.stacks.get(&stack_id)?;
        let program = self.programs.get(&stack.program_id)?;
        let (_, bc) = self.bytecode.get(&stack.program_id)?;
        let layout = &bc.statics;
        if layout.id == 0 {
            return None;
        }
        let follow = &*layout.follow;
        let table = &stack.statics;
        // A table filled under another lowering holds nothing of this one.
        let current = table.stamp().is_some_and(|(id, _)| id == layout.id);

        let (spans, cleared) = match changed_spans(change) {
            Ok((old, _)) => (old, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        let changed = changed_decls(program, follow, &spans);
        // A value a drag left stale, and one rebuilt beside it that may
        // have read it, are rebuilt by whatever comes next.
        let deferred: HashSet<u32> =
            table.deferred().iter().chain(table.redo()).copied().collect();
        let slots = layout
            .slots
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let i = i as u32;
                let dirty = cleared.is_some()
                    || info.enum_data
                    || deferred.contains(&i)
                    || written_inside(program, &info.origins, &spans)
                    || info
                        .deps
                        .as_deref()
                        .is_none_or(|deps| deps.iter().any(|d| changed.get(*d as usize) != Some(&false)));
                OldSlot {
                    key: info.key.clone(),
                    value: if current { table.get(i) } else { None },
                    dirty,
                    deps: info.deps.as_deref().map(|d| names_of(follow, d)),
                    build: table.build_time(layout, i),
                    enum_data: info.enum_data,
                }
            })
            .collect();
        Some(Followed {
            layout: layout.id,
            slots,
            timings: table.timings(),
            changed: follow
                .decls
                .iter()
                .zip(&changed)
                .filter(|(_, c)| **c)
                .map(|(d, _)| d.name.clone())
                .collect(),
            states: follow
                .decls
                .iter()
                .filter_map(|d| {
                    let (key, deps) = d.state.as_ref()?;
                    Some((d.name.clone(), *key, names_of(follow, deps)))
                })
                .collect(),
            cleared,
            files: file_table(program),
        })
    }

    /// The second half: the program is the new one (or the patched one) and
    /// the stack's table has been emptied. Lower it, put back what may be
    /// kept, and say what happened. `live` is a tick of
    /// [`set_config_value`](Self::set_config_value), which is when a slow
    /// rebuild may be put off.
    pub(super) fn finish_static_follow(
        &mut self,
        stack_id: StackKey,
        change: &ProgramChange,
        old: Followed,
        live: bool,
    ) -> Option<StaticReload> {
        let pid = self.stacks.get(&stack_id)?.program_id;
        // The lowering the next run uses. A structural reload has none yet;
        // making it here moves that cost from the first frame to the reload
        // and is what lets the report say what was kept.
        if self.ensure_bytecode(pid).is_err() {
            return None;
        }
        let debounce = self.static_debounce.filter(|_| live);
        let program = self.programs.get(&pid)?;
        let (_, bc) = self.bytecode.get(&pid)?;
        let layout: &StaticLayout = &bc.statics;
        let stack = self.stacks.get_mut(&stack_id)?;
        let heap = &mut self.contexts.get_mut(&stack.context)?.heap;
        if layout.id == 0 {
            // The cache went off between the two halves.
            return None;
        }
        let follow = &*layout.follow;
        let same_lowering = layout.id == old.layout;
        let mut cleared = old.cleared.clone();
        if cleared.is_none() && !same_lowering && file_table(program) != old.files {
            cleared = Some("the program's source files changed".to_string());
        }
        let new_spans = match changed_spans(change) {
            Ok((_, new)) => new,
            Err(_) => Vec::new(),
        };
        let changed_new = changed_decls(program, follow, &new_spans);
        let changed_names: HashSet<&str> = old
            .changed
            .iter()
            .map(String::as_str)
            .chain(
                follow
                    .decls
                    .iter()
                    .zip(&changed_new)
                    .filter(|(_, c)| **c)
                    .map(|(d, _)| d.name.as_str()),
            )
            .collect();

        // A drag continues when its last tick was recent; the first tick of
        // one rebuilds at once.
        let continues = debounce.is_some_and(|d| stack.statics.note_live_edit(d.quiet));
        let by_key: HashMap<&str, usize> = old
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((s.key.as_deref()?, i)))
            .collect();

        let mut report = StaticReload { cleared: cleared.clone(), ..Default::default() };
        let mut seeds: Vec<Seed> = Vec::with_capacity(layout.slots.len());
        for (i, info) in layout.slots.iter().enumerate() {
            let before = if same_lowering {
                old.slots.get(i)
            } else {
                info.key.as_deref().and_then(|k| by_key.get(k)).map(|&j| &old.slots[j])
            };
            let seed = match before {
                Some(OldSlot { value: Some(v), enum_data: false, dirty, deps, build, .. })
                    if !info.enum_data =>
                {
                    let clean = cleared.is_none()
                        && !*dirty
                        && (same_lowering || {
                            // The new program must agree: the same names,
                            // none of them changed there, and the value not
                            // written inside anything the edit added.
                            let new_deps = info.deps.as_deref().map(|d| names_of(follow, d));
                            new_deps.is_some()
                                && new_deps == *deps
                                && !info
                                    .deps
                                    .as_deref()
                                    .unwrap_or(&[])
                                    .iter()
                                    .any(|d| changed_new.get(*d as usize) != Some(&false))
                                && !written_inside(program, &info.origins, &new_spans)
                        });
                    // A slot that holds one literal, and nothing else, holds
                    // what the literal says now: no run has to say so. This
                    // is the dragged value itself, which must not wait with
                    // what is built from it.
                    let literal = match info.origins[..] {
                        [t] if cleared.is_none() => match program.get_term(t).op {
                            crate::program::TermOp::Constant(c) => {
                                Some(crate::backend::ops::constant_to_value(program, heap, c))
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    if clean {
                        Seed::Kept(*v)
                    } else if let Some(now) = literal {
                        Seed::Kept(now)
                    } else if cleared.is_none()
                        && same_lowering
                        && continues
                        && debounce.is_some_and(|d| *build >= d.threshold)
                    {
                        Seed::Deferred(*v)
                    } else {
                        Seed::Rebuild(*v)
                    }
                }
                _ => Seed::Empty,
            };
            match seed {
                Seed::Kept(_) => report.kept += 1,
                Seed::Deferred(_) => {
                    if let Some(name) = info.declaration() {
                        report.deferred.push(name.to_string());
                    }
                }
                Seed::Rebuild(_) | Seed::Empty => {
                    report.rebuilt += 1;
                    if let Some(name) = info.declaration()
                        && !report.rebuilt_names.iter().any(|n| n == name)
                    {
                        report.rebuilt_names.push(name.to_string());
                    }
                }
            }
            seeds.push(seed);
        }
        stack.statics.install(
            layout,
            program.constants.revision(),
            &seeds,
            same_lowering.then_some(old.timings),
        );
        stack.statics.set_redo(
            seeds
                .iter()
                .enumerate()
                .filter(|(_, s)| matches!(s, Seed::Rebuild(_) | Seed::Empty))
                .map(|(i, _)| i as u32)
                .collect(),
        );

        // `state` cells that kept a value whose initializer read something
        // the edit changed.
        if cleared.is_none() {
            let held: HashSet<StateKey> = stack.state.keys().map(|k| k.base).collect();
            let old_states: HashMap<&str, &Vec<String>> =
                old.states.iter().map(|(n, _, d)| (n.as_str(), d)).collect();
            for d in &follow.decls {
                let Some((key, deps)) = &d.state else { continue };
                if !held.contains(key) {
                    continue;
                }
                let names = names_of(follow, deps);
                let read = if changed_names.contains(d.name.as_str()) {
                    Some(None)
                } else if let Some(hit) = names.iter().find(|n| changed_names.contains(n.as_str())) {
                    Some(Some(hit.clone()))
                } else {
                    // It reads something else now: a name that resolves
                    // differently, a dependency that appeared.
                    old_states.get(d.name.as_str()).and_then(|before| {
                        names
                            .iter()
                            .find(|n| !before.contains(n))
                            .or_else(|| before.iter().find(|n| !names.contains(n)))
                            .map(|n| Some(n.clone()))
                    })
                };
                if let Some(read) = read {
                    report.stale_seeds.push(StaleSeed { name: d.name.clone(), read });
                }
            }
        }
        Some(report)
    }
}

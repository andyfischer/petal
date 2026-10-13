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
//! 4. A slot that passes is still rebuilt when a value its segment read
//!    (`StaticRun::reads`) or one the same store holds is: it was built
//!    from that very object, and beside a rebuilt one it would hold the
//!    object the rebuild replaced. A rebuilt number, boolean or `nil` does
//!    not count, since it has no identity to share.
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
//!   is matched across a recompile by its statement's text and shape
//!   (`static_cache::unnamed_keys`); one whose statement is written twice
//!   has no such key and is rebuilt by every reload that recompiles. A
//!   literal-only edit keeps it.
//! - An enum's constant data is kept or rebuilt whole, never compared: the
//!   run publishes the record it builds, so a kept record is published
//!   again by the reload and a rebuilt one replaces it.
//! - Other stacks of the same program start empty, as before.
//! - A rebuild is put off ([`Env::set_static_debounce`]) segment by
//!   segment, which is value by value for everything something outside its
//!   own statement reads: a cheap value beside a slow one does not wait
//!   with it. What waits together is what one store holds: a slow table
//!   nothing else reads and the value built from it, or the values one
//!   statement binds. A slot that holds one literal is given the new value
//!   directly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use super::reload::ProgramChange;
use super::*;
use crate::program::{StateKey, TermId};
use crate::source_diff::{ConstructEdit, SourceChange};
use crate::source_map::SourceSpan;
use crate::static_cache::{FollowDecl, FollowInfo, Seed, StaticDebounce, StaticLayout};

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
    /// `state` cells used as a cache — filled behind a test of the cell
    /// itself or of a key remembered beside it, at the top level or inside a
    /// function — that hold a value while something the cached value is
    /// built from was changed by the edit. `read` is that declaration. The
    /// cell keeps what it built until its test passes again. A cell that is
    /// already a stale seed is not listed twice.
    pub stale_caches: Vec<StaleSeed>,
    /// Why every value was rebuilt, when the reload could not follow the
    /// edit.
    pub cleared: Option<String>,
}

impl StaticReload {
    /// Nothing to say: no static value and no stale seed.
    pub fn is_empty(&self) -> bool {
        self.kept == 0
            && self.rebuilt == 0
            && self.deferred.is_empty()
            && self.stale_seeds.is_empty()
            && self.stale_caches.is_empty()
    }

    /// The report as a host prints it, one line per thing worth saying:
    ///
    /// ```text
    /// constants: 3 rebuilt (TILES, GRID, lib::PALETTE), 41 kept
    /// stale seed: state grid was initialized from TILES, which changed; it keeps its value
    /// stale cache: state DEFS is filled from GENERIC, which changed, behind a test; it keeps what it built
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
        for cache in self.stale_caches.iter().take(SHOWN) {
            out.push(format!(
                "stale cache: state {} is filled from {}, which changed, behind a test; it keeps what it built",
                cache.name,
                cache.read.as_deref().unwrap_or("code")
            ));
        }
        if self.stale_caches.len() > SHOWN {
            out.push(format!("stale caches: and {} more", self.stale_caches.len() - SHOWN));
        }
        out
    }

    /// The report as JSON: `{kept, rebuilt, rebuilt_names, deferred,
    /// stale_seeds: [{name, read}], stale_caches: [{name, read}], cleared}`.
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
            "stale_caches": self.stale_caches.iter().map(|s| serde_json::json!({
                "name": s.name,
                "read": s.read,
            })).collect::<Vec<_>>(),
            "cleared": self.cleared,
        })
    }
}

/// What [`Env::begin_static_follow`] learned from the old program and the
/// stack's table, before the reload replaced one and emptied the other.
pub(super) struct Followed {
    /// The lowering the table was for, and what it said about itself.
    layout: u64,
    follow: Arc<FollowInfo>,
    /// Per slot: the value held, if any.
    values: Vec<Option<Value>>,
    /// Per slot: the edit reached it, as far as the old program can tell.
    dirty: Vec<bool>,
    /// Per slot: how long the runs that rebuild it took last time.
    build: Vec<Duration>,
    timings: Vec<u64>,
    /// Per run: what the segment observed when it last executed (see
    /// `static_cache`, "Observations"). Empty when the table was stale.
    logs: Vec<Option<crate::static_cache::ObservationLog>>,
    /// Per run: the edit reached something a statement of the segment
    /// depends on, so what the segment observed may no longer hold.
    log_dirty: Vec<bool>,
    /// Per declaration of the old program: the edit changed it.
    changed: Vec<bool>,
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
            if !files.contains(&d.file.0) {
                return false;
            }
            let mut placed = false;
            for t in &d.defs {
                if let Some(span) = program.source_map.get(*t).filter(|s| s.start.line > 0) {
                    placed = true;
                    if inside(span, changed) {
                        return true;
                    }
                }
            }
            !placed
        })
        .collect()
}

/// Whether any term in `terms` is written inside `changed`. A term with no
/// position is taken to be: the layout leaves such a term out when a
/// declaration, which has one, stands for it.
fn written_inside(program: &Program, terms: &[TermId], changed: &[SourceSpan]) -> bool {
    if changed.is_empty() {
        return false;
    }
    terms.iter().any(|t| {
        match program.source_map.get(*t).filter(|s| s.start.line > 0) {
            Some(span) => inside(span, changed),
            None => true,
        }
    })
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

/// The names behind a dependency set, sorted.
fn names_of<'a>(follow: &'a FollowInfo, deps: &[u32]) -> Vec<&'a str> {
    let mut out: Vec<&str> = deps
        .iter()
        .filter_map(|d| follow.decls.get(*d as usize))
        .map(|d| d.name.as_str())
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
        let follow = &layout.follow;
        let table = &stack.statics;
        // A table filled under another lowering holds nothing of this one.
        // Nor does one filled under other constants: a literal patched
        // through another stack of the program (or a fork of this one)
        // moved the revision and left this table as it was, and only this
        // stack's next run would have emptied it. Nothing here knows what
        // that edit reached, so nothing of the table is kept.
        let current = table.stamp() == Some((layout.id, program.constants.revision()));

        let (spans, cleared) = match changed_spans(change) {
            Ok((old, _)) => (old, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        let changed = changed_decls(program, follow, &spans);
        let any_changed = changed.contains(&true);
        // A value a drag left stale, and one rebuilt beside it that may
        // have read it, are rebuilt by whatever comes next.
        let stale: HashSet<u32> = table.deferred().iter().chain(table.redo()).copied().collect();
        let n = layout.slots.len();
        let (mut values, mut dirty, mut build) =
            (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for (i, info) in follow.slots.iter().enumerate().take(n) {
            let i = i as u32;
            dirty.push(
                cleared.is_some()
                    || stale.contains(&i)
                    || written_inside(program, &info.origins, &spans)
                    || info.deps.as_deref().is_none_or(|deps| {
                        any_changed && deps.iter().any(|d| changed.get(*d as usize) != Some(&false))
                    }),
            );
            values.push(if current { table.get(i) } else { None });
            build.push(table.build_time(layout, i));
        }
        Some(Followed {
            layout: layout.id,
            follow: follow.clone(),
            values,
            dirty,
            build,
            timings: table.timings(),
            logs: if current { table.logs() } else { Vec::new() },
            log_dirty: follow
                .runs
                .iter()
                .map(|run| {
                    cleared.is_some()
                        || (!spans.is_empty()
                            && (written_inside(program, &run.origins, &spans)
                                || run.deps.as_deref().is_none_or(|deps| {
                                    any_changed && deps.iter().any(|d| changed.get(*d as usize) != Some(&false))
                                })))
                })
                .collect(),
            changed,
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
        // What the edit changed in the new program. A lowering that is still
        // the old one has the old program's declarations.
        let changed_new = if same_lowering {
            Vec::new()
        } else {
            let spans = changed_spans(change).map(|(_, new)| new).unwrap_or_default();
            changed_decls(program, follow, &spans)
        };
        let new_spans = if same_lowering {
            Vec::new()
        } else {
            changed_spans(change).map(|(_, new)| new).unwrap_or_default()
        };

        // A drag continues when its last tick was recent; the first tick of
        // one rebuilds at once.
        let continues = debounce.is_some_and(|d| stack.statics.note_live_edit(d.quiet));
        let by_key: HashMap<&str, usize> = if same_lowering {
            HashMap::new()
        } else {
            old.follow
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| Some((s.key.as_deref()?, i)))
                .collect()
        };

        let mut report = StaticReload { cleared: cleared.clone(), ..Default::default() };
        let mut seeds: Vec<Seed> = Vec::with_capacity(layout.slots.len());
        // Slots given a literal's new value without a run: what their
        // segment observed is the old one.
        let mut given: HashSet<usize> = HashSet::new();
        for (i, info) in follow.slots.iter().enumerate().take(layout.slots.len()) {
            let before = if same_lowering {
                Some(i)
            } else {
                info.key.as_deref().and_then(|k| by_key.get(k)).copied()
            };
            // A value built inside a function body is rebuilt by every
            // reload (`static_cache`, the `in_fn` module).
            let before = before.filter(|_| !layout.in_function(i as u32));
            let held = before.and_then(|j| Some((j, (*old.values.get(j)?)?)));
            let seed = match held {
                None => Seed::Empty,
                Some((j, v)) => {
                    let clean = cleared.is_none()
                        && !old.dirty[j]
                        && (same_lowering || {
                            // The new program must agree: the same names,
                            // none of them changed there, and the value not
                            // written inside anything the edit added.
                            match (&info.deps, &old.follow.slots[j].deps) {
                                (Some(new), Some(was)) => {
                                    names_of(follow, new) == names_of(&old.follow, was)
                                        && !new.iter().any(|d| changed_new.get(*d as usize) != Some(&false))
                                        && !written_inside(program, &info.origins, &new_spans)
                                }
                                _ => false,
                            }
                        });
                    // A slot that holds one literal, and nothing else, holds
                    // what the literal says now: no run has to say so. This
                    // is the dragged value itself, which must not wait with
                    // what is built from it.
                    let literal = info.literal.filter(|_| cleared.is_none()).and_then(|t| {
                        match program.get_term(t).op {
                            crate::program::TermOp::Constant(c) => {
                                Some(crate::backend::ops::constant_to_value(program, heap, c))
                            }
                            _ => None,
                        }
                    });
                    if clean {
                        Seed::Kept(v)
                    } else if let Some(now) = literal {
                        given.insert(i);
                        Seed::Kept(now)
                    } else if cleared.is_none()
                        && same_lowering
                        && continues
                        && info.enum_tag.is_none()
                        && debounce.is_some_and(|d| old.build[j] >= d.threshold)
                    {
                        Seed::Deferred(v)
                    } else if info.enum_tag.is_some() {
                        // The run publishes the record it builds; a record
                        // kept in its place would not be the published one.
                        Seed::Empty
                    } else {
                        Seed::Rebuild(v)
                    }
                }
            };
            seeds.push(seed);
        }
        // A value is kept only together with what its segment read and with
        // what the same store holds. Its segment built it from those very
        // objects: beside a rebuilt one it would hold the object the rebuild
        // replaced, where a rebuild of everything leaves one object under
        // both names (`freeze` through one of them is how a program could
        // tell). A rebuilt scalar is the exception: it has no identity, and
        // whether its new value reaches the reader is the dependency set's
        // question, answered above.
        let scalar = |v: Value| {
            matches!(v, Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Symbol(_))
        };
        loop {
            let mut moved = false;
            for run in layout.runs.iter().filter(|r| !r.is_group()) {
                let at = |s: &u32| seeds.get(*s as usize).copied();
                let beside = run.stores.iter().filter_map(at).any(|s| matches!(s, Seed::Empty | Seed::Rebuild(_)));
                let from = run.reads.iter().filter_map(at).any(|s| match s {
                    Seed::Empty => true,
                    Seed::Rebuild(v) => !scalar(v),
                    Seed::Kept(_) | Seed::Deferred(_) => false,
                });
                if !beside && !from {
                    continue;
                }
                for &s in &run.stores {
                    let Some(seed) = seeds.get_mut(s as usize) else { continue };
                    let held = match *seed {
                        Seed::Kept(v) => v,
                        // The segment runs for its other values; nothing is
                        // put off by leaving this one.
                        Seed::Deferred(v) if beside => v,
                        _ => continue,
                    };
                    let enum_data = follow.slots.get(s as usize).is_some_and(|i| i.enum_tag.is_some());
                    *seed = if enum_data { Seed::Empty } else { Seed::Rebuild(held) };
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        // The report counts top-level values, which are the first slots.
        for (seed, info) in seeds.iter().zip(&follow.slots).take(layout.root_slots) {
            match *seed {
                Seed::Kept(v) => {
                    report.kept += 1;
                    // An enum's data lives in the stack's enum table, which
                    // the reload emptied and only the skipped run would fill.
                    if let (Some(tag), Value::Map(record)) = (&info.enum_tag, v) {
                        stack.enums.sync(&bc.enum_layout, heap);
                        let tag = heap.intern_str(tag);
                        stack.enums.publish(heap, tag, record);
                    }
                }
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
        }
        // What a segment observed is what it would observe now when the
        // lowering (and so every term) is the one it ran under, it keeps
        // every value it stored and read, and the edit reached nothing any
        // of its statements depends on. A segment whose rebuild is put off
        // keeps its log as it keeps its values: both are the old ones.
        let holds: Vec<bool> = layout
            .runs
            .iter()
            .enumerate()
            .map(|(r, run)| {
                let at = |s: &u32| (seeds.get(*s as usize).copied(), given.contains(&(*s as usize)));
                if run.is_group() || !same_lowering {
                    false
                } else if run.stores.iter().all(|s| matches!(at(s), (Some(Seed::Deferred(_)), false))) {
                    true
                } else {
                    old.log_dirty.get(r) == Some(&false)
                        && run
                            .stores
                            .iter()
                            .chain(&run.reads)
                            .all(|s| matches!(at(s), (Some(Seed::Kept(_) | Seed::Deferred(_)), false)))
                }
            })
            .collect();
        stack.statics.install(
            layout,
            program.constants.revision(),
            &seeds,
            same_lowering.then_some(old.timings),
            same_lowering.then_some((old.logs, &holds[..])),
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
            // By name, for a program that was recompiled: what each cell
            // read before, and what the edit changed on either side.
            let before: HashMap<&str, &FollowDecl> = if same_lowering {
                HashMap::new()
            } else {
                old.follow.decls.iter().filter(|d| d.state.is_some()).map(|d| (d.name.as_str(), d)).collect()
            };
            let changed_names: HashSet<&str> = if same_lowering {
                HashSet::new()
            } else {
                let was = old.follow.decls.iter().zip(&old.changed);
                let now = follow.decls.iter().zip(&changed_new);
                was.chain(now).filter(|(_, c)| **c).map(|(d, _)| d.name.as_str()).collect()
            };
            for (i, d) in follow.decls.iter().enumerate() {
                let Some((key, deps)) = &d.state else { continue };
                if !held.contains(key) {
                    continue;
                }
                let read: Option<Option<String>> = if same_lowering {
                    if old.changed.get(i) == Some(&true) {
                        Some(None)
                    } else {
                        deps.iter()
                            .find(|x| old.changed.get(**x as usize) == Some(&true))
                            .and_then(|x| follow.decls.get(*x as usize))
                            .map(|x| Some(x.name.clone()))
                    }
                } else if changed_names.contains(d.name.as_str()) {
                    Some(None)
                } else {
                    let names = names_of(follow, deps);
                    match names.iter().find(|n| changed_names.contains(**n)) {
                        Some(hit) => Some(Some(hit.to_string())),
                        // It reads something else now: a name that resolves
                        // differently, a dependency that appeared.
                        None => before.get(d.name.as_str()).and_then(|was| {
                            let was = names_of(&old.follow, &was.state.as_ref()?.1);
                            names
                                .iter()
                                .find(|n| !was.contains(n))
                                .or_else(|| was.iter().find(|n| !names.contains(n)))
                                .map(|n| Some(n.to_string()))
                        }),
                    }
                };
                if let Some(read) = read {
                    report.stale_seeds.push(StaleSeed { name: d.name.clone(), read });
                    continue;
                }
                // A cache: what the values written into it behind a guard
                // are built from, against what the edit changed.
                let Some(cache) = &d.cache else { continue };
                // A cache that holds nothing yet has nothing stale in it.
                let empty = stack.state.iter().filter(|(k, _)| k.base == *key).all(|(_, v)| match v {
                    Value::Cell(id) => matches!(heap.cell_read(*id), Value::Nil),
                    other => matches!(other, Value::Nil),
                });
                if empty {
                    continue;
                }
                let hit = if same_lowering {
                    cache
                        .iter()
                        .find(|x| old.changed.get(**x as usize) == Some(&true))
                        .and_then(|x| follow.decls.get(*x as usize))
                        .map(|x| x.name.clone())
                } else {
                    names_of(follow, cache)
                        .into_iter()
                        .find(|n| changed_names.contains(*n))
                        .map(str::to_string)
                };
                if let Some(hit) = hit {
                    report.stale_caches.push(StaleSeed { name: d.name.clone(), read: Some(hit) });
                }
            }
        }
        Some(report)
    }
}

//! The static-value cache: top-level values that depend on no dynamic data
//! are built once and kept on the stack between runs.
//!
//! A host re-runs the whole script every frame, so without this every
//! top-level table is rebuilt every frame. [`crate::static_classify`] says
//! which top-level terms are *static*: a function of the source text alone.
//! Under [`OptFlags::static_cache`](crate::backend::OptFlags) the root code is
//! lowered with extra instructions around the static ones, and a stack keeps
//! what they computed:
//!
//! ```text
//!       static_skip run 0 [r7 = s0, r9 = s1] -> L     ; the group: every value kept? load, jump
//!       static_skip run 1 [r7 = s0] -> A              ; one value: kept? load, jump
//!       ...the instructions that build r7, exactly as they lower without the cache...
//!       static_store [s0 = r7]
//!   A:  static_skip run 2 [r9 = s1] -> L
//!       ...the instructions that build r9...
//!       static_store [s1 = r9]
//!   L:  ...
//! ```
//!
//! **No instruction is moved or removed.** With the flag off the code is the
//! same code without those instructions, which is the oracle the cache is
//! held to (design: `docs/proposals/static-values.md`, section 3; work order:
//! `docs/tasks/static-value-optimization.md`, S1 and "A store per value").
//!
//! # Stretches, segments and slots
//!
//! The pass works on the lowered root code, after every other pass, because
//! that is where it can see what it must not break:
//!
//! - A **candidate** instruction belongs to a top-level statement whose term
//!   is static *data* and is in the [`StaticPlan`] (not a function, not a
//!   value that reaches a write), and is of a kind that is safe to leave out:
//!   no `state`, no cell, no `return`, no call of a native that is not
//!   constant. A statement with one instruction that fails this is out as a
//!   whole. So is one that reads a register some dynamic instruction writes
//!   (the classifier says that cannot happen; lowering artifacts are checked
//!   here rather than trusted).
//! - A **stretch** is a maximal sequence of candidate instructions. Lowering
//!   can put one statement's instruction in the middle of another's (a
//!   constant materialized where a loop first uses it), so a statement may
//!   be several stretches; one that control flow enters or leaves anywhere
//!   but at its two ends is not a stretch, and the statement that caused it
//!   is out. A function declaration ends a stretch: its `MakeClosure` runs
//!   on every run, where it stands.
//! - A stretch is cut into **segments**, one behind each value that
//!   something outside its own segment reads (`cut`). A segment has a
//!   `StaticSkip` in front and a `StaticStore` behind, so each such value is
//!   built, stored and rebuilt alone. A cut is never placed inside a
//!   top-level statement or across a jump, so a loop and its result are one
//!   segment. A stretch of several segments has one more `StaticSkip` in
//!   front, the **group**, which is the only check a frame makes when every
//!   value of the stretch is kept.
//! - A register gets a **slot** when anything outside the segment that
//!   writes it reads it: dynamic code, a closure capture, a static statement
//!   that is not skippable (a top-level `assert`), the program's result,
//!   and another segment. The last is what keeps sharing intact: `let A =
//!   [P, 1]` and `let B = [P, 2]` hold one `P`, so when `B` is rebuilt it
//!   must be handed the `P` that the kept `A` holds, not a new one. A value
//!   only its own segment reads (the records of a table literal, a list
//!   mapped into the value that is read) has no slot and is built with it.
//! - A stretch whose values nothing reads always executes: an unused
//!   `let BAD = XS[7]` still raises at its line, on every run. So does a
//!   stretch that allocates nothing ([`MIN_WEIGHT`]): loading two numbers
//!   costs less than testing for them. What follows the last value of a
//!   stretch and is read by nothing belongs to the last segment.
//!
//! - **Enum constant data.** `enum Kind  park(code: 2) end` publishes one
//!   record per variant into the stack's enum table, by a call the
//!   declaration makes on every run. Where the data is static that call is a
//!   candidate like any other, and the record it publishes gets a slot
//!   whoever reads it, stored by the segment that publishes it: a frame that
//!   skips the segment leaves the published record where the first run put
//!   it. The enum table and this one are emptied together, so a kept slot
//!   always has its record published.
//!
//! A segment that raises never reaches its `StaticStore`, so its value is
//! not kept and it raises again on the next run, at the same place. The
//! values stored in front of it are kept.
//!
//! # What keeps a kept value from changing
//!
//! Three layers, the first two static and the last one at run time:
//!
//! 1. **A value that is mutated is never kept.** [`StaticPlan`] leaves out
//!    every top-level term whose value reaches the container operand of a
//!    write or of a mutating builtin (`let by_sec = [0, 0, 0]` then
//!    `by_sec[i] = v`), and the pass refuses a slot to any register that
//!    reaches an in-place instruction.
//! 2. **A slot load is never a fresh root.** Escape analysis and the last-use
//!    pass are handed the plan and will not rewrite a mutation in place on a
//!    term a slot may fill (`escape::analyze_with_static_roots`,
//!    `lastuse::apply_with_static_roots`).
//! 3. **The immutable bit.** `StaticStore` marks the value and every
//!    container under it ([`Heap::mark_immutable`](crate::heap::Heap::mark_immutable));
//!    the in-place heap operations copy such an object instead of editing
//!    it. This is what holds when a kept table is handed to a function that
//!    writes its parameter, or an element of it is edited through an alias.
//!
//! A kept value holds no function, `var` cell, host handle or pending
//! result: one that does is not stored, its slots are rejected for the life
//! of the table, and [`StaticSlots::rejected`] says so.
//!
//! # Lifetime
//!
//! The table belongs to the [`Stack`](crate::stack::Stack), is a GC root, and
//! is stamped with the lowering it was filled under
//! ([`StaticLayout::id`], new for every lowering, so for every program and
//! every flag set) and the revision of the program's constant table (moved
//! by a literal patched in place). A stack that meets any other stamp starts
//! empty. `transfer_stack_state` clears it with the closures, the enum data
//! and the memo records.
//!
//! # Following the diff
//!
//! [`Env::reload_program`](crate::env::Env::reload_program) and
//! [`Env::set_config_value`](crate::env::Env::set_config_value) put back,
//! after that clearing, the values the edit cannot have changed
//! (`env/static_follow.rs` decides which; [`StaticSlots::install`] takes the
//! answer). A slot is one of three things afterwards:
//!
//! - **kept**: filled with the value it held. Its declaration is not inside
//!   anything the edit touched, the set of declarations it depends on
//!   ([`SlotFollow::deps`], from the classifier) has the same names in
//!   the old and the new program, and none of them is inside anything the
//!   edit touched either.
//! - **rebuilt**: empty, with the old value remembered beside it. The run
//!   executes on the next frame, and a `StaticStore` that finds the new
//!   value [`identical`] to the remembered one keeps the old heap value, so
//!   whatever holds the old one keeps holding the current one.
//! - **deferred**: filled with the value it held although the edit changed
//!   it, because its last build was slow and the edit is one tick of a drag
//!   ([`StaticDebounce`]). It is rebuilt when the drag pauses.
//!
//! A segment executes whenever a slot it stores is empty, so a rebuilt value
//! is built alone: the segments around it are skipped, and it reads the
//! kept values of those it is built from out of their slots. Two things
//! keep that from being told apart from a rebuild of everything:
//!
//! - A value is kept only together with the values its segment read
//!   ([`StaticRun::reads`]) and with those of the same store. Beside a
//!   rebuilt one it would hold the object the rebuild replaced.
//! - A rebuilt value takes the place of an [`identical`] old one only when
//!   it holds no kept object the old one does not ([`replaces`]).
//!
//! An enum's data lives in the stack's enum table as well as in its slot,
//! and a reload empties that table: a kept record is published again by the
//! reload, and one that is rebuilt is never swapped for the old one, because
//! the segment has already published the new.
//!
//! # Observations
//!
//! A host that reads the last value bound to every named term
//! ([`crate::observe`]: Garden's panels, `--observe`) must read the same
//! thing from a frame that skipped a segment as from one that ran it. So
//! where the lowering preserves observations ([`StaticLayout::observes`]) a
//! segment that executes writes down what was observed while it did, in the
//! functions it called too, one value per term (the buffer is last write
//! wins, and nothing else runs inside a segment, so the last value of each
//! term is all a reader can ever see of it). The `StaticStore` keeps that
//! log with the stack, and a `StaticSkip` that is taken records it again,
//! where the segment stood. This is what a memoized scope does with
//! `Dep::Observed`.
//!
//! The log is part of what a skip needs: a segment counts as built only
//! when it has one ([`StaticSlots::keep_log`]). A reload that leaves the
//! lowering in place (moved text, a patched literal) hands on the log of a
//! segment the edit did not reach, which is asked of every statement in
//! the segment and not only of the ones whose values are stored
//! ([`RunFollow`]): `let K10 = K9` is in the code for its name alone, has
//! no slot, and its segment's log names its value. A reload that
//! recompiles hands on none, since a log names terms of the old program.
//! A segment left with its values and no log executes once more, on the
//! next frame, to write it again, and its `StaticStore` puts the kept
//! values back in the registers (and the kept record of an enum back in
//! the enum table): the values, and what holds them, are as kept as they
//! are with observations off. A log entry that named the value built in
//! place of a kept one names the kept one. A segment whose rebuild a drag
//! put off keeps its log with its old values.
//!
//! One thing is not kept that would be without observations: a statement
//! that edits in place a container another statement built (`apply_runs`).
//! The optimizer edits in place where the program can no longer reach the
//! old value, but the buffer still names it, and shows the edit.
//!
//! The values a log holds are roots for the collector. They are the ones
//! the observation buffer would hold until the next run anyway; with the
//! cache on they are held for as long as the segment is kept.
//!
//! # Inside function bodies
//!
//! A function that builds the same value on every call is given a slot
//! for it too, by the same two instructions in the function's own code:
//! the [`in_fn`] module, which has its own rules for what "the same value
//! on every call" takes (the classifier's "static" is not enough inside a
//! function) and for what it does not keep. Those slots and runs come
//! behind the top-level ones in the layout ([`StaticLayout::root_slots`],
//! [`StaticLayout::root_runs`]); everything above is about the top-level
//! ones, and where the two differ (reloads, observations, timing) that
//! module says how.
//!
//! The cache is off under the `baseline` and `explain` policies, for
//! run-once `petal run`, whenever the per-term trace is preserved (a
//! skipped run records nothing for it, and what it would have recorded is
//! every instruction, not one value per name), and under the static audit.

// The passes below walk instruction positions: one `ip` indexes the code and
// the per-instruction tables beside it (statement, reads, writes, origins),
// and is compared with other positions. Iterating one of the tables instead
// would hide that they are parallel.
#![allow(clippy::needless_range_loop)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// `std::time::Instant::now()` panics on `wasm32-unknown-unknown`, which has
/// no clock, and `begin_build` reads it on every static build — so without
/// this every web-canvas script trapped on its first frame. The stand-in
/// reads every instant as infinitely long ago: a live-edit tick never
/// continues a drag, so nothing is deferred, and a build is never timed
/// (`build_ns` stays 0, which already means "not timed").
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug)]
struct Instant;

#[cfg(target_arch = "wasm32")]
impl Instant {
    fn now() -> Self {
        Instant
    }
    fn elapsed(&self) -> Duration {
        Duration::MAX
    }
    fn duration_since(&self, _earlier: Instant) -> Duration {
        Duration::MAX
    }
}

/// Nanoseconds since `since`, or 0 where there is no clock.
fn build_nanos(since: Instant) -> u64 {
    if cfg!(target_arch = "wasm32") {
        return 0;
    }
    since.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

use crate::backend::bytecode::isa::{BytecodeProgram, Inst, Label, Reg};
use crate::native_fn::{NativeConstness, NativeFnTable};
use crate::heap::Heap;
use crate::program::{BlockId, Program, StateKey, TermId, TermOp};
use crate::source_map::FileId;
use crate::static_classify::{StaticAnalysis, TermClass};
use crate::value::Value;

mod in_fn;
pub use in_fn::FnPlan;

/// A stretch lighter than this is not worth a check per run. Counted in
/// allocations, with a call or a loop taken to be two. One is enough: a
/// kept list costs one instruction a frame and no allocation, where
/// building it costs at least the allocation. A stretch of weight zero
/// loads and computes scalars, which a check does not beat.
pub const MIN_WEIGHT: u32 = 1;

// ---------------------------------------------------------------------------
// The layout: what one lowering's static instructions refer to
// ---------------------------------------------------------------------------

/// What one slot holds, for a report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticSlotInfo {
    /// The declaration whose value it is (`TILES`, `lib::BASE`); `None` for
    /// an unnamed value a later statement reads.
    pub name: Option<String>,
    /// `file:line` of the term that produces it.
    pub at: Option<String>,
    /// That term.
    pub term: Option<TermId>,
    /// Declared with `const`: the author asked for it to be constant, so a
    /// value that cannot be kept is worth saying louder.
    pub is_const: bool,
    /// For a value built inside a function body (see [`in_fn`]): the
    /// function's name. `None` for a top-level value.
    pub function: Option<String>,
}

impl StaticSlotInfo {
    /// The words for a value built inside a function body that the check
    /// ([`StaticSlots::set_check`]) found different from the one kept.
    pub fn mismatch_message(&self) -> String {
        format!(
            "a value kept for {}{} was built different on a later call",
            self.function.as_deref().unwrap_or("a function"),
            self.at.as_ref().map(|a| format!(" ({a})")).unwrap_or_default()
        )
    }

    /// The words for the one difference a correct cache has: the kept value
    /// was frozen by the program and the same value built again is not
    /// ([`identical_but_for_frozen`]).
    pub fn frozen_message(&self) -> String {
        format!(
            "the value kept for {}{} has been frozen and the same value built again is not: \
             `freeze` reached the one value every call returns",
            self.function.as_deref().unwrap_or("a function"),
            self.at.as_ref().map(|a| format!(" ({a})")).unwrap_or_default()
        )
    }
}

/// What a reload needs to know about one slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotFollow {
    /// What a reload matches this slot by across two lowerings: the
    /// module-qualified name of the declaration whose value it is; for a
    /// value with no declaration of its own (the `[1, 2, 3]` written inside
    /// a call) a key made of its statement's text and shape, which starts
    /// with a NUL (see `unnamed_keys`). `None` for a key two slots of one
    /// lowering would share; such a slot is rebuilt by every reload that
    /// recompiles.
    pub key: Option<String>,
    /// Every declaration the value depends on, the slot's own included, as
    /// sorted indices into [`FollowInfo::decls`]. `None` when that is not
    /// known, which a reload takes to mean "everything".
    pub deps: Option<Box<[u32]>>,
    /// The terms whose instructions write the slot's register: where the
    /// value is written in the source, for a reload asking whether an edit
    /// was inside it.
    pub origins: Vec<TermId>,
    /// For the record an enum variant's constant data publishes: the
    /// variant's tag (`Kind.park`).
    pub enum_tag: Option<String>,
    /// The one literal whose load is the only thing that writes the slot's
    /// register, when that is all the slot holds: its value is the
    /// literal's, whatever a run would say.
    pub literal: Option<TermId>,
}

impl SlotFollow {
    /// The module-qualified name of the declaration whose value the slot
    /// holds, when it is one.
    pub fn declaration(&self) -> Option<&str> {
        if self.enum_tag.is_some() {
            return None;
        }
        self.key.as_deref().filter(|k| !k.starts_with('\u{0}'))
    }
}

/// One declaration of a program, as a reload needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowDecl {
    /// Module-qualified name (`TILES`, `lib::BASE`).
    pub name: String,
    pub file: FileId,
    /// The terms that bind it: where it is written.
    pub defs: Vec<TermId>,
    /// For a top-level `state`: its key, and what its initializer depends
    /// on (indices into [`FollowInfo::decls`], sorted, closed transitively).
    pub state: Option<(StateKey, Box<[u32]>)>,
    /// For a top-level `state` that code fills behind a test of a cell the
    /// same branch writes (a cache): what the values written there depend
    /// on, as `state`'s indices are
    /// ([`StaticAnalysis::state_cache_dependencies`]). `None` for every
    /// other declaration.
    pub cache: Option<Box<[u32]>>,
}

/// What a reload needs to know about a lowering to follow an edit through
/// it: every declaration with where it is written and what each `state`
/// initializer reads (in the classifier's report order), and for each slot
/// what it is matched by and what it depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowInfo {
    pub decls: Vec<FollowDecl>,
    /// One per slot of the layout.
    pub slots: Vec<SlotFollow>,
    /// One per run of the layout: what a segment's observation log depends
    /// on. Empty where the lowering does not observe.
    pub runs: Vec<RunFollow>,
}

/// What a reload needs to know about one segment to say whether what it
/// observed still holds (the module docs, "Observations"). This is wider
/// than what its slots depend on: a segment also holds the statements
/// nothing reads (`let K10 = K9`, kept in the code for its name alone),
/// and the log names their values too.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFollow {
    /// Every declaration a statement of the segment depends on, as sorted
    /// indices into [`FollowInfo::decls`]. `None` when that is not known.
    pub deps: Option<Box<[u32]>>,
    /// The terms the segment's instructions belong to: where it is written.
    pub origins: Vec<TermId>,
}

/// One `StaticSkip`: a segment of static instructions that builds some
/// values and stores them, or a group of consecutive segments that a frame
/// jumps over together when every one of them is built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticRun {
    /// Every slot that must be filled for the skip to be taken: for a
    /// segment the slots it stores, for a group those of all its segments.
    pub feeds: Vec<u32>,
    /// The slots its own `StaticStore` fills. Empty for a group.
    pub stores: Vec<u32>,
    /// The slots of other segments whose registers its instructions read:
    /// the kept values it is built from directly. Empty for a group.
    pub reads: Vec<u32>,
    /// The group the segment is inside, when its stretch has more than one
    /// segment.
    pub group: Option<u32>,
    /// How many root instructions a skip does not execute. The calls among
    /// them run more: this is a floor on what a skip saves.
    pub instructions: usize,
    /// How many of those allocate a list, record or element directly.
    pub allocations: usize,
}

impl StaticRun {
    /// A group of segments: it stores nothing itself.
    pub fn is_group(&self) -> bool {
        self.stores.is_empty()
    }
}

/// The static-value side of one lowering (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct StaticLayout {
    /// Unique to this lowering; 0 when the program was lowered without the
    /// cache. What a stack's table is stamped with.
    pub id: u64,
    pub slots: Vec<StaticSlotInfo>,
    pub runs: Vec<StaticRun>,
    /// Slot -> the runs that feed it (the inverse of [`StaticRun::feeds`]).
    feeders: Vec<Vec<u32>>,
    /// Run -> the segments it is the group of, in code order. Empty for a
    /// segment.
    members: Vec<Vec<u32>>,
    /// The lowering preserves observations: a segment keeps a log of what
    /// was observed while it executed, and a skip records it again (see the
    /// module docs, "Observations").
    pub observes: bool,
    /// Per run, the terms of its instructions, on the way to
    /// [`FollowInfo::runs`].
    run_origins: Vec<Vec<TermId>>,
    /// Runs of candidate instructions the pass found, skippable or not.
    pub candidate_runs: usize,
    /// Root instructions before the pass added its own.
    pub root_instructions: usize,
    /// How many of `slots` and of `runs` belong to the root code: the
    /// first ones. The rest are values built inside function bodies
    /// ([`in_fn`]), one run per kept stretch of a function's code.
    pub root_slots: usize,
    pub root_runs: usize,
    /// The program's declarations, for a reload (see "Following the diff").
    pub follow: Arc<FollowInfo>,
}

impl StaticLayout {
    /// Whether the lowering has anything to keep.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The runs that execute while `slot` is empty: its segment, and the
    /// group the segment is in.
    pub fn feeders(&self, slot: u32) -> &[u32] {
        self.feeders.get(slot as usize).map(|v| &v[..]).unwrap_or(&[])
    }

    /// The segment whose `StaticStore` fills `slot`.
    pub fn segment_of(&self, slot: u32) -> Option<&StaticRun> {
        self.feeders(slot).iter().filter_map(|r| self.runs.get(*r as usize)).find(|r| !r.is_group())
    }

    /// The segments of the group `run`, in code order; empty when `run` is
    /// a segment itself.
    pub fn segments_under(&self, run: u32) -> &[u32] {
        self.members.get(run as usize).map(|m| &m[..]).unwrap_or(&[])
    }

    /// The checks a frame makes in the root code when every value is kept:
    /// one per stretch of static instructions, whether it is a group or a
    /// lone segment.
    pub fn checks(&self) -> usize {
        self.runs.iter().take(self.root_runs).filter(|r| r.group.is_none()).count()
    }

    /// How many `StaticSkip`s of the root code guard one segment (the rest
    /// guard a group).
    pub fn segments(&self) -> usize {
        self.runs.iter().take(self.root_runs).filter(|r| !r.is_group()).count()
    }

    /// Whether `slot` holds a value built inside a function body.
    pub fn in_function(&self, slot: u32) -> bool {
        slot as usize >= self.root_slots
    }

    /// How many slots hold a value built inside a function body, and how
    /// many stretches of function code they are stored by.
    pub fn function_slots(&self) -> (usize, usize) {
        (self.slots.len() - self.root_slots.min(self.slots.len()), self.runs.len() - self.root_runs.min(self.runs.len()))
    }

    /// Work out the two inverse tables again, after runs were added.
    fn index(&mut self) {
        self.feeders = vec![Vec::new(); self.slots.len()];
        self.members = vec![Vec::new(); self.runs.len()];
        for (i, run) in self.runs.iter().enumerate() {
            for &s in &run.feeds {
                self.feeders[s as usize].push(i as u32);
            }
            if let Some(g) = run.group {
                self.members[g as usize].push(i as u32);
            }
        }
    }
}

static NEXT_LAYOUT_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// The table: what one stack holds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Slot {
    Empty,
    Filled(Value),
    /// The value could not be kept; the run executes on every run.
    Rejected,
}

/// Counters of one stack's table, cumulative since the stack was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StaticStats {
    /// Runs jumped over.
    pub skips: u64,
    /// Instructions those skips did not execute.
    pub skipped_instructions: u64,
    /// Slots filled.
    pub stored: u64,
    /// Slots whose value could not be kept.
    pub rejected: u64,
    /// Times the table was emptied: a reload, a re-lowering, a patched
    /// literal.
    pub clears: u64,
    /// Slots a reload filled again with the value they held.
    pub carried: u64,
    /// Rebuilt values that turned out identical to the one the slot held
    /// before the reload, which was kept instead.
    pub reused: u64,
    /// Observations recorded again for skipped segments (see the module
    /// docs, "Observations").
    pub replayed: u64,
    /// Segments executed with every value kept, only to write down what
    /// they observe: once each after a reload that recompiled.
    pub relogged: u64,
}

/// How a slow rebuild is put off while a value is being dragged (see
/// [`Env::set_static_debounce`](crate::env::Env::set_static_debounce)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticDebounce {
    /// A slot whose last build took at least this long is not rebuilt on
    /// every tick of a drag.
    pub threshold: Duration,
    /// Two `set_config_value` calls closer together than this are one drag;
    /// a frame that starts this long after the last one rebuilds what the
    /// drag put off.
    pub quiet: Duration,
}

impl Default for StaticDebounce {
    fn default() -> Self {
        StaticDebounce {
            threshold: Duration::from_millis(4),
            quiet: Duration::from_millis(150),
        }
    }
}

/// What a reload puts in one slot of the table it leaves (see the module
/// docs, "Following the diff").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seed {
    /// Nothing: the slot is new, or held nothing.
    Empty,
    /// The value it held, which the edit cannot have changed.
    Kept(Value),
    /// Empty, but this is what it held: a rebuilt value identical to it is
    /// dropped in its favour.
    Rebuild(Value),
    /// The value it held, which the edit did change; rebuilt when the drag
    /// that changed it pauses.
    Deferred(Value),
}

/// The static values one stack holds: a dense vector of slots indexed by the
/// lowering's [`StaticLayout`], and per run (a segment or a group) the number
/// of slots it feeds that are not filled yet (a `StaticSkip` is a test of
/// that against zero).
#[derive(Debug, Clone, Default)]
pub struct StaticSlots {
    /// (layout id, constant-table revision) the contents belong to.
    stamp: Option<(u64, u64)>,
    slots: Vec<Slot>,
    pending: Vec<u32>,
    rejected: Vec<(u32, &'static str)>,
    /// How many of `rejected` a host has been told about
    /// ([`take_unreported`](Self::take_unreported)).
    reported: usize,
    /// Per slot, the value it held before the reload that emptied it. Empty
    /// when no slot has one.
    prior: Vec<Option<Value>>,
    /// Slots that hold a value an edit changed, waiting for the drag to
    /// pause; and when the last tick of the drag was.
    deferred: Vec<u32>,
    /// Slots rebuilt while others were deferred: they may have been built
    /// from a deferred slot's old value, and are rebuilt again with it.
    redo: Vec<u32>,
    last_live_edit: Option<Instant>,
    /// Per run, how long its last execution took, in nanoseconds; 0 when it
    /// has not been timed, and for a group. Kept across a reload that leaves
    /// the lowering in place.
    build_ns: Vec<u64>,
    building: Option<(u32, Instant)>,
    /// The lowering preserves observations ([`StaticLayout::observes`]).
    observes: bool,
    /// Per run, what was observed while the segment last executed: the last
    /// value of each term. `None` for a segment that has not completed since
    /// the table was made, and for a group. Empty when `observes` is off.
    logs: Vec<Option<ObservationLog>>,
    /// What the segment in progress has observed so far.
    log: Vec<(TermId, Value)>,
    /// Check the values built inside function bodies instead of trusting
    /// them (see [`set_check`](Self::set_check)).
    check: bool,
    /// Print each mismatch on stderr as it is found: the check was asked
    /// for by the process environment (`PETAL_STATIC_CHECK=1`), so nobody
    /// reads [`mismatches`](Self::mismatches).
    announce_check: bool,
    /// Slots whose code built a value that is not the one kept, under
    /// `check`: each once.
    mismatches: Vec<u32>,
    pub stats: StaticStats,
}

/// What one segment observed: the last value of each named term bound while
/// it executed, in the order of those last bindings.
pub type ObservationLog = Arc<[(TermId, Value)]>;

impl StaticSlots {
    /// Make the table the one for `layout` at constant revision `revision`:
    /// a no-op when it already is, an empty table otherwise. Called on every
    /// dispatch into the VM, so the common case is two comparisons.
    #[inline]
    pub(crate) fn sync(&mut self, layout: &StaticLayout, revision: u64) {
        if self.stamp == Some((layout.id, revision)) {
            return;
        }
        self.reset(layout, revision);
    }

    #[cold]
    fn reset(&mut self, layout: &StaticLayout, revision: u64) {
        if self.slots.iter().any(|s| matches!(s, Slot::Filled(_))) {
            self.stats.clears += 1;
        }
        self.stamp = Some((layout.id, revision));
        self.slots.clear();
        self.slots.resize(layout.slots.len(), Slot::Empty);
        self.pending.clear();
        self.pending.extend(layout.runs.iter().map(|r| r.feeds.len() as u32));
        self.rejected.clear();
        self.reported = 0;
        self.prior.clear();
        self.deferred.clear();
        self.redo.clear();
        self.building = None;
        self.build_ns.clear();
        self.build_ns.resize(layout.runs.len(), 0);
        // Where observations are preserved a segment is not built until its
        // log is written: one more thing pending for it, and for its group.
        self.observes = layout.observes;
        self.logs.clear();
        self.log.clear();
        if self.observes {
            self.logs.resize(layout.runs.len(), None);
            for (i, run) in layout.runs.iter().enumerate() {
                if run.is_group() {
                    continue;
                }
                self.pending[i] += 1;
                if let Some(g) = run.group {
                    self.pending[g as usize] += 1;
                }
            }
        }
    }

    /// Check every value built inside a function body instead of trusting
    /// it: its skip is never taken, so the code runs on every call, and its
    /// store compares what it built with what the slot holds. A difference
    /// is a value that was not invariant; see
    /// [`mismatches`](Self::mismatches). A program runs the same with this
    /// on as with the cache off, at the cost of the cache.
    pub fn set_check(&mut self, on: bool) {
        self.check = on;
    }

    #[inline]
    pub(crate) fn checking(&self) -> bool {
        self.check
    }

    /// The slots found, under [`set_check`](Self::set_check), to have been
    /// built with a value that is not the one they hold. Not cleared by a
    /// reload.
    pub fn mismatches(&self) -> &[u32] {
        &self.mismatches
    }

    /// Print each mismatch on stderr as it is found: how a whole test suite
    /// (or a host's game) is run under the check without touching a test.
    /// A value that was built different is a `static check failure:` line.
    /// One that differs only in having been frozen is a `static check
    /// note:` line: a program that freezes what a function returned does
    /// that to a correct cache, and a host that greps for failures must not
    /// find it.
    pub(crate) fn set_check_announced(&mut self, on: bool) {
        self.announce_check = on;
    }

    /// Record that the value `slot` keeps is not [`identical`] to the one
    /// just built. `only_frozen`: they are [`identical_but_for_frozen`].
    pub(crate) fn note_mismatch(&mut self, layout: &StaticLayout, slot: u32, only_frozen: bool) {
        if self.mismatches.contains(&slot) {
            return;
        }
        self.mismatches.push(slot);
        if self.announce_check {
            match (layout.slots.get(slot as usize), only_frozen) {
                (Some(info), false) => eprintln!("static check failure: {}", info.mismatch_message()),
                (Some(info), true) => eprintln!("static check note: {}", info.frozen_message()),
                (None, _) => eprintln!("static check failure: slot {slot}"),
            }
        }
    }

    /// Empty every slot of `slots`, which hold values built inside function
    /// bodies: the next call that reaches each one's code builds it again.
    pub(crate) fn empty_function_slots(&mut self, layout: &StaticLayout) {
        for s in layout.root_slots..self.slots.len() {
            if matches!(self.slots[s], Slot::Filled(_)) {
                self.slots[s] = Slot::Empty;
                for &run in layout.feeders(s as u32) {
                    if let Some(p) = self.pending.get_mut(run as usize) {
                        *p += 1;
                    }
                }
            }
        }
    }

    /// Drop every value. The next run rebuilds them.
    pub fn clear(&mut self) {
        if self.slots.iter().any(|s| matches!(s, Slot::Filled(_))) {
            self.stats.clears += 1;
        }
        self.stamp = None;
        self.slots.clear();
        self.pending.clear();
        self.rejected.clear();
        self.reported = 0;
        self.prior.clear();
        self.deferred.clear();
        self.redo.clear();
        self.building = None;
        self.observes = false;
        self.logs.clear();
        self.log.clear();
        // `build_ns` stays: `install` asks for it when the same lowering is
        // filled again, and `reset` drops it for any other.
    }

    /// The lowering the table was last filled under, and the constant
    /// revision; `None` after a [`clear`](Self::clear).
    pub fn stamp(&self) -> Option<(u64, u64)> {
        self.stamp
    }

    /// Make the table the one for `layout` at `revision`, holding what
    /// `seeds` says slot by slot: what a reload that follows the diff leaves
    /// (see the module docs). `timings` are the per-run build times of the
    /// table this one replaces, kept when it was for the same lowering.
    /// `logs` are that table's observation logs, likewise, with for each
    /// run whether what it observed still holds.
    pub(crate) fn install(
        &mut self,
        layout: &StaticLayout,
        revision: u64,
        seeds: &[Seed],
        timings: Option<Vec<u64>>,
        logs: Option<(Vec<Option<ObservationLog>>, &[bool])>,
    ) {
        let last_live_edit = self.last_live_edit;
        self.reset(layout, revision);
        self.last_live_edit = last_live_edit;
        if let Some(t) = timings.filter(|t| t.len() == layout.runs.len()) {
            self.build_ns = t;
        }
        if let Some((logs, holds)) = logs.filter(|(l, _)| self.observes && l.len() == layout.runs.len()) {
            for (i, log) in logs.into_iter().enumerate() {
                if let (Some(&true), Some(log)) = (holds.get(i), log) {
                    self.set_log(layout, i as u32, log);
                }
            }
        }
        for (i, seed) in seeds.iter().enumerate().take(self.slots.len()) {
            match *seed {
                Seed::Empty => {}
                Seed::Kept(v) | Seed::Deferred(v) => {
                    self.slots[i] = Slot::Filled(v);
                    self.stats.carried += 1;
                    for &run in layout.feeders(i as u32) {
                        if let Some(p) = self.pending.get_mut(run as usize) {
                            *p = p.saturating_sub(1);
                        }
                    }
                    if matches!(seed, Seed::Deferred(_)) {
                        self.deferred.push(i as u32);
                    }
                }
                Seed::Rebuild(v) => {
                    if self.prior.is_empty() {
                        self.prior.resize(self.slots.len(), None);
                    }
                    self.prior[i] = Some(v);
                }
            }
        }
    }

    /// The per-run observation logs, for the table that replaces this one.
    pub(crate) fn logs(&self) -> Vec<Option<ObservationLog>> {
        self.logs.clone()
    }

    /// Whether segments keep a log of what they observe.
    #[inline]
    pub(crate) fn observes(&self) -> bool {
        self.observes
    }

    /// A named term was bound to `value`. Written down while a segment is
    /// executing, for the skips that will stand in for it.
    #[inline]
    pub(crate) fn note_observation(&mut self, term: TermId, value: Value) {
        if self.observes && self.building.is_some() {
            self.log.push((term, value));
        }
    }

    /// What `run` observed the last time it executed to its store.
    #[inline]
    pub(crate) fn log_of(&self, run: u32) -> Option<&ObservationLog> {
        self.logs.get(run as usize)?.as_ref()
    }

    /// Whether the segment `run` has its values but not its log: it was
    /// kept through a reload that recompiled.
    pub(crate) fn lacks_log(&self, run: u32) -> bool {
        self.observes && matches!(self.logs.get(run as usize), Some(None))
    }

    /// Whether every slot of a `StaticStore` holds a value.
    pub(crate) fn all_filled(&self, slots: &[(u32, Reg)]) -> bool {
        slots.iter().all(|(s, _)| matches!(self.slots.get(*s as usize), Some(Slot::Filled(_))))
    }

    /// In the log of the segment in progress, make every entry that names
    /// `from` name `to`: the store put `to` in the register in place of the
    /// value the segment built. A heap value is the same object wherever it
    /// is named; a number is only the slot's own where one of `origins`
    /// (the terms that write the slot's register) bound it.
    pub(crate) fn rename_logged(&mut self, from: Value, to: Value, origins: &[TermId]) {
        let scalar =
            matches!(from, Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Symbol(_));
        for (term, value) in &mut self.log {
            if *value == from && (!scalar || origins.contains(term)) {
                *value = to;
            }
        }
    }

    /// The segment `run` reached its store and its values are kept: keep
    /// what it observed, the last value of each term.
    pub(crate) fn keep_log(&mut self, layout: &StaticLayout, run: u32) {
        if !self.observes {
            return;
        }
        let mut seen: HashSet<TermId> = HashSet::with_capacity(self.log.len());
        let mut last: Vec<(TermId, Value)> =
            self.log.drain(..).rev().filter(|(t, _)| seen.insert(*t)).collect();
        last.reverse();
        self.set_log(layout, run, last.into());
    }

    fn set_log(&mut self, layout: &StaticLayout, run: u32, log: ObservationLog) {
        let Some(entry) = self.logs.get_mut(run as usize) else { return };
        if entry.replace(log).is_some() {
            return;
        }
        let group = layout.runs.get(run as usize).and_then(|r| r.group);
        for r in [Some(run), group].into_iter().flatten() {
            if let Some(p) = self.pending.get_mut(r as usize) {
                *p = p.saturating_sub(1);
            }
        }
    }

    /// The segment in progress is not kept: neither is what it observed.
    pub(crate) fn drop_log(&mut self) {
        self.log.clear();
    }

    /// The per-run build times, for the table that replaces this one.
    pub(crate) fn timings(&self) -> Vec<u64> {
        self.build_ns.clone()
    }

    /// How long the segment that builds `slot` took the last time it ran,
    /// with everything only it reads. (A group is never timed.)
    pub fn build_time(&self, layout: &StaticLayout, slot: u32) -> Duration {
        let ns: u64 = layout
            .feeders(slot)
            .iter()
            .map(|r| self.build_ns.get(*r as usize).copied().unwrap_or(0))
            .sum();
        Duration::from_nanos(ns)
    }

    /// The value `slot` held before the reload that emptied it, if a rebuilt
    /// value should be compared with it.
    #[inline]
    pub(crate) fn prior(&self, slot: u32) -> Option<Value> {
        self.prior.get(slot as usize).copied().flatten()
    }

    /// Whether any slot is waiting for a value to compare with.
    #[inline]
    pub(crate) fn has_prior(&self) -> bool {
        !self.prior.is_empty()
    }

    /// The slots holding a value an edit changed, not rebuilt yet.
    pub fn deferred(&self) -> &[u32] {
        &self.deferred
    }

    /// The slots that were rebuilt while others were deferred (see
    /// [`set_redo`](Self::set_redo)).
    pub fn redo(&self) -> &[u32] {
        &self.redo
    }

    /// Name the slots that the edit which deferred some rebuilds also
    /// reached and that are being rebuilt now. One of them may read a
    /// deferred slot, and would then be built from its old value, so
    /// [`flush_deferred`](Self::flush_deferred) rebuilds them again.
    pub(crate) fn set_redo(&mut self, slots: Vec<u32>) {
        self.redo = if self.deferred.is_empty() { Vec::new() } else { slots };
    }

    /// A tick of a drag happened now. Returns whether it continues one: the
    /// tick before it was less than `quiet` ago.
    #[allow(clippy::disallowed_methods, reason = "wasm32 builds read the `Instant` stand-in above")]
    pub(crate) fn note_live_edit(&mut self, quiet: Duration) -> bool {
        let now = Instant::now();
        let continues = self.last_live_edit.is_some_and(|t| now.duration_since(t) < quiet);
        self.last_live_edit = Some(now);
        continues
    }

    /// Whether the drag that deferred some rebuilds has paused for `quiet`.
    pub(crate) fn drag_paused(&self, quiet: Duration) -> bool {
        !self.deferred.is_empty() && self.last_live_edit.is_none_or(|t| t.elapsed() >= quiet)
    }

    /// Empty every deferred slot, remembering its value for the comparison:
    /// the next run rebuilds them. Returns how many there were.
    pub(crate) fn flush_deferred(&mut self, layout: &StaticLayout) -> usize {
        let slots = std::mem::take(&mut self.deferred);
        let redo = std::mem::take(&mut self.redo);
        for &s in slots.iter().chain(&redo) {
            let Some(entry) = self.slots.get_mut(s as usize) else { continue };
            let Slot::Filled(v) = *entry else { continue };
            *entry = Slot::Empty;
            if self.prior.is_empty() {
                self.prior.resize(self.slots.len(), None);
            }
            self.prior[s as usize] = Some(v);
            for &run in layout.feeders(s) {
                if let Some(p) = self.pending.get_mut(run as usize) {
                    *p += 1;
                }
            }
        }
        // A value built inside a function since the drag began may have
        // been built from one of those.
        if !slots.is_empty() {
            self.empty_function_slots(layout);
        }
        slots.len()
    }

    /// A run is about to execute (its `StaticSkip` fell through).
    #[inline]
    #[allow(clippy::disallowed_methods, reason = "wasm32 builds read the `Instant` stand-in above")]
    pub(crate) fn begin_build(&mut self, run: u32) {
        self.building = Some((run, Instant::now()));
        // A segment that raised left its half-written log behind.
        self.log.clear();
    }

    /// The run in progress reached its `StaticStore`: which one it was.
    #[inline]
    pub(crate) fn end_build(&mut self) -> Option<u32> {
        let (run, since) = self.building.take()?;
        if let Some(ns) = self.build_ns.get_mut(run as usize) {
            *ns = build_nanos(since);
        }
        Some(run)
    }

    /// How many slots the table has, and how many hold a value.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn filled(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s, Slot::Filled(_))).count()
    }

    /// The value in `slot`, if it holds one.
    pub fn get(&self, slot: u32) -> Option<Value> {
        match self.slots.get(slot as usize) {
            Some(Slot::Filled(v)) => Some(*v),
            _ => None,
        }
    }

    /// The slots whose value could not be kept, with what it held.
    pub fn rejected(&self) -> &[(u32, &'static str)] {
        &self.rejected
    }

    /// The rejections since the last call: each is handed out once per
    /// life of the table, so a host that prints them prints each once per
    /// load and once more after each reload that leaves it standing.
    pub fn take_unreported(&mut self) -> &[(u32, &'static str)] {
        let from = std::mem::replace(&mut self.reported, self.rejected.len());
        &self.rejected[from.min(self.rejected.len())..]
    }

    /// Whether `slot` was given up on (see [`rejected`](Self::rejected)).
    pub fn is_rejected(&self, slot: u32) -> bool {
        matches!(self.slots.get(slot as usize), Some(Slot::Rejected))
    }

    /// Whether every slot `run` feeds is filled.
    #[inline]
    pub(crate) fn ready(&self, run: u32) -> bool {
        self.pending.get(run as usize) == Some(&0)
    }

    /// Whether a `StaticStore` over `slots` has anything to do: none of them
    /// was rejected, and not all of them are filled already.
    pub(crate) fn wants(&self, slots: &[(u32, Reg)]) -> bool {
        let mut any_empty = false;
        for (s, _) in slots {
            match self.slots.get(*s as usize) {
                Some(Slot::Empty) => any_empty = true,
                Some(Slot::Filled(_)) => {}
                Some(Slot::Rejected) | None => return false,
            }
        }
        any_empty
    }

    /// Fill `slot`. A slot that already holds a value keeps it: the segment
    /// was executed again only because another slot of its store is empty,
    /// and what it computed is the same value.
    pub(crate) fn fill(&mut self, layout: &StaticLayout, slot: u32, value: Value) {
        let Some(entry) = self.slots.get_mut(slot as usize) else { return };
        if !matches!(entry, Slot::Empty) {
            return;
        }
        *entry = Slot::Filled(value);
        self.stats.stored += 1;
        if let Some(p) = self.prior.get_mut(slot as usize) {
            *p = None;
        }
        for &run in layout.feeders.get(slot as usize).map(|v| &v[..]).unwrap_or(&[]) {
            if let Some(p) = self.pending.get_mut(run as usize) {
                *p = p.saturating_sub(1);
            }
        }
    }

    /// Give up on every slot of one `StaticStore`: `slot` held `what`.
    pub(crate) fn reject(&mut self, slots: &[(u32, Reg)], slot: u32, what: &'static str) {
        for (s, _) in slots {
            if let Some(entry) = self.slots.get_mut(*s as usize)
                && matches!(entry, Slot::Empty)
            {
                *entry = Slot::Rejected;
                self.stats.rejected += 1;
            }
        }
        self.rejected.push((slot, what));
    }

    /// Every kept value, for the collector.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        for s in &self.slots {
            if let Slot::Filled(v) = s {
                mark(*v);
            }
        }
        for v in self.prior.iter().flatten() {
            mark(*v);
        }
        // What a skip records again, and what the segment in progress has
        // observed: a value bound inside a function the segment called is
        // held by nothing else.
        for log in self.logs.iter().flatten() {
            for (_, v) in log.iter() {
                mark(*v);
            }
        }
        for (_, v) in &self.log {
            mark(*v);
        }
    }
}

/// Whether two values are the same value to everything a program can ask:
/// the same kinds, numbers bit for bit (so `1` is not `1.0` and `0.0` is
/// not `-0.0`), strings by content, lists in order, records with the same
/// class and the same keys in the same order, and the same answer to
/// `frozen`. Stricter than `==`. Anything a kept value cannot hold (a
/// function, a cell, a handle) is never identical to anything.
pub fn identical(a: Value, b: Value, heap: &Heap) -> bool {
    same(a, b, heap, false, true)
}

/// [`identical`] but for the answer to `frozen`: the same contents, one of
/// them (or a part of one) frozen where the other is not. What a kept value
/// and the same value built again are when the program froze something that
/// holds the kept one: `freeze` is about the heap object, so it reaches the
/// one object every call returns. The cache did nothing wrong there.
pub fn identical_but_for_frozen(a: Value, b: Value, heap: &Heap) -> bool {
    same(a, b, heap, false, false)
}

/// Whether the old value `old` of a slot may stand in for `new`, the value
/// a rebuild just made, without anything being able to tell: the two are
/// [`identical`], and wherever they are not the same heap object the new
/// one is the rebuild's own (not yet immutable). A container of `new` that
/// is immutable already is a kept value, or part of one, that something
/// else holds as well; the old value holds another object there, and
/// `freeze` through one name would then not reach the other.
pub fn replaces(old: Value, new: Value, heap: &Heap) -> bool {
    same(old, new, heap, true, true)
}

/// [`identical`]; with `private`, also [`replaces`]: no container of `b`
/// that is a different object from its counterpart in `a` is immutable.
/// Without `frozen_too`, two values that differ only in what is frozen are
/// the same ([`identical_but_for_frozen`]).
fn same(a: Value, b: Value, heap: &Heap, private: bool, frozen_too: bool) -> bool {
    let frozen = |x: Value, y: Value| !frozen_too || heap.is_frozen(x) == heap.is_frozen(y);
    let own = |y: Value| !private || !heap.is_immutable(y);
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::String(x), Value::String(y)) => {
            x == y || (frozen(a, b) && heap.get_string(x) == heap.get_string(y))
        }
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::Vec2(ax, ay), Value::Vec2(bx, by)) => {
            ax.to_bits() == bx.to_bits() && ay.to_bits() == by.to_bits()
        }
        (Value::Vec3(x), Value::Vec3(y)) => {
            x == y || {
                let (p, q) = (heap.get_vec3(x), heap.get_vec3(y));
                frozen(a, b) && p.iter().zip(&q).all(|(m, n)| m.to_bits() == n.to_bits())
            }
        }
        (Value::Dual { value: av, derivative: ad }, Value::Dual { value: bv, derivative: bd }) => {
            av.to_bits() == bv.to_bits() && ad.to_bits() == bd.to_bits()
        }
        (Value::List(x), Value::List(y)) => {
            if x == y {
                return true;
            }
            let (xs, ys) = (heap.get_list(x), heap.get_list(y));
            frozen(a, b)
                && own(b)
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(p, q)| same(*p, *q, heap, private, frozen_too))
        }
        (Value::F64Array(x), Value::F64Array(y)) => {
            if x == y {
                return true;
            }
            let (xs, ys) = (heap.get_f64_array(x), heap.get_f64_array(y));
            frozen(a, b)
                && own(b)
                && xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(p, q)| p.to_bits() == q.to_bits())
        }
        (Value::Map(x), Value::Map(y)) => {
            if x == y {
                return true;
            }
            if !frozen(a, b) || !own(b) || heap.map_class_name(x) != heap.map_class_name(y) {
                return false;
            }
            let (xs, ys) = (heap.get_map(x), heap.get_map(y));
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|((k, p), (l, q))| k == l && same(*p, *q, heap, private, frozen_too))
        }
        (Value::Dict(x), Value::Dict(y)) => {
            if x == y {
                return true;
            }
            if !frozen(a, b) || !own(b) {
                return false;
            }
            let (xs, ys) = (heap.get_dict(x), heap.get_dict(y));
            xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|((k, p), (l, q))| {
                    same(k, l, heap, private, frozen_too) && same(p, q, heap, private, frozen_too)
                })
        }
        (Value::EnumVariant { tag: at, data: ad }, Value::EnumVariant { tag: bt, data: bd }) => {
            (at == bt || heap.get_string(at) == heap.get_string(bt))
                && same(Value::List(ad), Value::List(bd), heap, private, frozen_too)
        }
        (Value::Element(x), Value::Element(y)) => {
            x == y
                || (heap.get_string(heap.get_element_tag(x)) == heap.get_string(heap.get_element_tag(y))
                    && same(
                        Value::Map(heap.get_element_props(x)),
                        Value::Map(heap.get_element_props(y)),
                        heap,
                        private,
                        frozen_too,
                    )
                    && same(
                        Value::List(heap.get_element_children(x)),
                        Value::List(heap.get_element_children(y)),
                        heap,
                        private,
                        frozen_too,
                    ))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The plan: which top-level terms may be kept at all
// ---------------------------------------------------------------------------

/// The top-level terms a slot may fill, decided on the term graph before
/// lowering so the in-place analyses can be told (see the module docs,
/// "What keeps a kept value from changing").
#[derive(Debug, Clone, Default)]
pub struct StaticPlan {
    /// Root-block terms that are static data and whose value reaches no
    /// write.
    pub roots: HashSet<TermId>,
    /// Every term, in a function body too, whose value may be the
    /// container some mutation writes into (`mutated_terms`).
    pub mutated: HashSet<TermId>,
}

impl StaticPlan {
    pub fn of(program: &Program, analysis: &StaticAnalysis) -> StaticPlan {
        let mutated = mutated_terms(program);
        // The classifier's own answer for declarations, which follows the
        // same aliases; either one is enough to leave a term out.
        let declared_mutated: HashSet<TermId> = analysis
            .report
            .declarations
            .iter()
            .filter(|d| d.mutated_by.is_some())
            .map(|d| d.term)
            .collect();
        let mut roots = HashSet::new();
        if let Some(terms) = program.block_terms.get(&program.root_block) {
            for &t in terms {
                if analysis.term_class(t) == TermClass::Data
                    && !mutated.contains(&t)
                    && !declared_mutated.contains(&t)
                {
                    roots.insert(t);
                }
            }
        }
        StaticPlan { roots, mutated }
    }

    /// The root registers of those terms. The root block is the first block
    /// of the root function's register file, so a root term's register is
    /// its flat register.
    pub fn root_regs(&self, program: &Program) -> HashSet<Reg> {
        self.roots
            .iter()
            .filter_map(|t| program.terms.get(t.0 as usize))
            .map(|t| t.register.0)
            .collect()
    }
}

/// Whether `term` writes into its first operand's container: an indexed or
/// field write, or a mutating builtin called bare or as a method. These are
/// the terms lowering may turn into an in-place update.
fn is_mutation(program: &Program, term: &crate::program::Term) -> bool {
    let builtin = |c| {
        program
            .get_string_constant(c)
            .is_some_and(crate::builtins::is_mutating_builtin)
    };
    match term.op {
        TermOp::SetIndex | TermOp::SetField(_) => true,
        TermOp::BuiltinCall(c) => builtin(c),
        TermOp::MethodCall { name, .. } => builtin(name),
        _ => false,
    }
}

/// Every term whose value may be the container some mutation writes into:
/// the container operands themselves, and back from each through what
/// carries a value unchanged (a copy, a phi and its carry-outs, the value of
/// a branch or match arm, the container of an earlier mutation).
fn mutated_terms(program: &Program) -> HashSet<TermId> {
    let mut last: HashMap<BlockId, TermId> = HashMap::new();
    let mut carried: HashMap<TermId, Vec<TermId>> = HashMap::new();
    for block in &program.blocks {
        let mut cur = block.entry;
        let mut steps = 0;
        while let Some(t) = cur {
            last.insert(block.id, t);
            cur = program.terms.get(t.0 as usize).and_then(|t| t.block_next);
            steps += 1;
            if steps > program.terms.len() {
                break;
            }
        }
        for po in &block.phi_outs {
            carried.entry(po.dest_term).or_default().push(po.src_term);
        }
    }
    let mut work: Vec<TermId> = program
        .terms
        .iter()
        .filter(|t| is_mutation(program, t))
        .filter_map(|t| t.inputs.first().copied())
        .collect();
    let mut seen: HashSet<TermId> = HashSet::new();
    while let Some(t) = work.pop() {
        if !seen.insert(t) {
            continue;
        }
        let Some(term) = program.terms.get(t.0 as usize) else { continue };
        let arm_results = |work: &mut Vec<TermId>| {
            for b in &term.child_blocks {
                work.extend(last.get(b).copied());
            }
            if let Some(arms) = program.match_arms.get(&t) {
                for arm in arms {
                    work.extend(last.get(&arm.body_block).copied());
                }
            }
        };
        match term.op {
            TermOp::Copy => work.extend(term.inputs.iter().copied()),
            TermOp::Phi => {
                work.extend(term.inputs.iter().copied());
                work.extend(carried.get(&t).into_iter().flatten().copied());
            }
            TermOp::Branch | TermOp::Match => arm_results(&mut work),
            TermOp::And | TermOp::Or | TermOp::Coalesce => {
                work.extend(term.inputs.first().copied());
                arm_results(&mut work);
            }
            _ if is_mutation(program, term) => work.extend(term.inputs.first().copied()),
            _ => {}
        }
    }
    seen
}

/// A part of a declaration's build that is run again on every run although
/// the declaration is static data: see [`unkept_part`].
#[derive(Debug, Clone, PartialEq)]
pub struct UnkeptPart {
    /// The term that gets no slot: the container a mutation may write into.
    pub term: TermId,
    /// What may write into it: a builtin's name (`append`), or `None` for an
    /// indexed or field write.
    pub writer: Option<String>,
}

/// The first part of what builds the root term `decl` that is static data
/// and still gets no slot, because its value may be the container of an
/// in-place update ([`mutated_terms`], the first guard of "What keeps a kept
/// value from changing"). `decl` itself counts. The walk follows `decl`'s
/// operands through the root block and stops at `stop`: the terms of other
/// declarations, which answer for themselves.
///
/// This is what makes `const X = append(for … end, Y)` cost a frame what the
/// comprehension costs: `X` has a slot and the list `append` is handed has
/// none. `None` when every part is kept.
pub fn unkept_part(
    program: &Program,
    analysis: &StaticAnalysis,
    decl: TermId,
    stop: &HashSet<TermId>,
) -> Option<UnkeptPart> {
    let mutated = mutated_terms(program);
    let root: HashSet<TermId> = program
        .block_terms
        .get(&program.root_block)
        .map(|ts| ts.iter().copied().collect())
        .unwrap_or_default();
    let mut work = vec![decl];
    let mut seen: HashSet<TermId> = HashSet::new();
    let mut found = None;
    while let Some(t) = work.pop() {
        if !seen.insert(t) || !root.contains(&t) {
            continue;
        }
        if mutated.contains(&t) && analysis.term_class(t) == TermClass::Data {
            // The earliest in the source is the one to name.
            if found.is_none_or(|f: TermId| t.0 < f.0) {
                found = Some(t);
            }
        }
        let Some(term) = program.terms.get(t.0 as usize) else { continue };
        work.extend(term.inputs.iter().copied().filter(|i| !stop.contains(i)));
    }
    let term = found?;
    // What writes into it: the mutation whose container operand it is.
    let writer = program
        .terms
        .iter()
        .find(|w| is_mutation(program, w) && w.inputs.first() == Some(&term))
        .and_then(|w| match w.op {
            TermOp::BuiltinCall(c) => program.get_string_constant(c).map(str::to_string),
            TermOp::MethodCall { name, .. } => program.get_string_constant(name).map(str::to_string),
            _ => None,
        });
    Some(UnkeptPart { term, writer })
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Whether an instruction may sit in a run that is skipped. Everything a
/// static statement can lower to is here; what is not either cannot be
/// static (`state`, cells) or must run regardless (`return`).
fn skippable_inst(inst: &Inst, program: &Program, natives: &NativeFnTable) -> bool {
    match inst {
        Inst::BuiltinCall { name, .. } => {
            let Some(name) = program.get_string_constant(*name) else { return false };
            // The two enum reads answer from the program's own declarations,
            // which is how the classifier takes them. `__enum_data` publishes
            // a variant's record into the stack's enum table, which keeps it
            // for as long as the stack keeps its static values (both are
            // emptied by a reload, and a table rebuilt for another lowering
            // meets another layout id): a skipped publication leaves the
            // record of the run that was not skipped where it is. The record
            // gets a slot of its own (see `apply`), so the run is skipped
            // only while the kept record is the published one.
            natives.constness_of_name(name) != NativeConstness::Dynamic
                || name == crate::program::ENUM_ALL_BUILTIN
                || name == crate::program::ENUM_FROM_BUILTIN
                || name == crate::program::ENUM_DATA_BUILTIN
        }
        Inst::Return { .. }
        | Inst::CellNew { .. }
        | Inst::CellRead { .. }
        | Inst::CellWrite { .. }
        | Inst::StateInit { .. }
        | Inst::StateRead { .. }
        | Inst::StateWrite { .. }
        | Inst::StaticSkip { .. }
        | Inst::StaticStore { .. } => false,
        _ => true,
    }
}

/// What an instruction adds to its run's weight (see [`MIN_WEIGHT`]).
fn weight(inst: &Inst) -> u32 {
    match inst {
        Inst::AllocList { .. }
        | Inst::AllocMap { .. }
        | Inst::AllocMapSpread { .. }
        | Inst::AllocElement { .. }
        | Inst::Concat { .. }
        | Inst::SetField { .. }
        | Inst::SetIndex { .. } => 1,
        Inst::MakeEnumVariant { fields, .. } if !fields.is_empty() => 1,
        Inst::Call { .. }
        | Inst::MethodCall { .. }
        | Inst::BuiltinCall { .. }
        | Inst::ForEachInit { .. }
        | Inst::RangeInit { .. }
        | Inst::WhileInit { .. } => 2,
        _ => 0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Depends on dynamic data: what it writes is not static.
    Dynamic,
    /// Static, but executes on every run.
    Always,
    /// Static and may be skipped.
    Candidate,
}

/// One segment of a stretch of candidate instructions: `start..end` of the
/// root code, ending behind the last write of the registers it stores.
struct Segment {
    start: usize,
    end: usize,
    /// The registers it writes that anything outside it reads.
    own: Vec<Reg>,
    /// Index into [`StaticLayout::runs`].
    index: u32,
    first_slot: u32,
    /// The stretch it is part of, and whether it is its first segment.
    stretch: usize,
}

/// A stretch that is kept: its segments (indices into the pass's list),
/// and the group run in front of them when there is more than one.
struct Kept {
    start: usize,
    end: usize,
    segments: std::ops::Range<usize>,
    group: Option<u32>,
}

/// Cut the stretch `start..end` into segments, one behind each register
/// that something outside its own segment reads, and return for each
/// segment its end and those registers. `ext` are the registers something
/// outside the stretch reads. `None` when no cut is possible or sound, and
/// the stretch is then one segment that stores `ext`.
///
/// A cut is never placed inside a top-level statement, across a jump, or
/// between two writes of a register that gets a slot; a register's slot is
/// stored by the segment holding all its writes. A register read across a
/// cut needs a slot of its own, which may add a cut, so this runs to a fixed
/// point. The last segment runs to the end of the stretch, so static code
/// that nothing reads is skipped, and raises, with a value that is read.
#[allow(clippy::too_many_arguments)]
fn cut(
    start: usize,
    end: usize,
    ext: &[Reg],
    code: &[Inst],
    stmt: &[Option<TermId>],
    reads: &[Vec<Reg>],
    writes: &[Vec<Reg>],
    published: &HashMap<usize, Reg>,
    storable: &dyn Fn(Reg) -> bool,
) -> Option<Vec<(usize, Vec<Reg>)>> {
    let len = end - start;
    // `no_cut[c - start]`: no boundary between instruction `c - 1` and `c`.
    let mut no_cut = vec![false; len + 1];
    let forbid = |no_cut: &mut Vec<bool>, from: usize, to: usize| {
        for c in from.max(start + 1)..=to.min(end.saturating_sub(1)) {
            no_cut[c - start] = true;
        }
    };
    // Inside a statement: a loop's setup and its exit, a branch and its
    // arms, and whatever else lowering put between them.
    let mut first_of: HashMap<TermId, usize> = HashMap::new();
    let mut last_of: HashMap<TermId, usize> = HashMap::new();
    for ip in start..end {
        let s = stmt[ip]?;
        first_of.entry(s).or_insert(ip);
        last_of.insert(s, ip);
    }
    for (s, &f) in &first_of {
        forbid(&mut no_cut, f + 1, last_of[s]);
    }
    // Across a jump: over a forward one, or into the loop of a backward one.
    for ip in start..end {
        for to in code[ip].targets() {
            let to = to as usize;
            if to > ip {
                forbid(&mut no_cut, ip + 1, to.saturating_sub(1));
            } else {
                forbid(&mut no_cut, to + 1, ip);
            }
        }
    }
    // Where each register is first and last written. The publication of an
    // enum's record counts as its last write: the segment that stores the
    // record is the one that publishes it.
    let mut first: HashMap<Reg, usize> = HashMap::new();
    let mut last: HashMap<Reg, usize> = HashMap::new();
    let mut touched: HashMap<Reg, Vec<usize>> = HashMap::new();
    for ip in start..end {
        for &r in &writes[ip] {
            first.entry(r).or_insert(ip);
            last.insert(r, ip);
            touched.entry(r).or_default().push(ip);
        }
    }
    for ip in start..end {
        if let Some(r) = published.get(&ip)
            && let Some(l) = last.get_mut(r)
        {
            *l = (*l).max(ip);
        }
        for r in &reads[ip] {
            if let Some(at) = touched.get_mut(r) {
                at.push(ip);
            }
        }
    }
    let mut slots: BTreeSet<Reg> = ext.iter().copied().collect();
    let mut seg_of = vec![0usize; len];
    let mut ends: Vec<usize>;
    loop {
        if !slots.iter().all(|r| storable(*r)) {
            return None;
        }
        for r in &slots {
            forbid(&mut no_cut, first[r] + 1, last[r]);
        }
        let mut cuts: BTreeSet<usize> = BTreeSet::new();
        for r in &slots {
            let mut c = last[r] + 1;
            while c < end && no_cut[c - start] {
                c += 1;
            }
            cuts.insert(c);
        }
        ends = cuts.into_iter().collect();
        // What follows the last stored value runs with it.
        *ends.last_mut()? = end;
        let mut k = 0;
        for ip in start..end {
            while ip >= ends[k] {
                k += 1;
            }
            seg_of[ip - start] = k;
        }
        let before = slots.len();
        for (r, at) in &touched {
            if !slots.contains(r) && at.iter().any(|ip| seg_of[ip - start] != seg_of[at[0] - start]) {
                slots.insert(*r);
            }
        }
        if slots.len() == before {
            break;
        }
    }
    if ends.len() < 2 {
        return None;
    }
    let mut out: Vec<(usize, Vec<Reg>)> = ends.iter().map(|e| (*e, Vec::new())).collect();
    for r in &slots {
        // Every write of a stored register is in the segment that stores it.
        let k = seg_of[last[r] - start];
        if seg_of[first[r] - start] != k {
            return None;
        }
        out[k].1.push(*r);
    }
    // A record is published by the segment that stores it.
    for ip in start..end {
        if let Some(r) = published.get(&ip)
            && !out[seg_of[ip - start]].1.contains(r)
        {
            return None;
        }
    }
    out.iter().all(|(_, own)| !own.is_empty()).then_some(out)
}

/// Wrap the runs of static instructions in `bc`'s root code in
/// `StaticSkip` / `StaticStore` and record the layout in `bc.statics`. See
/// the module docs for the rules. Runs after every other pass: it reads
/// registers and labels as they finally are, and nothing after it moves
/// code.
///
/// `observes`: the lowering preserves observations, so a skipped segment
/// must report what it observed (the module docs, "Observations").
pub fn apply(
    bc: &mut BytecodeProgram,
    program: &Program,
    analysis: &StaticAnalysis,
    natives: &NativeFnTable,
    plan: &StaticPlan,
    observes: bool,
    in_functions: bool,
) {
    let mut slots = apply_runs(bc, program, analysis, natives, plan, observes);
    bc.statics.root_slots = bc.statics.slots.len();
    bc.statics.root_runs = bc.statics.runs.len();
    slots.resize(bc.statics.root_slots, SlotFollow::default());
    // Values built inside function bodies. Not where observations are
    // preserved: a skipped stretch would have to report the names bound
    // inside it, per call.
    if in_functions && !observes {
        in_fn::apply(bc, program, analysis, natives, plan, &mut slots);
    }
    follow(&mut bc.statics, slots, program, analysis);
}

/// Record what a reload needs to follow an edit through this lowering: the
/// program's declarations, and for each slot what it is matched by and what
/// it depends on (see the module docs, "Following the diff"). `slots` comes
/// with each slot's writers and enum tag filled in.
fn follow(
    layout: &mut StaticLayout,
    mut slots: Vec<SlotFollow>,
    program: &Program,
    analysis: &StaticAnalysis,
) {
    slots.resize(layout.slots.len(), SlotFollow::default());
    let decls: Vec<FollowDecl> = analysis
        .report
        .declarations
        .iter()
        .enumerate()
        .map(|(i, d)| FollowDecl {
            name: d.qualified.clone(),
            file: d.file_id,
            defs: d.defs.clone(),
            state: (d.kind == crate::static_classify::DeclKind::State)
                .then(|| program.get_term(d.term).state_key)
                .flatten()
                .map(|key| {
                    // What it was seeded from: its initializer's reads, not
                    // those of the statements that assign it afterwards (a
                    // counter set from a table on every run is not stale).
                    let mut deps = analysis.state_initializer_dependencies(d.term).unwrap_or_default();
                    deps.retain(|x| *x != i as u32);
                    (key, deps.into())
                }),
            cache: (d.kind == crate::static_classify::DeclKind::State)
                .then(|| analysis.state_cache_dependencies(d.term))
                .flatten()
                .map(|mut deps| {
                    deps.retain(|x| *x != i as u32);
                    deps.into()
                }),
        })
        .collect();
    let canonical: HashMap<TermId, &crate::static_classify::StaticDecl> =
        analysis.report.declarations.iter().map(|d| (d.term, d)).collect();
    let parents = crate::static_audit::block_parents(program);
    for (slot, info) in slots.iter_mut().zip(&layout.slots) {
        // Everything any writer of the register depends on. One writer the
        // classifier knows nothing about makes the whole set unknown.
        let mut deps: Option<BTreeSet<u32>> = Some(BTreeSet::new());
        if slot.origins.is_empty() {
            deps = None;
        }
        for &t in &slot.origins {
            let of = crate::static_audit::root_statement(program, &parents, t)
                .and_then(|root| analysis.root_dependencies(program, root));
            match (of, deps.as_mut()) {
                (Some(of), Some(all)) => all.extend(of),
                _ => deps = None,
            }
        }
        slot.deps = deps.map(|d| d.into_iter().collect());
        slot.literal = match slot.origins[..] {
            [t] if matches!(program.get_term(t).op, TermOp::Constant(_)) => Some(t),
            _ => None,
        };
        // A writer with no position of its own (the record of an enum's
        // data) that belongs to a declaration is placed by the declaration,
        // which is in the dependency set; only the writers a reload has to
        // place itself stay.
        slot.origins.retain(|t| {
            program.source_map.get(*t).is_some_and(|s| s.start.line > 0)
                || crate::static_audit::root_statement(program, &parents, *t)
                    .and_then(|root| analysis.declaration_of(root))
                    .is_none()
        });
        slot.key = match &slot.enum_tag {
            // A tag is unique in a program, and no declaration's name
            // holds a NUL.
            Some(tag) => Some(format!("\u{0}enum\u{0}{tag}")),
            None => info
                .term
                .and_then(|t| canonical.get(&t))
                .filter(|d| d.kind != crate::static_classify::DeclKind::Enum)
                .map(|d| d.qualified.clone()),
        };
    }
    unnamed_keys(&mut slots, program);
    // A value built inside a function is matched by nothing and depends on
    // nothing a reload asks about: every reload rebuilds it.
    for slot in slots.iter_mut().skip(layout.root_slots) {
        *slot = SlotFollow::default();
    }
    // A key two slots share names neither.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for slot in &slots {
        if let Some(k) = &slot.key {
            *seen.entry(k.clone()).or_default() += 1;
        }
    }
    for slot in &mut slots {
        if slot.key.as_ref().is_some_and(|k| seen.get(k).copied().unwrap_or(0) > 1) {
            slot.key = None;
        }
    }
    // What each segment's observation log depends on: every statement in
    // it, stored or not.
    let runs: Vec<RunFollow> = std::mem::take(&mut layout.run_origins)
        .into_iter()
        .map(|origins| {
            let mut deps: Option<BTreeSet<u32>> = Some(BTreeSet::new());
            for &t in &origins {
                let of = crate::static_audit::root_statement(program, &parents, t)
                    .and_then(|root| analysis.root_dependencies(program, root));
                match (of, deps.as_mut()) {
                    (Some(of), Some(all)) => all.extend(of),
                    _ => deps = None,
                }
            }
            RunFollow { deps: deps.map(|d| d.into_iter().collect()), origins }
        })
        .collect();
    layout.follow = Arc::new(FollowInfo { decls, slots, runs });
}

/// Keys for the slots that hold a value with no declaration of its own
/// (the `[1, 2, 3]` written inside a call, the `len(TILES)` hoisted out of
/// one): what a reload matches them by across two lowerings.
///
/// Such a value is written inside exactly one top-level statement. The key
/// is that statement's text, the shape the statement compiled to (the
/// operation, position and name of each of its top-level terms, and the
/// name of a `var` it initializes), and where in that sequence the value's
/// term is. Two programs that give the same
/// key compiled the same text to the same terms, so the term is the same
/// expression; whether what it refers to is unchanged is the dependency
/// set's question, as for a declaration. A statement written twice gives
/// its values the same keys, and a key two slots share is dropped.
fn unnamed_keys(slots: &mut [SlotFollow], program: &Program) {
    use std::hash::{Hash, Hasher};
    if slots.iter().all(|s| s.key.is_some()) {
        return;
    }
    let Some(root) = program.block_terms.get(&program.root_block) else { return };
    // Every positioned term of the root block, in block order.
    struct At {
        term: TermId,
        file: u16,
        start: u32,
        end: u32,
    }
    let placed: Vec<At> = root
        .iter()
        .filter_map(|t| {
            let span = program.source_map.get(*t).filter(|s| s.start.line > 0)?;
            Some(At {
                term: *t,
                file: span.file.0,
                start: span.start.offset,
                end: span.end.offset.max(span.start.offset),
            })
        })
        .collect();
    // The statements: the spans no other span contains.
    let mut order: Vec<usize> = (0..placed.len()).collect();
    order.sort_by_key(|&i| (placed[i].file, placed[i].start, std::cmp::Reverse(placed[i].end)));
    let mut statements: Vec<(u16, u32, u32)> = Vec::new();
    for &i in &order {
        let a = &placed[i];
        match statements.last() {
            Some(&(file, _, end)) if file == a.file && a.end <= end => {}
            _ => statements.push((a.file, a.start, a.end)),
        }
    }
    let statement_of = |file: u16, start: u32| -> Option<usize> {
        let at = statements.partition_point(|s| (s.0, s.1) <= (file, start));
        let i = at.checked_sub(1)?;
        let (f, s, e) = statements[i];
        (f == file && s <= start && start < e.max(s + 1)).then_some(i)
    };
    // The terms of each statement that holds an unnamed slot.
    let position: HashMap<TermId, usize> =
        placed.iter().enumerate().map(|(i, a)| (a.term, i)).collect();
    let mut wanted: HashMap<usize, Vec<usize>> = HashMap::new();
    let slot_at = |slot: &SlotFollow| -> Option<(usize, usize)> {
        if slot.key.is_some() {
            return None;
        }
        let [t] = slot.origins[..] else { return None };
        let i = *position.get(&t)?;
        Some((statement_of(placed[i].file, placed[i].start)?, i))
    };
    for slot in slots.iter() {
        if let Some((st, _)) = slot_at(slot) {
            wanted.entry(st).or_default();
        }
    }
    if wanted.is_empty() {
        return;
    }
    for (i, a) in placed.iter().enumerate() {
        if let Some(st) = statement_of(a.file, a.start)
            && let Some(terms) = wanted.get_mut(&st)
        {
            terms.push(i);
        }
    }
    // What a statement's value is bound to, when the binding has no
    // position of its own: the cell of `var PARTS = []`. Without it the
    // statement is the text `[]`, which the next `var` shares.
    let mut bound: HashMap<usize, Vec<&str>> = HashMap::new();
    for t in root {
        if position.contains_key(t) {
            continue;
        }
        let term = program.get_term(*t);
        let (Some(name), Some(i)) = (term.name.as_deref(), term.inputs.first().and_then(|i| position.get(i)))
        else {
            continue;
        };
        if let Some(st) = statement_of(placed[*i].file, placed[*i].start)
            && wanted.contains_key(&st)
        {
            bound.entry(st).or_default().push(name);
        }
    }
    let mut texts: HashMap<u16, Vec<char>> = HashMap::new();
    let mut prefix: HashMap<usize, String> = HashMap::new();
    for (&st, terms) in &wanted {
        let (file, start, end) = statements[st];
        let chars = texts.entry(file).or_insert_with(|| {
            match program.source_map.files.get(file as usize) {
                Some(f) => f.source.chars().collect(),
                None => program.source.chars().collect(),
            }
        });
        let Some(text) = chars.get(start as usize..end as usize) else { continue };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        bound.get(&st).hash(&mut h);
        terms.len().hash(&mut h);
        for &i in terms {
            let term = program.get_term(placed[i].term);
            std::mem::discriminant(&term.op).hash(&mut h);
            (placed[i].start - start, placed[i].end - start).hash(&mut h);
            term.name.hash(&mut h);
            term.inputs.len().hash(&mut h);
        }
        let name = program
            .source_map
            .files
            .get(file as usize)
            .map(|f| f.name.as_str())
            .unwrap_or("");
        prefix.insert(st, format!("\u{0}{name}\u{0}{:016x}", h.finish()));
    }
    let keys: Vec<Option<String>> = slots
        .iter()
        .map(|slot| {
            let (st, i) = slot_at(slot)?;
            let at = wanted.get(&st)?.iter().position(|x| *x == i)?;
            Some(format!("{}\u{0}{at}", prefix.get(&st)?))
        })
        .collect();
    for (slot, key) in slots.iter_mut().zip(keys) {
        if key.is_some() {
            slot.key = key;
        }
    }
}

/// The runs and slots themselves. Returns, per slot, the terms that write
/// its register and the enum tag it is the data of.
fn apply_runs(
    bc: &mut BytecodeProgram,
    program: &Program,
    analysis: &StaticAnalysis,
    natives: &NativeFnTable,
    plan: &StaticPlan,
    observes: bool,
) -> Vec<SlotFollow> {
    let mut written_by: Vec<SlotFollow> = Vec::new();
    let mut layout = StaticLayout {
        id: NEXT_LAYOUT_ID.fetch_add(1, Ordering::Relaxed),
        observes,
        ..Default::default()
    };
    let n = bc.root.code.len();
    layout.root_instructions = n;
    if n == 0 || plan.roots.is_empty() {
        bc.statics = layout;
        return written_by;
    }
    let class = |t: TermId| analysis.term_class(t);
    let origin = |ip: usize| bc.root.origins.get(ip).copied().flatten();

    // The top-level statement each root instruction belongs to. An
    // instruction with no origin of its own (a loop's bookkeeping) goes with
    // the one before it, and leading ones with the first statement.
    let parents = crate::static_audit::block_parents(program);
    let mut stmt: Vec<Option<TermId>> = Vec::with_capacity(n);
    for ip in 0..n {
        let own = origin(ip).and_then(|t| crate::static_audit::root_statement(program, &parents, t));
        let prev = stmt.last().copied().flatten();
        stmt.push(own.or(prev));
    }
    if let Some(first) = stmt.iter().copied().flatten().next() {
        for s in stmt.iter_mut().take_while(|s| s.is_none()) {
            *s = Some(first);
        }
    }

    let mut reads: Vec<Vec<Reg>> = Vec::with_capacity(n);
    let mut writes: Vec<Vec<Reg>> = Vec::with_capacity(n);
    for inst in &bc.root.code {
        reads.push(inst.read_regs().to_vec());
        let mut w = Vec::new();
        inst.for_each_write(&bc.match_binds, |r| w.push(r));
        writes.push(w);
    }

    // Statements that are out as a whole.
    let mut demoted: HashSet<TermId> = HashSet::new();
    for ip in 0..n {
        let Some(s) = stmt[ip] else { continue };
        if !plan.roots.contains(&s) {
            continue;
        }
        let own_dynamic = origin(ip).is_some_and(|t| class(t) == TermClass::Dynamic);
        if own_dynamic || !skippable_inst(&bc.root.code[ip], program, natives) {
            demoted.insert(s);
        }
    }
    // Under observation, an instruction that edits a container in place is
    // only left out with the statement that built the container. The
    // optimizer writes in place where the program can no longer reach the
    // old value, but the observation buffer still names it: `let BASE =
    // [..]` that is rebuilt on every run, then `let cur = drop_last(BASE)`
    // in place, shows the shortened list under both names. A frame that
    // skipped the second statement would show the whole list under `BASE`.
    if observes {
        for ip in 0..n {
            let Some(s) = stmt[ip] else { continue };
            let container = match &bc.root.code[ip] {
                Inst::SetFieldInPlace { obj, .. } | Inst::SetIndexInPlace { obj, .. } => Some(*obj),
                Inst::BuiltinCall { args, in_place: true, .. } => args.first().copied(),
                _ => None,
            };
            let Some(container) = container else { continue };
            // The registers the container may have come through unchanged.
            let mut carried: Vec<Reg> = vec![container];
            let mut at = 0;
            while at < carried.len() {
                let r = carried[at];
                at += 1;
                for inst in &bc.root.code {
                    let from = match inst {
                        Inst::Move { dst, src } if *dst == r => Some(*src),
                        Inst::MatchArm { subject, term, arm, .. }
                            if bc.match_binds.get(&(*term, *arm)).is_some_and(|b| b.iter().any(|(_, x)| *x == r)) =>
                        {
                            Some(*subject)
                        }
                        _ => None,
                    };
                    if let Some(from) = from.filter(|f| !carried.contains(f)) {
                        carried.push(from);
                    }
                }
            }
            let foreign = (0..n).any(|w| stmt[w] != Some(s) && writes[w].iter().any(|r| carried.contains(r)));
            if foreign {
                demoted.insert(s);
            }
        }
    }

    let mut kind = vec![Kind::Dynamic; n];
    let mut tainted: HashSet<Reg> = HashSet::new();
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut run_of: Vec<Option<usize>> = vec![None; n];
    // Each round takes at least one statement out, so this ends.
    loop {
        for ip in 0..n {
            let stmt_dynamic = stmt[ip].is_none_or(|s| class(s) == TermClass::Dynamic);
            let own_dynamic = origin(ip).is_some_and(|t| class(t) == TermClass::Dynamic);
            kind[ip] = if stmt_dynamic || own_dynamic {
                Kind::Dynamic
            } else if stmt[ip].is_some_and(|s| plan.roots.contains(&s) && !demoted.contains(&s)) {
                Kind::Candidate
            } else {
                Kind::Always
            };
        }
        // A register any dynamic instruction writes is not static, whoever
        // else writes it.
        tainted.clear();
        for ip in 0..n {
            if kind[ip] == Kind::Dynamic {
                tainted.extend(writes[ip].iter().copied());
            }
        }
        let mut changed = false;
        for ip in 0..n {
            if kind[ip] == Kind::Candidate
                && reads[ip].iter().any(|r| tainted.contains(r))
                && let Some(s) = stmt[ip]
            {
                changed |= demoted.insert(s);
            }
        }
        if changed {
            continue;
        }

        bounds.clear();
        run_of.iter_mut().for_each(|r| *r = None);
        let mut ip = 0;
        while ip < n {
            if kind[ip] != Kind::Candidate {
                ip += 1;
                continue;
            }
            let start = ip;
            while ip < n && kind[ip] == Kind::Candidate {
                run_of[ip] = Some(bounds.len());
                ip += 1;
            }
            bounds.push((start, ip));
        }
        // Control enters a run at its first instruction and leaves it at its
        // end, or the run cannot be jumped over.
        for ip in 0..n {
            for to in bc.root.code[ip].targets() {
                let to = to as usize;
                if let Some(k) = run_of[ip] {
                    let (s, e) = bounds[k];
                    if (to < s || to > e)
                        && let Some(st) = stmt[ip]
                    {
                        changed |= demoted.insert(st);
                    }
                }
                if to < n
                    && let Some(j) = run_of[to]
                    && run_of[ip] != Some(j)
                    && to != bounds[j].0
                    && let Some(st) = stmt[to]
                {
                    changed |= demoted.insert(st);
                }
            }
        }
        if !changed {
            break;
        }
    }
    layout.candidate_runs = bounds.len();
    if bounds.is_empty() {
        bc.statics = layout;
        return written_by;
    }

    // Registers that reach the container operand of an in-place instruction,
    // back through what carries a value unchanged. No slot may fill one.
    let mut in_place: HashSet<Reg> = HashSet::new();
    for inst in &bc.root.code {
        match inst {
            Inst::SetFieldInPlace { obj, .. } | Inst::SetIndexInPlace { obj, .. } => {
                in_place.insert(*obj);
            }
            Inst::BuiltinCall { args, in_place: true, .. } => {
                in_place.extend(args.first().copied());
            }
            _ => {}
        }
    }
    loop {
        let before = in_place.len();
        for inst in &bc.root.code {
            match inst {
                Inst::Move { dst, src } if in_place.contains(dst) => {
                    in_place.insert(*src);
                }
                Inst::MatchArm { subject, term, arm, .. } => {
                    let binds = bc.match_binds.get(&(*term, *arm));
                    if binds.is_some_and(|b| b.iter().any(|(_, r)| in_place.contains(r))) {
                        in_place.insert(*subject);
                    }
                }
                _ => {}
            }
        }
        if in_place.len() == before {
            break;
        }
    }

    // The record each `__enum_data` call publishes, and the variant it is
    // the data of. A run that publishes one keeps the record in a slot,
    // whoever else reads it: that is what marks it immutable, what refuses
    // a record that cannot be kept, and what makes the run something a
    // frame can skip at all (nothing reads the call's own result).
    let mut published: HashMap<usize, Reg> = HashMap::new();
    let mut published_name: HashMap<Reg, String> = HashMap::new();
    let mut published_tag: HashMap<Reg, String> = HashMap::new();
    for (ip, inst) in bc.root.code.iter().enumerate() {
        let Inst::BuiltinCall { name, args, .. } = inst else { continue };
        if program.get_string_constant(*name) != Some(crate::program::ENUM_DATA_BUILTIN) {
            continue;
        }
        let Some(&record) = args.get(1) else { continue };
        published.insert(ip, record);
        let tag = origin(ip)
            .and_then(|t| program.get_term(t).inputs.first().copied())
            .and_then(|t| match program.get_term(t).op {
                TermOp::Constant(c) => program.get_string_constant(c),
                _ => None,
            });
        if let Some(tag) = tag {
            published_name.insert(record, format!("enum data of {tag}"));
            published_tag.insert(record, tag.to_string());
        }
    }

    let mut readers: HashMap<Reg, Vec<usize>> = HashMap::new();
    for (ip, rs) in reads.iter().enumerate() {
        for r in rs {
            readers.entry(*r).or_default().push(ip);
        }
    }

    // Which stretches are kept, and where each is cut.
    let storable = |r: Reg| !tainted.contains(&r) && !in_place.contains(&r);
    let mut segments: Vec<Segment> = Vec::new();
    let mut kept: Vec<Kept> = Vec::new();
    for &(start, end) in &bounds {
        let stretch = &bc.root.code[start..end];
        if stretch.iter().map(weight).sum::<u32>() < MIN_WEIGHT {
            continue;
        }
        let mut written: Vec<Reg> = (start..end).flat_map(|ip| writes[ip].iter().copied()).collect();
        written.sort_unstable();
        written.dedup();
        let records: HashSet<Reg> = (start..end).filter_map(|ip| published.get(&ip).copied()).collect();
        // A record this stretch publishes but does not build cannot be kept
        // by it; the publication then runs every time.
        if !records.iter().all(|r| written.binary_search(r).is_ok()) {
            continue;
        }
        // What anything outside the stretch reads: every one of those gets
        // a slot, whoever the reader is. A later stretch that is rebuilt
        // while this one is skipped reads the kept value, not a new copy.
        let ext: Vec<Reg> = written
            .iter()
            .copied()
            .filter(|r| {
                bc.root.result_reg == Some(*r)
                    || records.contains(r)
                    || readers.get(r).is_some_and(|at| at.iter().any(|ip| !(start..end).contains(ip)))
            })
            .collect();
        // A stretch nothing reads always executes: an unused failing
        // binding raises at its line on every run.
        if ext.is_empty() {
            continue;
        }
        let parts = cut(start, end, &ext, &bc.root.code, &stmt, &reads, &writes, &published, &storable)
            .or_else(|| ext.iter().all(|r| storable(*r)).then(|| vec![(end, ext.clone())]));
        let Some(parts) = parts else { continue };
        let from = segments.len();
        let mut at = start;
        for (to, own) in parts {
            segments.push(Segment { start: at, end: to, own, index: 0, first_slot: 0, stretch: kept.len() });
            at = to;
        }
        kept.push(Kept { start, end, segments: from..segments.len(), group: None });
    }

    // Number the runs and the slots in code order.
    let decl_names: HashMap<TermId, &str> = analysis
        .report
        .declarations
        .iter()
        .map(|d| (d.term, d.qualified.as_str()))
        .collect();
    let mut writer_term: HashMap<Reg, TermId> = HashMap::new();
    let mut writer_terms: HashMap<Reg, Vec<TermId>> = HashMap::new();
    // A register some instruction with no term behind it writes: nothing
    // says what that value depends on.
    let mut unattributed: HashSet<Reg> = HashSet::new();
    for ip in 0..n {
        match origin(ip) {
            Some(t) => {
                for r in &writes[ip] {
                    writer_term.insert(*r, t);
                    let all = writer_terms.entry(*r).or_default();
                    if !all.contains(&t) {
                        all.push(t);
                    }
                }
            }
            None => unattributed.extend(writes[ip].iter().copied()),
        }
    }
    let allocations = |code: &[Inst]| {
        code.iter()
            .filter(|i| {
                matches!(
                    i,
                    Inst::AllocList { .. }
                        | Inst::AllocMap { .. }
                        | Inst::AllocMapSpread { .. }
                        | Inst::AllocElement { .. }
                )
            })
            .count()
    };
    let mut slot_of: HashMap<Reg, u32> = HashMap::new();
    for stretch in kept.iter_mut() {
        if stretch.segments.len() > 1 {
            stretch.group = Some(layout.runs.len() as u32);
            layout.runs.push(StaticRun {
                instructions: stretch.end - stretch.start,
                allocations: allocations(&bc.root.code[stretch.start..stretch.end]),
                ..Default::default()
            });
        }
        for seg in &mut segments[stretch.segments.clone()] {
            seg.index = layout.runs.len() as u32;
            seg.first_slot = layout.slots.len() as u32;
            for r in &seg.own {
                // A root term's register is its flat register, so the term
                // that names the value is the root term with this register
                // when there is one, else whatever instruction wrote it.
                let term = plan
                    .roots
                    .iter()
                    .copied()
                    .find(|t| program.get_term(*t).register.0 == *r && decl_names.contains_key(t))
                    .or_else(|| writer_term.get(r).copied());
                slot_of.insert(*r, layout.slots.len() as u32);
                layout.slots.push(StaticSlotInfo {
                    name: published_name.get(r).cloned().or_else(|| {
                        term.and_then(|t| {
                            decl_names
                                .get(&t)
                                .map(|s| s.to_string())
                                .or_else(|| program.get_term(t).name.clone())
                        })
                    }),
                    at: term.and_then(|t| crate::static_audit::position(program, t)),
                    term,
                    is_const: term.is_some_and(|t| program.get_term(t).is_const),
                    function: None,
                });
                written_by.push(SlotFollow {
                    key: None,
                    deps: None,
                    origins: if unattributed.contains(r) {
                        Vec::new()
                    } else {
                        writer_terms.get(r).cloned().unwrap_or_default()
                    },
                    enum_tag: published_tag.get(r).cloned(),
                    literal: None,
                });
            }
            let stores: Vec<u32> = (0..seg.own.len() as u32).map(|i| seg.first_slot + i).collect();
            if let Some(g) = stretch.group {
                layout.runs[g as usize].feeds.extend(stores.iter().copied());
            }
            layout.runs.push(StaticRun {
                feeds: stores.clone(),
                stores,
                reads: Vec::new(),
                group: stretch.group,
                instructions: seg.end - seg.start,
                allocations: allocations(&bc.root.code[seg.start..seg.end]),
            });
        }
    }
    // The kept values each segment is built from directly.
    for seg in &segments {
        let mut from: BTreeSet<u32> = BTreeSet::new();
        for ip in seg.start..seg.end {
            from.extend(reads[ip].iter().filter(|r| !seg.own.contains(r)).filter_map(|r| slot_of.get(r)));
        }
        layout.runs[seg.index as usize].reads = from.into_iter().collect();
    }
    if observes {
        layout.run_origins = vec![Vec::new(); layout.runs.len()];
        for seg in &segments {
            let mut seen: HashSet<TermId> = HashSet::new();
            layout.run_origins[seg.index as usize] =
                (seg.start..seg.end).filter_map(origin).filter(|t| seen.insert(*t)).collect();
        }
    }
    layout.index();
    if layout.runs.is_empty() {
        bc.statics = layout;
        return written_by;
    }

    // Insert the instructions and move every label. In front of an old
    // instruction go, in this order: the store of the segment that ends
    // there, the skip of the group that starts there, the skip of the
    // segment that starts there.
    let seg_start: HashMap<usize, usize> = segments.iter().enumerate().map(|(i, s)| (s.start, i)).collect();
    let seg_end: HashMap<usize, usize> = segments.iter().enumerate().map(|(i, s)| (s.end, i)).collect();
    let group_start: HashMap<usize, usize> = kept
        .iter()
        .enumerate()
        .filter(|(_, k)| k.group.is_some())
        .map(|(i, k)| (k.start, i))
        .collect();
    let mut seg_at: Vec<Option<usize>> = vec![None; n];
    for (i, s) in segments.iter().enumerate() {
        seg_at[s.start..s.end].iter_mut().for_each(|x| *x = Some(i));
    }
    let slots_of = |seg: &Segment| -> Vec<(u32, Reg)> {
        seg.own.iter().enumerate().map(|(i, r)| (seg.first_slot + i as u32, *r)).collect()
    };
    let old_code = std::mem::take(&mut bc.root.code);
    let old_origins = std::mem::take(&mut bc.root.origins);
    let mut code: Vec<Inst> = Vec::with_capacity(n + 2 * layout.runs.len());
    let mut origins: Vec<Option<TermId>> = Vec::with_capacity(code.capacity());
    // Where each old instruction lands, and where what was put in front of
    // it does.
    let mut new_pos: Vec<usize> = vec![0; n + 1];
    let mut skip_pos: Vec<usize> = vec![0; segments.len()];
    let mut store_pos: Vec<usize> = vec![0; segments.len()];
    let mut group_pos: Vec<usize> = vec![0; kept.len()];
    // Which old instruction each new one was, for the labels.
    let mut was: Vec<Option<usize>> = Vec::with_capacity(code.capacity());
    let mut old = old_code.into_iter().zip(old_origins.into_iter().chain(std::iter::repeat(None)));
    for ip in 0..=n {
        if let Some(&i) = seg_end.get(&ip) {
            store_pos[i] = code.len();
            code.push(Inst::StaticStore { slots: slots_of(&segments[i]).into() });
            origins.push(None);
            was.push(None);
        }
        if let Some(&k) = group_start.get(&ip) {
            // A frame that skips the group loads what anything outside the
            // stretch reads; the rest is only read by the stretch itself.
            let stretch = &kept[k];
            let outside = |r: &Reg| {
                bc.root.result_reg == Some(*r)
                    || readers
                        .get(r)
                        .is_some_and(|at| at.iter().any(|ip| !(stretch.start..stretch.end).contains(ip)))
            };
            let loads: Vec<(u32, Reg)> = segments[stretch.segments.clone()]
                .iter()
                .flat_map(slots_of)
                .filter(|(_, r)| outside(r))
                .collect();
            group_pos[k] = code.len();
            code.push(Inst::StaticSkip { after: 0, run: stretch.group.unwrap_or(0), slots: loads.into() });
            origins.push(None);
            was.push(None);
        }
        if let Some(&i) = seg_start.get(&ip) {
            skip_pos[i] = code.len();
            code.push(Inst::StaticSkip {
                after: 0,
                run: segments[i].index,
                slots: slots_of(&segments[i]).into(),
            });
            origins.push(None);
            was.push(None);
        }
        new_pos[ip] = code.len();
        if let Some((inst, o)) = old.next() {
            code.push(inst);
            origins.push(o);
            was.push(Some(ip));
        }
    }
    // Where control goes that enters segment `i` from outside it.
    let entry = |i: usize| -> usize {
        let stretch = &kept[segments[i].stretch];
        match stretch.group {
            Some(_) if stretch.segments.start == i => group_pos[segments[i].stretch],
            _ => skip_pos[i],
        }
    };
    let retarget = |from: usize, to: usize| -> Label {
        let from_seg = seg_at.get(from).copied().flatten();
        // A jump out of a segment to its end lands on the store.
        if let Some(i) = from_seg
            && segments[i].end == to
        {
            return store_pos[i] as Label;
        }
        // A jump to a segment's first instruction from outside lands on the
        // skip.
        if let Some(&i) = seg_start.get(&to)
            && from_seg != Some(i)
        {
            return entry(i) as Label;
        }
        new_pos[to.min(n)] as Label
    };
    let group_of: HashMap<u32, usize> =
        kept.iter().enumerate().filter_map(|(k, s)| Some((s.group?, k))).collect();
    let segment_of: HashMap<u32, usize> = segments.iter().enumerate().map(|(i, s)| (s.index, i)).collect();
    for (inst, from) in code.iter_mut().zip(&was) {
        match (inst, from) {
            (Inst::StaticSkip { after, run, .. }, _) => {
                *after = if let Some(&k) = group_of.get(run) {
                    // Past every segment and the last store.
                    new_pos[kept[k].end] as Label
                } else {
                    // Past the segment and its store: onto the next
                    // segment's skip, or out of the stretch.
                    let i = segment_of[run];
                    let stretch = &kept[segments[i].stretch];
                    if i + 1 < stretch.segments.end {
                        skip_pos[i + 1] as Label
                    } else {
                        new_pos[stretch.end] as Label
                    }
                };
            }
            (other, Some(from)) => other.for_each_target_mut(|to| *to = retarget(*from, *to as usize)),
            _ => {}
        }
    }
    bc.root.code = code;
    bc.root.origins = origins;
    bc.statics = layout;
    written_by
}

/// A summary of a layout and one line per slot, for `PETAL_OPT_STATS` and
/// tests.
pub fn describe(layout: &StaticLayout) -> String {
    let top = || layout.runs.iter().take(layout.root_runs).filter(|r| r.group.is_none());
    let instructions: usize = top().map(|r| r.instructions).sum();
    let allocations: usize = top().map(|r| r.allocations).sum();
    let mut out = format!(
        "static cache: {} candidate runs, {} skippable in {} segments, {} slots; a frame that \
         skips them all leaves out {} of {} root instructions ({} of them allocations) for {} checks\n",
        layout.candidate_runs,
        layout.checks(),
        layout.segments(),
        layout.root_slots,
        instructions,
        layout.root_instructions,
        allocations,
        layout.checks(),
    );
    let (fn_slots, fn_runs) = layout.function_slots();
    if fn_slots > 0 {
        let runs = &layout.runs[layout.root_runs..];
        out.push_str(&format!(
            "static cache: {fn_slots} slots for values built inside function bodies, in {fn_runs} stretches \
             of {} instructions ({} of them allocations)\n",
            runs.iter().map(|r| r.instructions).sum::<usize>(),
            runs.iter().map(|r| r.allocations).sum::<usize>(),
        ));
    }
    for (i, s) in layout.slots.iter().enumerate() {
        out.push_str(&format!(
            "  s{i}: {}{}\n",
            match (&s.function, s.name.as_deref()) {
                (Some(f), _) => format!("(in {f})"),
                (None, Some(name)) => name.to_string(),
                (None, None) => "(unnamed)".to_string(),
            },
            s.at.as_ref().map(|a| format!(" at {a}")).unwrap_or_default()
        ));
    }
    out
}

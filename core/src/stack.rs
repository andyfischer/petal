//! Stack - Runtime evaluation context.
//!
//! See docs/Architecture.md for the surrounding runtime design.

use std::collections::{HashMap, HashSet};

use crate::closure_table::ClosureTable;
use crate::heap::Heap;
use crate::run_deps::{RunDeps, state_changed};
use crate::symbol::SymbolId;

use smallvec::SmallVec;

use crate::execution_context::ContextKey;
use crate::program::{ProgramId, StateKey};
use crate::value::Value;

/// One dynamic step on the path from the program root to a `state`
/// declaration. See docs/dev/state-call-paths.md §2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathPart {
    /// A call: the compile-time hash identifying the *callsite* the frame was
    /// pushed from (`Term::call_site`), so a helper called from three places
    /// holds three independent slots.
    Call(u64),
    /// One loop iteration: the 0-based iteration index, pushed by a `for`/
    /// `while` at every level of the live frame stack.
    Index(usize),
    /// An explicit `state(expr)` key, hashed. Absolute: a keyed slot's path is
    /// exactly `[Key(h)]`, ignoring the call path entirely (§2.2).
    Key(u64),
}

/// Runtime state key: a declaration id (`base`) plus the call path that reached
/// it. Top-level state has an empty `path`; an explicit `state(expr)` key has
/// exactly one `Key` part; everything else carries one `Call` part per enclosing
/// call and one `Index` part per enclosing loop iteration, outermost first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeStateKey {
    pub base: StateKey,
    pub path: SmallVec<[PathPart; 4]>,
}

/// The hasher of the state table and of the per-run set of touched keys.
///
/// A `state` hit hashes its key four times a run (the touched-set probe and
/// insert, the table lookup, the end-of-run sweep), and under the standard
/// library's SipHash those four were most of what a hit cost. A key is a
/// declaration id, itself a compile-time hash, and a short path of callsite
/// hashes and loop indices: a multiply per word and a fold at the end spread it well,
/// and nothing here needs SipHash's resistance to chosen keys. The hash has no
/// per-process seed, so the table's iteration order is the same on every run
/// of the same program; nothing may depend on that order either way.
#[derive(Debug, Clone, Copy, Default)]
pub struct StateHasher(u64);

impl StateHasher {
    /// The multiplier of rustc's FxHasher.
    const K: u64 = 0xf135_7aea_2e62_a9c5;

    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.wrapping_add(word)).wrapping_mul(Self::K);
    }
}

impl std::hash::Hasher for StateHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Not reached by a `RuntimeStateKey`, whose every field is a word;
        // here so the hasher is correct for any key.
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
        self.add(bytes.len() as u64);
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn write_isize(&mut self, n: isize) {
        self.add(n as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        // A multiply only carries entropy upward, and the table takes its
        // bucket from the low bits and its control byte from the top seven:
        // fold the high half down, multiply once more and fold again, so
        // every input bit reaches both ends (keys that differ only in their
        // high bits otherwise share half their control bytes).
        let h = self.0 ^ (self.0 >> 32);
        let h = h.wrapping_mul(Self::K);
        h ^ (h >> 29)
    }
}

/// Builds a [`StateHasher`].
pub type StateHashBuilder = std::hash::BuildHasherDefault<StateHasher>;

/// The persistent state of a stack: one value per runtime key.
pub type StateMap = HashMap<RuntimeStateKey, Value, StateHashBuilder>;

/// A set of runtime state keys.
pub type StateKeySet = HashSet<RuntimeStateKey, StateHashBuilder>;

/// An open touch capture: a position in the stack's touch journal. Returned by
/// [`Stack::begin_touch_capture`] and consumed by [`Stack::end_touch_capture`].
#[derive(Debug)]
#[must_use = "a touch capture must be ended with Stack::end_touch_capture"]
pub struct TouchCapture {
    start: usize,
}

/// The distinct state keys a section of the program touched while it ran, in
/// first-touch order. Hand them to [`Stack::retain_touches`] on a run where
/// that section is skipped, so the sweep keeps its state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateTouches(Vec<RuntimeStateKey>);

impl StateTouches {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RuntimeStateKey> {
        self.0.iter()
    }
}

/// A test in progress on a stack: begun by
/// [`Env::test_begin`](crate::env::Env::test_begin), driven by
/// [`Env::test_resume`](crate::env::Env::test_resume). Between two resumes
/// its activation records are parked here, off the frame stack, so the host
/// can run the program's ordinary frames on the same stack in the meantime.
#[derive(Clone)]
pub struct TestRun {
    /// The function being run, as the stack's function table names it.
    pub function: String,
    /// The test's frames while it is not executing. Values in them are GC
    /// roots ([`Stack::gc_roots`]).
    pub(crate) parked: Vec<crate::backend::bytecode::VmFrame>,
    /// What the test has recorded so far, across its suspensions.
    pub(crate) failures: Vec<crate::expect::ExpectFailure>,
    pub(crate) checked: u64,
    pub(crate) failed: u64,
    /// `Env`'s lowering count when the test began: the frames index the
    /// bytecode of that lowering and no other.
    pub(crate) lowered: u64,
    /// Set when something the frames point into was replaced under them (a
    /// reload, a restored snapshot). The next resume ends the test with
    /// this as its error.
    pub(crate) invalidated: Option<&'static str>,
}

/// Unique identifier for a stack within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StackKey(pub u32);

/// Runtime execution state for a program.
#[derive(Clone)]
pub struct Stack {
    pub id: StackKey,
    pub program_id: ProgramId,
    /// The ExecutionContext this stack draws its heap and registries from.
    pub context: ContextKey,
    pub state: StateMap,
    pub status: StackStatus,
    /// Temporary storage for the result of the last popped frame.
    /// Used by synchronous closure calls (map/filter/reduce) to capture return values.
    pub last_pop_result: Option<Value>,
    /// RuntimeStateKeys touched (read or written) since the last call to
    /// `start_run_tracking`. Used to garbage-collect persistent state entries
    /// whose source-level declaration was not visited this run — for example
    /// per-iteration state for an item that was removed from the iterated
    /// list, or a top-level `state` declaration that was deleted on hot
    /// reload. Cleared at the start of each top-level `run`. Write through
    /// [`touch_state`](Self::touch_state), which also feeds open captures.
    pub touched_state_keys: StateKeySet,
    /// Every touch made while at least one [`TouchCapture`] is open, in order.
    /// Empty whenever no capture is open, so an ordinary run pays nothing for
    /// it. See [`begin_touch_capture`](Self::begin_touch_capture).
    touch_journal: Vec<RuntimeStateKey>,
    /// How many captures are open. Captures nest; the journal is kept until the
    /// outermost one ends, so an enclosing capture sees its children's touches.
    open_touch_captures: usize,
    /// Top-level named functions (and lambdas bound to a name) captured from
    /// the root block when the program runs. Lets the host invoke a named
    /// Petal function via `Env::call_function` without re-running the whole
    /// program. Refreshed each time the root frame completes; cleared on hot
    /// reload since the underlying closure IDs are invalidated.
    pub functions: HashMap<String, Value>,
    /// User-declared methods, indexed `class name -> method name -> callable`.
    /// Populated as the root block runs: a `fn Rect.area(...)` declaration
    /// compiles to a closure plus a `__declare_method` builtin call that lands
    /// the closure here, so a method — like a function — is callable from the
    /// point its declaration runs. Consulted by `recv.method(...)` dispatch
    /// after a callable record field and before the built-in class methods.
    ///
    /// Rebuilt by every run (the declarations re-execute), and cleared with
    /// `functions` on hot reload, whose new program invalidates the closure ids.
    pub methods: HashMap<String, HashMap<String, Value>>,
    /// What this stack holds for the program's enums, by variant slot (see
    /// [`crate::enum_table`]): chiefly the constant data of each variant, the
    /// record of `key: expr` values its declaration attached. Populated as
    /// the root block runs — an `enum` whose variants carry data compiles to
    /// one `__enum_data` builtin call per variant — and read by a field
    /// access on a variant value (`k.shape`). Republished by every run and
    /// cleared with `methods` on hot reload, so a variant kept in `state`
    /// across an edit reads the data its enum declares *now*. A read is a
    /// memo dependency on [`Stack::enum_data_revision`].
    pub enums: crate::enum_table::EnumTable,
    /// Bumped when a run publishes data for a variant that differs from what
    /// the run before published. A memoized scope that read a variant's field
    /// is valid only while this has not moved (`memo::Dep::EnumData`).
    pub enum_data_revision: u64,
    /// Activation records for the bytecode VM. Stored on the stack (not on the
    /// VM, which is rebuilt per step) so execution state survives across steps
    /// and is reachable as GC roots.
    pub vm_frames: Vec<crate::backend::bytecode::VmFrame>,
    /// Whether the bytecode root frame has been pushed for the current run.
    /// Distinguishes "not started" (push root) from "completed" (`vm_frames`
    /// empty again → done). Reset by [`Stack::reset_execution`].
    pub vm_started: bool,
    /// Recycled VM activation records: popped frames land here (registers,
    /// cursors, and loop context cleared) and calls reuse their allocations
    /// instead of hitting the allocator per call. Cleared frames hold no
    /// values, so this is deliberately *not* a GC root — keep it that way.
    pub vm_frame_pool: Vec<crate::backend::bytecode::VmFrame>,
    /// A reusable buffer the VM gathers a call's argument registers into, so
    /// a call neither allocates nor copies a large inline array. Empty
    /// between calls (taken while one is being set up), so not a GC root.
    pub vm_arg_scratch: Vec<Value>,
    /// How many synchronous closure calls (a `map` callback, a host
    /// `call_function`) are running inside one another right now. Each one
    /// nests a Rust-level step loop, so unlike an ordinary call it uses native
    /// stack. See `Vm::call_closure_sync`.
    pub sync_depth: u32,
    /// The native stack address when the outermost synchronous call began, so
    /// nested ones can measure how much native stack they have used.
    pub sync_stack_base: usize,
    /// What the most recent run depended on — the record behind
    /// [`Env::run_needed`](crate::env::Env::run_needed). See [`crate::run_deps`].
    pub run_deps: RunDeps,
    /// The contents of every `state var` cell as the run began, so the end of
    /// the run can tell whether a `set` changed one. Cells are written through
    /// `CellWrite`, which cannot know whether the cell it writes is persistent,
    /// so the check is made once per run over the slots instead of once per
    /// write. A `set xs[i] = v` may mutate the cell's container in place, which
    /// an id-and-contents snapshot would compare equal to itself, so the
    /// snapshot also keeps the cell's mutation count
    /// ([`Heap::cell_mutations`]) and asks the heap to fingerprint the
    /// contents before the first such mutation ([`Heap::cell_watch`]); see
    /// [`cell_changed`].
    cells_at_run_start: Vec<(crate::heap::CellId, Value, u64)>,
    /// Memoized-scope records and the scopes open right now. See
    /// [`crate::memo`].
    pub memo: crate::memo::MemoTable,
    /// Instructions retired on this stack since it was created. The memo
    /// decides whether a scope is worth a record by how much it ran.
    pub insts: u64,
    /// The top-level values that depend on no dynamic data, kept between
    /// runs so the code that builds them is skipped (see
    /// [`crate::static_cache`]). Like `enums` it lives here so a fork or a
    /// restored snapshot resolves its heap ids in its own heap; it is
    /// stamped with the lowering it was filled under and starts empty under
    /// any other, and it is cleared with `enums` and `memo` on hot reload.
    pub statics: crate::static_cache::StaticSlots,
    /// The test a host has begun on this stack and not finished
    /// ([`Env::test_begin`](crate::env::Env::test_begin)), with its frames
    /// while it is set aside between two
    /// [`Env::test_resume`](crate::env::Env::test_resume) calls.
    pub test: Option<TestRun>,
}

#[derive(Debug, Clone)]
pub enum StackStatus {
    Ready,
    Running,
    Complete(Value),
    Error(String),
}

/// Whether a `state var` cell that held `before` when the run started, at
/// mutation count `mutations`, holds something different now.
///
/// With the count unmoved nothing was edited in place, so `before` is still
/// what the cell held and the two compare by content. Otherwise some
/// container was edited under its id, and the heap fingerprinted the first one
/// just beforehand: if that container is `before` itself, the fingerprint is
/// of the run-start contents, and the cell is unchanged when today's contents
/// fingerprint alike (a `set xs[i] = v` that stores what was already there,
/// every frame, must still let the frame gate settle). If it is some other
/// container, `before` was replaced rather than edited, is intact, and
/// compares by content as usual.
fn cell_changed(
    id: crate::heap::CellId,
    before: Value,
    mutations: u64,
    heap: &Heap,
    closures: &ClosureTable,
) -> bool {
    let now = heap.cell_read(id);
    if heap.cell_mutations(id) == mutations {
        return state_changed(Some(before), now, heap, closures);
    }
    match heap.cell_fingerprint_before(id) {
        Some((of, was)) if of == before => {
            match (was, crate::run_deps::fingerprint_contents(&now, heap)) {
                (Some(was), Some(is)) => was != is,
                // Too large or not fingerprintable: assume the worst.
                _ => true,
            }
        }
        Some(_) => state_changed(Some(before), now, heap, closures),
        // Edited in place with no fingerprint taken: assume the worst.
        None => true,
    }
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId, context: ContextKey) -> Self {
        Self {
            id,
            program_id,
            context,
            state: StateMap::default(),
            status: StackStatus::Ready,
            last_pop_result: None,
            touched_state_keys: StateKeySet::default(),
            touch_journal: Vec::new(),
            open_touch_captures: 0,
            functions: HashMap::new(),
            methods: HashMap::new(),
            enums: crate::enum_table::EnumTable::default(),
            enum_data_revision: 0,
            vm_frames: Vec::new(),
            vm_started: false,
            vm_frame_pool: Vec::new(),
            vm_arg_scratch: Vec::new(),
            sync_depth: 0,
            sync_stack_base: 0,
            run_deps: RunDeps::default(),
            cells_at_run_start: Vec::new(),
            memo: crate::memo::MemoTable::default(),
            insts: 0,
            statics: crate::static_cache::StaticSlots::default(),
            test: None,
        }
    }

    /// Begin the dependency record of a run: reset the read-set and snapshot
    /// the `state var` cells. Called by `Env::run` right after
    /// [`start_run_tracking`](Self::start_run_tracking), with the heap the
    /// cells live in and the context's RNG state.
    pub fn begin_run_deps(&mut self, heap: &mut Heap, rng_state: u64) {
        self.run_deps.begin_run(rng_state);
        self.cells_at_run_start.clear();
        for v in self.state.values() {
            if let Value::Cell(id) = v {
                heap.cell_watch(*id);
                self.cells_at_run_start
                    .push((*id, heap.cell_read(*id), heap.cell_mutations(*id)));
            }
        }
    }

    /// A `state var` cell was created by this run (its `state` declaration
    /// initialized), holding `contents`. Added to the run-start snapshot so a
    /// `set` later in the same run is seen as a change at run end.
    pub fn note_cell_created(&mut self, id: crate::heap::CellId, contents: Value, mutations: u64) {
        self.cells_at_run_start.push((id, contents, mutations));
    }

    /// Complete the dependency record of a run that finished (or stopped on an
    /// error). Compares the `state var` cells against the snapshot taken at
    /// the start, then fingerprints the bindings the run read.
    pub fn finish_run_deps(
        &mut self,
        bindings: &HashMap<SymbolId, Value>,
        binding_revisions: &HashMap<SymbolId, u64>,
        heap: &Heap,
        closures: &ClosureTable,
        rng_state: u64,
        resources_revision: u64,
    ) {
        if !self.run_deps.state_unsettled()
            && self.cells_at_run_start.iter().any(|(id, before, mutations)| {
                heap.is_live(Value::Cell(*id))
                    && cell_changed(*id, *before, *mutations, heap, closures)
            })
        {
            self.run_deps.note_state_unsettled();
        }
        self.cells_at_run_start.clear();
        self.run_deps
            .finish_run(bindings, binding_revisions, heap, rng_state, resources_revision);
    }

    /// Clear all per-run execution state, leaving the stack `Ready` with no
    /// frames. Persistent `state` and captured `functions` are kept (callers
    /// that invalidate them, like hot reload, clear them separately). The
    /// bytecode VM pushes its own root frame on the first step of the next run
    /// (`vm_started == false` signals it).
    ///
    /// This is the single reset point shared by `Env::reset_stack` and
    /// `Env::transfer_state`.
    /// The closures, bytecode or heap a test in progress points into are
    /// about to be replaced (`why` says by what): drop its frames, and let
    /// the next [`Env::test_resume`](crate::env::Env::test_resume) end it
    /// with that as its error.
    pub fn invalidate_test(&mut self, why: &'static str) {
        if let Some(test) = &mut self.test {
            test.parked.clear();
            test.invalidated.get_or_insert(why);
        }
    }

    pub fn reset_execution(&mut self) {
        self.vm_frames.clear();
        self.vm_started = false;
        self.status = StackStatus::Ready;
        self.last_pop_result = None;
    }

    /// Reset the touched-keys set. Called at the start of a top-level run
    /// so that `sweep_untouched_state` can drop entries no longer reachable
    /// from current source.
    pub fn start_run_tracking(&mut self) {
        self.touched_state_keys.clear();
        // Captures do not span runs: one left open by an aborted run is dropped.
        self.touch_journal.clear();
        self.open_touch_captures = 0;
        // Nor do memo scopes.
        self.memo.begin_run();
    }

    /// Record that `key` was read or written this run, so the end-of-run sweep
    /// keeps its slot. Every state instruction goes through here.
    pub fn touch_state(&mut self, key: &RuntimeStateKey) {
        // A slot read and then written is touched twice running; the journal
        // only has to name it once (a capture returns distinct keys anyway).
        if self.open_touch_captures > 0 && self.touch_journal.last() != Some(key) {
            self.touch_journal.push(key.clone());
        }
        // Check before inserting: a slot touched many times in one run (a
        // widget's state read, then written) clones its key only once.
        if !self.touched_state_keys.contains(key) {
            self.touched_state_keys.insert(key.clone());
        }
    }

    /// Start recording which state keys are touched from here on.
    ///
    /// This is how a section of the program that is *skipped* on a later run
    /// keeps its state alive. The end-of-run sweep deletes every slot the run
    /// did not touch, so a memoized scope that reuses last frame's result
    /// instead of executing would otherwise lose the state inside it. The
    /// scope brackets a real execution with `begin_touch_capture` /
    /// [`end_touch_capture`](Self::end_touch_capture), keeps the returned
    /// [`StateTouches`], and on a run where it skips, hands them to
    /// [`retain_touches`](Self::retain_touches) in place of executing.
    ///
    /// Captures nest and must end in LIFO order, all within one run.
    pub fn begin_touch_capture(&mut self) -> TouchCapture {
        self.open_touch_captures += 1;
        TouchCapture {
            start: self.touch_journal.len(),
        }
    }

    /// End a capture and return the distinct keys touched since it began,
    /// including those touched by nested captures and by
    /// [`retain_touches`](Self::retain_touches) calls inside it.
    pub fn end_touch_capture(&mut self, capture: TouchCapture) -> StateTouches {
        debug_assert!(self.open_touch_captures > 0, "no touch capture is open");
        debug_assert!(
            capture.start <= self.touch_journal.len(),
            "touch captures must end in LIFO order within one run"
        );
        let start = capture.start.min(self.touch_journal.len());
        let mut seen = HashSet::new();
        let keys = self.touch_journal[start..]
            .iter()
            .filter(|k| seen.insert(*k))
            .cloned()
            .collect();
        self.open_touch_captures = self.open_touch_captures.saturating_sub(1);
        if self.open_touch_captures == 0 {
            self.touch_journal.clear();
        }
        StateTouches(keys)
    }

    /// End a capture whose keys nobody wants: a memo scope that turned out
    /// not to be recordable. [`end_touch_capture`](Self::end_touch_capture)
    /// copies every key touched since the capture began, which a scope
    /// enclosing a long run pays for each time one of its ancestors closes.
    pub fn discard_touch_capture(&mut self, capture: TouchCapture) {
        debug_assert!(self.open_touch_captures > 0, "no touch capture is open");
        let _ = capture;
        self.open_touch_captures = self.open_touch_captures.saturating_sub(1);
        if self.open_touch_captures == 0 {
            self.touch_journal.clear();
        }
    }

    /// Mark every key in `touches` as touched this run, as if the section that
    /// produced them had executed again. Keys whose slot no longer exists are
    /// harmless: touching never creates a slot. Also feeds any open capture, so
    /// an enclosing scope that is executing records its skipped child's state.
    pub fn retain_touches(&mut self, touches: &StateTouches) {
        for key in &touches.0 {
            self.touch_state(key);
        }
    }

    /// Drop persistent state entries that were not touched (read or written)
    /// since the last `start_run_tracking`. Returns the number of entries
    /// removed. Called once per top-level run after the program completes.
    pub fn sweep_untouched_state(&mut self) -> usize {
        let before = self.state.len();
        self.state
            .retain(|key, _| self.touched_state_keys.contains(key));
        before - self.state.len()
    }

    /// Enumerate this stack's GC roots — every heap `Value` reachable from its
    /// live execution state — by handing each to `mark`. These are the
    /// register files and snapshotted for-each cursors of the VM frames, the
    /// persistent state values, and the last synchronous-call pop result. The
    /// recycled frame pool is deliberately *not* walked: recycled frames hold
    /// no values (see `vm_frame_pool`). Used by `Env::collect_garbage`.
    pub fn gc_roots(&self, mut mark: impl FnMut(Value)) {
        // VM frames are GC roots: their register files and any snapshotted
        // for-each cursors hold live values.
        let parked = self.test.iter().flat_map(|t| t.parked.iter());
        for frame in self.vm_frames.iter().chain(parked) {
            for val in &frame.regs {
                mark(*val);
            }
            for cursor in frame.loops.iter().flatten() {
                if let crate::backend::bytecode::vm::LoopCursor::ForEach { elems, .. } = cursor {
                    for val in elems {
                        mark(*val);
                    }
                }
                // A collecting loop's in-progress accumulator holds live values
                // until it is materialized into a list at loop exit.
                for val in cursor.acc() {
                    mark(*val);
                }
            }
        }
        // Persistent state values
        for val in self.state.values() {
            mark(*val);
        }
        // The `state var` contents snapshotted at run start are compared at
        // run end; a cell overwritten mid-run would otherwise leave the
        // snapshot pointing at a collected object.
        for (_, val, _) in &self.cells_at_run_start {
            mark(*val);
        }
        // Last pop result (used by synchronous closure calls)
        if let Some(val) = self.last_pop_result {
            mark(val);
        }
        // Enum constant data is read by any later field access on a variant;
        // the table's tag strings are what its index is keyed by.
        self.enums.gc_roots(&mut mark);
        // Memo records replay their values and compare against them across
        // runs; open scopes hold what they have recorded so far.
        self.memo.gc_roots(&mut mark);
        // Static values are loaded into registers by the runs that skip
        // building them.
        self.statics.gc_roots(&mut mark);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Env, RunOutcome};

    fn key(base: u64, path: &[PathPart]) -> RuntimeStateKey {
        RuntimeStateKey {
            base: StateKey(base),
            path: path.iter().copied().collect(),
        }
    }

    /// The state hasher spreads the keys a program really produces: one
    /// declaration under consecutive loop indices, and consecutive declaration
    /// ids at the top level. A table takes its bucket from the low bits of a
    /// hash and its control byte from the top seven, so both are checked
    /// against what a random function would give (about 63% of 4096 buckets
    /// filled by 4096 keys).
    #[test]
    fn the_state_hasher_spreads_consecutive_keys() {
        use std::hash::BuildHasher;
        let build = StateHashBuilder::default();
        let families: [Vec<RuntimeStateKey>; 3] = [
            (0..4096)
                .map(|i| key(7, &[PathPart::Index(i), PathPart::Call(0x9e37)]))
                .collect(),
            (0..4096).map(|i| key(i, &[])).collect(),
            (0..4096)
                .map(|i| key(7, &[PathPart::Key(i << 32)]))
                .collect(),
        ];
        for keys in &families {
            let hashes: Vec<u64> = keys.iter().map(|k| build.hash_one(k)).collect();
            let distinct: HashSet<u64> = hashes.iter().copied().collect();
            assert_eq!(distinct.len(), keys.len(), "two keys share a hash");
            let low: HashSet<u64> = hashes.iter().map(|h| h & 0xfff).collect();
            assert!(low.len() > 2300, "low bits fill {} of 4096", low.len());
            let top: HashSet<u64> = hashes.iter().map(|h| h >> 57).collect();
            assert_eq!(top.len(), 128, "top seven bits");
        }
        // The parts of a path are told apart by kind and by order.
        let a = build.hash_one(key(1, &[PathPart::Index(3)]));
        let b = build.hash_one(key(1, &[PathPart::Call(3)]));
        let c = build.hash_one(key(1, &[PathPart::Index(3), PathPart::Index(4)]));
        let d = build.hash_one(key(1, &[PathPart::Index(4), PathPart::Index(3)]));
        assert!(a != b && c != d && a != c);
    }

    fn stack() -> Stack {
        Stack::new(StackKey(0), ProgramId(0), ContextKey(0))
    }

    #[test]
    fn retained_touches_survive_the_sweep() {
        let (a, b) = (key(1, &[PathPart::Call(7)]), key(2, &[PathPart::Key(9)]));
        let mut s = stack();
        s.state.insert(a.clone(), Value::Int(1));
        s.state.insert(b.clone(), Value::Int(2));

        // Run 1: the section executes and touches both slots.
        s.start_run_tracking();
        let cap = s.begin_touch_capture();
        s.touch_state(&a);
        s.touch_state(&b);
        s.touch_state(&a);
        let touches = s.end_touch_capture(cap);
        assert_eq!(
            touches.iter().cloned().collect::<Vec<_>>(),
            vec![a.clone(), b.clone()]
        );
        assert_eq!(s.sweep_untouched_state(), 0);

        // Run 2: the section is skipped and retains what it touched.
        s.start_run_tracking();
        s.retain_touches(&touches);
        assert_eq!(s.sweep_untouched_state(), 0);
        assert_eq!(s.state.len(), 2);

        // Run 3: skipped without retaining, so both slots go.
        s.start_run_tracking();
        assert_eq!(s.sweep_untouched_state(), 2);
    }

    #[test]
    fn an_enclosing_capture_sees_nested_and_retained_touches() {
        let (outer, inner, skipped) = (key(1, &[]), key(2, &[]), key(3, &[]));
        let mut s = stack();
        s.start_run_tracking();

        let parent = s.begin_touch_capture();
        s.touch_state(&outer);
        let child = s.begin_touch_capture();
        s.touch_state(&inner);
        let child_touches = s.end_touch_capture(child);
        s.retain_touches(&StateTouches(vec![skipped.clone()]));
        let parent_touches = s.end_touch_capture(parent);

        assert_eq!(
            child_touches.iter().cloned().collect::<Vec<_>>(),
            vec![inner.clone()]
        );
        assert_eq!(
            parent_touches.iter().cloned().collect::<Vec<_>>(),
            vec![outer, inner, skipped]
        );
    }

    #[test]
    fn the_journal_is_empty_outside_captures() {
        let mut s = stack();
        s.start_run_tracking();
        for i in 0..100 {
            s.touch_state(&key(i, &[]));
        }
        assert!(s.touch_journal.is_empty());

        let cap = s.begin_touch_capture();
        s.touch_state(&key(1, &[]));
        let _ = s.end_touch_capture(cap);
        assert!(
            s.touch_journal.is_empty(),
            "the outermost capture clears it"
        );

        // A capture abandoned by an aborted run does not leak into the next.
        let _abandoned = s.begin_touch_capture();
        s.touch_state(&key(1, &[]));
        s.start_run_tracking();
        assert_eq!(s.open_touch_captures, 0);
        assert!(s.touch_journal.is_empty());
    }

    /// Drive one frame, calling `mid` after its first instruction: the point a
    /// host (or, later, a memoized scope in the VM) acts mid-run. A capture
    /// `mid` opens is ended once the frame completes and its touches returned.
    fn frame(
        env: &mut Env,
        sid: StackKey,
        mid: impl FnOnce(&mut Stack) -> Option<TouchCapture>,
    ) -> StateTouches {
        env.reset_stack(sid).unwrap();
        let first = env.run_bounded(sid, 1).unwrap();
        assert!(
            matches!(first, RunOutcome::Yielded { .. }),
            "frame finished in one step"
        );
        let cap = mid(env.stack_mut(sid).unwrap());
        let rest = env.run_bounded(sid, u64::MAX).unwrap();
        assert!(matches!(rest, RunOutcome::Done(_)));
        match cap {
            Some(cap) => env.stack_mut(sid).unwrap().end_touch_capture(cap),
            None => StateTouches::default(),
        }
    }

    #[test]
    fn a_section_that_stops_running_keeps_its_state_when_retained() {
        let src = "\
fn widget()
  state clicks = 0
  clicks += 1
  clicks
end
state var frame = 0
set frame = get frame + 1
if get frame == 1 || get frame == 3 then
  widget()
end
";
        let mut env = Env::new();
        let pid = env.load_program(src).unwrap();
        let sid = env.create_stack(pid).unwrap();
        let widget_base = StateKey(crate::compiler::Compiler::hash_state_name("widget/clicks"));
        let clicks = |env: &Env| {
            env.get_all_state(sid)
                .unwrap()
                .iter()
                .find(|(k, _)| k.base == widget_base)
                .map(|(_, v)| *v)
        };

        // Frame 1: the widget runs. Capture what the frame touches.
        let touches = frame(&mut env, sid, |s| Some(s.begin_touch_capture()));
        assert!(touches.iter().any(|k| k.base == widget_base));
        assert_eq!(clicks(&env), Some(Value::Int(1)));

        // Frame 2: the widget's branch does not run, but the frame retains
        // the widget's touches, as a skipped memoized scope would.
        frame(&mut env, sid, |s| {
            s.retain_touches(&touches);
            None
        });
        assert_eq!(clicks(&env), Some(Value::Int(1)), "retained state survives");

        // Frame 3: the widget runs again and continues from its kept state.
        frame(&mut env, sid, |_| None);
        assert_eq!(clicks(&env), Some(Value::Int(2)));

        // Frame 4: skipped and not retained, so it is swept as before.
        frame(&mut env, sid, |_| None);
        assert_eq!(clicks(&env), None);
    }
}

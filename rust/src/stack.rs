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
    pub state: HashMap<RuntimeStateKey, Value>,
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
    pub touched_state_keys: HashSet<RuntimeStateKey>,
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
    /// write. Cell contents are never mutated in place (a container that enters
    /// a `var` is kept out of the in-place rewrite), so an id-and-contents
    /// snapshot is exact.
    cells_at_run_start: Vec<(crate::heap::CellId, Value)>,
    /// Memoized-scope records and the scopes open right now. See
    /// [`crate::memo`].
    pub memo: crate::memo::MemoTable,
    /// Instructions retired on this stack since it was created. The memo
    /// decides whether a scope is worth a record by how much it ran.
    pub insts: u64,
}

#[derive(Debug, Clone)]
pub enum StackStatus {
    Ready,
    Running,
    Complete(Value),
    Error(String),
}

impl Stack {
    pub fn new(id: StackKey, program_id: ProgramId, context: ContextKey) -> Self {
        Self {
            id,
            program_id,
            context,
            state: HashMap::new(),
            status: StackStatus::Ready,
            last_pop_result: None,
            touched_state_keys: HashSet::new(),
            touch_journal: Vec::new(),
            open_touch_captures: 0,
            functions: HashMap::new(),
            methods: HashMap::new(),
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
        }
    }

    /// Begin the dependency record of a run: reset the read-set and snapshot
    /// the `state var` cells. Called by `Env::run` right after
    /// [`start_run_tracking`](Self::start_run_tracking), with the heap the
    /// cells live in and the context's RNG state.
    pub fn begin_run_deps(&mut self, heap: &Heap, rng_state: u64) {
        self.run_deps.begin_run(rng_state);
        self.cells_at_run_start.clear();
        for v in self.state.values() {
            if let Value::Cell(id) = v {
                self.cells_at_run_start.push((*id, heap.cell_read(*id)));
            }
        }
    }

    /// A `state var` cell was created by this run (its `state` declaration
    /// initialized), holding `contents`. Added to the run-start snapshot so a
    /// `set` later in the same run is seen as a change at run end.
    pub fn note_cell_created(&mut self, id: crate::heap::CellId, contents: Value) {
        self.cells_at_run_start.push((id, contents));
    }

    /// Complete the dependency record of a run that finished (or stopped on an
    /// error). Compares the `state var` cells against the snapshot taken at
    /// the start, then fingerprints the bindings the run read.
    pub fn finish_run_deps(
        &mut self,
        bindings: &HashMap<SymbolId, Value>,
        heap: &Heap,
        closures: &ClosureTable,
        rng_state: u64,
        resources_revision: u64,
    ) {
        if !self.run_deps.state_unsettled()
            && self.cells_at_run_start.iter().any(|(id, before)| {
                heap.is_live(Value::Cell(*id))
                    && state_changed(Some(*before), heap.cell_read(*id), heap, closures)
            })
        {
            self.run_deps.note_state_unsettled();
        }
        self.cells_at_run_start.clear();
        self.run_deps
            .finish_run(bindings, heap, rng_state, resources_revision);
    }

    /// Clear all per-run execution state, leaving the stack `Ready` with no
    /// frames. Persistent `state` and captured `functions` are kept (callers
    /// that invalidate them, like hot reload, clear them separately). The
    /// bytecode VM pushes its own root frame on the first step of the next run
    /// (`vm_started == false` signals it).
    ///
    /// This is the single reset point shared by `Env::reset_stack` and
    /// `Env::transfer_state`.
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
        if self.open_touch_captures > 0 {
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
        for frame in &self.vm_frames {
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
        for (_, val) in &self.cells_at_run_start {
            mark(*val);
        }
        // Last pop result (used by synchronous closure calls)
        if let Some(val) = self.last_pop_result {
            mark(val);
        }
        // Memo records replay their values and compare against them across
        // runs; open scopes hold what they have recorded so far.
        self.memo.gc_roots(&mut mark);
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

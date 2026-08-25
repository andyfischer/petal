//! Frame management: the [`VmFrame`] activation record and [`LoopCursor`]
//! iteration state, the frame pool, and the loop-cursor / state-key plumbing
//! shared by the executor.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop.

use super::*;

use super::super::isa::LoopSlot;
use crate::program::StateKey;
use crate::stack::{PathPart, RuntimeStateKey};

/// A frame's state path. Sized for a UI tree's typical depth (a handful of
/// nested calls and loops), so the common case never allocates.
pub type FramePath = SmallVec<[PathPart; 4]>;

/// A per-call activation record: one flat register file plus loop cursors.
#[derive(Clone)]
pub struct VmFrame {
    /// The function this frame is executing (`None` = the implicit root).
    pub func: Option<FunctionId>,
    /// Instruction pointer into the function's `code`.
    pub ip: usize,
    /// Flat register file (`Value` is `Copy`, so this is a plain `Vec`).
    pub regs: Vec<Value>,
    /// Caller register that receives this frame's return value. `None` for the
    /// root frame and for frames pushed by a synchronous intrinsic call (whose
    /// result is read from `stack.last_pop_result`, not written to a register).
    pub dst_in_caller: Option<Reg>,
    /// Loop cursors, indexed by [`LoopSlot`]. A slot is `Some` while its loop is
    /// active (set by a `*Init` op, cleared by `LoopPop`); grown on demand.
    pub loops: Vec<Option<LoopCursor>>,
    /// This frame's **state path**: the caller's path plus the `Call` part for
    /// the callsite that pushed it, plus one `Index` part per loop currently
    /// iterating *in this frame* (pushed by `*Init`, bumped by `*Next`, popped
    /// by `LoopPop`). Composed incrementally at push time, so resolving a
    /// state key is a clone of this vector rather than a walk of the frame
    /// stack. The root frame's path is empty.
    pub path: FramePath,
    /// The `Call` term that created this frame (for stack-trace annotation).
    /// `None` for the root frame and synchronous-intrinsic frames.
    pub call_site: Option<TermId>,
}

impl VmFrame {
    /// A fresh frame for `func` with a zeroed register file of `reg_count`.
    pub fn new(
        func: Option<FunctionId>,
        reg_count: u16,
        dst_in_caller: Option<Reg>,
        call_site: Option<TermId>,
    ) -> Self {
        VmFrame {
            func,
            ip: 0,
            regs: vec![Value::Nil; reg_count as usize],
            dst_in_caller,
            loops: Vec::new(),
            path: FramePath::new(),
            call_site,
        }
    }

    /// Re-initialize a recycled frame to the state `new` would produce, keeping
    /// the register file's allocation. `recycle` already emptied the frame, so
    /// the resize is a pure `Value::Nil` fill.
    fn reset(
        &mut self,
        func: Option<FunctionId>,
        reg_count: u16,
        dst_in_caller: Option<Reg>,
        call_site: Option<TermId>,
    ) {
        self.func = func;
        self.ip = 0;
        self.regs.resize(reg_count as usize, Value::Nil);
        self.dst_in_caller = dst_in_caller;
        self.call_site = call_site;
    }

    /// Empty the frame for the pool: registers, cursors, and the state path are
    /// cleared so a pooled frame holds no values (the pool is not a GC root)
    /// and no stale path can leak into the next call that reuses it.
    pub(super) fn recycle(&mut self) {
        self.regs.clear();
        self.loops.clear();
        self.path.clear();
    }
}

/// A live loop's iteration state (replaces the graph engine's `LoopState`).
///
/// `acc` is the collection accumulator for a value-position loop (`x = for …`):
/// `LoopCollect` pushes each iteration's body result and `LoopCollectEnd`
/// materializes it into a list. It stays empty (and never allocates) for a
/// plain side-effect loop.
#[derive(Clone)]
pub enum LoopCursor {
    /// `for x in <list>`: the snapshotted elements and the next index.
    ForEach {
        elems: Vec<Value>,
        i: usize,
        acc: Vec<Value>,
    },
    /// `for i in range(a, b)`: the current value, exclusive end, and 0-based
    /// iteration count (the state-key index, which differs from the value when
    /// the range does not start at 0).
    Range {
        cur: i64,
        end: i64,
        iter: usize,
        acc: Vec<Value>,
    },
    /// A `while` loop tracks only its iteration counter (for state keying).
    While { iteration: usize, acc: Vec<Value> },
}

impl LoopCursor {
    /// Push a value onto this cursor's collection accumulator.
    pub(super) fn push_acc(&mut self, v: Value) {
        match self {
            LoopCursor::ForEach { acc, .. }
            | LoopCursor::Range { acc, .. }
            | LoopCursor::While { acc, .. } => acc.push(v),
        }
    }

    /// Take this cursor's collection accumulator, leaving it empty.
    pub(super) fn take_acc(&mut self) -> Vec<Value> {
        match self {
            LoopCursor::ForEach { acc, .. }
            | LoopCursor::Range { acc, .. }
            | LoopCursor::While { acc, .. } => std::mem::take(acc),
        }
    }

    /// This cursor's collection accumulator (GC root).
    pub(crate) fn acc(&self) -> &[Value] {
        match self {
            LoopCursor::ForEach { acc, .. }
            | LoopCursor::Range { acc, .. }
            | LoopCursor::While { acc, .. } => acc,
        }
    }
}

impl<'a> Vm<'a> {
    /// An initialized frame, reusing a pooled register file when one is
    /// available (the steady-state case for every call after warm-up).
    ///
    /// `site` is the callsite id this frame is entered through: the new frame's
    /// path is the innermost live frame's path (its own callsite chain plus any
    /// loop iterations it is inside right now) with `Call(site)` appended. With
    /// no frames on the stack — the host entry point — that is the root path
    /// `[Call(site)]`; `None` is the program root, which runs on the empty path.
    ///
    /// The path is *extended into* the pooled frame rather than assigned from a
    /// freshly built one: `recycle` empties the vector but keeps its buffer, so
    /// a warm pool copies the caller's parts without an allocation per call.
    pub(super) fn frame_from_pool(
        &mut self,
        func: Option<FunctionId>,
        reg_count: u16,
        dst_in_caller: Option<Reg>,
        call_site: Option<TermId>,
        site: Option<u64>,
    ) -> VmFrame {
        let mut frame = match self.stack.vm_frame_pool.pop() {
            Some(mut f) => {
                f.reset(func, reg_count, dst_in_caller, call_site);
                f
            }
            None => VmFrame::new(func, reg_count, dst_in_caller, call_site),
        };
        if let Some(site) = site {
            if let Some(caller) = self.stack.vm_frames.last() {
                frame.path.extend_from_slice(&caller.path);
            }
            frame.path.push(PathPart::Call(site));
        }
        frame
    }

    /// The compile-time callsite id of the call term that is pushing a frame.
    /// Hand-written IR and synthetic calls carry none; they share id 0, which
    /// is exactly the "one callsite" behaviour they had before.
    pub(super) fn site_of(&self, call_site: Option<TermId>) -> u64 {
        call_site
            .and_then(|tid| self.program.terms.get(tid.0 as usize))
            .and_then(|t| t.call_site)
            .unwrap_or(0)
    }

    /// Grow frame `fi`'s loop-cursor vector so `slot` is addressable.
    pub(super) fn ensure_slot(&mut self, fi: usize, slot: LoopSlot) {
        let loops = &mut self.stack.vm_frames[fi].loops;
        if slot as usize >= loops.len() {
            loops.resize_with(slot as usize + 1, || None);
        }
    }

    /// Push an `Index` part for a loop that is starting in frame `fi`. Every
    /// loop pushes one — a state declaration inside a loop body is keyed per
    /// iteration wherever the loop runs, at any frame depth.
    pub(super) fn push_loop_idx(&mut self, fi: usize) {
        self.stack.vm_frames[fi].path.push(PathPart::Index(0));
    }

    /// Set the innermost active loop's `Index` part to `idx` (the current
    /// 0-based iteration). The innermost active loop's part is always last:
    /// the frame's `Call` prefix is fixed, and any nested loop pushed after it
    /// pops its own part at `LoopPop`. Guarded on the part actually being an
    /// `Index` so unbalanced hand-written IR cannot rewrite a `Call` part and
    /// silently misroute every slot below it.
    pub(super) fn set_loop_idx_top(&mut self, fi: usize, idx: usize) {
        if let Some(last @ PathPart::Index(_)) = self.stack.vm_frames[fi].path.last_mut() {
            *last = PathPart::Index(idx);
        }
    }

    /// Pop the innermost active loop's `Index` part (at `LoopPop`), with the
    /// same guard as [`set_loop_idx_top`](Self::set_loop_idx_top).
    pub(super) fn pop_loop_idx(&mut self, fi: usize) {
        let path = &mut self.stack.vm_frames[fi].path;
        if matches!(path.last(), Some(PathPart::Index(_))) {
            path.pop();
        }
    }

    /// Resolve a state declaration's runtime slot key.
    ///
    /// An explicit `state(expr)` key is **absolute**: it hashes its value and
    /// ignores the call path entirely, so two callsites asking for the same
    /// entity get the same slot (plan §2.2). Otherwise the slot is the
    /// declaration id under the current frame's path — the callsite chain that
    /// reached it and the loop iterations it is inside — which the frame has
    /// already composed incrementally.
    ///
    /// `path_pop` drops that many innermost loop `Index` parts, so a
    /// reassignment nested deeper in loops than its declaration still addresses
    /// the declaration's slot (`Term::path_pop`); it is 0 at the declaration
    /// itself and ignored under an explicit key.
    pub(super) fn state_key(
        &self,
        fi: usize,
        base: StateKey,
        explicit: Option<Value>,
        path_pop: u32,
    ) -> RuntimeStateKey {
        let path = match explicit {
            Some(kv) => {
                let mut v = SmallVec::new();
                v.push(PathPart::Key(crate::value::hash_value(&kv, self.heap)));
                v
            }
            None => {
                let live = &self.stack.vm_frames[fi].path;
                let keep = live.len().saturating_sub(path_pop as usize);
                live[..keep].into()
            }
        };
        RuntimeStateKey { base, path }
    }
}

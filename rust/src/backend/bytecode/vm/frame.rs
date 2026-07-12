//! Frame management: the [`VmFrame`] activation record and [`LoopCursor`]
//! iteration state, the frame pool, and the loop-cursor / state-key plumbing
//! shared by the executor.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop.

use super::*;

use super::super::isa::LoopSlot;
use crate::program::StateKey;
use crate::stack::{LoopKeyPart, RuntimeStateKey};

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
    /// Active loop-index context, outermost-first, for state-key resolution.
    pub loop_idx: SmallVec<[LoopKeyPart; 2]>,
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
            loop_idx: SmallVec::new(),
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

    /// Empty the frame for the pool: registers, cursors, and loop context are
    /// cleared so a pooled frame holds no values (the pool is not a GC root).
    pub(super) fn recycle(&mut self) {
        self.regs.clear();
        self.loops.clear();
        self.loop_idx.clear();
    }
}

/// A live loop's iteration state (replaces the graph engine's `LoopState`).
#[derive(Clone)]
pub enum LoopCursor {
    /// `for x in <list>`: the snapshotted elements and the next index.
    ForEach { elems: Vec<Value>, i: usize },
    /// `for i in range(a, b)`: the current value, exclusive end, and 0-based
    /// iteration count (the state-key index, which differs from the value when
    /// the range does not start at 0).
    Range { cur: i64, end: i64, iter: usize },
    /// A `while` loop tracks only its iteration counter (for state keying).
    While { iteration: usize },
}

impl<'a> Vm<'a> {
    /// An initialized frame, reusing a pooled register file when one is
    /// available (the steady-state case for every call after warm-up).
    pub(super) fn frame_from_pool(
        &mut self,
        func: Option<FunctionId>,
        reg_count: u16,
        dst_in_caller: Option<Reg>,
        call_site: Option<TermId>,
    ) -> VmFrame {
        match self.stack.vm_frame_pool.pop() {
            Some(mut f) => {
                f.reset(func, reg_count, dst_in_caller, call_site);
                f
            }
            None => VmFrame::new(func, reg_count, dst_in_caller, call_site),
        }
    }

    /// Grow frame `fi`'s loop-cursor vector so `slot` is addressable.
    pub(super) fn ensure_slot(&mut self, fi: usize, slot: LoopSlot) {
        let loops = &mut self.stack.vm_frames[fi].loops;
        if slot as usize >= loops.len() {
            loops.resize_with(slot as usize + 1, || None);
        }
    }

    /// Set the innermost active loop-index context entry to `idx` (the current
    /// 0-based iteration), for per-iteration state keying.
    pub(super) fn set_loop_idx_top(&mut self, fi: usize, idx: usize) {
        if let Some(last) = self.stack.vm_frames[fi].loop_idx.last_mut() {
            *last = LoopKeyPart::Index(idx);
        }
    }

    /// Resolve a state term's runtime key. An explicit `state(expr)` key hashes
    /// its value; otherwise a term inside a loop keys by the loop-index context
    /// gathered across *all* active frames (outermost first), matching the graph
    /// engine's `loop_key_parts`; a non-loop term keys by base alone.
    pub(super) fn state_key(
        &self,
        base: StateKey,
        in_loop: bool,
        explicit: Option<Value>,
    ) -> RuntimeStateKey {
        let loop_indices = match explicit {
            Some(kv) => {
                let mut v = SmallVec::new();
                v.push(LoopKeyPart::Explicit(crate::value::hash_value(
                    &kv, self.heap,
                )));
                v
            }
            None if in_loop => {
                let mut parts: SmallVec<[LoopKeyPart; 2]> = SmallVec::new();
                for frame in &self.stack.vm_frames {
                    parts.extend(frame.loop_idx.iter().cloned());
                }
                parts
            }
            None => SmallVec::new(),
        };
        RuntimeStateKey { base, loop_indices }
    }
}

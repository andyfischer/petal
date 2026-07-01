//! The register VM that executes lowered [`BytecodeProgram`]s.
//!
//! Arrives in M1 (core ops + calls). For now this module only defines the frame
//! shape so the rest of the backend can compile against it; execution is a
//! `todo!`.

use smallvec::SmallVec;

use super::isa::{BytecodeFn, LoopSlot};
use crate::stack::LoopKeyPart;
use crate::value::Value;

/// A per-call activation record: one flat register file plus loop cursors.
pub struct VmFrame<'p> {
    /// The function this frame is executing.
    pub func: &'p BytecodeFn,
    /// Instruction pointer into `func.code`.
    pub ip: usize,
    /// Flat register file (`Value` is `Copy`, so this is a plain `Vec`).
    pub regs: Vec<Value>,
    /// Caller register that receives this frame's return value.
    pub dst_in_caller: u16,
    /// Active loop cursors, indexed by [`LoopSlot`].
    pub loops: SmallVec<[LoopCursor; 2]>,
    /// Active loop-index context, outermost-first, for state-key resolution.
    pub loop_idx: SmallVec<[LoopKeyPart; 2]>,
}

/// A live loop's iteration state (replaces the graph engine's `LoopState`).
pub enum LoopCursor {
    /// `for x in <list>`: the snapshotted elements and the next index.
    ForEach { elems: Vec<Value>, i: usize },
    /// `for i in range(a, b)`: the current value and exclusive end.
    Range { cur: i64, end: i64 },
    /// A `while` loop tracks only its iteration counter (for state keying).
    While { iteration: usize },
}

impl LoopCursor {
    /// The slot this cursor occupies is positional; this helper exists so future
    /// code reads clearly at call sites.
    pub fn slot_placeholder() -> LoopSlot {
        0
    }
}

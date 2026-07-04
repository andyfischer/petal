//! Bytecode instruction set and program representation.
//!
//! The term-graph IR (`crate::program`) is *lowered* (see [`super::lower`]) into
//! a linear, register-based instruction stream. One [`BytecodeFn`] is produced
//! per `FunctionDef` (with the program root block as an implicit function); each
//! holds a flat register file into which every one of that function's blocks is
//! linearized.
//!
//! The instruction is an enum-of-structs (rather than a packed byte encoding):
//! operand counts are heterogeneous (`Call`, `AllocMap`, …), the disassembler
//! and `ShowBytecode` tool stay trivial, and Rust compiles the dispatch `match`
//! to a jump table. A packed encoding is a later, profiling-gated option behind
//! this same type.

use smallvec::SmallVec;

use crate::constant_table::ConstantId;
use crate::program::{FunctionId, MapSpreadEntry, StateKey, TermId};

/// A flat register index within a [`BytecodeFn`]'s register file.
pub type Reg = u16;

/// An index into a [`BytecodeFn`]'s `code` array — a jump target.
pub type Label = u32;

/// Which loop-cursor slot on the current VM frame a loop opcode drives.
pub type LoopSlot = u16;

/// A single bytecode instruction.
///
/// Most variants map 1:1 from a `TermOp`; the exceptions are the linearization
/// forms — [`Move`](Inst::Move) (lowered `Copy` *and* `Phi`), the jump family,
/// the loop family (which replaces the graph engine's per-frame `loop_states`),
/// and [`MatchArm`](Inst::MatchArm).
#[derive(Debug, Clone)]
pub enum Inst {
    // --- constants / moves ---
    /// `dst = constants[k]`
    LoadConst { dst: Reg, k: ConstantId },
    /// `dst = nil` — a branch's default result before an arm overwrites it (so
    /// an empty/untaken arm yields `nil`, matching the graph's `block_result`).
    LoadNil { dst: Reg },
    /// `dst = <bool>` — short-circuit results of `And` (`false`) / `Or` (`true`).
    LoadBool { dst: Reg, val: bool },
    /// `dst = src` — lowered `Copy`, `Phi` init, `phi_out`, and arm-result joins.
    Move { dst: Reg, src: Reg },

    // --- arithmetic (1:1 with TermOp) ---
    Add { dst: Reg, a: Reg, b: Reg },
    Sub { dst: Reg, a: Reg, b: Reg },
    Mul { dst: Reg, a: Reg, b: Reg },
    Div { dst: Reg, a: Reg, b: Reg },
    Mod { dst: Reg, a: Reg, b: Reg },
    Neg { dst: Reg, a: Reg },

    // --- comparison ---
    Eq { dst: Reg, a: Reg, b: Reg },
    Ne { dst: Reg, a: Reg, b: Reg },
    Lt { dst: Reg, a: Reg, b: Reg },
    Le { dst: Reg, a: Reg, b: Reg },
    Gt { dst: Reg, a: Reg, b: Reg },
    Ge { dst: Reg, a: Reg, b: Reg },

    // --- logical / string ---
    Not { dst: Reg, a: Reg },
    Concat { dst: Reg, a: Reg, b: Reg },

    // --- control flow (new forms; no TermOp equivalent) ---
    Jump { to: Label },
    JumpIfFalse { cond: Reg, to: Label },
    JumpIfTrue { cond: Reg, to: Label },

    // --- loops (replace Frame.loop_states) ---
    /// Snapshot `iter`'s list into loop slot `slot`; push a loop-index context
    /// entry if `idx_ctx` (state keying).
    ForEachInit { iter: Reg, slot: LoopSlot, idx_ctx: bool },
    /// Advance the ForEach cursor: on exhaustion jump to `exit`; else write the
    /// element into `var` and bump the loop-index context.
    ForEachNext { slot: LoopSlot, var: Reg, exit: Label },
    /// Initialize an integer range cursor `[start, end)` into `slot`.
    RangeInit { start: Reg, end: Reg, slot: LoopSlot, idx_ctx: bool },
    /// Advance the range cursor: on exhaustion jump to `exit`; else write the
    /// current integer into `var`.
    RangeNext { slot: LoopSlot, var: Reg, exit: Label },
    /// Initialize a while-loop's iteration counter (loop-index context) in `slot`.
    WhileInit { slot: LoopSlot },
    /// Bump a while-loop's per-iteration index context in `slot`.
    LoopBumpIdx { slot: LoopSlot },
    /// Pop the loop-index context for `slot` on loop exit / break.
    LoopPop { slot: LoopSlot },

    // --- calls ---
    Call { dst: Reg, callee: Reg, args: SmallVec<[Reg; 4]> },
    MethodCall { dst: Reg, recv: Reg, name: ConstantId, args: SmallVec<[Reg; 4]> },
    /// `dst = name(args…)`. `in_place` is set by escape analysis (M4) when the
    /// builtin is a mutation (`append`/`set`/…) whose container argument is
    /// provably unique + non-escaping — the VM then lets it mutate + reuse the
    /// backing store instead of cloning. Always false unless
    /// `OptFlags::in_place_mutation` proved it.
    BuiltinCall { dst: Reg, name: ConstantId, args: SmallVec<[Reg; 4]>, in_place: bool },
    MakeClosure { dst: Reg, func: FunctionId, caps: SmallVec<[Reg; 4]> },
    MakeOverloadSet { dst: Reg, closures: SmallVec<[Reg; 4]> },
    Return { val: Option<Reg> },

    // --- data structures (1:1 with TermOp) ---
    AllocList { dst: Reg, elems: SmallVec<[Reg; 4]> },
    AllocMap { dst: Reg, fields: Vec<ConstantId>, vals: SmallVec<[Reg; 4]> },
    AllocMapSpread { dst: Reg, entries: Vec<MapSpreadEntry>, ins: SmallVec<[Reg; 4]> },
    AllocElement { dst: Reg, tag: ConstantId, prop_keys: Vec<ConstantId>, ins: SmallVec<[Reg; 4]> },
    MakeEnumVariant { dst: Reg, name: ConstantId, fields: SmallVec<[Reg; 4]> },
    GetField { dst: Reg, obj: Reg, field: ConstantId },
    SetField { dst: Reg, obj: Reg, field: ConstantId, val: Reg },
    GetIndex { dst: Reg, obj: Reg, idx: Reg },
    SetIndex { dst: Reg, obj: Reg, idx: Reg, val: Reg },

    // --- in-place mutation variants (M4; selected by escape analysis) ---
    /// Like [`SetField`](Inst::SetField) but mutates `obj`'s backing store and
    /// reuses its id. Only emitted when proven unique + non-escaping.
    SetFieldInPlace { dst: Reg, obj: Reg, field: ConstantId, val: Reg },
    /// Like [`SetIndex`](Inst::SetIndex) but in place.
    SetIndexInPlace { dst: Reg, obj: Reg, idx: Reg, val: Reg },

    // --- state (nested keys resolved from the frame's loop-index context) ---
    /// Lazy state init. The init expression's block is lowered *inline*
    /// immediately after this op (followed by a `StateWrite` that commits it).
    /// On a cache hit the slot's value is loaded into `dst` and control jumps to
    /// `after` (past the inline init block); on a miss it falls through to run
    /// the init block. `key` is the explicit `state(expr)` key register, if any.
    StateInit { dst: Reg, base: StateKey, in_loop: bool, after: Label, key: Option<Reg> },
    StateRead { dst: Reg, base: StateKey, in_loop: bool },
    StateWrite { dst: Reg, base: StateKey, in_loop: bool, val: Reg, key: Option<Reg> },

    // --- match (fat op; reuses the graph engine's match_pattern) ---
    /// Test the subject in `subject` against arm `arm` of match term `term`.
    /// On mismatch (or failed guard) jump to `next`; on success bind pattern
    /// variables into their flat registers and fall through. `dst` is the match
    /// term's result register (written by the arm body's join `Move`).
    MatchArm { subject: Reg, term: TermId, arm: u16, next: Label, dst: Reg },

    /// No match arm matched the subject — raise the same runtime error the graph
    /// engine does (`No matching pattern for value: …`), formatting `subject`.
    MatchFail { subject: Reg },

    /// A compile-time error term reached at runtime.
    Error { msg: ConstantId },
}

impl Inst {
    /// The destination register this instruction writes its result to, if it
    /// produces a value in the current frame. Used by the VM's best-effort
    /// trace hook to record `(origin term, result)` at instruction retire.
    ///
    /// `Call`/`MethodCall` have a `dst` but do not fill it in the current frame
    /// — the result is delivered when the callee frame returns — so they are
    /// deliberately excluded here and traced from the frame-return path instead.
    /// Control-flow, loop-cursor, and store-only ops produce no traceable value.
    pub fn dst(&self) -> Option<Reg> {
        match self {
            Inst::LoadConst { dst, .. }
            | Inst::LoadNil { dst }
            | Inst::LoadBool { dst, .. }
            | Inst::Move { dst, .. }
            | Inst::Add { dst, .. }
            | Inst::Sub { dst, .. }
            | Inst::Mul { dst, .. }
            | Inst::Div { dst, .. }
            | Inst::Mod { dst, .. }
            | Inst::Neg { dst, .. }
            | Inst::Eq { dst, .. }
            | Inst::Ne { dst, .. }
            | Inst::Lt { dst, .. }
            | Inst::Le { dst, .. }
            | Inst::Gt { dst, .. }
            | Inst::Ge { dst, .. }
            | Inst::Not { dst, .. }
            | Inst::Concat { dst, .. }
            | Inst::BuiltinCall { dst, .. }
            | Inst::MakeClosure { dst, .. }
            | Inst::MakeOverloadSet { dst, .. }
            | Inst::AllocList { dst, .. }
            | Inst::AllocMap { dst, .. }
            | Inst::AllocMapSpread { dst, .. }
            | Inst::AllocElement { dst, .. }
            | Inst::MakeEnumVariant { dst, .. }
            | Inst::GetField { dst, .. }
            | Inst::SetField { dst, .. }
            | Inst::GetIndex { dst, .. }
            | Inst::SetIndex { dst, .. }
            | Inst::SetFieldInPlace { dst, .. }
            | Inst::SetIndexInPlace { dst, .. }
            | Inst::StateInit { dst, .. }
            | Inst::StateRead { dst, .. }
            | Inst::StateWrite { dst, .. } => Some(*dst),
            // Value delivered on frame return, not in this frame.
            Inst::Call { .. } | Inst::MethodCall { .. } => None,
            // No traceable single-value result.
            Inst::Jump { .. }
            | Inst::JumpIfFalse { .. }
            | Inst::JumpIfTrue { .. }
            | Inst::ForEachInit { .. }
            | Inst::ForEachNext { .. }
            | Inst::RangeInit { .. }
            | Inst::RangeNext { .. }
            | Inst::WhileInit { .. }
            | Inst::LoopBumpIdx { .. }
            | Inst::LoopPop { .. }
            | Inst::Return { .. }
            | Inst::MatchArm { .. }
            | Inst::MatchFail { .. }
            | Inst::Error { .. } => None,
        }
    }

    /// The source registers this instruction reads, for the trace's input list.
    /// Best-effort: gathered before the instruction executes (a `dst` that
    /// aliases a source would otherwise clobber it). Inputs enrich the full
    /// trace dump (`--record-trace`); `explain` needs only the result, so an
    /// approximate or empty list here is acceptable.
    pub fn input_regs(&self) -> SmallVec<[Reg; 4]> {
        let mut v: SmallVec<[Reg; 4]> = SmallVec::new();
        match self {
            Inst::Move { src, .. } | Inst::Neg { a: src, .. } | Inst::Not { a: src, .. } => {
                v.push(*src)
            }
            Inst::Add { a, b, .. }
            | Inst::Sub { a, b, .. }
            | Inst::Mul { a, b, .. }
            | Inst::Div { a, b, .. }
            | Inst::Mod { a, b, .. }
            | Inst::Eq { a, b, .. }
            | Inst::Ne { a, b, .. }
            | Inst::Lt { a, b, .. }
            | Inst::Le { a, b, .. }
            | Inst::Gt { a, b, .. }
            | Inst::Ge { a, b, .. }
            | Inst::Concat { a, b, .. } => {
                v.push(*a);
                v.push(*b);
            }
            Inst::GetField { obj, .. } => v.push(*obj),
            Inst::GetIndex { obj, idx, .. } => {
                v.push(*obj);
                v.push(*idx);
            }
            Inst::SetField { obj, val, .. }
            | Inst::SetFieldInPlace { obj, val, .. } => {
                v.push(*obj);
                v.push(*val);
            }
            Inst::SetIndex { obj, idx, val, .. }
            | Inst::SetIndexInPlace { obj, idx, val, .. } => {
                v.push(*obj);
                v.push(*idx);
                v.push(*val);
            }
            Inst::BuiltinCall { args, .. } => v.extend(args.iter().copied()),
            Inst::AllocList { elems, .. } => v.extend(elems.iter().copied()),
            Inst::AllocMap { vals, .. } => v.extend(vals.iter().copied()),
            Inst::MakeEnumVariant { fields, .. } => v.extend(fields.iter().copied()),
            Inst::StateWrite { val, .. } => v.push(*val),
            // Everything else contributes no simple input registers.
            _ => {}
        }
        v
    }
}

/// One lowered function: the program root block, or a `FunctionDef` body.
#[derive(Debug, Clone)]
pub struct BytecodeFn {
    /// The source function, or `None` for the implicit root function.
    pub func_id: Option<FunctionId>,
    /// Human-readable name for disassembly / stack traces.
    pub name: Option<String>,
    /// The linear instruction stream.
    pub code: Vec<Inst>,
    /// Size of this function's flat register file.
    pub reg_count: u16,
    /// Flat registers that receive positional parameters, in order.
    pub param_regs: Vec<Reg>,
    /// Flat registers that receive captured values, in capture order.
    pub capture_regs: Vec<Reg>,
    /// Flat register that receives the self-reference (recursion), if any.
    pub self_ref_reg: Option<Reg>,
    /// Number of loop-cursor slots this function needs.
    pub loop_slots: u16,
    /// Flat register holding this function's result — the register of the last
    /// term in the entry block (mirrors the graph engine's `block_result`).
    /// `None` for an empty body (result is `Nil`). Read when a frame runs off
    /// the end of its code without an explicit `Return`.
    pub result_reg: Option<Reg>,
    /// Source term each instruction was lowered from, parallel to `code`. Used
    /// to annotate a runtime error with the failing term's source position.
    pub origins: Vec<Option<TermId>>,
}

/// A whole program lowered to bytecode.
#[derive(Debug, Clone)]
pub struct BytecodeProgram {
    /// The implicit root function (the program's top-level block).
    pub root: BytecodeFn,
    /// Lowered function bodies, indexed by `FunctionId`.
    pub fns: Vec<BytecodeFn>,
    /// Precomputed pattern-binding targets for each match arm: `(match term,
    /// arm index) -> [(binding name, flat register)]`. A `MatchArm` op runs the
    /// shared `match_pattern`, then writes each captured value into these
    /// registers (the flat-register equivalent of the graph engine's
    /// `apply_pattern_bindings`).
    pub match_binds: std::collections::HashMap<(TermId, u16), Vec<(String, Reg)>>,
}

impl BytecodeProgram {
    /// The `BytecodeFn` for a given `FunctionId`.
    pub fn function(&self, id: FunctionId) -> &BytecodeFn {
        &self.fns[id.0 as usize]
    }

    /// Resolve a frame's function reference: `None` is the implicit root
    /// function, `Some(id)` a lowered `FunctionDef` body.
    pub fn function_or_root(&self, id: Option<FunctionId>) -> &BytecodeFn {
        match id {
            None => &self.root,
            Some(fid) => self.function(fid),
        }
    }
}

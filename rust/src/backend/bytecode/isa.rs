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
    LoadConst {
        dst: Reg,
        k: ConstantId,
    },
    /// `dst = nil` — a branch's default result before an arm overwrites it (so
    /// an empty/untaken arm yields `nil`, matching the graph's `block_result`).
    LoadNil {
        dst: Reg,
    },
    /// `dst = <bool>` — short-circuit results of `And` (`false`) / `Or` (`true`).
    LoadBool {
        dst: Reg,
        val: bool,
    },
    /// `dst = src` — lowered `Copy`, `Phi` init, `phi_out`, and arm-result joins.
    Move {
        dst: Reg,
        src: Reg,
    },

    // --- arithmetic (1:1 with TermOp) ---
    Add {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Sub {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mul {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Div {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mod {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Neg {
        dst: Reg,
        a: Reg,
    },

    // --- comparison ---
    Eq {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Ne {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Lt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Le {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Gt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Ge {
        dst: Reg,
        a: Reg,
        b: Reg,
    },

    // --- logical / string ---
    Not {
        dst: Reg,
        a: Reg,
    },
    Concat {
        dst: Reg,
        a: Reg,
        b: Reg,
    },

    // --- control flow (new forms; no TermOp equivalent) ---
    Jump {
        to: Label,
    },
    JumpIfFalse {
        cond: Reg,
        to: Label,
    },
    JumpIfTrue {
        cond: Reg,
        to: Label,
    },
    /// `??` coalescing: jump to `to` when `cond` is present (not `Nil`/`Pending`),
    /// leaving the LHS in the result register; otherwise fall through to the RHS arm.
    JumpIfPresent {
        cond: Reg,
        to: Label,
    },
    /// Control-flow Pending absorption: jump to `to` when `cond` is a
    /// `Value::Pending`. The lowering routes `to` to an arm that copies `cond`
    /// into the control expression's result register, so `if`/`while`/`for`/
    /// `match` on a Pending run no branch and evaluate to that Pending.
    JumpIfPending {
        cond: Reg,
        to: Label,
    },

    // --- loops (replace Frame.loop_states) ---
    /// Snapshot `iter`'s list into loop slot `slot`; push a loop-index context
    /// entry if `idx_ctx` (state keying).
    ForEachInit {
        iter: Reg,
        slot: LoopSlot,
        idx_ctx: bool,
    },
    /// Advance the ForEach cursor: on exhaustion jump to `exit`; else write the
    /// element into `var` and bump the loop-index context.
    ForEachNext {
        slot: LoopSlot,
        var: Reg,
        exit: Label,
    },
    /// Initialize an integer range cursor `[start, end)` into `slot`.
    RangeInit {
        start: Reg,
        end: Reg,
        slot: LoopSlot,
        idx_ctx: bool,
    },
    /// Advance the range cursor: on exhaustion jump to `exit`; else write the
    /// current integer into `var`.
    RangeNext {
        slot: LoopSlot,
        var: Reg,
        exit: Label,
    },
    /// Initialize a while-loop's iteration counter (loop-index context) in `slot`.
    WhileInit {
        slot: LoopSlot,
    },
    /// Bump a while-loop's per-iteration index context in `slot`.
    LoopBumpIdx {
        slot: LoopSlot,
    },
    /// Pop the loop-index context for `slot` on loop exit / break.
    LoopPop {
        slot: LoopSlot,
    },
    /// Push the current iteration's body result (`src`) onto loop slot `slot`'s
    /// collection accumulator. Emitted once per iteration for a collecting loop
    /// (`x = for …`), after the body and its phi-outs; `continue` jumps past it
    /// (that iteration contributes nothing) and `break` exits before it.
    LoopCollect {
        slot: LoopSlot,
        src: Reg,
    },
    /// Materialize loop slot `slot`'s collection accumulator into a list and
    /// write it to `dst` (the collecting loop's result value). Emitted at the
    /// loop's normal-termination exit, just before [`LoopPop`](Inst::LoopPop).
    LoopCollectEnd {
        slot: LoopSlot,
        dst: Reg,
    },

    // --- calls ---
    Call {
        dst: Reg,
        callee: Reg,
        args: SmallVec<[Reg; 4]>,
    },
    MethodCall {
        dst: Reg,
        recv: Reg,
        name: ConstantId,
        args: SmallVec<[Reg; 4]>,
        /// Class to fall back on when the receiver's own label answers
        /// nothing — see [`crate::program::TermOp::MethodCall`].
        hint: Option<ConstantId>,
    },
    /// `dst = name(args…)`. `in_place` is set by escape analysis (M4) when the
    /// builtin is a mutation (`append`/`set`/…) whose container argument is
    /// provably unique + non-escaping — the VM then lets it mutate + reuse the
    /// backing store instead of cloning. Always false unless
    /// `OptFlags::in_place_mutation` proved it.
    BuiltinCall {
        dst: Reg,
        name: ConstantId,
        args: SmallVec<[Reg; 4]>,
        in_place: bool,
    },
    MakeClosure {
        dst: Reg,
        func: FunctionId,
        caps: SmallVec<[Reg; 4]>,
    },
    MakeOverloadSet {
        dst: Reg,
        closures: SmallVec<[Reg; 4]>,
    },
    Return {
        val: Option<Reg>,
    },

    // --- data structures (1:1 with TermOp) ---
    AllocList {
        dst: Reg,
        elems: SmallVec<[Reg; 4]>,
    },
    AllocMap {
        dst: Reg,
        fields: Vec<ConstantId>,
        vals: SmallVec<[Reg; 4]>,
        /// The class name constant when this allocates a class instance (see
        /// `crate::classes`); `None` for a plain record.
        class: Option<ConstantId>,
    },
    AllocMapSpread {
        dst: Reg,
        entries: Vec<MapSpreadEntry>,
        ins: SmallVec<[Reg; 4]>,
    },
    AllocElement {
        dst: Reg,
        tag: ConstantId,
        prop_keys: Vec<ConstantId>,
        ins: SmallVec<[Reg; 4]>,
    },
    MakeEnumVariant {
        dst: Reg,
        name: ConstantId,
        fields: SmallVec<[Reg; 4]>,
    },
    GetField {
        dst: Reg,
        obj: Reg,
        field: ConstantId,
    },
    SetField {
        dst: Reg,
        obj: Reg,
        field: ConstantId,
        val: Reg,
    },
    GetIndex {
        dst: Reg,
        obj: Reg,
        idx: Reg,
    },
    SetIndex {
        dst: Reg,
        obj: Reg,
        idx: Reg,
        val: Reg,
    },

    // --- in-place mutation variants (M4; selected by escape analysis) ---
    /// Like [`SetField`](Inst::SetField) but mutates `obj`'s backing store and
    /// reuses its id. Only emitted when proven unique + non-escaping.
    SetFieldInPlace {
        dst: Reg,
        obj: Reg,
        field: ConstantId,
        val: Reg,
    },
    /// Like [`SetIndex`](Inst::SetIndex) but in place.
    SetIndexInPlace {
        dst: Reg,
        obj: Reg,
        idx: Reg,
        val: Reg,
    },

    // --- cells (`var` bindings) ---
    /// Allocate a cell holding `init` and put the cell in `dst`.
    CellNew {
        dst: Reg,
        init: Reg,
    },
    /// Dereference the cell in `cell` into `dst`.
    CellRead {
        dst: Reg,
        cell: Reg,
    },
    /// Write `val` through the cell in `cell`, mirroring it into `dst`.
    CellWrite {
        dst: Reg,
        cell: Reg,
        val: Reg,
    },

    // --- state (nested keys resolved from the frame's loop-index context) ---
    /// Lazy state init. The init expression's block is lowered *inline*
    /// immediately after this op (followed by a `StateWrite` that commits it).
    /// On a cache hit the slot's value is loaded into `dst` and control jumps to
    /// `after` (past the inline init block); on a miss it falls through to run
    /// the init block. `key` is the explicit `state(expr)` key register, if any.
    StateInit {
        dst: Reg,
        base: StateKey,
        in_loop: bool,
        after: Label,
        key: Option<Reg>,
    },
    StateRead {
        dst: Reg,
        base: StateKey,
        in_loop: bool,
    },
    /// Commit `val` into the state slot and mirror it into `dst`. `init` marks
    /// the write that commits a `StateInit` block's result: such a write does
    /// NOT commit a `Pending` value (the slot stays uninitialized so the init
    /// re-runs next frame), whereas an ordinary `state x = …` reassignment
    /// (`init = false`) commits whatever it is given.
    StateWrite {
        dst: Reg,
        base: StateKey,
        in_loop: bool,
        val: Reg,
        key: Option<Reg>,
        init: bool,
    },

    // --- match (fat op; reuses the graph engine's match_pattern) ---
    /// Test the subject in `subject` against arm `arm` of match term `term`.
    /// On mismatch (or failed guard) jump to `next`; on success bind pattern
    /// variables into their flat registers and fall through. `dst` is the match
    /// term's result register (written by the arm body's join `Move`).
    MatchArm {
        subject: Reg,
        term: TermId,
        arm: u16,
        next: Label,
        dst: Reg,
    },

    /// No match arm matched the subject — raise the same runtime error the graph
    /// engine does (`No matching pattern for value: …`), formatting `subject`.
    MatchFail {
        subject: Reg,
    },

    /// A compile-time error term reached at runtime.
    Error {
        msg: ConstantId,
    },
}

/// Declare the [`Opcode`] tag set from one list of [`Inst`] variant names, and
/// derive from it everything that must agree with that list: the enum,
/// [`Opcode::ALL`], [`Opcode::COUNT`], [`Opcode::name`], and [`Inst::opcode`].
/// Written this way because the four were previously transcribed by hand, and a
/// new `Inst` variant that reached only three of them would silently mis-index
/// every per-opcode counter after it.
macro_rules! define_opcodes {
    ($($name:ident),* $(,)?) => {
        /// The opcode of an [`Inst`], stripped of its operands — a dense, `Copy`
        /// tag for counting and grouping instructions (see [`crate::profile`]).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Opcode {
            $($name),*
        }

        impl Opcode {
            /// Every opcode, in declaration order. Index equals the
            /// discriminant, so `ALL[op as usize] == op`.
            pub const ALL: &'static [Opcode] = &[$(Opcode::$name),*];

            /// Number of distinct opcodes — the width of a per-opcode counter
            /// array.
            pub const COUNT: usize = Opcode::ALL.len();

            /// The variant name, as written in the source ("LoadConst", "Add", …).
            pub fn name(self) -> &'static str {
                match self {
                    $(Opcode::$name => stringify!($name)),*
                }
            }
        }

        impl Inst {
            /// This instruction's [`Opcode`] — its variant tag without operands.
            pub fn opcode(&self) -> Opcode {
                match self {
                    $(Inst::$name { .. } => Opcode::$name),*
                }
            }
        }
    };
}

define_opcodes!(
    LoadConst,
    LoadNil,
    LoadBool,
    Move,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Not,
    Concat,
    Jump,
    JumpIfFalse,
    JumpIfTrue,
    JumpIfPresent,
    JumpIfPending,
    ForEachInit,
    ForEachNext,
    RangeInit,
    RangeNext,
    WhileInit,
    LoopBumpIdx,
    LoopPop,
    LoopCollect,
    LoopCollectEnd,
    Call,
    MethodCall,
    BuiltinCall,
    MakeClosure,
    MakeOverloadSet,
    Return,
    AllocList,
    AllocMap,
    AllocMapSpread,
    AllocElement,
    MakeEnumVariant,
    GetField,
    SetField,
    GetIndex,
    SetIndex,
    SetFieldInPlace,
    SetIndexInPlace,
    CellNew,
    CellRead,
    CellWrite,
    StateInit,
    StateRead,
    StateWrite,
    MatchArm,
    MatchFail,
    Error,
);

/// Match the single backpatchable branch-target label of a branching op, over
/// whichever borrow `$e` carries — match ergonomics yield `&Label` from a
/// shared borrow and `&mut Label` from a mutable one. One variant list backs
/// both [`Inst::branch_target`] and [`Inst::branch_target_mut`], so the CFG's
/// successors and lowering's backpatch can never drift apart.
macro_rules! branch_target_of {
    ($e:expr) => {
        match $e {
            Inst::Jump { to }
            | Inst::JumpIfFalse { to, .. }
            | Inst::JumpIfTrue { to, .. }
            | Inst::JumpIfPresent { to, .. }
            | Inst::JumpIfPending { to, .. }
            | Inst::ForEachNext { exit: to, .. }
            | Inst::RangeNext { exit: to, .. }
            | Inst::MatchArm { next: to, .. }
            | Inst::StateInit { after: to, .. } => Some(to),
            _ => None,
        }
    };
}

impl Inst {
    /// The `dst` operand this instruction carries, if it has one at all. The
    /// single list of value-producing variants, shared by [`dst`](Inst::dst)
    /// (which excludes the calls) and [`for_each_write`](Inst::for_each_write)
    /// (which includes them) so the two cannot disagree about which ops write.
    fn dst_field(&self) -> Option<Reg> {
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
            | Inst::StateWrite { dst, .. }
            | Inst::CellNew { dst, .. }
            | Inst::CellRead { dst, .. }
            | Inst::CellWrite { dst, .. }
            | Inst::LoopCollectEnd { dst, .. }
            | Inst::Call { dst, .. }
            | Inst::MethodCall { dst, .. } => Some(*dst),
            // No single-value result.
            Inst::Jump { .. }
            | Inst::JumpIfFalse { .. }
            | Inst::JumpIfTrue { .. }
            | Inst::JumpIfPresent { .. }
            | Inst::JumpIfPending { .. }
            | Inst::ForEachInit { .. }
            | Inst::ForEachNext { .. }
            | Inst::RangeInit { .. }
            | Inst::RangeNext { .. }
            | Inst::WhileInit { .. }
            | Inst::LoopBumpIdx { .. }
            | Inst::LoopPop { .. }
            | Inst::LoopCollect { .. }
            | Inst::Return { .. }
            | Inst::MatchArm { .. }
            | Inst::MatchFail { .. }
            | Inst::Error { .. } => None,
        }
    }

    /// The destination register this instruction writes its result to *in the
    /// current frame*, if any. Used by the VM's per-instruction hooks to record
    /// `(origin term, result)` at instruction retire.
    ///
    /// `Call`/`MethodCall` carry a `dst` but do not fill it in the current
    /// frame — the result is delivered when the callee frame returns — so they
    /// are excluded here and recorded from the frame-return path instead. For
    /// the full dataflow write set, use [`for_each_write`](Inst::for_each_write).
    pub fn dst(&self) -> Option<Reg> {
        match self {
            Inst::Call { .. } | Inst::MethodCall { .. } => None,
            other => other.dst_field(),
        }
    }

    /// Every register this instruction writes — the full dataflow write set the
    /// liveness / last-use analyses need. Beyond the plain `dst` operand (see
    /// [`dst_field`](Inst::dst_field)) that means `Call`/`MethodCall` (the
    /// return lands in `dst`), the loop variable of `ForEachNext`/`RangeNext`,
    /// and a `MatchArm`'s precomputed pattern-binding registers (whose own
    /// `dst` is written later by the arm body's join `Move`, not by the op).
    pub fn for_each_write(
        &self,
        match_binds: &std::collections::HashMap<(TermId, u16), Vec<(String, Reg)>>,
        mut f: impl FnMut(Reg),
    ) {
        if let Some(dst) = self.dst_field() {
            f(dst);
            return;
        }
        match self {
            Inst::ForEachNext { var, .. } | Inst::RangeNext { var, .. } => f(*var),
            Inst::MatchArm { term, arm, .. } => {
                if let Some(binds) = match_binds.get(&(*term, *arm)) {
                    for (_, r) in binds {
                        f(*r);
                    }
                }
            }
            _ => {}
        }
    }

    /// Every register this instruction reads, by mutable reference so a pass can
    /// rewrite operands in place (copy propagation) as well as enumerate them.
    ///
    /// This is the sole definition of the read set — the dataflow passes and the
    /// VM's trace hook (via [`read_regs`](Inst::read_regs)) both come here.
    /// It is the read-side counterpart of [`for_each_write`](Inst::for_each_write);
    /// the two together define the dataflow of an instruction, so a new `Inst`
    /// variant must be added to both.
    pub fn for_each_read_mut(&mut self, mut f: impl FnMut(&mut Reg)) {
        match self {
            Inst::Move { src, .. }
            | Inst::Neg { a: src, .. }
            | Inst::Not { a: src, .. }
            | Inst::JumpIfFalse { cond: src, .. }
            | Inst::JumpIfTrue { cond: src, .. }
            | Inst::JumpIfPresent { cond: src, .. }
            | Inst::JumpIfPending { cond: src, .. }
            | Inst::ForEachInit { iter: src, .. }
            | Inst::GetField { obj: src, .. }
            | Inst::CellNew { init: src, .. }
            | Inst::CellRead { cell: src, .. }
            | Inst::LoopCollect { src, .. }
            | Inst::MatchArm { subject: src, .. }
            | Inst::MatchFail { subject: src } => f(src),
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
            | Inst::Concat { a, b, .. }
            | Inst::GetIndex { obj: a, idx: b, .. }
            | Inst::RangeInit {
                start: a, end: b, ..
            }
            | Inst::CellWrite {
                cell: a, val: b, ..
            }
            | Inst::SetField { obj: a, val: b, .. }
            | Inst::SetFieldInPlace { obj: a, val: b, .. } => {
                f(a);
                f(b);
            }
            Inst::SetIndex { obj, idx, val, .. } | Inst::SetIndexInPlace { obj, idx, val, .. } => {
                f(obj);
                f(idx);
                f(val);
            }
            Inst::Call { callee, args, .. } => {
                f(callee);
                for a in args {
                    f(a);
                }
            }
            Inst::MethodCall { recv, args, .. } => {
                f(recv);
                for a in args {
                    f(a);
                }
            }
            Inst::BuiltinCall { args, .. }
            | Inst::MakeClosure { caps: args, .. }
            | Inst::MakeOverloadSet { closures: args, .. }
            | Inst::AllocList { elems: args, .. }
            | Inst::AllocMap { vals: args, .. }
            | Inst::MakeEnumVariant { fields: args, .. } => {
                for a in args {
                    f(a);
                }
            }
            Inst::AllocMapSpread { ins, .. } | Inst::AllocElement { ins, .. } => {
                for a in ins {
                    f(a);
                }
            }
            Inst::Return { val } => {
                if let Some(v) = val {
                    f(v);
                }
            }
            Inst::StateWrite { val, key, .. } => {
                f(val);
                if let Some(k) = key {
                    f(k);
                }
            }
            Inst::StateInit { key, .. } => {
                if let Some(k) = key {
                    f(k);
                }
            }
            Inst::LoadConst { .. }
            | Inst::LoadNil { .. }
            | Inst::LoadBool { .. }
            | Inst::Jump { .. }
            | Inst::ForEachNext { .. }
            | Inst::RangeNext { .. }
            | Inst::WhileInit { .. }
            | Inst::LoopBumpIdx { .. }
            | Inst::LoopPop { .. }
            | Inst::LoopCollectEnd { .. }
            | Inst::StateRead { .. }
            | Inst::Error { .. } => {}
        }
    }

    /// Every register this instruction reads. A read-only view over
    /// [`for_each_read_mut`](Inst::for_each_read_mut), which stays the single
    /// definition of the read set rather than being transcribed a second time.
    /// Clones the instruction to borrow it mutably; that is a compile-time
    /// pass cost (small enum, at most a `SmallVec` of register indices), not a
    /// runtime one — nothing on the VM's hot path calls this.
    pub fn read_regs(&self) -> SmallVec<[Reg; 4]> {
        let mut v: SmallVec<[Reg; 4]> = SmallVec::new();
        self.clone().for_each_read_mut(|r| v.push(*r));
        v
    }

    /// Whether control can fall through to the following instruction. False only
    /// for the unconditional terminators (`Jump` diverts; `Return`/`Error`/
    /// `MatchFail` end the path).
    pub fn falls_through(&self) -> bool {
        !matches!(
            self,
            Inst::Jump { .. } | Inst::Return { .. } | Inst::Error { .. } | Inst::MatchFail { .. }
        )
    }

    /// This instruction's explicit branch target label, if any — the CFG's
    /// non-fall-through successor. Every branching op carries at most one; the
    /// variant list lives in the `branch_target_of!` macro, shared with
    /// [`branch_target_mut`](Inst::branch_target_mut).
    pub fn branch_target(&self) -> Option<Label> {
        branch_target_of!(self).copied()
    }

    /// Mutable view of the single backpatchable branch-target label — the write
    /// counterpart of [`branch_target`](Inst::branch_target). Lowering resolves
    /// a placeholder block-id to a code offset through this.
    pub fn branch_target_mut(&mut self) -> Option<&mut Label> {
        branch_target_of!(self)
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
    /// `ConstantId` → `NativeFnId`, for the string constants that name a
    /// builtin — the resolution every `BuiltinCall` needs, done once per
    /// program instead of once per call. `u32::MAX` means "not a builtin name"
    /// (or unresolved, when the program was lowered without a native table);
    /// the VM falls back to a by-name lookup then, which is also where the
    /// "Unknown builtin" error comes from.
    ///
    /// Filled by [`resolve_builtin_names`](BytecodeProgram::resolve_builtin_names)
    /// after lowering, because lowering has no access to the host's native
    /// table — a host may register natives of its own.
    pub builtin_ids: Vec<u32>,
}

/// Sentinel in [`BytecodeProgram::builtin_ids`]: this constant does not name a
/// registered native.
pub const NOT_A_BUILTIN: u32 = u32::MAX;

impl BytecodeProgram {
    /// Resolve every string constant against the native table, filling
    /// [`builtin_ids`](Self::builtin_ids). Idempotent, and cheap: one pass over
    /// the constant table (hundreds of entries) against a hash index.
    pub fn resolve_builtin_names(
        &mut self,
        constants: &crate::constant_table::ConstantTable,
        lookup: impl Fn(&str) -> Option<u32>,
    ) {
        use crate::constant_table::ConstantValue;
        self.builtin_ids = constants
            .values()
            .iter()
            .map(|c| match c {
                ConstantValue::String(s) => lookup(s).unwrap_or(NOT_A_BUILTIN),
                _ => NOT_A_BUILTIN,
            })
            .collect();
    }

    /// The native id a constant names, if it was resolved as a builtin.
    #[inline]
    pub fn builtin_id(&self, k: crate::constant_table::ConstantId) -> Option<u32> {
        match self.builtin_ids.get(k.0 as usize).copied() {
            Some(NOT_A_BUILTIN) | None => None,
            Some(id) => Some(id),
        }
    }

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

#[cfg(test)]
mod opcode_tests {
    use super::*;

    /// `Opcode::ALL` is hand-written; this catches the case where a new `Inst`
    /// variant is added and its opcode is left out of the array (which would
    /// silently mis-index every per-opcode counter after it).
    #[test]
    fn all_is_dense_and_in_discriminant_order() {
        assert_eq!(Opcode::ALL.len(), Opcode::COUNT);
        for (i, op) in Opcode::ALL.iter().enumerate() {
            assert_eq!(*op as usize, i, "Opcode::ALL is out of order at {i}");
        }
    }

    #[test]
    fn opcode_name_matches_debug() {
        for op in Opcode::ALL {
            assert_eq!(op.name(), format!("{op:?}"));
        }
    }

    #[test]
    fn inst_reports_its_opcode() {
        assert_eq!(Inst::LoadNil { dst: 3 }.opcode(), Opcode::LoadNil);
        assert_eq!(Inst::Jump { to: 7 }.opcode(), Opcode::Jump);
        assert_eq!(
            Inst::GetIndex {
                dst: 0,
                obj: 1,
                idx: 2
            }
            .opcode(),
            Opcode::GetIndex
        );
    }
}

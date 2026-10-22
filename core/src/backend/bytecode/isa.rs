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
use std::collections::HashMap;

use crate::constant_table::ConstantId;
use crate::fxhash::FxBuildHasher;
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
///
/// `Serialize` backs the `inst` field of `show-bytecode --json` rows (see
/// [`super::disasm`]): the derived externally-tagged form,
/// `{"Add": {"dst": 4, "a": 0, "b": 0}}`, matching how the IR's `TermOp`
/// serializes. Constant-table operands (`ConstantId`) serialize as their index
/// number. Serialize-only: the bytecode is not an interchange format — the IR
/// JSON is — so there is no `Deserialize`.
/// The interned name of each argument of a call instruction, parallel to its
/// `args`; `None` for a positional argument. Empty for an all-positional call.
/// Carried straight over from [`crate::program::Term::arg_names`] — the VM
/// binds against the callee's parameter list, so these are metadata, never
/// operands: no register accessor touches them.
pub type ArgNames = SmallVec<[Option<ConstantId>; 4]>;

/// `skip_serializing_if` helper for [`ArgNames`] (the derive needs a path).
fn arg_names_empty(v: &ArgNames) -> bool {
    v.is_empty()
}

/// The builtins an [`Inst::Intrinsic`] stands for: the cheap, pure,
/// fixed-arity ones a program calls thousands of times a frame, where the
/// call machinery (argument vector, native context, result stack) costs
/// several times what the builtin computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum IntrinsicOp {
    Float,
    Int,
    Min,
    Max,
    Clamp,
    Abs,
    Floor,
    Len,
    Type,
}

impl IntrinsicOp {
    pub const COUNT: usize = 9;

    /// Every intrinsic, in discriminant order.
    pub const ALL: [IntrinsicOp; IntrinsicOp::COUNT] = [
        IntrinsicOp::Float,
        IntrinsicOp::Int,
        IntrinsicOp::Min,
        IntrinsicOp::Max,
        IntrinsicOp::Clamp,
        IntrinsicOp::Abs,
        IntrinsicOp::Floor,
        IntrinsicOp::Len,
        IntrinsicOp::Type,
    ];

    /// The builtin's name, as a program writes it.
    pub const fn name(self) -> &'static str {
        match self {
            IntrinsicOp::Float => "float",
            IntrinsicOp::Int => "int",
            IntrinsicOp::Min => "min",
            IntrinsicOp::Max => "max",
            IntrinsicOp::Clamp => "clamp",
            IntrinsicOp::Abs => "abs",
            IntrinsicOp::Floor => "floor",
            IntrinsicOp::Len => "len",
            IntrinsicOp::Type => "type",
        }
    }

    /// How many arguments the builtin takes. A call with any other count is
    /// left a `BuiltinCall`, whose native reports the mistake.
    pub const fn arity(self) -> usize {
        match self {
            IntrinsicOp::Float
            | IntrinsicOp::Int
            | IntrinsicOp::Abs
            | IntrinsicOp::Floor
            | IntrinsicOp::Len
            | IntrinsicOp::Type => 1,
            IntrinsicOp::Min | IntrinsicOp::Max => 2,
            IntrinsicOp::Clamp => 3,
        }
    }
}

/// What a [`CellRead`](Inst::CellRead) is for. Chosen by
/// `super::cells::analyze`; everything but `Shared` is an optimization that
/// `OptFlags::in_place_cells` gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum CellReadMode {
    /// The contents may be kept by whatever reads the register, so the cell
    /// stops owning them. Always correct.
    #[default]
    Shared,
    /// The contents are only looked into (`get xs[i]`, `get r.f`, `len`) by
    /// the instructions right after, and the register is never read again:
    /// the cell goes on owning them.
    Peek,
    /// The first half of an in-place write (`set xs[i] = v`): the register
    /// receives a container nothing else holds — the cell's own if it owns
    /// one, a copy otherwise — for the in-place mutation that follows.
    Take,
}

impl CellReadMode {
    fn is_shared(&self) -> bool {
        *self == CellReadMode::Shared
    }
}

/// How a [`CellWrite`](Inst::CellWrite) hands its value on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum CellWriteMode {
    /// `val` may be held elsewhere, and is mirrored into `dst`. Always correct.
    #[default]
    Shared,
    /// The second half of an in-place write whose value nothing reads: `val`
    /// is the container its [`Take`](CellReadMode::Take) produced, mutated,
    /// and the cell becomes its only holder.
    Put,
    /// Like `Put`, but the write is the last thing its function does, so its
    /// value is the function's result. Whether that is read is the caller's
    /// business ([`ResultUse`]): the frame knows, and the write behaves as
    /// `Put` when it is not and as `Shared` when it is.
    PutTail,
}

impl CellWriteMode {
    fn is_shared(&self) -> bool {
        *self == CellWriteMode::Shared
    }
}

/// Whether a [`Call`](Inst::Call)'s result is read, as far as the caller can
/// tell statically. It reaches the callee as a flag on its frame, where a
/// [`PutTail`](CellWriteMode::PutTail) consults it: `fn put(i, v) set xs[i] =
/// v end` evaluates to the whole list, and a caller that drops that on the
/// floor should not cost the cell its ownership of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum ResultUse {
    /// Something may read it. Always correct.
    #[default]
    Read,
    /// Nothing reads it: the call is a statement.
    Dropped,
    /// The call is the last thing *its* function does, so the answer is
    /// whatever that function's own caller said.
    Forwarded,
}

impl ResultUse {
    fn is_read(&self) -> bool {
        *self == ResultUse::Read
    }
}

#[derive(Debug, Clone, serde::Serialize)]
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
    /// Snapshot `iter`'s list into loop slot `slot` and push an `Index` part
    /// onto the frame's state path (per-iteration state keying).
    ForEachInit {
        iter: Reg,
        slot: LoopSlot,
    },
    /// Advance the ForEach cursor: on exhaustion jump to `exit`; else write the
    /// element into `var` and bump the frame path's innermost `Index` part.
    ForEachNext {
        slot: LoopSlot,
        var: Reg,
        exit: Label,
    },
    /// Initialize an integer range cursor `[start, end)` into `slot` and push an
    /// `Index` part onto the frame's state path.
    RangeInit {
        start: Reg,
        end: Reg,
        slot: LoopSlot,
    },
    /// Advance the range cursor: on exhaustion jump to `exit`; else write the
    /// current integer into `var`.
    RangeNext {
        slot: LoopSlot,
        var: Reg,
        exit: Label,
    },
    /// Initialize a while-loop's iteration counter in `slot` and push its
    /// `Index` part onto the frame's state path.
    WhileInit {
        slot: LoopSlot,
    },
    /// Bump a while-loop's per-iteration path index in `slot`.
    LoopBumpIdx {
        slot: LoopSlot,
    },
    /// `for … key k`: replace the counted loop `slot`'s positional `Index`
    /// part at the end of the frame path with `Key(hash(key))` for the rest of
    /// this iteration, and write nil to `dst`. An equal key twice in one run of
    /// the loop is a runtime error (docs/features/inline-state.md, Keyed loops).
    LoopKey {
        dst: Reg,
        slot: LoopSlot,
        key: Reg,
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
        /// The written name of each argument (`f(x, limit: 10)`), parallel to
        /// `args`. Empty means every argument is positional — the common case,
        /// and the one the VM's fast path takes. Serialization-skipped when
        /// empty, so a positional call's JSON row is unchanged.
        #[serde(default, skip_serializing_if = "arg_names_empty")]
        arg_names: ArgNames,
        /// The call is never a memoized scope: the caller mutates its result
        /// in place (escape analysis, `InPlaceSet::memoizes_call`), so no
        /// record may hold that result. Serialization-skipped when false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_memo: bool,
        /// Whether anything reads this call's result (see [`ResultUse`]).
        /// Serialization-skipped when it is the default.
        #[serde(default, skip_serializing_if = "ResultUse::is_read")]
        result: ResultUse,
    },
    MethodCall {
        dst: Reg,
        recv: Reg,
        name: ConstantId,
        args: SmallVec<[Reg; 4]>,
        /// Class to fall back on when the receiver's own label answers
        /// nothing — see [`crate::program::TermOp::MethodCall`].
        hint: Option<ConstantId>,
        /// The written name of each argument (`f(x, limit: 10)`), parallel to
        /// `args`. Empty means every argument is positional — the common case,
        /// and the one the VM's fast path takes. Serialization-skipped when
        /// empty, so a positional call's JSON row is unchanged.
        #[serde(default, skip_serializing_if = "arg_names_empty")]
        arg_names: ArgNames,
    },
    /// `dst = name(args…)`. `in_place` is set by escape analysis (M4) when the
    /// builtin is a mutation (`append`/`set`/…) whose container argument is
    /// provably unique + non-escaping — the VM then lets it mutate + reuse the
    /// backing store instead of cloning. Always false unless
    /// `OptFlags::in_place_mutation` proved it.
    ///
    /// On `__enum_all` (`Enum.all`) the flag means the result is the caller's
    /// own: the VM hands out a copy of the stack's shared list, because the
    /// same analysis took this read for the fresh root of an in-place web
    /// (`escape::Analysis::copies_enum_all`).
    BuiltinCall {
        dst: Reg,
        name: ConstantId,
        args: SmallVec<[Reg; 4]>,
        in_place: bool,
        /// The written name of each argument (`f(x, limit: 10)`), parallel to
        /// `args`. Empty means every argument is positional — the common case,
        /// and the one the VM's fast path takes. Serialization-skipped when
        /// empty, so a positional call's JSON row is unchanged.
        #[serde(default, skip_serializing_if = "arg_names_empty")]
        arg_names: ArgNames,
    },
    /// `dst = name(args…)` for one of the trivial builtins the VM's
    /// straight-line loop retires itself ([`IntrinsicOp`]): a positional,
    /// right-arity [`BuiltinCall`](Inst::BuiltinCall) whose name resolved to
    /// the core native, rewritten by
    /// [`BytecodeProgram::select_intrinsics`] once the native table is in
    /// hand. The fast loop computes the common cases (numbers; a list or a
    /// string for `len`; anything for `type`) inline. Every other case — an
    /// error, a `Pending` or dual-number argument, a string to convert, a
    /// profiled, traced or audited run — is the general executor's, which
    /// runs it as exactly the builtin call it was (`name` is kept for that),
    /// so results, errors and profile attribution are the builtin's.
    ///
    /// `args` holds `op.arity()` registers; the rest are unused and zero.
    Intrinsic {
        dst: Reg,
        op: IntrinsicOp,
        name: ConstantId,
        args: [Reg; 3],
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
        /// The literal's shared shape (`fields` in order), built at lowering;
        /// `None` when a field name repeats.
        #[serde(skip)]
        shape: Option<std::sync::Arc<crate::record::Shape>>,
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
        /// Absence-tolerant read: a record without the field, or a Nil object,
        /// yields Nil instead of erroring. Set only for `TermOp::GetFieldOpt`.
        opt: bool,
        /// Inline cache: the record shape this site last read and the field's
        /// slot in it.
        #[serde(skip)]
        cache: crate::record::FieldCache,
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
        /// Absence-tolerant read — see [`Inst::GetField`]'s `opt`.
        opt: bool,
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
    /// Dereference the cell in `cell` into `dst`. `mode` says what the read
    /// is for, which decides whether the cell goes on owning its container
    /// (see [`CellReadMode`]). Serialization-skipped when it is the default.
    CellRead {
        dst: Reg,
        cell: Reg,
        #[serde(default, skip_serializing_if = "CellReadMode::is_shared")]
        mode: CellReadMode,
    },
    /// Write `val` through the cell in `cell`, mirroring it into `dst` (see
    /// [`CellWriteMode`] for the in-place forms, which do not mirror).
    /// Serialization-skipped when it is the default.
    CellWrite {
        dst: Reg,
        cell: Reg,
        val: Reg,
        #[serde(default, skip_serializing_if = "CellWriteMode::is_shared")]
        mode: CellWriteMode,
    },

    // --- state (slots resolved from the frame's call path; see `Vm::state_key`) ---
    /// Lazy state init. The init expression's block is lowered *inline*
    /// immediately after this op (followed by a `StateWrite` that commits it).
    /// On a cache hit the slot's value is loaded into `dst` and control jumps to
    /// `after` (past the inline init block); on a miss it falls through to run
    /// the init block. `key` is the explicit `state(expr)` key register, if any;
    /// when present it makes the slot absolute, ignoring the frame's path.
    StateInit {
        dst: Reg,
        base: StateKey,
        after: Label,
        key: Option<Reg>,
        /// A `memo(key)` declaration: the key resolves to a
        /// [`PathPart::Memo`](crate::stack::PathPart::Memo) slot, not a `Key` one.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        memo: bool,
    },
    StateRead {
        dst: Reg,
        base: StateKey,
        /// Loop `Index` parts to drop from the frame path to reach the
        /// declaration's slot (see [`crate::program::Term::path_pop`]).
        path_pop: u32,
    },
    /// Commit `val` into the state slot and mirror it into `dst`. `init` marks
    /// the write that commits a `StateInit` block's result: such a write does
    /// NOT commit a `Pending` value (the slot stays uninitialized so the init
    /// re-runs next frame), whereas an ordinary `state x = …` reassignment
    /// (`init = false`) commits whatever it is given.
    StateWrite {
        dst: Reg,
        base: StateKey,
        val: Reg,
        key: Option<Reg>,
        init: bool,
        /// Loop `Index` parts to drop from the frame path to reach the
        /// declaration's slot — nonzero for a reassignment nested deeper in
        /// loops than its `state` declaration, which is what makes the
        /// top-level accumulator idiom persist to one slot.
        path_pop: u32,
        /// The committed value was produced by an in-place mutation (escape
        /// analysis rewrote its producer, see `escape.rs`), so the slot may
        /// already hold the same heap id with different contents. The VM then
        /// treats the write as a change without comparing — the comparison
        /// that decides whether the run reached a fixed point
        /// ([`crate::run_deps`]) cannot see through an in-place edit.
        mutated: bool,
        /// The commit write of a `memo(key)` declaration (see
        /// [`Inst::StateInit`]); `init` is always set with it.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        memo: bool,
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

    /// Jump table over a run of match arms keyed on string literals, int
    /// literals and enum variants (see [`SwitchTable`]). A string, int or
    /// variant subject jumps to the first arm of the run that can match it,
    /// or past the run when none can. Any other subject falls through to the
    /// run's first [`MatchArm`](Inst::MatchArm), which tests the arms in
    /// order as if the switch were not there.
    MatchSwitch {
        subject: Reg,
        table: Box<SwitchTable>,
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

    // --- static values (`crate::static_cache`; root code, and function
    // code for the values of its `in_fn` module) ---
    /// In front of a run of static instructions. When every slot the run
    /// *feeds* is filled on the running stack (the stack keeps that as one
    /// counter per run, indexed by `run`), each slot in `slots` is loaded
    /// into its register and control jumps to `after`, past the run and its
    /// [`StaticStore`](Inst::StaticStore). Otherwise falls through and the
    /// run executes.
    ///
    /// `slots` is what the run stores itself. What it feeds is that plus the
    /// slots of every later run that reads one of its registers
    /// ([`StaticRun::feeds`](crate::static_cache::StaticRun)): the run is
    /// only skipped when those later runs will be skipped too.
    StaticSkip {
        after: Label,
        run: u32,
        slots: Box<[(u32, Reg)]>,
    },
    /// Behind a run of static instructions: keep each register in its slot,
    /// marking the value immutable. A value that cannot be kept (it holds a
    /// function, a `var` cell, a handle or a pending result) rejects every
    /// slot of the instruction for good, and the run then executes on every
    /// run as it would without the cache.
    StaticStore {
        slots: Box<[(u32, Reg)]>,
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
    LoopKey,
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
    MatchSwitch,
    MatchFail,
    Error,
    StaticSkip,
    StaticStore,
    Intrinsic,
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
            | Inst::StateInit { after: to, .. }
            | Inst::StaticSkip { after: to, .. } => Some(to),
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
            | Inst::Intrinsic { dst, .. }
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
            | Inst::LoopKey { dst, .. }
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
            | Inst::MatchSwitch { .. }
            | Inst::MatchFail { .. }
            | Inst::StaticSkip { .. }
            | Inst::StaticStore { .. }
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
            Inst::StaticSkip { slots, .. } => {
                for (_, r) in slots.iter() {
                    f(*r);
                }
            }
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
            | Inst::LoopKey { key: src, .. }
            | Inst::MatchArm { subject: src, .. }
            | Inst::MatchSwitch { subject: src, .. }
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
            Inst::Intrinsic { op, args, .. } => {
                for a in &mut args[..op.arity()] {
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
            Inst::StaticStore { slots } => {
                for (_, r) in slots.iter_mut() {
                    f(r);
                }
            }
            Inst::StaticSkip { .. }
            | Inst::LoadConst { .. }
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

    /// This instruction's single explicit branch target label, if it is a
    /// one-target branch. The variant list lives in the `branch_target_of!`
    /// macro, shared with [`branch_target_mut`](Inst::branch_target_mut).
    ///
    /// A [`MatchSwitch`](Inst::MatchSwitch) has many targets and answers
    /// `None`: a pass that needs the CFG's edges uses
    /// [`targets`](Inst::targets) and
    /// [`for_each_target_mut`](Inst::for_each_target_mut) instead.
    fn branch_target(&self) -> Option<Label> {
        branch_target_of!(self).copied()
    }

    /// Every label this instruction can jump to — the CFG's non-fall-through
    /// successors. One for an ordinary branch, the whole table for a
    /// [`MatchSwitch`](Inst::MatchSwitch), none for anything else.
    pub fn targets(&self) -> SmallVec<[Label; 2]> {
        match self {
            Inst::MatchSwitch { table, .. } => table.targets().collect(),
            other => other.branch_target().into_iter().collect(),
        }
    }

    /// Mutable view of every label in [`targets`](Inst::targets), for a pass
    /// that moves code and has to retarget the jumps into it.
    pub fn for_each_target_mut(&mut self, mut f: impl FnMut(&mut Label)) {
        match self {
            Inst::MatchSwitch { table, .. } => table.targets_mut().for_each(f),
            other => {
                if let Some(to) = other.branch_target_mut() {
                    f(to);
                }
            }
        }
    }

    /// Mutable view of the single backpatchable branch-target label — the write
    /// counterpart of [`branch_target`](Inst::branch_target). Lowering resolves
    /// a placeholder block-id to a code offset through this.
    pub fn branch_target_mut(&mut self) -> Option<&mut Label> {
        branch_target_of!(self)
    }
}

/// What a [`SwitchCase`] is keyed on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub enum SwitchKey {
    /// A string literal pattern.
    Str(String),
    /// An int literal pattern.
    Int(i64),
    /// A variant pattern, by its tag (`Kind.park`).
    Variant(String),
}

/// One entry of a [`SwitchTable`]: subjects with this key jump to `to`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SwitchCase {
    pub key: SwitchKey,
    pub to: Label,
    /// The key decides the arm, so `to` is the arm's body: an unguarded
    /// literal, or an unguarded variant without payload. Otherwise `to` is the
    /// arm's own [`MatchArm`](Inst::MatchArm), which still tests the payload,
    /// binds, and runs the guard, and goes on down the arms when that fails.
    pub whole: bool,
}

/// The jump table of a [`Inst::MatchSwitch`], over a run of consecutive match
/// arms whose every alternative is a string literal, an int literal or an
/// enum variant.
///
/// Each key maps to the *first* arm of the run that names it. No earlier arm
/// of the run can match a subject with that key — their keys differ — so
/// jumping there is what testing the arms in order would have arrived at.
/// A subject of a keyed kind that no case names matches nothing in the run
/// and goes to `miss`, the instruction after the run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SwitchTable {
    /// The cases, in arm order.
    pub cases: Vec<SwitchCase>,
    pub miss: Label,
    // Index into `cases`, by key. Labels live in `cases` only, so retargeting
    // them (`targets_mut`) cannot leave the lookup behind.
    #[serde(skip)]
    strs: HashMap<Box<str>, u32, FxBuildHasher>,
    #[serde(skip)]
    ints: HashMap<i64, u32, FxBuildHasher>,
    #[serde(skip)]
    variants: HashMap<Box<str>, u32, FxBuildHasher>,
}

impl SwitchTable {
    /// Build the table. A key listed twice keeps its first case.
    pub fn new(mut cases: Vec<SwitchCase>, miss: Label) -> SwitchTable {
        let mut seen: Vec<SwitchKey> = Vec::new();
        cases.retain(|c| {
            let first = !seen.contains(&c.key);
            if first {
                seen.push(c.key.clone());
            }
            first
        });
        let mut table = SwitchTable {
            cases,
            miss,
            strs: HashMap::default(),
            ints: HashMap::default(),
            variants: HashMap::default(),
        };
        for (i, case) in table.cases.iter().enumerate() {
            let i = i as u32;
            match &case.key {
                SwitchKey::Str(s) => table.strs.insert(s.as_str().into(), i),
                SwitchKey::Int(n) => table.ints.insert(*n, i),
                SwitchKey::Variant(tag) => table.variants.insert(tag.as_str().into(), i),
            };
        }
        table
    }

    fn targets(&self) -> impl Iterator<Item = Label> + '_ {
        self.cases
            .iter()
            .map(|c| c.to)
            .chain(std::iter::once(self.miss))
    }

    fn targets_mut(&mut self) -> impl Iterator<Item = &mut Label> + '_ {
        self.cases
            .iter_mut()
            .map(|c| &mut c.to)
            .chain(std::iter::once(&mut self.miss))
    }

    /// Where a string subject goes.
    #[inline]
    pub fn str_target(&self, s: &str) -> Label {
        self.strs
            .get(s)
            .map_or(self.miss, |&i| self.cases[i as usize].to)
    }

    /// Where an int subject goes.
    #[inline]
    pub fn int_target(&self, n: i64) -> Label {
        self.ints
            .get(&n)
            .map_or(self.miss, |&i| self.cases[i as usize].to)
    }

    /// Where a variant subject goes, or `None` to test the arms in order: a
    /// case that skips its arm's test is only right for the payload-free
    /// value its pattern describes, and a host may build a variant with any
    /// payload under any tag.
    #[inline]
    pub fn variant_target(&self, tag: &str, payload_free: bool) -> Option<Label> {
        match self.variants.get(tag) {
            None => Some(self.miss),
            Some(&i) => {
                let case = &self.cases[i as usize];
                (payload_free || !case.whole).then_some(case.to)
            }
        }
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
    /// Flat registers that receive the parameters, in order, followed by the
    /// was-it-passed flag of each parameter that has a default value
    /// (`FunctionDef::bound_arg_count` registers in all).
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
    /// Every variant the program declares, numbered — what the running
    /// stack's [`EnumTable`](crate::enum_table::EnumTable) is built from.
    pub enum_layout: std::sync::Arc<crate::enum_table::EnumLayout>,
    /// `ConstantId` → the slot of the variant that string constant is the tag
    /// of ([`NO_SLOT`](crate::enum_table::NO_SLOT) for every other constant),
    /// so `MakeEnumVariant` takes its tag from the stack's table instead of
    /// interning the text. Short of the constant table after a patched
    /// reload added constants; those are never tags.
    pub variant_slots: Vec<u32>,
    /// What the root code's static-value instructions refer to: how many
    /// slots a stack running this lowering needs, what each holds, and the
    /// identity the stack's table is stamped with. Empty unless the program
    /// was lowered with `OptFlags::static_cache`.
    pub statics: crate::static_cache::StaticLayout,
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

    /// Rewrite the calls of the trivial builtins to [`Inst::Intrinsic`]:
    /// every positional `BuiltinCall` of the right arity whose name resolved
    /// (in [`resolve_builtin_names`](Self::resolve_builtin_names), which must
    /// have run) to the *core* native of an [`IntrinsicOp`]. A host that
    /// registered a native of its own under one of those names gets its
    /// native called as before: the name then resolves to an id that is not
    /// the core builtin's. A call the compiler bound to a user or module
    /// definition was never a `BuiltinCall` to begin with. Returns how many
    /// calls were rewritten.
    ///
    /// One instruction replaces one instruction, with the same registers, so
    /// no label, origin or static-slot range moves. It runs after every
    /// other pass for that reason, and because those passes know the calls
    /// by name.
    pub fn select_intrinsics(&mut self, natives: &crate::native_fn::NativeFnTable) -> usize {
        let mut rewritten = 0;
        let ids = &self.builtin_ids;
        for func in std::iter::once(&mut self.root).chain(self.fns.iter_mut()) {
            for inst in &mut func.code {
                let Inst::BuiltinCall {
                    dst,
                    name,
                    args,
                    in_place: false,
                    arg_names,
                } = &*inst
                else {
                    continue;
                };
                if !arg_names.is_empty() {
                    continue;
                }
                let id = match ids.get(name.0 as usize).copied() {
                    Some(NOT_A_BUILTIN) | None => continue,
                    Some(id) => id,
                };
                let Some(op) = natives.intrinsic_op(crate::native_fn::NativeFnId(id)) else {
                    continue;
                };
                if args.len() != op.arity() {
                    continue;
                }
                let mut regs = [0; 3];
                regs[..args.len()].copy_from_slice(args);
                *inst = Inst::Intrinsic {
                    dst: *dst,
                    op,
                    name: *name,
                    args: regs,
                };
                rewritten += 1;
            }
        }
        rewritten
    }

    /// The native id a constant names, if it was resolved as a builtin.
    #[inline]
    pub fn builtin_id(&self, k: crate::constant_table::ConstantId) -> Option<u32> {
        match self.builtin_ids.get(k.0 as usize).copied() {
            Some(NOT_A_BUILTIN) | None => None,
            Some(id) => Some(id),
        }
    }

    /// The slot of the declared variant a constant is the tag of.
    #[inline]
    pub fn variant_slot(&self, k: crate::constant_table::ConstantId) -> Option<u32> {
        match self.variant_slots.get(k.0 as usize).copied() {
            Some(crate::enum_table::NO_SLOT) | None => None,
            Some(slot) => Some(slot),
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
                idx: 2,
                opt: false
            }
            .opcode(),
            Opcode::GetIndex
        );
    }
}

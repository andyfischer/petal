//! Program - A block of code represented as a collection of terms and blocks.
//!
//! See docs/Architecture.md for the surrounding compiler design.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::ast::Pattern;
use crate::constant_table::{ConstantId, ConstantTable, ConstantValue};
use crate::ir_serialize::{deserialize_termid_map, serialize_termid_map};
use crate::source_map::SourceMap;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique identifier for a program within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramId(pub u32);

/// Unique identifier for a term within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TermId(pub u32);

/// Unique identifier for a block within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub u32);

/// Global term identifier - unique within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GlobalTermId {
    pub program: ProgramId,
    pub term: TermId,
}

/// Register index within a Frame's register file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegisterIndex(pub u16);

/// Unique key for persistent state values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StateKey(pub u64);

/// Identifier for a function definition within a Program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FunctionId(pub u32);

/// Identifier for a runtime closure instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClosureId(pub u32);

/// Identifier for a runtime overload set (multi-arity function dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OverloadSetId(pub u32);

/// Entry in a map-with-spread allocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MapSpreadEntry {
    /// Spread all fields from the input at the given index
    Spread(usize),
    /// Set a named field from the input at the given index
    Named(ConstantId, usize),
}

// ---------------------------------------------------------------------------
// TermOp
// ---------------------------------------------------------------------------

/// The operation a term performs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermOp {
    // --- Core ---
    /// Load a constant from the constant table
    Constant(ConstantId),
    /// A parse error - message stored as a constant
    Error(ConstantId),

    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,

    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,

    // Logical
    Not,
    /// Short-circuit AND: inputs=[left], child_blocks=[rhs_block]
    And,
    /// Short-circuit OR: inputs=[left], child_blocks=[rhs_block]
    Or,
    /// Short-circuit coalesce `??`: inputs=[left], child_blocks=[rhs_block].
    /// Yields the RHS when the left is absent (`Nil` or `Pending`), else the left.
    Coalesce,

    // String
    Concat,

    // Binding & identity
    /// Variable reference / identity copy: inputs=[source_term]
    Copy,
    /// Pure-dataflow join point for values rebound inside a child block
    /// (conditional branches, loop bodies). Sits in the parent block *before*
    /// its associated control-flow term (`Branch`, `Match`, `ForLoop`,
    /// `WhileLoop`). On exec it initializes its register from `inputs[0]` —
    /// the pre-control-flow value of the name being joined. Child frames
    /// that rebind the name overwrite the phi's register on pop via
    /// `Block.phi_outs`; branches that don't rebind leave the init value in
    /// place. For loops, each iteration's pop updates the register, and
    /// subsequent iterations read the updated value.
    Phi,

    // Control flow
    /// if/else: inputs=[cond], child_blocks=[then_block, else_block]
    Branch,
    /// for-in loop: inputs=[iterable], child_blocks=[body_block]
    ForLoop,
    /// Numeric for-loop over an integer range (`for i in range(a, b)`):
    /// inputs=[start, end] (both Int-producing terms), child_blocks=[body_block].
    /// Iterates the half-open range [start, end) with no list allocation.
    /// For single-arg `range(n)` the compiler supplies a constant 0 as start.
    NumericForLoop,
    /// while loop: child_blocks=[cond_block, body_block]
    WhileLoop,
    Break,
    Continue,
    /// Return from function: inputs=[value] or empty for bare return
    Return,

    // Functions
    /// Create a closure: inputs=[captured values]
    MakeClosure(FunctionId),
    /// Create an overload set from multiple closures: inputs=[closure0, closure1, ...]
    /// Each closure handles a different arity.
    MakeOverloadSet,
    /// Dynamic call: inputs=[callable, arg0, arg1, ...]
    Call,
    /// Method call: inputs=[object, arg0, arg1, ...], method name as constant.
    /// At runtime: tries record field first, then the receiver's class, then a
    /// builtin with the receiver prepended.
    ///
    /// `hint` is an optional class name for the last-resort case: the receiver
    /// carries a class label naming nothing in *this* program (or carries none
    /// at all), while the declaration of the slot it came from named a class.
    /// That is what a live edit looks like from the inside — the value predates
    /// the code — and the hint is how the call reaches the method anyway. It is
    /// consulted only after the label has failed, so it never overrides the
    /// receiver's own class. See `crate::typecheck::MethodDispatch`.
    MethodCall {
        name: ConstantId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<ConstantId>,
    },
    /// Static builtin call: inputs=[arg0, arg1, ...], builtin name as constant.
    /// Emitted when a bare, unshadowed builtin (e.g. `print`) is called directly,
    /// replacing the dynamic `Call` through a phantom `Copy` of the builtin.
    BuiltinCall(ConstantId),

    // State
    /// Initialize state if not yet set: inputs=[init_value], state_key set
    StateInit,
    /// Read persistent state: state_key set
    StateRead,
    /// Write persistent state: inputs=[value], state_key set
    StateWrite,

    // Cells (`var` bindings)
    /// Allocate a cell holding the initial value: inputs=[init].
    /// The term's *value* is the cell itself — the one place a `Value::Cell`
    /// is produced. Emitted for `var x = init`; a `state var` wraps this in the
    /// `StateInit` init block so the cell is created once and then persists.
    CellNew,
    /// Dereference a cell: inputs=[cell]. Every source-level read of a `var`
    /// name compiles to this, which is what keeps the containment invariant
    /// (§6d) — no other op forwards a cell into the value domain.
    CellRead,
    /// Write through a cell: inputs=[cell, value]. Emitted for `set x = ...`.
    /// Yields the written value so the op has something to put in its register;
    /// nothing reads it.
    CellWrite,

    // Data structures
    /// Allocate a list: inputs=[elem0, elem1, ...]
    AllocList,
    /// Allocate a map/record: inputs=[val0, val1, ...], field names stored here.
    /// `class` names the class this record instantiates (a `class` declaration's
    /// generated constructor sets it); `None` for an ordinary record literal.
    /// See `crate::classes`.
    AllocMap {
        fields: Vec<ConstantId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        class: Option<ConstantId>,
    },
    /// Allocate a map with spread: entries describe the order of spreads and named fields.
    /// inputs = [spread_source_0, ..., named_value_0, ...]
    /// Each entry is either Spread (index into inputs for the spread source map)
    /// or Named (field name constant + index into inputs for the value).
    AllocMapSpread {
        entries: Vec<MapSpreadEntry>,
    },
    /// Read a field: inputs=[object], field name as constant
    GetField(ConstantId),
    /// Tolerant field read: like [`TermOp::GetField`], but a record/class that
    /// lacks the field — or a Nil object — yields Nil instead of erroring. Only
    /// emitted where the program explicitly asked for absence-tolerance (the
    /// left side of `??`); a wrong-typed object is still a hard error.
    GetFieldOpt(ConstantId),
    /// Write a field: inputs=[object, value]
    SetField(ConstantId),
    /// Read by index: inputs=[object, index]
    GetIndex,
    /// Tolerant index read: the [`TermOp::GetIndex`] counterpart of
    /// [`TermOp::GetFieldOpt`] — a missing record key or a Nil object yields
    /// Nil. A list index out of bounds still errors.
    GetIndexOpt,
    /// Write by index: inputs=[object, index, value]
    SetIndex,

    // Elements (JSX-like)
    /// Allocate an element: inputs=[prop_val0, ..., child0, ...]
    /// prop_keys.len() determines where prop values end and children begin
    AllocElement {
        tag: ConstantId,
        prop_keys: Vec<ConstantId>,
    },

    // Enums
    /// Construct an enum variant: inputs=[field values], variant name as constant
    MakeEnumVariant(ConstantId),

    // Pattern matching
    /// Match expression: inputs=[subject], child_blocks=[arm body blocks]
    /// Arm metadata stored in Program.match_arms
    Match,
}

impl TermOp {
    /// The constant-table ids this op references into `Program.constants`.
    /// Single source of truth for the (previously duplicated) enumeration of
    /// which variants carry constants — used by IR validation to range-check
    /// them.
    pub fn constant_ids(&self) -> Vec<ConstantId> {
        match self {
            TermOp::Constant(c)
            | TermOp::Error(c)
            | TermOp::GetField(c)
            | TermOp::GetFieldOpt(c)
            | TermOp::SetField(c)
            | TermOp::BuiltinCall(c)
            | TermOp::MakeEnumVariant(c) => vec![*c],
            TermOp::MethodCall { name, hint } => {
                let mut v = vec![*name];
                v.extend(hint.iter().copied());
                v
            }
            TermOp::AllocMap { fields, class } => {
                let mut v = fields.clone();
                v.extend(class.iter().copied());
                v
            }
            TermOp::AllocElement { tag, prop_keys } => {
                let mut v = vec![*tag];
                v.extend(prop_keys.iter().copied());
                v
            }
            TermOp::AllocMapSpread { entries } => entries
                .iter()
                .filter_map(|e| match e {
                    MapSpreadEntry::Named(c, _) => Some(*c),
                    MapSpreadEntry::Spread(_) => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Term
// ---------------------------------------------------------------------------

/// `skip_serializing_if` helper for SmallVec fields (the derive needs a
/// callable path, and inherent `is_empty` doesn't name the generic).
fn smallvec_empty<A: smallvec::Array>(v: &SmallVec<A>) -> bool {
    v.is_empty()
}

/// Sentinel a deserialized term's `register` defaults to when the wire form
/// omits it (schema v0.2 makes registers optional). A real register can never
/// be `u16::MAX`: `register_count` is a `u16`, so an assignment reaching
/// 65535 registers would already overflow the frame. The IR loader recomputes
/// every register when it sees this value — see
/// `Program::recompute_registers` in `ir_validate.rs`.
pub const REGISTER_UNSET: RegisterIndex = RegisterIndex(u16::MAX);

fn register_unset() -> RegisterIndex {
    REGISTER_UNSET
}

/// `skip_serializing_if` helper for count fields whose default is 0.
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// A single expression/node in the program graph.
#[derive(Serialize, Deserialize)]
pub struct Term {
    pub id: TermId,
    pub op: TermOp,
    /// Input terms (dataflow edges)
    #[serde(default, skip_serializing_if = "smallvec_empty")]
    pub inputs: SmallVec<[TermId; 4]>,
    /// The block this term belongs to
    pub block_id: BlockId,
    /// Linked list ordering within the block. In-memory only since schema
    /// v0.2: the wire form is the block's ordered `terms` array, and the
    /// loader rebuilds these links from it (`Program::relink_block_terms`).
    /// Legacy v0 documents that still carry the links deserialize them
    /// (`skip_serializing` without `skip_deserializing`), and the loader
    /// reconstructs the `terms` arrays from the walk instead.
    #[serde(default, skip_serializing)]
    pub block_next: Option<TermId>,
    #[serde(default, skip_serializing)]
    pub block_prev: Option<TermId>,
    /// Optional name for binding terms (variable declarations)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Register assignment for evaluation. Optional on the wire: when any
    /// term omits it, the loader recomputes the whole assignment (see
    /// [`REGISTER_UNSET`]). `show-ir --json` always emits it.
    #[serde(default = "register_unset")]
    pub register: RegisterIndex,
    /// For state terms: unique identifier for state reconciliation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_key: Option<StateKey>,
    /// Child blocks for control flow terms (Branch, ForLoop, WhileLoop, Match, And, Or)
    #[serde(default, skip_serializing_if = "smallvec_empty")]
    pub child_blocks: SmallVec<[BlockId; 2]>,
    /// For a `StateWrite`/`StateRead`: how many innermost loop `Index` parts to
    /// drop from the live frame path so the access lands on the slot its
    /// **declaration** owns — the number of loop bodies between the declaration
    /// and this access, which are always in the same function (assigning to a
    /// captured binding is rejected). Zero for a same-depth access, and always
    /// zero on a `StateInit`, which *is* the declaration. This is what keeps the
    /// top-level accumulator idiom (`state xs = []` plus `xs = append(xs, i)`
    /// inside a `for`) writing to the one persisted slot.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub path_pop: u32,
    /// For a call term (`Call`/`MethodCall`/`BuiltinCall`): the stable
    /// **callsite id** — a hash of the canonical callee text plus its ordinal
    /// among identically-spelled callees in the enclosing function, qualified
    /// by module and enclosing-function chain. Pushed onto the callee frame's
    /// path as [`crate::stack::PathPart::Call`], which is what gives each
    /// callsite of a function its own `state` slots. Name/structure-derived so
    /// it survives a hot reload; see
    /// `Compiler::call_site_for` and docs/dev/state-call-paths.md §3.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_site: Option<u64>,
    /// For a loop control term (`ForLoop`/`NumericForLoop`/`WhileLoop`): collect
    /// each iteration's body result into a list and yield it as the term's
    /// value. Set only when the loop is used in value position (`x = for …`);
    /// a bare statement loop leaves this false so it allocates nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collect: bool,
    /// The binding this term defines was declared `config let` — a tuning
    /// knob. Direct manipulation defaults to editing config bindings and
    /// pinning the rest (see `direct_manipulation::propose_edits`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_config: bool,
}

impl Term {
    /// A term carrying no optional metadata: `state_key`, `child_blocks`,
    /// `path_pop`, `call_site`, `collect` and `is_config` all at their
    /// serialization defaults, and the block links unset. The single place
    /// those defaults are written down, so adding a `Term` field does not mean
    /// hunting down every construction site.
    pub fn new(
        id: TermId,
        op: TermOp,
        inputs: SmallVec<[TermId; 4]>,
        block_id: BlockId,
        name: Option<String>,
        register: RegisterIndex,
    ) -> Term {
        Term {
            id,
            op,
            inputs,
            block_id,
            block_next: None,
            block_prev: None,
            name,
            register,
            state_key: None,
            child_blocks: SmallVec::new(),
            path_pop: 0,
            call_site: None,
            collect: false,
            is_config: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// A control flow block within a Program.
#[derive(Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    /// The term that creates this block's scope. None for the root block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_term_id: Option<TermId>,
    /// Entry point for this block's term list. None for empty blocks.
    /// In-memory only since schema v0.2 (rebuilt from `terms` on load); still
    /// deserialized so legacy v0 documents can be walked. See `Term::block_next`.
    #[serde(default, skip_serializing)]
    pub entry: Option<TermId>,
    /// The block's terms in execution order — the schema v0.2 wire form of
    /// the intra-block ordering (replacing the `entry`/`block_next`/
    /// `block_prev` linked list, which is derived from this on load).
    /// Maintained by the compiler in lockstep with the linked list
    /// (`Compiler::emit_term` is the single append site). Binding phantoms
    /// (params, captures, self-refs, builtins) are deliberately *not* listed —
    /// they don't execute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<TermId>,
    /// Parameter names for function body blocks and for-loop bodies
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_names: Vec<String>,
    /// Total registers needed for this block's frame. Optional on the wire
    /// (recomputed when registers are omitted; filled from the max register
    /// when absent).
    #[serde(default)]
    pub register_count: u16,
    /// Phi carry-outs: when this block's frame pops, copy each `src_term`'s
    /// register value to the parent frame at each `dest_term`'s register.
    /// Emitted when a conditional branch rebinds a name that was bound in
    /// an outer scope — see the phi-join discussion in `docs/dev/Architecture.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phi_outs: Vec<PhiOut>,
}

/// One phi-slot copy: read `src_term`'s value and write to `dest_term`'s
/// register in the parent frame when a child frame pops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhiOut {
    pub src_term: TermId,
    pub dest_term: TermId,
}

// ---------------------------------------------------------------------------
// FunctionDef
// ---------------------------------------------------------------------------

/// Compile-time function metadata.
#[derive(Serialize, Deserialize)]
pub struct FunctionDef {
    pub id: FunctionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
    pub body_block: BlockId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture_names: Vec<String>,
    /// Which body registers get capture values (indexed same as captures).
    /// Optional on the wire when registers are omitted: the loader re-derives
    /// each entry from the body block's binding phantom of the same name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture_registers: Vec<RegisterIndex>,
    /// Which body register gets the self-reference for recursion. Re-derived
    /// (from a body-block phantom named after the function) when registers
    /// are omitted on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_ref_register: Option<RegisterIndex>,
    #[serde(default)]
    pub register_count: u16,
}

/// Strip the internal `#arity` overload suffix from a function's internal name
/// (e.g. `"foo#2"` → `"foo"`), returning the base source name. Function names
/// carry at most one `#` (source identifiers can't contain it), so splitting on
/// the last one recovers the original source name.
pub fn base_fn_name(name: &str) -> &str {
    match name.rfind('#') {
        Some(pos) => &name[..pos],
        None => name,
    }
}

// ---------------------------------------------------------------------------
// OverloadSet
// ---------------------------------------------------------------------------

/// A set of function closures dispatched by argument count.
/// Created at runtime by MakeOverloadSet terms.
#[derive(Debug, Clone)]
pub struct OverloadEntry {
    pub arity: usize,
    pub closure_id: ClosureId,
}

// ---------------------------------------------------------------------------
// MatchArmMeta
// ---------------------------------------------------------------------------

/// Metadata for a compiled match arm.
#[derive(Serialize, Deserialize)]
pub struct MatchArmMeta {
    pub pattern: Pattern,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_block: Option<BlockId>,
    pub body_block: BlockId,
}

// ---------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------

/// The IR JSON schema version this build reads and writes. The loader also
/// accepts documents with no `schema` field at all (pre-v0.2 "schema v0"
/// shapes — see docs/dev/ir-as-target.md).
pub const IR_SCHEMA_VERSION: &str = "0.2";

fn default_schema() -> String {
    IR_SCHEMA_VERSION.to_string()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A compiled program ready for execution.
#[derive(Serialize, Deserialize)]
pub struct Program {
    /// Wire-format version tag (see [`IR_SCHEMA_VERSION`]). Serialized on
    /// every dump; a document without one deserializes to the current version
    /// and `Program::validate` rejects any other value.
    #[serde(default = "default_schema")]
    pub schema: String,
    pub id: ProgramId,
    /// Original source text. Optional for imported IR (see docs/ir-as-target.md).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,

    // IR data
    pub terms: Vec<Term>,
    pub blocks: Vec<Block>,
    pub root_block: BlockId,
    pub constants: ConstantTable,
    /// Source spans. Optional for imported IR.
    #[serde(default, skip_serializing_if = "SourceMap::is_empty")]
    pub source_map: SourceMap,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_errors: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionDef>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        serialize_with = "serialize_termid_map",
        deserialize_with = "deserialize_termid_map"
    )]
    pub match_arms: HashMap<TermId, Vec<MatchArmMeta>>,
    /// Index from block to all terms in that block (including phantoms).
    /// Built once at compile time to avoid O(N) scans over all terms.
    #[serde(skip)]
    pub block_terms: HashMap<BlockId, Vec<TermId>>,
    /// Non-fatal compile-time diagnostics (type-checker warnings). A compile-time
    /// artifact, NOT part of the portable IR — skipped in (de)serialization.
    #[serde(skip)]
    pub warnings: Vec<crate::diagnostic::Diagnostic>,
    /// Every class name this program declares, built-ins included.
    ///
    /// The class *table* is compile-time only, but the VM needs one question
    /// answered at runtime: is the label on this value a class that still
    /// exists here? A value can outlive the program that built it — that is
    /// what `transfer_state` is for — so a label alone does not prove its class
    /// is real. This is what tells a live instance apart from a leftover one,
    /// and it gates the `hint` on [`TermOp::MethodCall`].
    ///
    /// A `BTreeSet` rather than a `HashSet`: this is part of the serialized IR,
    /// and the lint round-trip asserts that formatting a file leaves its IR
    /// byte-identical — which a hash set's iteration order would break at
    /// random.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub class_names: std::collections::BTreeSet<String>,
}

impl Program {
    pub fn get_term(&self, id: TermId) -> &Term {
        &self.terms[id.0 as usize]
    }

    /// Iterate the program's state-bearing terms as `(state key, optional
    /// variable name)`. The single scan behind both `Env::state_key_names`
    /// (which keeps the named keys) and cross-run state transfer (which keeps
    /// every key) — each caller applies its own filter to this.
    pub fn state_terms(&self) -> impl Iterator<Item = (StateKey, Option<&String>)> {
        self.terms
            .iter()
            .filter_map(|t| t.state_key.map(|k| (k, t.name.as_ref())))
    }

    pub fn get_block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }

    /// Resolve a ConstantId that's expected to be a string. Returns None if not a string.
    pub fn get_string_constant(&self, cid: ConstantId) -> Option<&str> {
        match self.constants.get(cid) {
            ConstantValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Method name -> the function terms a `fn Class.name` declaration
    /// publishes under it, keyed by the same interned constant a
    /// [`TermOp::MethodCall`] carries.
    ///
    /// **Why this exists.** A user method is dispatched at runtime by name, so
    /// `base.dist2()` has no operand naming the function it calls — its inputs
    /// are the receiver and the arguments, nothing else. Every dataflow answer
    /// (`show-provenance`, `show-slice`, `explain`, `show-graph`) would then
    /// omit the code that computes the value, which is the one thing a slice
    /// must never do. This recovers the edge from the registration statement
    /// the compiler emits for each declaration
    /// (`__declare_method(class, name, func)`).
    ///
    /// It is a **may**-edge, like a cell's: dispatch is by name, so a call
    /// links to every method of that name in the program, not only the one the
    /// receiver's class would pick. Over-approximating is the safe direction —
    /// a slice that is too big loses precision, one that is too small computes
    /// a different value.
    pub fn dispatch_targets(&self) -> HashMap<ConstantId, Vec<TermId>> {
        let mut out: HashMap<ConstantId, Vec<TermId>> = HashMap::new();
        for term in &self.terms {
            let TermOp::BuiltinCall(name) = term.op else {
                continue;
            };
            if self.get_string_constant(name) != Some(crate::classes::DECLARE_METHOD_BUILTIN) {
                continue;
            }
            // inputs = [class name, method name, function] (`emit_declare_method`).
            let (Some(&method_tid), Some(&func)) = (term.inputs.get(1), term.inputs.get(2)) else {
                continue;
            };
            let TermOp::Constant(method_cid) = self.get_term(method_tid).op else {
                continue;
            };
            let slot = out.entry(method_cid).or_default();
            if !slot.contains(&func) {
                slot.push(func);
            }
        }
        out
    }
}

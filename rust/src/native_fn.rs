//! Native function FFI — Lua-inspired plugin system.
//!
//! Allows Rust functions to be registered and called from Petal code
//! via a stack-based API.

use std::collections::HashMap;

use serde::Serialize;

use crate::handle::{HandleClass, HandleClassId, HandleVal};
use crate::heap::Heap;
use crate::symbol::{SymbolId, SymbolTable};
use crate::value::Value;

/// Identifier for a registered native function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct NativeFnId(pub u32);

/// Result type for native functions: Ok(count) = number of results pushed.
pub type NativeResult = Result<u32, String>;

/// Signature for native functions.
pub type NativeFn = fn(&mut PetalCxt) -> NativeResult;

/// A native that owns captured state: a closure rather than a bare function
/// pointer. Registered with [`NativeFnTable::register_boxed`] /
/// [`Env::register_native_boxed`](crate::env::Env::register_native_boxed).
///
/// This is what an embedder reaches for when one Rust function serves many
/// natives — a C bridge forwarding to a host callback plus its `userdata`, a
/// scripting layer registering one native per host command — so it does not
/// need a pool of monomorphized trampolines or a global dispatch table.
///
/// It is `Fn`, not `FnMut`: the table is shared by every run of the `Env`, so
/// a native that mutates its captures does so through `Cell`/`RefCell`. It
/// need not be `Send` (an `Env` is not). The captured state belongs to the
/// `Env`, not to an execution: a forked execution
/// ([`Env::fork_execution`](crate::env::Env::fork_execution),
/// `run_speculative`) calls the same closure and sees the same captures.
pub type BoxedNativeFn = Box<dyn Fn(&mut PetalCxt) -> NativeResult>;

/// How a native function behaves when handed a `Value::Pending` argument.
/// Consulted at the single native-call boundary (see the bytecode VM's
/// `call_native_or_intrinsic`) only when a Pending arg is actually present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeClass {
    /// Default. A Pending argument is absorbed: the call returns the leftmost
    /// Pending arg without invoking the native (`sqrt(pending) -> pending`).
    Strict,
    /// Side-effecting emitter (`print`, `push_output`, …). A Pending argument
    /// makes the call a no-op returning `Nil` — it emits nothing.
    Effectful,
    /// The native inspects Pendings itself and must run normally
    /// (`__pending`/`__resolve`/`__reject`). Never intercepted.
    AllowPending,
}

/// The external inputs a native can read, as a bitmask — one bit per class
/// of host state that changes independently of the others. Declared at the
/// leaf where the knowledge lives ([`NativeEffects::reads`]) and propagated
/// interprocedurally by the reactive layers, so a scope, a block or a frame
/// can be re-run only when the class it depends on actually moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
pub struct InputClasses(pub u16);

impl InputClasses {
    /// Reads nothing outside the program's own state.
    pub const NONE: InputClasses = InputClasses(0);
    /// Pointer position and buttons.
    pub const POINTER: InputClasses = InputClasses(1 << 0);
    /// Key state and typed text.
    pub const KEYBOARD: InputClasses = InputClasses(1 << 1);
    /// Wall-clock or frame time.
    pub const CLOCK: InputClasses = InputClasses(1 << 2);
    /// Window or pane geometry.
    pub const VIEWPORT: InputClasses = InputClasses(1 << 3);
    /// Host-owned data the binding table does not cover (a data provider, a
    /// query cache, an editor buffer) — the same thing `note_host_read` says
    /// at runtime ([`PetalCxt::note_host_read`]).
    pub const HOST_DATA: InputClasses = InputClasses(1 << 4);
    /// The resource table: a `Pending` this native may answer differently
    /// once the resource resolves.
    pub const RESOURCES: InputClasses = InputClasses(1 << 5);
    /// The per-run random stream (which the native also advances).
    pub const RNG: InputClasses = InputClasses(1 << 6);
    /// A host→script binding outside the named classes: one chosen by
    /// *argument* (`binding(sym)`, whichever binding the symbol names), or a
    /// host-specific one — a font metric table, an injected theme, a model
    /// record. The class is resolved per binding symbol rather than at the
    /// leaf.
    pub const BINDINGS: InputClasses = InputClasses(1 << 7);

    pub const fn union(self, other: InputClasses) -> InputClasses {
        InputClasses(self.0 | other.0)
    }

    pub const fn contains(self, other: InputClasses) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for InputClasses {
    type Output = InputClasses;
    fn bitor(self, rhs: InputClasses) -> InputClasses {
        self.union(rhs)
    }
}

/// What a native does, declared once at registration
/// ([`NativeFnTable::register`]) so the reactive layers — the frame gate,
/// memoized scopes, dependency classes — can ask instead of inferring it
/// from activity counters after the call. Every native has one: registration
/// takes the row, so there is no such thing as an undeclared native.
///
/// A declaration is the *union over every path* through the native: a native
/// that consults the resource table only when handed a `Pending` still
/// declares [`RESOURCES`](InputClasses::RESOURCES). Under-declaring is the
/// bug this row exists to prevent (a memoized scope that called the native
/// would replay stale); over-declaring only costs a validation.
///
/// Nothing here says how much a native *costs*; scope-worthiness is a
/// separate decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeEffects {
    /// What this native reads, by input class. Empty = nothing external.
    pub reads: InputClasses,
    /// The result is a pure function of the arguments and `reads`, and the
    /// call may be re-evaluated at validation time without observable
    /// consequence (`hovered`, `mouse_x`, `time`). A probe is what gives a
    /// memoized scope its early cutoff: an unchanged answer keeps the scope
    /// valid even though the input moved.
    pub probe: bool,
    /// Pushes into an output buffer (a draw command, an event).
    pub emits: bool,
    /// Does something no replay can reproduce: prints, reseeds noise,
    /// creates or resolves a resource, advances a counter, reaches through a
    /// handle into host state, publishes a method.
    pub effect: bool,
    /// What to do with a `Pending` argument ([`NativeFnTable::set_class`]
    /// can still change it after registration).
    pub pending: NativeClass,
}

impl NativeEffects {
    /// Reads nothing, emits nothing, does nothing a replay could not
    /// reproduce: a pure function of its arguments. The row most natives
    /// want; the rest start from it.
    pub const PURE: NativeEffects = NativeEffects {
        reads: InputClasses::NONE,
        probe: false,
        emits: false,
        effect: false,
        pending: NativeClass::Strict,
    };

    /// Something a replay cannot reproduce (`print`, `noise_seed`).
    pub const EFFECT: NativeEffects = NativeEffects {
        effect: true,
        ..NativeEffects::PURE
    };

    /// Pushes into an output buffer, and no-ops on a `Pending` argument.
    pub const EMITS: NativeEffects = NativeEffects {
        emits: true,
        pending: NativeClass::Effectful,
        ..NativeEffects::PURE
    };

    /// A pure function of its arguments and the given input classes, safe to
    /// re-evaluate at validation.
    pub const fn probe(reads: InputClasses) -> NativeEffects {
        NativeEffects {
            reads,
            probe: true,
            ..NativeEffects::PURE
        }
    }

    /// Reads the given classes but is not re-evaluable as a probe (its
    /// answer is not a pure function of them, or re-running it would show).
    pub const fn reads(reads: InputClasses) -> NativeEffects {
        NativeEffects {
            reads,
            ..NativeEffects::PURE
        }
    }

    /// This row pushing into an output buffer, with the `Pending` policy
    /// left as it is (unlike [`EMITS`](Self::EMITS), which also no-ops on a
    /// `Pending` argument).
    pub const fn with_emits(self) -> NativeEffects {
        NativeEffects {
            emits: true,
            ..self
        }
    }

    /// This row with an effect.
    pub const fn with_effect(self) -> NativeEffects {
        NativeEffects {
            effect: true,
            ..self
        }
    }

    /// This row with the given `Pending`-argument policy.
    pub const fn with_pending(self, pending: NativeClass) -> NativeEffects {
        NativeEffects { pending, ..self }
    }
}

/// How a registered native is invoked: a bare function pointer (every
/// builtin, and most host natives) or a closure owning captured state.
enum NativeImpl {
    Fn(NativeFn),
    Boxed(BoxedNativeFn),
}

/// Entry in the native function table.
struct NativeFnEntry {
    name: String,
    func: NativeImpl,
    /// The Pending-argument policy: `effects.pending`, kept as its own field
    /// so the hot `intercept_pending` check is one load.
    class: NativeClass,
    /// The declared effect row.
    effects: NativeEffects,
}

/// Registry of native functions, mapping IDs to names and function pointers.
pub struct NativeFnTable {
    entries: Vec<NativeFnEntry>,
    /// Name → id index over `entries`. Every `BuiltinCall` resolves a name
    /// through it, several million times in a compute-heavy run, so the lookup
    /// must not be a scan of the table (which is ~150 entries deep and answers
    /// with a string compare per entry).
    by_name: HashMap<String, NativeFnId>,
    /// IDs for higher-order builtins that need evaluator intrinsic dispatch.
    pub intrinsic_map: Option<NativeFnId>,
    pub intrinsic_filter: Option<NativeFnId>,
    pub intrinsic_reduce: Option<NativeFnId>,
    pub intrinsic_for_each: Option<NativeFnId>,
    /// `sort`, which is *conditionally* an intrinsic: the one-argument
    /// `sort(list)` is an ordinary native, while `sort(list, cmp)` calls a
    /// user comparator and so must be driven by the VM. The dispatcher picks by
    /// argument count.
    pub intrinsic_sort: Option<NativeFnId>,
    /// `sort_by(list, key_fn)` / `sort_by(list, key_fn, descending)` — always an
    /// intrinsic; it calls the key function once per element.
    pub intrinsic_sort_by: Option<NativeFnId>,
    /// `__declare_method`, which the VM intercepts instead of calling: it
    /// publishes a user-declared `fn Class.method` into the running stack's
    /// method table, which no native can reach through [`PetalCxt`].
    pub intrinsic_declare_method: Option<NativeFnId>,
    /// Built-in class methods, indexed `class -> method -> native`. The same
    /// natives are in `entries` under their qualified names (`Rect.inset`);
    /// this index exists so method dispatch is a two-hop lookup on borrowed
    /// `&str`s rather than a formatted name per call.
    class_methods: HashMap<String, HashMap<String, NativeFnId>>,
}

impl NativeFnTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            intrinsic_map: None,
            intrinsic_filter: None,
            intrinsic_reduce: None,
            intrinsic_for_each: None,
            intrinsic_sort: None,
            intrinsic_sort_by: None,
            intrinsic_declare_method: None,
            class_methods: HashMap::new(),
        }
    }

    /// Index an already-registered native as the built-in implementation of
    /// `class.method`. See [`NativeFnTable::class_methods`].
    pub fn register_class_method(&mut self, class: &str, method: &str, id: NativeFnId) {
        self.class_methods
            .entry(class.to_string())
            .or_default()
            .insert(method.to_string(), id);
    }

    /// The built-in native implementing `class.method`, if any.
    pub fn lookup_class_method(&self, class: &str, method: &str) -> Option<NativeFnId> {
        self.class_methods.get(class)?.get(method).copied()
    }

    /// Register a native function together with its declared
    /// [`NativeEffects`], returning its ID. The row is what the reactive
    /// layers consult on every call, so it is not optional: a native that
    /// reaches host state without saying so would look pure, and every
    /// memoized scope that called it would replay stale.
    pub fn register(&mut self, name: &str, func: NativeFn, effects: NativeEffects) -> NativeFnId {
        self.insert(name, NativeImpl::Fn(func), effects)
    }

    /// [`register`](Self::register) for a native that owns captured state
    /// (see [`BoxedNativeFn`]). Everything else is identical: the id is
    /// allocated the same way, the row means the same thing, and
    /// [`set_class`](Self::set_class), the effect audit and the compiler's
    /// name resolution treat the two kinds alike.
    pub fn register_boxed(
        &mut self,
        name: &str,
        func: BoxedNativeFn,
        effects: NativeEffects,
    ) -> NativeFnId {
        self.insert(name, NativeImpl::Boxed(func), effects)
    }

    fn insert(&mut self, name: &str, func: NativeImpl, effects: NativeEffects) -> NativeFnId {
        let id = NativeFnId(self.entries.len() as u32);
        self.entries.push(NativeFnEntry {
            name: name.to_string(),
            func,
            class: effects.pending,
            effects,
        });
        // Last registration of a name wins, matching the scan this replaced:
        // it returned the *first* match, so a re-registration under an existing
        // name was previously unreachable. Overwriting is the more useful of
        // the two readings (a host replacing a builtin gets its own), and no
        // caller registers a duplicate today.
        self.by_name.insert(name.to_string(), id);
        id
    }

    /// Override the Pending-handling class of an already-registered native.
    /// Registration stays append-only (indices are stable); classification is
    /// applied afterward by id. The row's `pending` field is updated too, so
    /// the two never disagree.
    pub fn set_class(&mut self, id: NativeFnId, class: NativeClass) {
        let entry = &mut self.entries[id.0 as usize];
        entry.class = class;
        entry.effects.pending = class;
    }

    /// The declared effect row of a native.
    pub fn effects(&self, id: NativeFnId) -> NativeEffects {
        self.entries[id.0 as usize].effects
    }

    /// The Pending-handling class of a native (defaults to `Strict`).
    pub fn get_class(&self, id: NativeFnId) -> NativeClass {
        self.entries[id.0 as usize].class
    }

    /// Look up a native function by name.
    pub fn lookup_name(&self, name: &str) -> Option<NativeFnId> {
        self.by_name.get(name).copied()
    }

    /// Get the name of a native function by ID.
    pub fn get_name(&self, id: NativeFnId) -> &str {
        &self.entries[id.0 as usize].name
    }

    /// The function pointer of a native registered with
    /// [`register`](Self::register); `None` for a boxed native. To invoke
    /// either kind, use [`call`](Self::call).
    pub fn get_func(&self, id: NativeFnId) -> Option<NativeFn> {
        match self.entries[id.0 as usize].func {
            NativeImpl::Fn(f) => Some(f),
            NativeImpl::Boxed(_) => None,
        }
    }

    /// Whether the native was registered with a closure
    /// ([`register_boxed`](Self::register_boxed)).
    pub fn is_boxed(&self, id: NativeFnId) -> bool {
        matches!(self.entries[id.0 as usize].func, NativeImpl::Boxed(_))
    }

    /// Invoke a native against `cxt`. The one call site for both kinds: a
    /// bare function pointer is called directly, a boxed native through its
    /// closure.
    #[inline]
    pub fn call(&self, id: NativeFnId, cxt: &mut PetalCxt) -> NativeResult {
        match &self.entries[id.0 as usize].func {
            NativeImpl::Fn(f) => f(cxt),
            NativeImpl::Boxed(f) => f(cxt),
        }
    }

    /// Number of registered native functions.
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for NativeFnTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The handle passed to native functions, providing access to arguments,
/// result pushing, output, and heap.
pub struct PetalCxt<'a> {
    pub(crate) args: &'a [Value],
    pub(crate) heap: &'a mut Heap,
    pub(crate) output: &'a mut Vec<String>,
    pub(crate) symbols: &'a mut SymbolTable,
    pub(crate) output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
    /// Whether emits record their call site (copied from the owning
    /// `ExecutionContext`). Gates the push in [`push_output`](Self::push_output).
    pub(crate) trace_emit: bool,
    /// The owning context's call-site attribution for buffered output, borrowed
    /// so [`push_output`](Self::push_output) can stamp this call's
    /// [`origin`](Self::origin) onto the value it emits while `trace_emit` is on.
    /// See [`crate::execution_context::ExecutionContext::emit_origins`].
    pub(crate) emit_origins: &'a mut HashMap<SymbolId, Vec<crate::execution_context::EmitSite>>,
    /// The call chain that reached this native — its own call site, then the
    /// return address of each enclosing call, innermost first. Built by the VM
    /// only while `trace_emit` is on, and copied onto each value this call
    /// emits. Empty otherwise.
    pub(crate) emit_chain: &'a [crate::program::TermId],
    pub(crate) bindings: &'a mut HashMap<SymbolId, Value>,
    /// The running stack's dependency record (see [`crate::run_deps`]),
    /// borrowed so a binding read is noted and a host-data native can declare
    /// itself ([`note_host_read`](Self::note_host_read)).
    pub(crate) run_deps: &'a mut crate::run_deps::RunDeps,
    pub(crate) counters: &'a mut HashMap<SymbolId, u64>,
    /// Per-run xorshift64* PRNG state, borrowed from the owning
    /// `ExecutionContext` so the RNG builtins advance that context's stream.
    pub(crate) rng_state: &'a mut u64,
    /// Per-run Perlin-noise seed, borrowed from the owning `ExecutionContext`.
    pub(crate) noise_seed: &'a mut u64,
    /// The owning context's resource table, borrowed so the pending-resource
    /// builtins (`__pending`/`__resolve`/`__reject`) can create/resolve entries.
    pub(crate) resources: &'a mut crate::resource_table::ResourceTable,
    /// Whether the debug-gated absorption log records (copied from the owning
    /// `ExecutionContext`). Gates the push in [`note_absorbed`](Self::note_absorbed).
    pub(crate) trace_pending: bool,
    /// The owning context's per-frame absorption log, borrowed so an aggregate
    /// that absorbs a Pending element (`sort`/`join`) can record `(origin, id)`
    /// when `trace_pending` is on. See
    /// [`crate::execution_context::ExecutionContext::absorption_log`].
    pub(crate) absorption_log:
        &'a mut Vec<(Option<crate::program::TermId>, crate::value::PendingId)>,
    /// The call site (`TermId`) of the instruction invoking this native, when
    /// known — stamped onto any resource this call creates for the observability
    /// tooling. `None` when the caller has no origin term to attribute.
    pub(crate) origin: Option<crate::program::TermId>,
    /// The owning context's current frame, stamped onto any resource this call
    /// creates (`ResourceEntry::frame_started`).
    pub(crate) frame: u64,
    /// Whether `print` echoes to real stdout. False for speculative forks so
    /// their output stays captured in the buffer instead of leaking to stdout.
    pub(crate) echo: bool,
    pub(crate) handle_classes: &'a [HandleClass],
    pub(crate) results: Vec<Value>,
    /// When true, the caller (the bytecode VM, under `OptFlags::in_place_mutation`)
    /// has proven this call's container argument is uniquely owned and
    /// non-escaping, so a mutating builtin (`append`, `drop_last`, `set`, …) may
    /// mutate the backing store in place and reuse its id instead of cloning.
    /// Always false with optimizations off (the clone-and-alloc baseline).
    pub(crate) in_place: bool,
}

impl<'a> PetalCxt<'a> {
    /// Whether a mutating builtin may mutate its container argument in place
    /// (and reuse its id) rather than cloning. See [`set_in_place`](Self::set_in_place).
    pub fn in_place(&self) -> bool {
        self.in_place
    }

    // --- Argument access (1-indexed, like Lua) ---

    /// Number of arguments passed to the function.
    pub fn arg_count(&self) -> usize {
        self.args.len()
    }

    /// Get the raw Value at 1-indexed position.
    pub fn get_value(&self, index: usize) -> Result<Value, String> {
        if index == 0 || index > self.args.len() {
            return Err(format!(
                "Argument index {} out of range (1..{})",
                index,
                self.args.len()
            ));
        }
        Ok(self.args[index - 1])
    }

    /// Get an integer argument at 1-indexed position.
    /// Also accepts floats (truncated to int) for ergonomic creative coding.
    pub fn get_int(&self, index: usize) -> Result<i64, String> {
        match self.get_value(index)? {
            Value::Int(n) => Ok(n),
            Value::Float(f) => Ok(f as i64),
            other => Err(format!(
                "Expected int at arg {}, got {}",
                index,
                other.type_name()
            )),
        }
    }

    /// Get a float argument at 1-indexed position.
    /// Also accepts Dual numbers (extracts the primal value).
    pub fn get_float(&self, index: usize) -> Result<f64, String> {
        match self.get_value(index)? {
            Value::Float(f) => Ok(f),
            Value::Int(n) => Ok(n as f64),
            Value::Dual { value, .. } => Ok(value),
            other => Err(format!(
                "Expected float at arg {}, got {}",
                index,
                other.type_name()
            )),
        }
    }

    /// Get a string argument at 1-indexed position.
    pub fn get_string(&self, index: usize) -> Result<String, String> {
        match self.get_value(index)? {
            Value::String(id) => Ok(self.heap.get_string(id).to_string()),
            other => Err(format!(
                "Expected string at arg {}, got {}",
                index,
                other.type_name()
            )),
        }
    }

    /// Look up a registered handle class by id (`None` if unregistered).
    pub fn handle_class(&self, id: HandleClassId) -> Option<&HandleClass> {
        self.handle_classes.get(id.0 as usize)
    }

    /// Get a handle argument at 1-indexed position, checked against the
    /// expected class and the class's liveness predicate.
    pub fn get_handle(&self, index: usize, class: HandleClassId) -> Result<HandleVal, String> {
        let expected = &self.handle_classes[class.0 as usize];
        let h = match self.get_value(index)? {
            Value::Handle(h) => h,
            other => {
                return Err(format!(
                    "Expected {} handle at arg {}, got {}",
                    expected.name,
                    index,
                    other.type_name()
                ));
            }
        };
        if h.class != class {
            let got = self
                .handle_classes
                .get(h.class.0 as usize)
                .map(|c| c.name.as_str())
                .unwrap_or("unknown class");
            return Err(format!(
                "Expected {} handle at arg {}, got {} handle",
                expected.name, index, got
            ));
        }
        if !(expected.is_valid)(h.slot, h.serial) {
            return Err(format!(
                "Stale {} handle at arg {}: {}",
                expected.name,
                index,
                (expected.describe)(h.slot, h.serial)
            ));
        }
        Ok(h)
    }

    /// Get a boolean argument at 1-indexed position.
    pub fn get_bool(&self, index: usize) -> Result<bool, String> {
        match self.get_value(index)? {
            Value::Bool(b) => Ok(b),
            other => Err(format!(
                "Expected bool at arg {}, got {}",
                index,
                other.type_name()
            )),
        }
    }

    // --- Push results ---

    pub fn push_nil(&mut self) {
        self.results.push(Value::Nil);
    }

    pub fn push_int(&mut self, n: i64) {
        self.results.push(Value::Int(n));
    }

    pub fn push_float(&mut self, f: f64) {
        self.results.push(Value::Float(f));
    }

    pub fn push_bool(&mut self, b: bool) {
        self.results.push(Value::Bool(b));
    }

    pub fn push_string(&mut self, s: String) {
        let id = self.heap.alloc_string(s);
        self.results.push(Value::String(id));
    }

    /// Return borrowed text. Prefer this to [`push_string`](Self::push_string)
    /// whenever the value is not already an owned `String`: interning a `&str`
    /// allocates only when the content is new to the heap.
    pub fn push_str(&mut self, s: &str) {
        let id = self.heap.intern_str(s);
        self.results.push(Value::String(id));
    }

    /// Return a byte range of an existing heap string, interning it without
    /// building the substring first. See [`Heap::intern_substring`].
    pub fn push_substring(&mut self, src: crate::heap::StringId, start: usize, end: usize) {
        let id = self.heap.intern_substring(src, start, end);
        self.results.push(Value::String(id));
    }

    pub fn push_list(&mut self, items: Vec<Value>) {
        let id = self.heap.alloc_list(items);
        self.results.push(Value::List(id));
    }

    pub fn push_value(&mut self, v: Value) {
        self.results.push(v);
    }

    // --- Output ---

    pub fn print(&mut self, line: String) {
        self.run_deps.note_effect();
        if self.echo {
            println!("{}", line);
        }
        self.output.push(line);
    }

    // --- Randomness & noise ---

    /// Draw the next uniform `f64` in [0, 1), advancing the owning context's
    /// per-run PRNG state. Backs `random`, `random_int`, and `choose`.
    pub fn rng_next_f64(&mut self) -> f64 {
        crate::builtins::rng_next_f64(self.rng_state)
    }

    /// The owning context's current Perlin-noise seed.
    pub fn noise_seed(&self) -> u64 {
        *self.noise_seed
    }

    /// Set the owning context's Perlin-noise seed (the `noise_seed()` builtin).
    pub fn set_noise_seed(&mut self, seed: u64) {
        self.run_deps.note_effect();
        *self.noise_seed = seed;
    }

    // --- Symbols & buffered output ---

    /// Intern a symbol name, returning its stable id. Idempotent.
    pub fn intern_symbol(&mut self, name: &str) -> SymbolId {
        self.symbols.intern(name)
    }

    /// Get a symbol argument at 1-indexed position.
    pub fn get_symbol(&self, index: usize) -> Result<SymbolId, String> {
        match self.get_value(index)? {
            Value::Symbol(id) => Ok(id),
            other => Err(format!(
                "Expected symbol at arg {}, got {}",
                index,
                other.type_name()
            )),
        }
    }

    /// Push a value into the buffered-output channel bound to `sym`.
    /// The host pulls it later via `Env::take_output_buffer`.
    ///
    /// While the owning context has emit tracing on, this also records the
    /// call's [`origin`](Self::origin) at the same index, so the host can
    /// attribute the emitted value back to the code that produced it
    /// (`Env::take_output_origins`). Off — the default — it is the same single
    /// push it always was.
    pub fn push_output(&mut self, sym: SymbolId, value: Value) {
        self.run_deps.note_emit();
        self.output_buffers.entry(sym).or_default().push(value);
        if self.trace_emit {
            // Pad rather than assume alignment: tracing can be switched on
            // mid-frame, leaving values already in the buffer with no origin.
            // Padding keeps index i of the origins the attribution of index i
            // of the values, which is the whole contract.
            let origins = self.emit_origins.entry(sym).or_default();
            let values_len = self.output_buffers[&sym].len();
            origins.resize_with(values_len.saturating_sub(1), Default::default);
            origins.push(crate::execution_context::EmitSite {
                chain: self.emit_chain.into(),
            });
        }
    }

    /// Convenience: build a `Value::EnumVariant { tag, data }` on the heap and
    /// push it into the buffer bound to `sym`. This is the standard encoding for
    /// host command streams (e.g. draw commands): a string tag plus a flat list
    /// of argument values.
    pub fn emit(&mut self, sym: SymbolId, tag: &str, data: Vec<Value>) {
        let tag = self.heap.alloc_string(tag.to_string());
        let data = self.heap.alloc_list(data);
        self.push_output(sym, Value::EnumVariant { tag, data });
    }

    /// Read the host→script value bound to `sym` (a GLSL-uniform-style input),
    /// or `Nil` if nothing is bound. The read is recorded in the run's
    /// dependency record, so the host can skip the next frame if nothing the
    /// script read has changed (see [`crate::run_deps`]).
    pub fn binding(&mut self, sym: SymbolId) -> Value {
        self.run_deps.note_binding_read(sym);
        self.bindings.get(&sym).copied().unwrap_or(Value::Nil)
    }

    /// Declare that this native answered from host-owned data the binding
    /// table does not cover (a data provider, a query cache, an editor
    /// buffer). The run is then re-run when the host reports that data changed
    /// ([`crate::env::Env::note_host_data_changed`]); a native that reads such
    /// data and does not call this can leave a skipped frame stale.
    pub fn note_host_read(&mut self) {
        self.run_deps.note_host_read();
    }

    /// Declare that this native did something a replay could not reproduce
    /// — wrote host state, advanced a counter, printed. A memoized scope
    /// (see [`crate::memo`]) that calls it is never skipped. The `PetalCxt`
    /// operations with an effect of their own (`print`, the counters, the
    /// mutable resource table, the noise seed) declare it themselves; a
    /// native that reaches host state some other way must call this.
    pub fn note_effect(&mut self) {
        self.run_deps.note_effect();
    }

    /// Read the value bound to the symbol named `name`. Convenience for native
    /// fns that address a well-known uniform by name.
    pub fn binding_named(&mut self, name: &str) -> Value {
        let sym = self.symbols.intern(name);
        self.binding(sym)
    }

    /// Return the current value of the counter for `sym`, then increment it.
    /// Used for per-run id allocation (offscreen canvases, element ids).
    pub fn next_counter(&mut self, sym: SymbolId) -> u64 {
        self.run_deps.note_effect();
        let c = self.counters.entry(sym).or_insert(0);
        let v = *c;
        *c += 1;
        v
    }

    /// Read the counter for `sym` without advancing it (0 if unset). With
    /// [`set_counter`](Self::set_counter) this makes a counter usable as a
    /// per-run scalar cell — petal-ui keeps the active offscreen render
    /// target in one, so `draw_to` can hand back the target it replaced.
    pub fn peek_counter(&self, sym: SymbolId) -> u64 {
        self.counters.get(&sym).copied().unwrap_or(0)
    }

    /// Overwrite the counter for `sym`.
    pub fn set_counter(&mut self, sym: SymbolId, value: u64) {
        self.run_deps.note_effect();
        self.counters.insert(sym, value);
    }

    // --- Heap access ---

    pub fn heap(&self) -> &Heap {
        self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        self.heap
    }

    // --- Pending resources ---

    /// The owning context's resource table (read-only). See
    /// [`crate::resource_table`].
    pub fn resources(&mut self) -> &crate::resource_table::ResourceTable {
        self.run_deps.note_resource_read();
        self.resources
    }

    /// The owning context's resource table (mutable) — for creating/resolving
    /// pending resource entries.
    pub fn resources_mut(&mut self) -> &mut crate::resource_table::ResourceTable {
        self.run_deps.note_effect();
        self.resources
    }

    /// Record that this native absorbed the resource `id` (an aggregate like
    /// `sort`/`join` swallowing a Pending element): bump its always-on
    /// `absorbed_count` and, when the debug-gated log is on, push `(origin, id)`
    /// to the per-frame absorption log. The counterpart to
    /// [`Vm::note_absorption`](crate::backend::bytecode::vm) on the native path.
    pub fn note_absorbed(&mut self, id: crate::value::PendingId) {
        self.resources.note_absorbed(id);
        if self.trace_pending {
            self.absorption_log.push((self.origin, id));
        }
    }

    /// The call site of the instruction invoking this native, when known — the
    /// origin to stamp onto a resource this call creates. See [`origin`](Self::origin).
    pub fn origin(&self) -> Option<crate::program::TermId> {
        self.origin
    }

    /// The owning context's current frame — the `frame_started` to stamp onto a
    /// resource this call creates.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Consume the state and return the single value a call yields to Petal:
    /// the first result the native pushed, or `Nil` if it pushed none. `count`
    /// is what the native itself reported — a native that returns `Ok(0)` has
    /// no result even if it left something on the stack.
    pub fn take_result(self, count: u32) -> Value {
        if count == 0 {
            return Value::Nil;
        }
        self.results.first().copied().unwrap_or(Value::Nil)
    }
}

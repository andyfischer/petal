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

/// Entry in the native function table.
struct NativeFnEntry {
    name: String,
    func: NativeFn,
    class: NativeClass,
}

/// Registry of native functions, mapping IDs to names and function pointers.
pub struct NativeFnTable {
    entries: Vec<NativeFnEntry>,
    /// IDs for higher-order builtins that need evaluator intrinsic dispatch.
    pub intrinsic_map: Option<NativeFnId>,
    pub intrinsic_filter: Option<NativeFnId>,
    pub intrinsic_reduce: Option<NativeFnId>,
    pub intrinsic_for_each: Option<NativeFnId>,
}

impl NativeFnTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            intrinsic_map: None,
            intrinsic_filter: None,
            intrinsic_reduce: None,
            intrinsic_for_each: None,
        }
    }

    /// Register a native function, returning its ID.
    pub fn register(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        let id = NativeFnId(self.entries.len() as u32);
        self.entries.push(NativeFnEntry {
            name: name.to_string(),
            func,
            class: NativeClass::Strict,
        });
        id
    }

    /// Override the Pending-handling class of an already-registered native.
    /// Registration stays append-only (indices are stable); classification is
    /// applied afterward by id.
    pub fn set_class(&mut self, id: NativeFnId, class: NativeClass) {
        self.entries[id.0 as usize].class = class;
    }

    /// The Pending-handling class of a native (defaults to `Strict`).
    pub fn get_class(&self, id: NativeFnId) -> NativeClass {
        self.entries[id.0 as usize].class
    }

    /// Look up a native function by name.
    pub fn lookup_name(&self, name: &str) -> Option<NativeFnId> {
        self.entries
            .iter()
            .position(|e| e.name == name)
            .map(|i| NativeFnId(i as u32))
    }

    /// Get the name of a native function by ID.
    pub fn get_name(&self, id: NativeFnId) -> &str {
        &self.entries[id.0 as usize].name
    }

    /// Get the function pointer for a native function.
    pub fn get_func(&self, id: NativeFnId) -> NativeFn {
        self.entries[id.0 as usize].func
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
    args: &'a [Value],
    heap: &'a mut Heap,
    output: &'a mut Vec<String>,
    symbols: &'a mut SymbolTable,
    output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
    bindings: &'a mut HashMap<SymbolId, Value>,
    counters: &'a mut HashMap<SymbolId, u64>,
    /// Per-run xorshift64* PRNG state, borrowed from the owning
    /// `ExecutionContext` so the RNG builtins advance that context's stream.
    rng_state: &'a mut u64,
    /// Per-run Perlin-noise seed, borrowed from the owning `ExecutionContext`.
    noise_seed: &'a mut u64,
    /// The owning context's resource table, borrowed so the pending-resource
    /// builtins (`__pending`/`__resolve`/`__reject`) can create/resolve entries.
    resources: &'a mut crate::resource_table::ResourceTable,
    /// Whether `print` echoes to real stdout. False for speculative forks so
    /// their output stays captured in the buffer instead of leaking to stdout.
    echo: bool,
    handle_classes: &'a [HandleClass],
    results: Vec<Value>,
    /// When true, the caller (the bytecode VM, under `OptFlags::in_place_mutation`)
    /// has proven this call's container argument is uniquely owned and
    /// non-escaping, so a mutating builtin (`append`, `drop_last`, `set`, …) may
    /// mutate the backing store in place and reuse its id instead of cloning.
    /// Always false with optimizations off (the clone-and-alloc baseline).
    in_place: bool,
}

impl<'a> PetalCxt<'a> {
    pub fn new(
        args: &'a [Value],
        heap: &'a mut Heap,
        output: &'a mut Vec<String>,
        symbols: &'a mut SymbolTable,
        output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
        bindings: &'a mut HashMap<SymbolId, Value>,
        counters: &'a mut HashMap<SymbolId, u64>,
        rng_state: &'a mut u64,
        noise_seed: &'a mut u64,
        resources: &'a mut crate::resource_table::ResourceTable,
        echo: bool,
        handle_classes: &'a [HandleClass],
    ) -> Self {
        Self {
            args,
            heap,
            output,
            symbols,
            output_buffers,
            bindings,
            counters,
            rng_state,
            noise_seed,
            resources,
            echo,
            handle_classes,
            results: Vec::new(),
            in_place: false,
        }
    }

    /// Mark this call as in-place-eligible (see [`in_place`](Self::in_place)).
    /// The VM sets this before invoking a mutating builtin when its escape
    /// analysis proved the container argument unique + non-escaping.
    pub fn set_in_place(&mut self, in_place: bool) {
        self.in_place = in_place;
    }

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
            other => Err(format!("Expected int at arg {}, got {}", index, other.type_name())),
        }
    }

    /// Get a float argument at 1-indexed position.
    /// Also accepts Dual numbers (extracts the primal value).
    pub fn get_float(&self, index: usize) -> Result<f64, String> {
        match self.get_value(index)? {
            Value::Float(f) => Ok(f),
            Value::Int(n) => Ok(n as f64),
            Value::Dual { value, .. } => Ok(value),
            other => Err(format!("Expected float at arg {}, got {}", index, other.type_name())),
        }
    }

    /// Get a string argument at 1-indexed position.
    pub fn get_string(&self, index: usize) -> Result<String, String> {
        match self.get_value(index)? {
            Value::String(id) => Ok(self.heap.get_string(id).to_string()),
            other => Err(format!("Expected string at arg {}, got {}", index, other.type_name())),
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
                ))
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
            other => Err(format!("Expected bool at arg {}, got {}", index, other.type_name())),
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

    pub fn push_list(&mut self, items: Vec<Value>) {
        let id = self.heap.alloc_list(items);
        self.results.push(Value::List(id));
    }

    pub fn push_value(&mut self, v: Value) {
        self.results.push(v);
    }

    // --- Output ---

    pub fn print(&mut self, line: String) {
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
    pub fn push_output(&mut self, sym: SymbolId, value: Value) {
        self.output_buffers.entry(sym).or_default().push(value);
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
    /// or `Nil` if nothing is bound.
    pub fn binding(&self, sym: SymbolId) -> Value {
        self.bindings.get(&sym).copied().unwrap_or(Value::Nil)
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
        let c = self.counters.entry(sym).or_insert(0);
        let v = *c;
        *c += 1;
        v
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
    pub fn resources(&self) -> &crate::resource_table::ResourceTable {
        self.resources
    }

    /// The owning context's resource table (mutable) — for creating/resolving
    /// pending resource entries.
    pub fn resources_mut(&mut self) -> &mut crate::resource_table::ResourceTable {
        self.resources
    }

    /// Consume the state and return the results vector.
    pub fn take_results(self) -> Vec<Value> {
        self.results
    }
}

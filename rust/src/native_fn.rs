//! Native function FFI — Lua-inspired plugin system.
//!
//! Allows Rust functions to be registered and called from Petal code
//! via a stack-based API.

use serde::Serialize;

use crate::heap::Heap;
use crate::value::Value;

/// Identifier for a registered native function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct NativeFnId(pub u32);

/// Result type for native functions: Ok(count) = number of results pushed.
pub type NativeResult = Result<u32, String>;

/// Signature for native functions.
pub type NativeFn = fn(&mut PetalState) -> NativeResult;

/// Entry in the native function table.
struct NativeFnEntry {
    name: String,
    func: NativeFn,
}

/// Registry of native functions, mapping IDs to names and function pointers.
pub struct NativeFnTable {
    entries: Vec<NativeFnEntry>,
}

impl NativeFnTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Register a native function, returning its ID.
    pub fn register(&mut self, name: &str, func: NativeFn) -> NativeFnId {
        let id = NativeFnId(self.entries.len() as u32);
        self.entries.push(NativeFnEntry {
            name: name.to_string(),
            func,
        });
        id
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
pub struct PetalState<'a> {
    args: &'a [Value],
    heap: &'a mut Heap,
    output: &'a mut Vec<String>,
    results: Vec<Value>,
}

impl<'a> PetalState<'a> {
    pub fn new(args: &'a [Value], heap: &'a mut Heap, output: &'a mut Vec<String>) -> Self {
        Self {
            args,
            heap,
            output,
            results: Vec::new(),
        }
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
    pub fn get_int(&self, index: usize) -> Result<i64, String> {
        match self.get_value(index)? {
            Value::Int(n) => Ok(n),
            other => Err(format!("Expected int at arg {}, got {}", index, other.type_name())),
        }
    }

    /// Get a float argument at 1-indexed position.
    pub fn get_float(&self, index: usize) -> Result<f64, String> {
        match self.get_value(index)? {
            Value::Float(f) => Ok(f),
            Value::Int(n) => Ok(n as f64),
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
        println!("{}", line);
        self.output.push(line);
    }

    // --- Heap access ---

    pub fn heap(&self) -> &Heap {
        self.heap
    }

    pub fn heap_mut(&mut self) -> &mut Heap {
        self.heap
    }

    /// Consume the state and return the results vector.
    pub fn take_results(self) -> Vec<Value> {
        self.results
    }
}

//! ExecutionContext - one isolated execution's mutable runtime bundle.
//!
//! Bundles the heap together with the runtime registries that reference it
//! (closures, overload sets, buffered output, host bindings, counters). An
//! `Env` holds a map of these keyed by [`ContextKey`]; each `Stack` links to
//! its context by key. With a single default context, behavior is identical to
//! the pre-extraction `Env`.
//!
//! See docs/dev/speculative-execution-plan.md §3.

use std::collections::HashMap;

use crate::eval::RuntimeClosure;
use crate::heap::Heap;
use crate::program::OverloadEntry;
use crate::symbol::SymbolId;
use crate::value::Value;

/// Key identifying one ExecutionContext within an Env.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextKey(pub u32);

/// One isolated execution's mutable bundle: the heap + the runtime registries
/// that reference it. Does NOT own the Stack. (Forking added in a later chunk.)
pub struct ExecutionContext {
    pub heap: Heap,
    pub closures: Vec<RuntimeClosure>,
    pub overload_sets: Vec<Vec<OverloadEntry>>,
    pub output: Vec<String>,
    pub output_buffers: HashMap<SymbolId, Vec<Value>>,
    pub bindings: HashMap<SymbolId, Value>,
    pub counters: HashMap<SymbolId, u64>,
}

impl ExecutionContext {
    pub fn new() -> Self {
        Self {
            heap: Heap::new(),
            closures: Vec::new(),
            overload_sets: Vec::new(),
            output: Vec::new(),
            output_buffers: HashMap::new(),
            bindings: HashMap::new(),
            counters: HashMap::new(),
        }
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

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
/// that reference it. Does NOT own the Stack. [`fork`](Self::fork) yields an
/// isolated copy sharing no mutable state with the source.
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

    /// Fork this context into an isolated copy. Heap + registries are deep-cloned
    /// (pre-fork ids resolve to equal objects in both); output sinks start fresh
    /// so the fork's output is captured separately from the source's.
    pub fn fork(&self) -> ExecutionContext {
        ExecutionContext {
            heap: self.heap.fork(),
            closures: self.closures.clone(),
            overload_sets: self.overload_sets.clone(),
            bindings: self.bindings.clone(),
            counters: self.counters.clone(),
            output: Vec::new(),
            output_buffers: HashMap::new(),
        }
    }

    // ── Data operations ──────────────────────────────────────────
    //
    // The host-facing operations on this context's owned registries. `Env`
    // routes its default-context and per-stack (`*_for`) accessors here so both
    // share one implementation.

    /// Drain and return the print output, leaving it empty.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }

    /// Drain and return the buffer bound to `sym`, leaving it empty.
    pub fn take_output_buffer(&mut self, sym: SymbolId) -> Vec<Value> {
        self.output_buffers.get_mut(&sym).map(std::mem::take).unwrap_or_default()
    }

    /// Peek at the buffer bound to `sym` without draining it.
    pub fn output_buffer(&self, sym: SymbolId) -> &[Value] {
        self.output_buffers.get(&sym).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Clear the buffer bound to `sym` (e.g. at the top of a frame).
    pub fn clear_output_buffer(&mut self, sym: SymbolId) {
        if let Some(buf) = self.output_buffers.get_mut(&sym) {
            buf.clear();
        }
    }

    /// Bind `value` to `sym`.
    pub fn set_binding(&mut self, sym: SymbolId, value: Value) {
        self.bindings.insert(sym, value);
    }

    /// Read the value bound to `sym`, if any.
    pub fn binding(&self, sym: SymbolId) -> Option<Value> {
        self.bindings.get(&sym).copied()
    }

    /// Remove the binding for `sym`.
    pub fn clear_binding(&mut self, sym: SymbolId) {
        self.bindings.remove(&sym);
    }

    /// Reset the counter for `sym` to `start`.
    pub fn reset_counter(&mut self, sym: SymbolId, start: u64) {
        self.counters.insert(sym, start);
    }

    /// Return the current counter value for `sym`, then increment it.
    /// An unset counter starts at 0.
    pub fn next_counter(&mut self, sym: SymbolId) -> u64 {
        let c = self.counters.entry(sym).or_insert(0);
        let v = *c;
        *c += 1;
        v
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

//! Host-side I/O: draining print output, interning symbols, buffered output,
//! host→script bindings, and per-run counters.
//!
//! These are thin delegators over the owned registries of an
//! [`ExecutionContext`] — the default context for the plain accessors, and a
//! specific stack's context for the `*_for` variants.
//!
//! Split out of `env/mod.rs`; see that module for the `Env` struct and core
//! accessors.

use super::*;

use crate::symbol::SymbolId;

impl Env {
    /// Get the output buffer contents and clear it.
    pub fn take_output(&mut self) -> Vec<String> {
        let ck = self.default_context;
        self.ctx_mut(ck).take_output()
    }

    /// Drain the print output of a specific stack's context. A fork accumulates
    /// its `print` output in its own (fresh) sink; this is how a host reads it
    /// before [`drop_fork`](Self::drop_fork). Empty `Vec` for an unknown stack.
    pub fn take_output_for(&mut self, stack_id: StackKey) -> Vec<String> {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx_mut(ck).take_output())
            .unwrap_or_default()
    }

    // ── Symbols & buffered output (host side) ────────────────────

    /// Intern a symbol name, returning its stable id. Idempotent — the host and
    /// the script share an id by interning the same name. Use the returned id to
    /// address an output buffer with `take_output_buffer`.
    pub fn intern_symbol(&mut self, name: &str) -> SymbolId {
        self.symbols.intern(name)
    }

    /// Resolve a symbol id back to its name.
    pub fn symbol_name(&self, sym: SymbolId) -> Option<&str> {
        self.symbols.name(sym)
    }

    /// Drain and return everything pushed into the buffer bound to `sym` since
    /// the last drain. The buffer is left empty.
    pub fn take_output_buffer(&mut self, sym: SymbolId) -> Vec<Value> {
        let ck = self.default_context;
        self.ctx_mut(ck).take_output_buffer(sym)
    }

    /// Peek at the buffer bound to `sym` without draining it.
    pub fn output_buffer(&self, sym: SymbolId) -> &[Value] {
        self.ctx(self.default_context).output_buffer(sym)
    }

    /// [`take_output_buffer`](Self::take_output_buffer) for a specific stack's
    /// context. The drained `Value`s reference *that* context's heap — decode
    /// them with [`heap_for`](Self::heap_for), not [`heap`](Self::heap). This is
    /// how a host drains a fork's draw-command (or other) buffer.
    pub fn take_output_buffer_for(&mut self, stack_id: StackKey, sym: SymbolId) -> Vec<Value> {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx_mut(ck).take_output_buffer(sym))
            .unwrap_or_default()
    }

    /// Peek at a specific stack's context buffer without draining it.
    pub fn output_buffer_for(&self, stack_id: StackKey, sym: SymbolId) -> &[Value] {
        self.ctx_for(stack_id)
            .map(|ck| self.ctx(ck).output_buffer(sym))
            .unwrap_or(&[])
    }

    /// Clear the buffer bound to `sym` (e.g. at the top of a frame).
    pub fn clear_output_buffer(&mut self, sym: SymbolId) {
        let ck = self.default_context;
        self.ctx_mut(ck).clear_output_buffer(sym);
    }

    // ── Bindings (host→script uniforms) ──────────────────────────

    /// Bind a `Value` to `sym`, readable by native fns/scripts (`binding`).
    /// Any heap `Value` passed must already live on this Env's heap.
    pub fn set_binding(&mut self, sym: SymbolId, value: Value) {
        let ck = self.default_context;
        self.ctx_mut(ck).set_binding(sym, value);
    }

    /// [`set_binding`](Self::set_binding) for a specific stack's context, e.g.
    /// to feed a fork different host inputs than its source. Any heap `Value`
    /// must already live on that stack's context heap
    /// ([`heap_for_mut`](Self::heap_for_mut)). No-op for an unknown stack.
    pub fn set_binding_for(&mut self, stack_id: StackKey, sym: SymbolId, value: Value) {
        if let Some(ck) = self.ctx_for(stack_id) {
            self.ctx_mut(ck).set_binding(sym, value);
        }
    }

    /// Read the value bound to `sym`, if any.
    pub fn binding(&self, sym: SymbolId) -> Option<Value> {
        self.ctx(self.default_context).binding(sym)
    }

    /// [`binding`](Self::binding) read from a specific stack's context.
    pub fn binding_for(&self, stack_id: StackKey, sym: SymbolId) -> Option<Value> {
        self.ctx_for(stack_id)
            .and_then(|ck| self.ctx(ck).binding(sym))
    }

    /// Remove the binding for `sym`.
    pub fn clear_binding(&mut self, sym: SymbolId) {
        let ck = self.default_context;
        self.ctx_mut(ck).clear_binding(sym);
    }

    // ── Counters (per-run sequence allocation) ───────────────────

    /// Reset the counter for `sym` to `start` (call at frame start so
    /// `next_counter` hands out stable ids across the per-frame re-run model).
    pub fn reset_counter(&mut self, sym: SymbolId, start: u64) {
        let ck = self.default_context;
        self.ctx_mut(ck).reset_counter(sym, start);
    }

    /// Return the current counter value for `sym`, then increment it.
    /// An unset counter starts at 0.
    pub fn next_counter(&mut self, sym: SymbolId) -> u64 {
        let ck = self.default_context;
        self.ctx_mut(ck).next_counter(sym)
    }
}

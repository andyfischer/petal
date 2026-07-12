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

use crate::backend::RuntimeClosure;
use crate::heap::Heap;
use crate::program::{OverloadEntry, TermId};
use crate::resource_table::ResourceTable;
use crate::stats::{AllocStats, DupStats};
use crate::symbol::SymbolId;
use crate::value::{PendingId, Value};

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
    /// When true, `print` echoes to real stdout (the sole stdout path for
    /// `petal run`, which never drains `output`). A speculative [`fork`](Self::fork)
    /// clears this so its output stays captured in the buffer and never leaks to
    /// the primary run's stdout.
    pub echo: bool,
    /// Per-context xorshift64* PRNG state (see [`crate::builtins`]). Owned here so
    /// each run/fork has isolated randomness instead of sharing a process global.
    pub rng_state: u64,
    /// Per-context Perlin-noise seed, set via the `noise_seed()` builtin.
    pub noise_seed: u64,
    /// Table of pending/unresolved resources (the home for `Value::Pending`).
    /// Lives here so it survives `reset_stack` (the cross-frame home for
    /// between-frame resolution) and forks consistently with the heap.
    pub resources: ResourceTable,
    /// Monotonic frame counter, advanced once per host frame via
    /// [`advance_frame`](Self::advance_frame). Stamped onto every resource at
    /// creation (`ResourceEntry::frame_started`) so age-in-frames is computable.
    /// The core lib has no frame loop, so this stays 0 under the CLI and tests
    /// unless a host advances it.
    frame: u64,
    /// Whether the debug-gated absorption log ([`absorption_log`](Self::absorption_log))
    /// records. Off by default — a host, `--trace-pending`, or the debug protocol
    /// flips it on via [`enable_pending_trace`](Self::enable_pending_trace). When
    /// off, absorptions pay only the always-on `absorbed_count`, never a push.
    pub trace_pending: bool,
    /// Debug-gated, per-frame absorption log: `(origin call site, absorbed
    /// resource)` for every absorption in the current frame while
    /// [`trace_pending`](Self::trace_pending) is on. This is the data a dataflow
    /// viz paints — the set of spans a given resource flowed through is its
    /// downstream cone. Off by default (an unbounded per-absorption push is real
    /// memory pressure in a hot frame). Per-frame: cleared by
    /// [`reset_frame_absorption`](Self::reset_frame_absorption) at the stack
    /// reset, unlike the cross-frame [`resources`](Self::resources) table.
    pub absorption_log: Vec<(Option<TermId>, PendingId)>,
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
            echo: true,
            rng_state: crate::builtins::initial_seed(),
            noise_seed: 0,
            resources: ResourceTable::new(),
            frame: 0,
            trace_pending: false,
            absorption_log: Vec::new(),
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
            // A speculative fork must not print to real stdout.
            echo: false,
            // Copy the parent's RNG/noise state so the fork starts from the same
            // point, then advances independently — the parent's stream is
            // unaffected by anything the fork draws.
            rng_state: self.rng_state,
            noise_seed: self.noise_seed,
            // Snapshot resource state so a fork observes the same resolution
            // status as its source at fork time, then diverges independently —
            // exactly how the heap is forked above.
            resources: self.resources.clone(),
            // A fork observes the same frame as its source at fork time.
            frame: self.frame,
            // A fork inherits the trace setting but starts with an empty log —
            // its absorptions are its own, captured separately from the source's.
            trace_pending: self.trace_pending,
            absorption_log: Vec::new(),
        }
    }

    /// The current frame number (see [`advance_frame`](Self::advance_frame)).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Advance to the next frame. A host calls this once per rendered frame so
    /// resource ages (`current_frame - frame_started`) grow over time.
    pub fn advance_frame(&mut self) {
        self.frame += 1;
    }

    /// Turn on the debug-gated absorption log (see
    /// [`absorption_log`](Self::absorption_log)). Off by default; a host, the
    /// `--trace-pending` flag, or the debug protocol flips it on.
    pub fn enable_pending_trace(&mut self) {
        self.trace_pending = true;
    }

    /// Clear the per-frame absorption state at the start of a frame: empty the
    /// debug [`absorption_log`](Self::absorption_log) and zero every resource's
    /// `absorbed_count`, so both describe just the frame about to run. The
    /// [`resources`](Self::resources) entries themselves are cross-frame and
    /// kept (a resource keeps loading across frames). Called from
    /// [`Env::reset_stack`](crate::env::Env::reset_stack), the per-frame stack
    /// reset. The enable flag is not touched — it persists across frames.
    pub fn reset_frame_absorption(&mut self) {
        self.absorption_log.clear();
        self.resources.reset_absorbed_counts();
    }

    /// This context's value-duplication statistics, accumulated by its heap's
    /// copy-on-write operations plus the fork (if any) that created it. See
    /// [`crate::stats`].
    pub fn dup_stats(&self) -> &DupStats {
        self.heap.dup_stats()
    }

    /// This context's heap-allocation statistics (objects created per kind).
    /// See [`crate::stats`].
    pub fn alloc_stats(&self) -> &AllocStats {
        self.heap.alloc_stats()
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
        self.output_buffers
            .get_mut(&sym)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Peek at the buffer bound to `sym` without draining it.
    pub fn output_buffer(&self, sym: SymbolId) -> &[Value] {
        self.output_buffers
            .get(&sym)
            .map(Vec::as_slice)
            .unwrap_or(&[])
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A speculative fork must isolate the noise seed: it starts equal to the
    /// parent's at fork time, but mutating the child never touches the parent.
    #[test]
    fn fork_isolates_noise_seed() {
        let mut parent = ExecutionContext::new();
        parent.noise_seed = 42;

        let mut child = parent.fork();
        // The fork copies the seed as of fork time.
        assert_eq!(child.noise_seed, 42);

        // The child then advances independently — the parent is unaffected.
        child.noise_seed = 99;
        assert_eq!(parent.noise_seed, 42);
        assert_eq!(child.noise_seed, 99);
    }

    /// A fork's RNG stream starts from the same state as the parent's at fork
    /// time (then each advances independently). Deterministic to assert on
    /// because it only checks the copied seed, not any time-seeded draw.
    #[test]
    fn fork_copies_rng_state() {
        let mut parent = ExecutionContext::new();
        parent.rng_state = 0xDEAD_BEEF;

        let child = parent.fork();
        assert_eq!(child.rng_state, 0xDEAD_BEEF);
    }

    /// `new()` matches the pre-refactor process-global defaults: echo on,
    /// noise seed zero.
    #[test]
    fn new_defaults_preserve_primary_run_behavior() {
        let ctx = ExecutionContext::new();
        assert!(ctx.echo);
        assert_eq!(ctx.noise_seed, 0);
        // A fork never echoes to real stdout.
        assert!(!ctx.fork().echo);
    }
}

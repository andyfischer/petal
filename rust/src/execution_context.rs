//! ExecutionContext - one isolated execution's mutable runtime bundle.
//!
//! Bundles the heap together with the runtime registries that reference it
//! (closures, overload sets, buffered output, host bindings, counters). An
//! `Env` holds a map of these keyed by [`ContextKey`]; each `Stack` links to
//! its context by key. With a single default context, behavior is identical to
//! the pre-extraction `Env`.

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
/// that reference it. Does NOT own the Stack.
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
    /// Whether [`emit_origins`](Self::emit_origins) records. Off by default — a
    /// host flips it on with [`enable_emit_trace`](Self::enable_emit_trace) when
    /// it wants to attribute buffered output back to source. When off an emit
    /// pays one bool check and never touches the map.
    pub trace_emit: bool,
    /// Call-site attribution for buffered output, parallel to
    /// [`output_buffers`](Self::output_buffers): `emit_origins[sym][i]` is the
    /// term that pushed `output_buffers[sym][i]`, when the caller had an origin
    /// to attribute. Recorded only while [`trace_emit`](Self::trace_emit) is on,
    /// and drained/cleared in lockstep with the values so the two stay aligned.
    ///
    /// This is what lets a drawn shape point back at the code that drew it: the
    /// recorded terms resolve through the program's
    /// [`SourceMap`](crate::source_map) to spans, and through their `inputs` to
    /// each argument's own span (see [`crate::provenance`]). Keeping that
    /// resolution lazy — done off the recorded ids, not at emit time — is why
    /// the on-cost here is one short id list per emit.
    pub emit_origins: HashMap<SymbolId, Vec<EmitSite>>,
}

/// EmitSite - Where one buffered value was emitted from: the native's own call site,
/// followed by the return address of each enclosing call, innermost first.
///
/// A chain rather than a single site because the call a *user* means is rarely
/// the innermost one. `draw_circle` in the `petal-ui` prelude is a Petal
/// function wrapping the native, so the leaf call site is a line in the prelude;
/// the line the user wrote is one frame further out. With the whole chain
/// recorded, a host picks the frame it cares about
/// ([`crate::provenance::pick_frame`]) instead of being handed the wrong one.
///
/// Four inline slots covers a draw call through a wrapper or two without
/// allocating, which is the shape essentially every frame has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmitSite {
    pub chain: smallvec::SmallVec<[TermId; 4]>,
}

impl EmitSite {
    /// The innermost call site — the native call itself.
    pub fn leaf(&self) -> Option<TermId> {
        self.chain.first().copied()
    }
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
            trace_emit: false,
            emit_origins: HashMap::new(),
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
            rng_state: self.rng_state,
            noise_seed: self.noise_seed,
            // Snapshot resource state so a fork observes the same resolution
            // status as its source at fork time, then diverges independently —
            // exactly how the heap is forked above.
            resources: self.resources.clone(),
            frame: self.frame,
            // A fork inherits the trace setting but starts with an empty log —
            // its absorptions are its own, captured separately from the source's.
            trace_pending: self.trace_pending,
            absorption_log: Vec::new(),
            // Likewise for emit attribution: the setting is inherited, but the
            // recorded origins belong to whichever context did the emitting —
            // and the fork's output buffers start empty, so its origins must too
            // or the two would be misaligned from the first push.
            trace_emit: self.trace_emit,
            emit_origins: HashMap::new(),
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

    /// Turn call-site attribution of buffered output on or off (see
    /// [`emit_origins`](Self::emit_origins)). Turning it *off* drops what was
    /// recorded, so a later drain can't hand back origins that no longer line up
    /// with the values still in the buffers.
    pub fn enable_emit_trace(&mut self, on: bool) {
        self.trace_emit = on;
        if !on {
            self.emit_origins.clear();
        }
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

    /// Drain the call-site origins recorded for `sym`'s buffer (see
    /// [`emit_origins`](Self::emit_origins)). Empty when tracing is off.
    ///
    /// Element `i` attributes element `i` of the matching
    /// [`take_output_buffer`](Self::take_output_buffer), so a caller that wants
    /// both must drain both — draining only the values would leave stale origins
    /// to be misattributed to the next frame's emits.
    pub fn take_output_origins(&mut self, sym: SymbolId) -> Vec<EmitSite> {
        self.emit_origins
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

    /// Clear the buffer bound to `sym` (e.g. at the top of a frame), along with
    /// any origins recorded for it — the two are index-aligned, so clearing one
    /// without the other would misattribute every value that follows.
    pub fn clear_output_buffer(&mut self, sym: SymbolId) {
        if let Some(buf) = self.output_buffers.get_mut(&sym) {
            buf.clear();
        }
        if let Some(origins) = self.emit_origins.get_mut(&sym) {
            origins.clear();
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

//! Execution for Petal.
//!
//! Petal compiles source to a canonical term-graph IR (`crate::program`), which
//! the [`bytecode`] backend runs: a linear register VM over a *lowering* of the
//! term graph (see `bytecode::lower`), with an in-place mutation optimization
//! gated by escape analysis. Static analyses (slicing, `explain`'s provenance
//! walk, autodiff-as-graph) still reason about the term graph directly; the VM
//! populates the trace buffer those analyses read at runtime.
//!
//! [`OptFlags`] chooses which optimizer passes a program is lowered with; the
//! run's full configuration is [`crate::policy::RunPolicy`].

pub mod bytecode;
pub mod calls;
pub mod errors;
pub mod ops;
pub mod pattern;

use crate::program::FunctionId;
use crate::value::Value;

/// Result of a single execution step. The contract between `Env`'s run loops
/// and the bytecode `Vm`.
#[derive(Debug)]
pub enum StepResult {
    Continue,
    Complete(Value),
    Error(String),
}

/// Runtime closure — a function reference plus its captured values. Stored in
/// the execution context and referenced by `Value::Closure` ids.
#[derive(Clone)]
pub struct RuntimeClosure {
    pub function_id: FunctionId,
    pub captures: Vec<Value>,
}

/// The optimizer passes bytecode is lowered with. Every pass is individually
/// switchable so it can be disabled to isolate a bug: "bytecode with all opts
/// off" is a differential-testing oracle.
///
/// This is the lowering half of a run's configuration, and the bytecode
/// cache's key. The rest — memoization and frame gating, which change what a
/// run does but not what it was compiled to — lives beside it in
/// [`crate::policy::RunPolicy`], which is what hosts and tools set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptFlags {
    /// Lower a provably-unique, non-escaping collection mutation
    /// (`SetIndex`/`SetField`/append/…) to an in-place heap write instead of
    /// clone-and-alloc. Requires the escape analysis in `bytecode::escape`
    /// (M4 route B — loop-carried accumulators).
    pub in_place_mutation: bool,
    /// Rewrite straight-line mutations of freshly-allocated, dead-after
    /// containers to in-place form via last-use liveness on the lowered
    /// bytecode (M4 route A; `bytecode::lastuse`). Independent of
    /// `in_place_mutation` so either route can be disabled to isolate a bug.
    pub in_place_straight_line: bool,
    /// Let a write through a `var` cell (`set xs[i] = v`, `set r.f = v`,
    /// `set xs = append(xs, v)`) mutate the cell's container instead of
    /// copying it, whenever the cell is the container's only holder — which
    /// the heap tracks per cell at run time (`bytecode::cells`). This is the
    /// in-place route that works across function boundaries: the other two
    /// prove uniqueness from an allocation in the same function, and a cell's
    /// contents were allocated somewhere else.
    pub in_place_cells: bool,
    /// Propagate register copies and drop the `Move`s left dead by it
    /// (`bytecode::copyprop`). Lowering emits a move per variable use and per
    /// phi edge, which is about half of a typical program's instructions.
    pub copy_propagation: bool,
    /// Enter a run of `match` arms keyed on string literals, int literals or
    /// enum variants through a jump table (`Inst::MatchSwitch`) instead of
    /// testing them one after another. With it off the arms are the plain
    /// chain of tests, which is the oracle the table is held to.
    pub match_dispatch: bool,
    /// Keep the top-level values that depend on no dynamic data across runs
    /// (`crate::static_cache`): the root code gets a `StaticSkip` in front of
    /// each run of static instructions and a `StaticStore` behind it, and a
    /// run whose values the stack already holds is jumped over. No
    /// instruction is moved or removed, so with it off the code is the same
    /// code without those two instructions, which is the oracle the cache is
    /// held to.
    ///
    /// It needs the classifier's answer for the program, which only an `Env`
    /// has (the natives decide what is constant), so
    /// [`bytecode::lower_with_flags`] ignores it and
    /// [`bytecode::lower_with_statics`] honours it. An `Env` also drops it
    /// whenever the per-term trace is preserved, and under the static
    /// audit: a skipped run records nothing for them to read. With
    /// `preserve_observations` it stays on, and a skipped segment reports
    /// what it observed when it last ran (`crate::static_cache`,
    /// "Observations").
    pub static_cache: bool,
    /// With `static_cache`: also keep the values a function body builds
    /// that depend on nothing of the call (`crate::static_cache`, the
    /// `in_fn` module). Function code gets the same two instructions around
    /// the code that builds one. Off, function code is lowered as it is
    /// without the cache. Ignored where `preserve_observations` is set.
    pub static_in_functions: bool,
    /// Hold `copy_propagation` back from deleting a dead `Move` whose term is
    /// **named**, so the observation buffer still reports that binding. Set by
    /// `Env` when the observation buffer or the `explain` trace is enabled;
    /// it is part of the flags, so switching observation on re-lowers the
    /// program rather than reusing code compiled without the guard.
    pub preserve_observations: bool,
    /// Hold `copy_propagation` back from deleting **any** instruction that
    /// carries an origin term, so the execution trace still records a value for
    /// every term the program computes. Set by `Env` when the trace buffer is
    /// enabled.
    ///
    /// Broader than `preserve_observations`, and deliberately so: the
    /// observation buffer only reports *named* bindings, while the trace is read
    /// per-term by provenance, `explain` and direct manipulation — solving
    /// `x0 + i * spacing` for `spacing` needs the value the anonymous `i` read
    /// took, which is exactly the kind of dead `Move` copy propagation deletes.
    pub preserve_trace: bool,
}

impl OptFlags {
    /// All optimizations disabled — the correctness baseline.
    pub const fn none() -> OptFlags {
        OptFlags {
            in_place_mutation: false,
            in_place_straight_line: false,
            in_place_cells: false,
            copy_propagation: false,
            match_dispatch: false,
            static_cache: false,
            // Inert without `static_cache`; on, so that switching the cache
            // on over these flags (`baseline+static`) is the whole cache.
            static_in_functions: true,
            preserve_observations: false,
            preserve_trace: false,
        }
    }

    /// All optimizations enabled.
    pub const fn all() -> OptFlags {
        OptFlags {
            in_place_mutation: true,
            in_place_straight_line: true,
            in_place_cells: true,
            copy_propagation: true,
            match_dispatch: true,
            static_cache: true,
            static_in_functions: true,
            preserve_observations: false,
            preserve_trace: false,
        }
    }

    /// The passes on by default. In-place mutation is **on** for both M4
    /// routes: route B (loop accumulators, default-on since the M4 flip) and
    /// route A (straight-line last-use, default-on after earning the same
    /// 300k-seed four-oracle fuzz soak). Both are at full differential parity
    /// with clone-and-alloc, so sketches get the zero-copy wins without opting
    /// in; the `baseline` run policy recovers the clone-and-alloc oracle. This
    /// is spelled out field-by-field rather than delegating to
    /// [`OptFlags::all`] so a future, not-yet-proven pass added to `all()`
    /// does not auto-default-on.
    pub const fn default_on() -> OptFlags {
        OptFlags {
            in_place_mutation: true,
            in_place_straight_line: true,
            in_place_cells: true,
            copy_propagation: true,
            match_dispatch: true,
            static_cache: true,
            static_in_functions: true,
            preserve_observations: false,
            preserve_trace: false,
        }
    }

    /// These passes, with `other`'s preservation guards: switching the
    /// optimizer on or off does not change what a tool asked to keep.
    pub const fn preserving(self, other: OptFlags) -> OptFlags {
        OptFlags {
            preserve_observations: other.preserve_observations,
            preserve_trace: other.preserve_trace,
            ..self
        }
    }

    /// These passes with the values built inside function bodies kept or
    /// not (it takes `static_cache` as well for any to be).
    pub const fn with_static_in_functions(self, on: bool) -> OptFlags {
        OptFlags {
            static_in_functions: on,
            ..self
        }
    }

    /// These passes with the static-value cache switched on or off.
    pub const fn with_static_cache(self, on: bool) -> OptFlags {
        OptFlags {
            static_cache: on,
            ..self
        }
    }
}

impl Default for OptFlags {
    fn default() -> OptFlags {
        OptFlags::default_on()
    }
}

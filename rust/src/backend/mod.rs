//! Execution for Petal.
//!
//! Petal compiles source to a canonical term-graph IR (`crate::program`), which
//! the [`bytecode`] backend runs: a linear register VM over a *lowering* of the
//! term graph (see `bytecode::lower`), with an in-place mutation optimization
//! gated by escape analysis. Static analyses (slicing, `explain`'s provenance
//! walk, autodiff-as-graph) still reason about the term graph directly; the VM
//! populates the trace buffer those analyses read at runtime.
//!
//! [`OptFlags`] chooses which optimizations a run enables.

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

/// Per-run optimization toggles. Every optimization is individually switchable
/// so it can be disabled to isolate a bug: "bytecode with all opts off" is a
/// differential-testing oracle alongside the graph backend.
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
}

impl OptFlags {
    /// All optimizations disabled — the correctness baseline.
    pub const fn none() -> OptFlags {
        OptFlags {
            in_place_mutation: false,
            in_place_straight_line: false,
        }
    }

    /// All optimizations enabled.
    pub const fn all() -> OptFlags {
        OptFlags {
            in_place_mutation: true,
            in_place_straight_line: true,
        }
    }
}

impl Default for OptFlags {
    /// In-place mutation is **on by default** for both M4 routes: route B
    /// (loop accumulators, default-on since the M4 flip) and route A
    /// (straight-line last-use, default-on after earning the same 300k-seed
    /// four-oracle fuzz soak). Both are at full differential parity with
    /// clone-and-alloc (graph / BC-noopt / BC-route-A-only / BC-all), so
    /// sketches get the zero-copy wins without opting in. Disable per-run
    /// with `--no-opt` / `PETAL_OPT=off` (which map to [`OptFlags::none`])
    /// to recover the clone-and-alloc oracle. This is spelled out
    /// field-by-field rather than delegating to [`OptFlags::all`] so a
    /// future, not-yet-proven opt added to `all()` does not auto-default-on.
    fn default() -> OptFlags {
        OptFlags {
            in_place_mutation: true,
            in_place_straight_line: true,
        }
    }
}

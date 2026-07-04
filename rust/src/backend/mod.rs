//! Execution backends for Petal.
//!
//! Petal compiles source to a canonical term-graph IR (`crate::program`). That
//! IR can be executed two ways:
//!
//! - [`graph`] — the original step evaluator, which walks the term graph
//!   node-by-node. It is the reference/introspection engine; provenance,
//!   slicing, autodiff, `explain`, and hot-reload all reason about the same
//!   graph it executes.
//! - [`bytecode`] — a linear register VM that runs a *lowering* of the term
//!   graph (see `bytecode::lower`). It exists for speed and for the in-place
//!   mutation optimization gated by escape analysis.
//!
//! Both engines share the [`StepResult`] contract, so `Env`'s run loops are
//! backend-agnostic. The active engine and its enabled optimizations are chosen
//! by [`Backend`] and [`OptFlags`].

pub mod bytecode;
pub mod calls;
pub mod errors;
pub mod graph;
pub mod ops;
pub mod pattern;

// The graph engine's public surface is re-exported here so callers depend on
// `crate::backend::…` rather than a specific engine module.
pub use graph::Evaluator;

use crate::program::FunctionId;
use crate::value::Value;

/// Result of a single execution step. Shared contract between the graph
/// `Evaluator` and the bytecode `Vm` so `Env`'s run loops are engine-agnostic.
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

/// Which execution engine `Env` uses to run a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The term-graph step evaluator — the reference engine and correctness
    /// oracle. Select with `--backend=graph` / `PETAL_BACKEND=graph`.
    Graph,
    /// The linear register VM — the default engine. At full behavioral parity
    /// with `Graph` (value, output, state, and error text; enforced by the
    /// differential tests in `bytecode::tests` and the example sweep).
    #[default]
    Bytecode,
}

impl Backend {
    /// Parse a backend name (`"graph"` / `"bytecode"`), e.g. from `--backend`
    /// or the `PETAL_BACKEND` env var. Case-insensitive; `"ir"` is an alias for
    /// `graph`.
    pub fn parse(s: &str) -> Option<Backend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "graph" | "ir" | "steps" => Some(Backend::Graph),
            "bytecode" | "bc" | "vm" => Some(Backend::Bytecode),
            _ => None,
        }
    }
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

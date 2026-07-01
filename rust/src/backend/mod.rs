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
pub mod graph;

// The graph engine's public surface is re-exported here so callers depend on
// `crate::backend::…` rather than a specific engine module.
pub use graph::{Evaluator, RuntimeClosure, StepResult};

/// Which execution engine `Env` uses to run a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// The term-graph step evaluator (reference engine). Default until the
    /// bytecode VM reaches parity (see `docs` / the bytecode plan).
    #[default]
    Graph,
    /// The linear register VM.
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
    /// clone-and-alloc. Requires the escape analysis in `bytecode::escape`.
    pub in_place_mutation: bool,
}

impl OptFlags {
    /// All optimizations disabled — the correctness baseline.
    pub const fn none() -> OptFlags {
        OptFlags {
            in_place_mutation: false,
        }
    }

    /// All optimizations enabled.
    pub const fn all() -> OptFlags {
        OptFlags {
            in_place_mutation: true,
        }
    }
}

impl Default for OptFlags {
    fn default() -> OptFlags {
        OptFlags::none()
    }
}

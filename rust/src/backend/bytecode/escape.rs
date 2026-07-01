//! Escape / uniqueness analysis over the term graph.
//!
//! Determines which collection-mutating terms (`SetField`/`SetIndex` and the
//! mutating builtins) operate on a container that has exactly one live consumer
//! and never escapes — so lowering may emit an *in-place* opcode instead of
//! clone-and-alloc, without changing observable value semantics.
//!
//! Arrives in M4. The analysis reuses the reverse-dataflow index built by
//! `Program::trace_dependents` and the phi-source set from `trace_provenance`.
//! Until then, lowering treats every mutation as clone-and-alloc.

use std::collections::HashSet;

use crate::program::{Program, TermId};

/// Terms whose container input is provably unique + non-escaping, and may
/// therefore be lowered to an in-place mutation.
#[derive(Debug, Default, Clone)]
pub struct InPlaceSet {
    terms: HashSet<TermId>,
}

impl InPlaceSet {
    /// Whether the mutation term `t` may be lowered in place.
    pub fn allows(&self, t: TermId) -> bool {
        self.terms.contains(&t)
    }
}

/// Analyze a program and return the set of in-place-eligible mutation terms.
///
/// M4 stub: returns the empty set (every mutation stays clone-and-alloc), so the
/// bytecode backend is correct with `OptFlags::none()` semantics regardless of
/// whether the flag is on.
pub fn analyze(_program: &Program) -> InPlaceSet {
    InPlaceSet::default()
}

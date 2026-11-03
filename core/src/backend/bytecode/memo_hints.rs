//! The compile-time memo decisions of the dependency classes
//! (`crate::static_classify::dep_classes`), written into a lowered program.
//!
//! Two things are recorded, and neither moves or removes an instruction:
//!
//! - each function's [`MemoHint`](super::isa::MemoHint): a call of a
//!   function with an effect on every path never opens a scope; one that
//!   reads a ticking input on every path opens none unless a caller is
//!   recording; a constant function with static arguments is replayed
//!   without checking captures or dependencies;
//! - [`Inst::Call::static_args`](super::isa::Inst::Call) on each call whose
//!   arguments are all static, so a replay check does not compare them.
//!
//! The VM reads them only while memoization is on, and ignores them all
//! under `MemoTable::no_hints`, which is how a test holds the hinted run to
//! the runtime-only one.

use super::isa::{BytecodeProgram, Inst};
use crate::program::Program;
use crate::static_classify::DepClasses;

/// Write the hints of `deps` into `bc`, lowered from `program`. Returns the
/// number of calls marked as having static arguments.
pub fn apply(bc: &mut BytecodeProgram, program: &Program, deps: &DepClasses) -> usize {
    for f in &deps.fns {
        if let Some(bf) = bc.fns.get_mut(f.id.0 as usize) {
            bf.memo_hint = f.hint();
        }
    }
    let mut marked = 0;
    let fns = std::iter::once(&mut bc.root).chain(bc.fns.iter_mut());
    for f in fns {
        for (inst, origin) in f.code.iter_mut().zip(f.origins.iter()) {
            if let Inst::Call { static_args, .. } = inst
                && let Some(t) = origin
                && (t.0 as usize) < program.terms.len()
                && deps.static_args(*t)
            {
                *static_args = true;
                marked += 1;
            }
        }
    }
    marked
}

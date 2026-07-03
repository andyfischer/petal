//! The bytecode backend: a linear register VM that runs a lowering of the
//! term-graph IR.
//!
//! Pipeline: [`lower::lower_program`] turns a `Program` into an [`isa::BytecodeProgram`];
//! [`vm`] executes it; [`disasm`] renders it for `show-bytecode` / `ShowBytecode`;
//! [`escape`] supplies the in-place-mutation analysis for loop accumulators
//! (M4 route B, graph-side, feeds lowering); [`lastuse`] rewrites straight-line
//! mutations in place after lowering (M4 route A, bytecode-side).
//!
//! See the bytecode plan for the milestone breakdown.

pub mod disasm;
pub mod escape;
pub mod isa;
pub mod lastuse;
pub mod lower;
pub mod vm;

pub use escape::{analyze as analyze_escapes, InPlaceSet};
pub use lastuse::apply as apply_last_use;
pub use isa::{BytecodeFn, BytecodeProgram, Inst};
pub use lower::{lower_program, lower_program_opt};
pub use vm::{Vm, VmFrame};

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod tests;

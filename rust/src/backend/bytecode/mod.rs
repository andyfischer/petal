//! The bytecode backend: a linear register VM that runs a lowering of the
//! term-graph IR.
//!
//! Pipeline: [`lower::lower_program`] turns a `Program` into an [`isa::BytecodeProgram`];
//! [`vm`] executes it; [`disasm`] renders it for `show-bytecode` / `ShowBytecode`;
//! [`escape`] supplies the in-place-mutation analysis (M4).
//!
//! See the bytecode plan for the milestone breakdown.

pub mod disasm;
pub mod escape;
pub mod isa;
pub mod lower;
pub mod vm;

pub use isa::{BytecodeFn, BytecodeProgram, Inst};
pub use lower::lower_program;

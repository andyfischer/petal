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

pub use escape::{InPlaceSet, analyze as analyze_escapes};
pub use isa::{BytecodeFn, BytecodeProgram, Inst};
pub use lastuse::apply as apply_last_use;
pub use lower::{lower_program, lower_program_opt};
pub use vm::{Vm, VmFrame};

use crate::backend::OptFlags;
use crate::program::Program;

/// Lower `program` to bytecode under `flags` — the single definition of the
/// optimization pipeline a run executes. Both `show-bytecode` and the embedder
/// inspector go through it so a disassembly always matches the opcodes the VM
/// would run.
///
/// Escape analysis (M4 route B) is a pure function of the program; honoring its
/// in-place set is gated on the flag, so "opts off" reproduces the
/// clone-and-alloc oracle byte-for-byte. Route A's straight-line last-use
/// rewriting runs on the lowered code, after route B's opcode selection.
pub fn lower_with_flags(program: &Program, flags: OptFlags) -> Result<BytecodeProgram, String> {
    let in_place = if flags.in_place_mutation {
        analyze_escapes(program)
    } else {
        InPlaceSet::default()
    };
    let mut bc = lower_program_opt(program, &in_place)
        .map_err(|e| format!("bytecode lowering failed: {e}"))?;
    if flags.in_place_straight_line {
        apply_last_use(&mut bc, program);
    }
    Ok(bc)
}

#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod tests;

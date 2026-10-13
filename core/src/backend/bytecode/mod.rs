//! The bytecode backend: a linear register VM that runs a lowering of the
//! term-graph IR.
//!
//! Pipeline: [`lower::lower_program`] turns a `Program` into an [`isa::BytecodeProgram`];
//! [`vm`] executes it; [`disasm`] renders it for `show-bytecode` / `ShowBytecode`;
//! [`escape`] supplies the in-place-mutation analysis for loop accumulators
//! (M4 route B, graph-side, feeds lowering); [`lastuse`] rewrites straight-line
//! mutations in place after lowering (M4 route A, bytecode-side); [`cells`]
//! picks the `var` reads and writes that can share one backing store, which
//! is decided per cell at run time rather than proven ahead of it.
//!
//! See the bytecode plan for the milestone breakdown.

pub mod cells;
pub mod copyprop;
pub mod disasm;
pub mod escape;
pub mod isa;
pub mod lastuse;
pub mod lower;
pub mod vm;

pub use copyprop::{Preserve, apply as apply_copy_prop};
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
/// clone-and-alloc oracle byte-for-byte. The cell plan is likewise a pure
/// function of the program, gated on its own flag. Route A's straight-line
/// last-use rewriting runs on the lowered code, after route B's opcode
/// selection.
pub fn lower_with_flags(program: &Program, flags: OptFlags) -> Result<BytecodeProgram, String> {
    lower(program, flags, None)
}

/// [`lower_with_flags`], honouring [`OptFlags::static_cache`]: the lowering
/// an `Env` runs. The static-value cache needs the classifier's answer for
/// the program, and that depends on the natives of the `Env` that compiled
/// it, which is why this is a separate entry point: without `analysis` the
/// flag has nothing to act on.
pub fn lower_with_statics(
    program: &Program,
    flags: OptFlags,
    analysis: &crate::static_classify::StaticAnalysis,
    natives: &crate::native_fn::NativeFnTable,
) -> Result<BytecodeProgram, String> {
    lower(program, flags, Some((analysis, natives)))
}

fn lower(
    program: &Program,
    flags: OptFlags,
    statics: Option<(&crate::static_classify::StaticAnalysis, &crate::native_fn::NativeFnTable)>,
) -> Result<BytecodeProgram, String> {
    // Which top-level terms a static slot may fill. The in-place analyses
    // are told, so neither takes a value shared across runs for a fresh one.
    let statics = statics.filter(|_| flags.static_cache);
    let plan = statics.map(|(analysis, _)| crate::static_cache::StaticPlan::of(program, analysis));
    let in_place = if flags.in_place_mutation {
        match &plan {
            Some(plan) => escape::analyze_with_static_roots(program, &plan.roots),
            None => escape::analyze(program),
        }
    } else {
        InPlaceSet::default()
    };
    // Writes through a cell keep a container the trace would otherwise hold a
    // history of, so the two do not mix (see `cells`, "Who else looks").
    let in_place = if flags.in_place_cells && !flags.preserve_trace {
        in_place.with_cells(cells::analyze(program))
    } else {
        in_place
    };
    let mut bc = lower_program_opt(program, &in_place, flags.match_dispatch)
        .map_err(|e| format!("bytecode lowering failed: {e}"))?;
    if flags.in_place_straight_line {
        match &plan {
            Some(plan) => {
                lastuse::apply_with_static_roots(&mut bc, program, &plan.root_regs(program))
            }
            None => apply_last_use(&mut bc, program),
        };
    }
    // Runs last of the rewriting passes: it rewrites operands and drops
    // instructions, so letting it precede route A would move the registers
    // that pass's alias groups are stated in terms of.
    if flags.copy_propagation {
        let stats = apply_copy_prop(
            &mut bc,
            program,
            Preserve {
                observations: flags.preserve_observations,
                trace: flags.preserve_trace,
            },
        );
        if std::env::var("PETAL_OPT_STATS").is_ok() {
            eprintln!(
                "copyprop: {} instructions -> {} ({} moves removed, {} reads rewritten, \
                 {} jumps threaded)",
                stats.before, stats.after, stats.removed, stats.rewritten, stats.threaded
            );
        }
    }
    // The static-value cache wraps runs of the code as it finally is: it
    // adds two instructions per run and moves or removes none.
    if let (Some((analysis, natives)), Some(plan)) = (statics, &plan) {
        // A segment that is skipped must still report what it observes.
        crate::static_cache::apply(
            &mut bc,
            program,
            analysis,
            natives,
            plan,
            flags.preserve_observations,
            flags.static_in_functions,
        );
        if std::env::var("PETAL_OPT_STATS").is_ok() {
            eprint!("{}", crate::static_cache::describe(&bc.statics));
        }
    }
    Ok(bc)
}

#[cfg(test)]
mod cell_fuzz;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod static_fn_fuzz;
#[cfg(test)]
mod static_fuzz;
#[cfg(test)]
mod tests;

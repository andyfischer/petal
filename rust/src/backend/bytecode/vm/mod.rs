//! The register VM that executes lowered [`BytecodeProgram`]s — Petal's only
//! execution engine.
//!
//! [`Vm`] is a bundle of borrows over the runtime data owned by `Env`, rebuilt
//! for each `step`. Execution state (the frame stack) lives on the [`Stack`] so
//! it survives across steps and is reachable for garbage collection.
//!
//! The implementation is split across sibling files, each adding an `impl`
//! block to the [`Vm`] struct defined here:
//! - [`frame`] — the [`VmFrame`]/[`LoopCursor`] types, frame pooling, and
//!   loop-cursor / state-key plumbing.
//! - [`dispatch`] — `exec_inst` (the per-`Inst` executor) and the arithmetic
//!   helpers it calls.
//! - [`calls`] — user-function call/return handling and frame push/pop for
//!   calls.
//! - [`native`] — native-function and `BuiltinCall` dispatch.
//! - [`intrinsics`] — the higher-order intrinsics (map/filter/reduce/forEach)
//!   and the synchronous closure driver they share with the host.

use std::collections::HashMap;

use smallvec::SmallVec;

use super::isa::{BytecodeFn, BytecodeProgram, Reg};
use crate::backend::{RuntimeClosure, StepResult};
use crate::handle::HandleClass;
use crate::heap::Heap;
use crate::native_fn::{NativeFnId, NativeFnTable};
use crate::backend::errors::TraceFrame;
use crate::program::{base_fn_name, FunctionId, OverloadEntry, Program, TermId};
use crate::stack::Stack;
use crate::symbol::{SymbolId, SymbolTable};
use crate::value::Value;

mod frame;
mod dispatch;
mod calls;
mod native;
mod intrinsics;

pub use frame::{LoopCursor, VmFrame};

/// Retention cap for [`Stack::vm_frame_pool`]: deep recursion can push the
/// pool's high-water mark far above steady-state needs; beyond this many
/// pooled frames, popped frames are dropped instead.
const FRAME_POOL_MAX: usize = 1024;

/// The bytecode VM: a bundle of borrows over `Env`'s runtime data, rebuilt for
/// each `step`. Frame state lives on `stack.vm_frames`.
pub struct Vm<'a> {
    pub program: &'a Program,
    pub bc: &'a BytecodeProgram,
    pub stack: &'a mut Stack,
    pub heap: &'a mut Heap,
    pub closures: &'a mut Vec<RuntimeClosure>,
    pub overload_sets: &'a mut Vec<Vec<OverloadEntry>>,
    pub native_fns: &'a NativeFnTable,
    pub handle_classes: &'a [HandleClass],
    pub output: &'a mut Vec<String>,
    pub symbols: &'a mut SymbolTable,
    pub output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
    pub bindings: &'a mut HashMap<SymbolId, Value>,
    pub counters: &'a mut HashMap<SymbolId, u64>,
    /// Per-run PRNG state and noise seed, borrowed from the `ExecutionContext`
    /// so the RNG/noise builtins mutate the owning context's isolated state.
    pub rng_state: &'a mut u64,
    pub noise_seed: &'a mut u64,
    /// Whether `print` echoes to real stdout (true for the primary run, false
    /// for speculative forks). Copied from the `ExecutionContext`.
    pub echo: bool,
    /// Structured execution trace (off by default). When enabled, the VM records
    /// `(origin term, inputs, result)` per retired instruction so `explain` /
    /// `ExplainTerm` work under the VM — see [`Vm::step`] and
    /// [`Vm::deliver_value`]. Best-effort: in-place mutation and register reuse
    /// can thin coverage relative to the graph engine.
    pub trace: &'a mut crate::trace::TraceBuffer,
    /// Set by `call_closure_sync` when a synchronous closure call (map/filter/
    /// reduce/forEach, or the host `Env::call_function`) unwinds with an error
    /// that `step` already annotated. The intrinsic returns that error via `?`,
    /// so it re-enters the outer `step`'s error path — this flag tells that path
    /// to pass the message through instead of annotating a second time (which
    /// would splice the outer call site's position and snippet into an
    /// already-complete message). Consumed with `mem::take`; a fresh `Vm` starts
    /// it clear, so a host-path error that leaves it set can't leak to a later run.
    pub error_already_annotated: bool,
}

impl<'a> Vm<'a> {
    /// Push the root activation record for a fresh run. Native function values
    /// are seeded into the low registers exactly as the graph engine's
    /// `push_root_frame` does (the compiler assigns builtin phantom terms to
    /// those registers).
    pub fn push_root_frame(&mut self) {
        let mut frame = self.frame_from_pool(None, self.bc.root.reg_count, None, None);
        for i in 0..self.native_fns.count() {
            if i < frame.regs.len() {
                frame.regs[i] = Value::NativeFunction(NativeFnId(i as u32));
            }
        }
        self.stack.vm_frames.push(frame);
    }

    /// Execute one instruction and advance. Returns the same [`StepResult`]
    /// contract as the graph engine so `Env`'s run loops stay backend-agnostic.
    pub fn step(&mut self) -> StepResult {
        let bc = self.bc; // &'a BytecodeProgram is Copy — detaches from `self`.
        let frame_idx = match self.stack.vm_frames.len().checked_sub(1) {
            Some(i) => i,
            None => return StepResult::Complete(Value::Nil),
        };
        let func: &'a BytecodeFn = bc.function_or_root(self.stack.vm_frames[frame_idx].func);
        let ip = self.stack.vm_frames[frame_idx].ip;
        if ip >= func.code.len() {
            return self.finish_frame(func);
        }
        // Advance past this instruction before executing; call/jump handlers
        // overwrite `ip` when they need to.
        self.stack.vm_frames[frame_idx].ip = ip + 1;
        let inst = &func.code[ip];
        let origin = func.origins.get(ip).copied().flatten();
        // Gather trace inputs before execution — a `dst` that aliases a source
        // register would clobber it otherwise. The `enabled` check comes first
        // so the disabled hot path pays only one bool test, never `dst()` /
        // `input_regs()`.
        let trace_inputs: Option<(TermId, Reg, SmallVec<[Value; 4]>)> = if self.trace.enabled {
            match (origin, inst.dst()) {
                (Some(term), Some(dst)) => {
                    let inputs = inst.input_regs().iter().map(|&r| self.reg(frame_idx, r)).collect();
                    Some((term, dst, inputs))
                }
                _ => None,
            }
        } else {
            None
        };
        match self.exec_inst(frame_idx, inst, origin) {
            Ok(sr) => {
                // Record the retired instruction's result, but only if it stayed
                // in this frame (a pushed call frame means `dst` isn't written
                // yet — call results are traced from `deliver_value` instead).
                if let Some((term, dst, inputs)) = trace_inputs {
                    if self.stack.vm_frames.len() == frame_idx + 1 {
                        let result = self.reg(frame_idx, dst);
                        self.trace.push(term, &inputs, result);
                    }
                }
                sr
            }
            Err(e) => {
                // A synchronous closure call already annotated this error; don't
                // re-annotate at the intrinsic's call site (see the flag's docs).
                if std::mem::take(&mut self.error_already_annotated) {
                    StepResult::Error(e)
                } else {
                    StepResult::Error(self.annotate(e, origin))
                }
            }
        }
    }

    /// Execute up to `budget` instructions, stopping early on completion,
    /// error, or when the heap wants a garbage collection (the caller owns the
    /// GC, so we yield `Continue` and let it collect between instructions —
    /// exactly where the per-step run loop would have). Returns the final
    /// [`StepResult`] and the number of instructions consumed.
    ///
    /// This is the VM's performance lever over per-instruction dispatch: `Env`
    /// re-resolves the stack/program/context maps and rebuilds this struct on
    /// every call, which costs more than executing a typical instruction. Run
    /// in batches, that overhead amortizes to nothing.
    pub fn run_batch(&mut self, budget: u64) -> (StepResult, u64) {
        let mut consumed = 0;
        while consumed < budget {
            consumed += 1;
            match self.step() {
                StepResult::Continue => {
                    if self.heap.should_collect() {
                        return (StepResult::Continue, consumed);
                    }
                }
                done => return (done, consumed),
            }
        }
        (StepResult::Continue, consumed)
    }

    /// Dress a raw runtime error with source position, snippet, provenance, and
    /// a stack trace built from the VM frames — identical formatting to the
    /// graph engine (see [`crate::backend::errors`]). Unannotated when the
    /// instruction has no source origin.
    fn annotate(&self, msg: String, origin: Option<TermId>) -> String {
        let Some(failing) = origin else {
            return msg;
        };
        let frames: Vec<TraceFrame> = self
            .stack
            .vm_frames
            .iter()
            .map(|f| TraceFrame {
                name: f.func.and_then(|fid| self.fn_display_name(fid)),
                call_site: f.call_site,
            })
            .collect();
        crate::backend::errors::annotate_error(self.program, failing, msg, &frames)
    }

    /// The display name for a function frame in a stack trace — the source name
    /// with any internal `#arity` overload suffix stripped (matching the graph).
    fn fn_display_name(&self, fid: FunctionId) -> Option<String> {
        self.program.functions[fid.0 as usize]
            .name
            .as_deref()
            .map(|n| base_fn_name(n).to_string())
    }

    // -- register access -----------------------------------------------------

    fn reg(&self, fi: usize, r: Reg) -> Value {
        self.stack.vm_frames[fi]
            .regs
            .get(r as usize)
            .copied()
            .unwrap_or(Value::Nil)
    }

    fn set(&mut self, fi: usize, r: Reg, v: Value) {
        let regs = &mut self.stack.vm_frames[fi].regs;
        if r as usize >= regs.len() {
            regs.resize(r as usize + 1, Value::Nil);
        }
        regs[r as usize] = v;
    }

    /// Gather operand registers. Inline capacity covers typical arities, so
    /// the hot call path (`Call`/`BuiltinCall` args) stays allocation-free.
    fn regs(&self, fi: usize, rs: &[Reg]) -> SmallVec<[Value; 8]> {
        rs.iter().map(|&r| self.reg(fi, r)).collect()
    }
}

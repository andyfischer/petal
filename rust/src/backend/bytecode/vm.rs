//! The register VM that executes lowered [`BytecodeProgram`]s.
//!
//! [`Vm`] mirrors the graph engine's [`Evaluator`](crate::backend::graph::Evaluator):
//! a bundle of borrows over the runtime data owned by `Env`, rebuilt for each
//! `step`. Execution state (the frame stack) lives on the [`Stack`] so it
//! survives across steps and is reachable for garbage collection, exactly like
//! the graph engine's `Frame`s.
//!
//! ## Milestone status
//! M1b executes the straight-line op set (constants, arithmetic/compare/logical,
//! `Move`, the data-structure allocators, field/index access). Calls, control
//! flow, state, and match return an error until M1c–M3 land — matching the
//! lowering, which does not yet emit those instructions.

use std::collections::HashMap;

use smallvec::SmallVec;

use super::isa::{BytecodeFn, BytecodeProgram, Inst, LoopSlot, Reg};
use crate::backend::ops;
use crate::backend::{RuntimeClosure, StepResult};
use crate::heap::Heap;
use crate::native_fn::NativeFnTable;
use crate::program::{FunctionId, OverloadEntry, Program, TermOp};
use crate::stack::{LoopKeyPart, Stack};
use crate::symbol::{SymbolId, SymbolTable};
use crate::value::Value;

/// A per-call activation record: one flat register file plus loop cursors.
#[derive(Clone)]
pub struct VmFrame {
    /// The function this frame is executing (`None` = the implicit root).
    pub func: Option<FunctionId>,
    /// Instruction pointer into the function's `code`.
    pub ip: usize,
    /// Flat register file (`Value` is `Copy`, so this is a plain `Vec`).
    pub regs: Vec<Value>,
    /// Caller register that receives this frame's return value.
    pub dst_in_caller: Reg,
    /// Active loop cursors, indexed by [`LoopSlot`].
    pub loops: SmallVec<[LoopCursor; 2]>,
    /// Active loop-index context, outermost-first, for state-key resolution.
    pub loop_idx: SmallVec<[LoopKeyPart; 2]>,
}

impl VmFrame {
    /// A fresh frame for `func` with a zeroed register file of `reg_count`.
    pub fn new(func: Option<FunctionId>, reg_count: u16, dst_in_caller: Reg) -> Self {
        VmFrame {
            func,
            ip: 0,
            regs: vec![Value::Nil; reg_count as usize],
            dst_in_caller,
            loops: SmallVec::new(),
            loop_idx: SmallVec::new(),
        }
    }
}

/// A live loop's iteration state (replaces the graph engine's `LoopState`).
#[derive(Clone)]
pub enum LoopCursor {
    /// `for x in <list>`: the snapshotted elements and the next index.
    ForEach { elems: Vec<Value>, i: usize },
    /// `for i in range(a, b)`: the current value and exclusive end.
    Range { cur: i64, end: i64 },
    /// A `while` loop tracks only its iteration counter (for state keying).
    While { iteration: usize },
}

impl LoopCursor {
    /// The slot this cursor occupies is positional; this helper exists so future
    /// code reads clearly at call sites.
    pub fn slot_placeholder() -> LoopSlot {
        0
    }
}

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
    pub output: &'a mut Vec<String>,
    pub symbols: &'a mut SymbolTable,
    pub output_buffers: &'a mut HashMap<SymbolId, Vec<Value>>,
    pub bindings: &'a mut HashMap<SymbolId, Value>,
    pub counters: &'a mut HashMap<SymbolId, u64>,
}

impl<'a> Vm<'a> {
    /// Push the root activation record for a fresh run. Native function values
    /// are seeded into the low registers exactly as the graph engine's
    /// `push_root_frame` does (the compiler assigns builtin phantom terms to
    /// those registers).
    pub fn push_root_frame(&mut self) {
        let mut frame = VmFrame::new(None, self.bc.root.reg_count, 0);
        for i in 0..self.native_fns.count() {
            if i < frame.regs.len() {
                frame.regs[i] = Value::NativeFunction(crate::native_fn::NativeFnId(i as u32));
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
        // (later milestones) overwrite `ip` when they need to.
        self.stack.vm_frames[frame_idx].ip = ip + 1;
        match self.exec_inst(frame_idx, &func.code[ip]) {
            Ok(()) => StepResult::Continue,
            Err(e) => StepResult::Error(e),
        }
    }

    /// A frame ran off the end of its code without an explicit `Return`: its
    /// value is the entry block's last-term register (mirrors the graph
    /// engine's `block_result`). Pop it and deliver the value.
    fn finish_frame(&mut self, func: &BytecodeFn) -> StepResult {
        let top = self.stack.vm_frames.len() - 1;
        let result = func
            .result_reg
            .map(|r| self.reg(top, r))
            .unwrap_or(Value::Nil);
        let frame = self.stack.vm_frames.pop().unwrap();
        self.stack.last_pop_result = Some(result);

        if self.stack.vm_frames.is_empty() {
            return StepResult::Complete(result);
        }
        let caller = self.stack.vm_frames.len() - 1;
        self.set(caller, frame.dst_in_caller, result);
        StepResult::Continue
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

    fn regs(&self, fi: usize, rs: &[Reg]) -> Vec<Value> {
        rs.iter().map(|&r| self.reg(fi, r)).collect()
    }

    // -- instruction dispatch ------------------------------------------------

    fn exec_inst(&mut self, fi: usize, inst: &Inst) -> Result<(), String> {
        match inst {
            Inst::LoadConst { dst, k } => {
                let v = ops::constant_to_value(self.program, self.heap, *k);
                self.set(fi, *dst, v);
            }
            Inst::Move { dst, src } => {
                let v = self.reg(fi, *src);
                self.set(fi, *dst, v);
            }

            Inst::Add { dst, a, b } => self.binop(fi, TermOp::Add, *dst, *a, *b)?,
            Inst::Sub { dst, a, b } => self.binop(fi, TermOp::Sub, *dst, *a, *b)?,
            Inst::Mul { dst, a, b } => self.binop(fi, TermOp::Mul, *dst, *a, *b)?,
            Inst::Div { dst, a, b } => self.binop(fi, TermOp::Div, *dst, *a, *b)?,
            Inst::Mod { dst, a, b } => self.binop(fi, TermOp::Mod, *dst, *a, *b)?,
            Inst::Neg { dst, a } => {
                let v = ops::negate(self.reg(fi, *a))?;
                self.set(fi, *dst, v);
            }

            Inst::Eq { dst, a, b } => {
                let v = Value::Bool(ops::equals(self.reg(fi, *a), self.reg(fi, *b), self.heap));
                self.set(fi, *dst, v);
            }
            Inst::Ne { dst, a, b } => {
                let v = Value::Bool(!ops::equals(self.reg(fi, *a), self.reg(fi, *b), self.heap));
                self.set(fi, *dst, v);
            }
            Inst::Lt { dst, a, b } => self.cmp(fi, TermOp::Lt, *dst, *a, *b)?,
            Inst::Le { dst, a, b } => self.cmp(fi, TermOp::Le, *dst, *a, *b)?,
            Inst::Gt { dst, a, b } => self.cmp(fi, TermOp::Gt, *dst, *a, *b)?,
            Inst::Ge { dst, a, b } => self.cmp(fi, TermOp::Ge, *dst, *a, *b)?,

            Inst::Not { dst, a } => {
                let v = ops::not(self.reg(fi, *a));
                self.set(fi, *dst, v);
            }
            Inst::Concat { dst, a, b } => {
                let v = ops::concat(self.reg(fi, *a), self.reg(fi, *b), self.heap)?;
                self.set(fi, *dst, v);
            }

            Inst::AllocList { dst, elems } => {
                let vals = self.regs(fi, elems);
                let v = ops::alloc_list(self.heap, &vals);
                self.set(fi, *dst, v);
            }
            Inst::AllocMap { dst, fields, vals } => {
                let inputs = self.regs(fi, vals);
                let v = ops::alloc_map(self.program, self.heap, fields, &inputs)?;
                self.set(fi, *dst, v);
            }
            Inst::AllocMapSpread { dst, entries, ins } => {
                let inputs = self.regs(fi, ins);
                let v = ops::alloc_map_spread(self.program, self.heap, entries, &inputs)?;
                self.set(fi, *dst, v);
            }
            Inst::AllocElement { dst, tag, prop_keys, ins } => {
                let inputs = self.regs(fi, ins);
                let v = ops::alloc_element(self.program, self.heap, *tag, prop_keys, &inputs)?;
                self.set(fi, *dst, v);
            }
            Inst::MakeEnumVariant { dst, name, fields } => {
                let inputs = self.regs(fi, fields);
                let v = ops::make_enum_variant(self.program, self.heap, *name, &inputs)?;
                self.set(fi, *dst, v);
            }

            Inst::GetField { dst, obj, field } => {
                let v = ops::get_field(self.program, self.heap, *field, self.reg(fi, *obj))?;
                self.set(fi, *dst, v);
            }
            Inst::SetField { dst, obj, field, val } => {
                let v = ops::set_field(
                    self.program,
                    self.heap,
                    *field,
                    self.reg(fi, *obj),
                    self.reg(fi, *val),
                )?;
                self.set(fi, *dst, v);
            }
            Inst::GetIndex { dst, obj, idx } => {
                let v = ops::get_index(self.heap, self.reg(fi, *obj), self.reg(fi, *idx))?;
                self.set(fi, *dst, v);
            }
            Inst::SetIndex { dst, obj, idx, val } => {
                let v = ops::set_index(
                    self.heap,
                    self.reg(fi, *obj),
                    self.reg(fi, *idx),
                    self.reg(fi, *val),
                )?;
                self.set(fi, *dst, v);
            }

            Inst::Error { msg } => {
                return Err(self
                    .program
                    .get_string_constant(*msg)
                    .unwrap_or("Unknown error")
                    .to_string());
            }

            other => {
                return Err(format!(
                    "bytecode VM: unimplemented instruction {:?} (arrives in a later milestone)",
                    std::mem::discriminant(other)
                ));
            }
        }
        Ok(())
    }

    fn binop(&mut self, fi: usize, op: TermOp, dst: Reg, a: Reg, b: Reg) -> Result<(), String> {
        let v = ops::arithmetic(&op, self.reg(fi, a), self.reg(fi, b), self.heap)?;
        self.set(fi, dst, v);
        Ok(())
    }

    fn cmp(&mut self, fi: usize, op: TermOp, dst: Reg, a: Reg, b: Reg) -> Result<(), String> {
        let v = ops::comparison(&op, self.reg(fi, a), self.reg(fi, b), self.heap)?;
        self.set(fi, dst, v);
        Ok(())
    }
}

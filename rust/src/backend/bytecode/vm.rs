//! The register VM that executes lowered [`BytecodeProgram`]s.
//!
//! [`Vm`] mirrors the graph engine's [`Evaluator`](crate::backend::graph::Evaluator):
//! a bundle of borrows over the runtime data owned by `Env`, rebuilt for each
//! `step`. Execution state (the frame stack) lives on the [`Stack`] so it
//! survives across steps and is reachable for garbage collection, exactly like
//! the graph engine's `Frame`s.
//!
//! ## Milestone status
//! M1 executes the straight-line op set plus calls, closures, overload sets,
//! returns, native/builtin dispatch, and the synchronous higher-order
//! intrinsics (map/filter/reduce/forEach). Control flow, state, and match
//! return an error until M2–M3 — matching the lowering.

use std::collections::HashMap;

use smallvec::SmallVec;

use super::isa::{BytecodeFn, BytecodeProgram, Inst, LoopSlot, Reg};
use crate::backend::{calls, ops};
use crate::backend::{RuntimeClosure, StepResult};
use crate::heap::Heap;
use crate::native_fn::{NativeFnId, NativeFnTable, PetalCxt};
use crate::backend::errors::TraceFrame;
use crate::program::{ClosureId, FunctionId, OverloadEntry, Program, StateKey, TermId, TermOp};
use crate::stack::{LoopKeyPart, RuntimeStateKey, Stack};
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
    /// Caller register that receives this frame's return value. `None` for the
    /// root frame and for frames pushed by a synchronous intrinsic call (whose
    /// result is read from `stack.last_pop_result`, not written to a register).
    pub dst_in_caller: Option<Reg>,
    /// Loop cursors, indexed by [`LoopSlot`]. A slot is `Some` while its loop is
    /// active (set by a `*Init` op, cleared by `LoopPop`); grown on demand.
    pub loops: Vec<Option<LoopCursor>>,
    /// Active loop-index context, outermost-first, for state-key resolution.
    pub loop_idx: SmallVec<[LoopKeyPart; 2]>,
    /// The `Call` term that created this frame (for stack-trace annotation).
    /// `None` for the root frame and synchronous-intrinsic frames.
    pub call_site: Option<TermId>,
}

impl VmFrame {
    /// A fresh frame for `func` with a zeroed register file of `reg_count`.
    pub fn new(
        func: Option<FunctionId>,
        reg_count: u16,
        dst_in_caller: Option<Reg>,
        call_site: Option<TermId>,
    ) -> Self {
        VmFrame {
            func,
            ip: 0,
            regs: vec![Value::Nil; reg_count as usize],
            dst_in_caller,
            loops: Vec::new(),
            loop_idx: SmallVec::new(),
            call_site,
        }
    }
}

/// A live loop's iteration state (replaces the graph engine's `LoopState`).
#[derive(Clone)]
pub enum LoopCursor {
    /// `for x in <list>`: the snapshotted elements and the next index.
    ForEach { elems: Vec<Value>, i: usize },
    /// `for i in range(a, b)`: the current value, exclusive end, and 0-based
    /// iteration count (the state-key index, which differs from the value when
    /// the range does not start at 0).
    Range { cur: i64, end: i64, iter: usize },
    /// A `while` loop tracks only its iteration counter (for state keying).
    While { iteration: usize },
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
        let mut frame = VmFrame::new(None, self.bc.root.reg_count, None, None);
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
        let origin = func.origins.get(ip).copied().flatten();
        match self.exec_inst(frame_idx, &func.code[ip], origin) {
            Ok(sr) => sr,
            Err(e) => StepResult::Error(self.annotate(e, origin)),
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
            .as_ref()
            .map(|n| match n.rfind('#') {
                Some(pos) => n[..pos].to_string(),
                None => n.clone(),
            })
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
        self.deliver_value(result)
    }

    /// Pop the current frame and deliver `value`: to the caller's `dst`
    /// register, or up as `StepResult::Complete` when the root frame finishes.
    fn deliver_value(&mut self, value: Value) -> StepResult {
        let frame = self.stack.vm_frames.pop().unwrap();
        self.stack.last_pop_result = Some(value);
        if self.stack.vm_frames.is_empty() {
            // The root frame just completed — capture top-level named functions
            // so `Env::call_function` can invoke them without a re-run.
            if frame.func.is_none() {
                self.capture_root_functions(&frame);
            }
            return StepResult::Complete(value);
        }
        if let Some(dst) = frame.dst_in_caller {
            let caller = self.stack.vm_frames.len() - 1;
            self.set(caller, dst, value);
        }
        StepResult::Continue
    }

    /// Record top-level named `Closure`/`OverloadSet` bindings from the root
    /// frame into `stack.functions` (mirrors the graph engine).
    fn capture_root_functions(&mut self, frame: &VmFrame) {
        let root = self.program.root_block;
        let Some(term_ids) = self.program.block_terms.get(&root) else {
            return;
        };
        let mut captured = Vec::new();
        for &tid in term_ids {
            let term = self.program.get_term(tid);
            if let Some(name) = term.name.as_ref() {
                let val = frame
                    .regs
                    .get(term.register.0 as usize)
                    .copied()
                    .unwrap_or(Value::Nil);
                if matches!(val, Value::Closure(_) | Value::OverloadSet(_)) {
                    captured.push((name.clone(), val));
                }
            }
        }
        for (name, val) in captured {
            self.stack.functions.insert(name, val);
        }
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

    /// Grow frame `fi`'s loop-cursor vector so `slot` is addressable.
    fn ensure_slot(&mut self, fi: usize, slot: LoopSlot) {
        let loops = &mut self.stack.vm_frames[fi].loops;
        if slot as usize >= loops.len() {
            loops.resize_with(slot as usize + 1, || None);
        }
    }

    /// Set the innermost active loop-index context entry to `idx` (the current
    /// 0-based iteration), for per-iteration state keying.
    fn set_loop_idx_top(&mut self, fi: usize, idx: usize) {
        if let Some(last) = self.stack.vm_frames[fi].loop_idx.last_mut() {
            *last = LoopKeyPart::Index(idx);
        }
    }

    /// Resolve a state term's runtime key. An explicit `state(expr)` key hashes
    /// its value; otherwise a term inside a loop keys by the loop-index context
    /// gathered across *all* active frames (outermost first), matching the graph
    /// engine's `loop_key_parts`; a non-loop term keys by base alone.
    fn state_key(&self, base: StateKey, in_loop: bool, explicit: Option<Value>) -> RuntimeStateKey {
        let loop_indices = match explicit {
            Some(kv) => {
                let mut v = SmallVec::new();
                v.push(LoopKeyPart::Explicit(crate::value::hash_value(&kv, self.heap)));
                v
            }
            None if in_loop => {
                let mut parts: SmallVec<[LoopKeyPart; 2]> = SmallVec::new();
                for frame in &self.stack.vm_frames {
                    parts.extend(frame.loop_idx.iter().cloned());
                }
                parts
            }
            None => SmallVec::new(),
        };
        RuntimeStateKey { base, loop_indices }
    }

    // -- instruction dispatch ------------------------------------------------

    fn exec_inst(
        &mut self,
        fi: usize,
        inst: &Inst,
        origin: Option<TermId>,
    ) -> Result<StepResult, String> {
        match inst {
            Inst::LoadConst { dst, k } => {
                let v = ops::constant_to_value(self.program, self.heap, *k);
                self.set(fi, *dst, v);
            }
            Inst::LoadNil { dst } => self.set(fi, *dst, Value::Nil),
            Inst::LoadBool { dst, val } => self.set(fi, *dst, Value::Bool(*val)),
            Inst::Move { dst, src } => {
                let v = self.reg(fi, *src);
                self.set(fi, *dst, v);
            }

            // Jumps rewrite the current frame's instruction pointer (which
            // `step` already advanced past this instruction).
            Inst::Jump { to } => self.stack.vm_frames[fi].ip = *to as usize,
            Inst::JumpIfFalse { cond, to } => {
                if !self.reg(fi, *cond).is_truthy() {
                    self.stack.vm_frames[fi].ip = *to as usize;
                }
            }
            Inst::JumpIfTrue { cond, to } => {
                if self.reg(fi, *cond).is_truthy() {
                    self.stack.vm_frames[fi].ip = *to as usize;
                }
            }

            // --- loops ---
            Inst::ForEachInit { iter, slot, idx_ctx } => {
                let v = self.reg(fi, *iter);
                let Value::List(list_id) = v else {
                    return Err(format!("Cannot iterate over {}", v.type_name()));
                };
                let elems = self.heap.get_list(list_id).to_vec();
                self.ensure_slot(fi, *slot);
                self.stack.vm_frames[fi].loops[*slot as usize] =
                    Some(LoopCursor::ForEach { elems, i: 0 });
                if *idx_ctx {
                    self.stack.vm_frames[fi].loop_idx.push(LoopKeyPart::Index(0));
                }
            }
            Inst::ForEachNext { slot, var, exit } => {
                let action = match self.stack.vm_frames[fi].loops.get_mut(*slot as usize) {
                    Some(Some(LoopCursor::ForEach { elems, i })) => {
                        if *i >= elems.len() {
                            None
                        } else {
                            let e = elems[*i];
                            let idx = *i;
                            *i += 1;
                            Some((e, idx))
                        }
                    }
                    _ => return Err("foreach_next: no active cursor".into()),
                };
                match action {
                    None => self.stack.vm_frames[fi].ip = *exit as usize,
                    Some((e, idx)) => {
                        self.set(fi, *var, e);
                        self.set_loop_idx_top(fi, idx);
                    }
                }
            }
            Inst::RangeInit { start, end, slot, idx_ctx } => {
                let (s, e) = (self.reg(fi, *start), self.reg(fi, *end));
                let (Value::Int(s), Value::Int(e)) = (s, e) else {
                    return Err("numeric for-loop bounds must be integers".into());
                };
                self.ensure_slot(fi, *slot);
                self.stack.vm_frames[fi].loops[*slot as usize] =
                    Some(LoopCursor::Range { cur: s, end: e, iter: 0 });
                if *idx_ctx {
                    self.stack.vm_frames[fi].loop_idx.push(LoopKeyPart::Index(0));
                }
            }
            Inst::RangeNext { slot, var, exit } => {
                let action = match self.stack.vm_frames[fi].loops.get_mut(*slot as usize) {
                    Some(Some(LoopCursor::Range { cur, end, iter })) => {
                        if *cur < *end {
                            let v = *cur;
                            let it = *iter;
                            *cur += 1;
                            *iter += 1;
                            Some((v, it))
                        } else {
                            None
                        }
                    }
                    _ => return Err("range_next: no active cursor".into()),
                };
                match action {
                    None => self.stack.vm_frames[fi].ip = *exit as usize,
                    Some((v, it)) => {
                        self.set(fi, *var, Value::Int(v));
                        self.set_loop_idx_top(fi, it);
                    }
                }
            }
            Inst::WhileInit { slot } => {
                self.ensure_slot(fi, *slot);
                self.stack.vm_frames[fi].loops[*slot as usize] =
                    Some(LoopCursor::While { iteration: 0 });
                self.stack.vm_frames[fi].loop_idx.push(LoopKeyPart::Index(0));
            }
            Inst::LoopBumpIdx { slot } => {
                let it = match self.stack.vm_frames[fi].loops.get_mut(*slot as usize) {
                    Some(Some(LoopCursor::While { iteration })) => {
                        *iteration += 1;
                        *iteration
                    }
                    _ => return Err("loop_bump_idx: no active while cursor".into()),
                };
                self.set_loop_idx_top(fi, it);
            }
            Inst::LoopPop { slot } => {
                if let Some(cell) = self.stack.vm_frames[fi].loops.get_mut(*slot as usize) {
                    *cell = None;
                }
                self.stack.vm_frames[fi].loop_idx.pop();
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

            // --- in-place mutation (M4; escape analysis proved unique) ---
            Inst::SetFieldInPlace { dst, obj, field, val } => {
                let v = ops::set_field_in_place(
                    self.program,
                    self.heap,
                    *field,
                    self.reg(fi, *obj),
                    self.reg(fi, *val),
                )?;
                self.set(fi, *dst, v);
            }
            Inst::SetIndexInPlace { dst, obj, idx, val } => {
                let v = ops::set_index_in_place(
                    self.heap,
                    self.reg(fi, *obj),
                    self.reg(fi, *idx),
                    self.reg(fi, *val),
                )?;
                self.set(fi, *dst, v);
            }

            // --- calls / closures ---
            Inst::MakeClosure { dst, func, caps } => {
                let captures = self.regs(fi, caps);
                let cid = ClosureId(self.closures.len() as u32);
                self.closures.push(RuntimeClosure {
                    function_id: *func,
                    captures,
                });
                self.set(fi, *dst, Value::Closure(cid));
            }
            Inst::MakeOverloadSet { dst, closures } => {
                let inputs = self.regs(fi, closures);
                let v = calls::make_overload_set(
                    self.program,
                    self.closures,
                    self.overload_sets,
                    &inputs,
                );
                self.set(fi, *dst, v);
            }
            Inst::Call { dst, callee, args } => {
                let callable = self.reg(fi, *callee);
                let argv = self.regs(fi, args);
                self.do_call(fi, *dst, callable, &argv, origin)?;
            }
            Inst::MethodCall { dst, recv, name, args } => {
                let receiver = self.reg(fi, *recv);
                let argv = self.regs(fi, args);
                self.do_method_call(fi, *dst, receiver, *name, &argv, origin)?;
            }
            Inst::BuiltinCall { dst, name, args, in_place } => {
                let argv = self.regs(fi, args);
                self.do_builtin_call(fi, *dst, *name, &argv, *in_place)?;
            }
            Inst::Return { val } => {
                let value = val.map(|r| self.reg(fi, r)).unwrap_or(Value::Nil);
                return Ok(self.deliver_value(value));
            }

            // --- state ---
            Inst::StateRead { dst, base, in_loop } => {
                let key = self.state_key(*base, *in_loop, None);
                self.stack.touched_state_keys.insert(key.clone());
                let v = self.stack.state.get(&key).copied().unwrap_or(Value::Nil);
                self.set(fi, *dst, v);
            }
            Inst::StateWrite { dst, base, in_loop, val, key } => {
                let val_v = self.reg(fi, *val);
                let explicit = key.map(|r| self.reg(fi, r));
                let k = self.state_key(*base, *in_loop, explicit);
                self.stack.touched_state_keys.insert(k.clone());
                self.stack.state.insert(k, val_v);
                self.set(fi, *dst, val_v);
            }
            Inst::StateInit { dst, base, in_loop, after, key } => {
                let explicit = key.map(|r| self.reg(fi, r));
                let k = self.state_key(*base, *in_loop, explicit);
                self.stack.touched_state_keys.insert(k.clone());
                // Cache hit: load the slot and skip the inline init block; miss:
                // fall through to compute and commit the init value.
                if let Some(existing) = self.stack.state.get(&k).copied() {
                    self.set(fi, *dst, existing);
                    self.stack.vm_frames[fi].ip = *after as usize;
                }
            }

            // --- match ---
            Inst::MatchArm { subject, term, arm, next, dst: _ } => {
                let program = self.program;
                let bc = self.bc;
                let subj = self.reg(fi, *subject);
                let arms = program
                    .match_arms
                    .get(term)
                    .ok_or("Match: no arm metadata")?;
                let meta = &arms[*arm as usize];
                let mut binds = Vec::new();
                let matched =
                    crate::backend::pattern::match_pattern(&meta.pattern, subj, self.heap, &mut binds);
                if !matched {
                    self.stack.vm_frames[fi].ip = *next as usize;
                } else if let Some(bind_regs) = bc.match_binds.get(&(*term, *arm)) {
                    // Write each captured value into every register bound to
                    // that name in the arm body (mirrors apply_pattern_bindings).
                    for (name, val) in &binds {
                        for (n, reg) in bind_regs {
                            if n == name {
                                self.set(fi, *reg, *val);
                            }
                        }
                    }
                }
            }
            Inst::MatchFail { subject } => {
                let v = self.reg(fi, *subject);
                return Err(format!(
                    "No matching pattern for value: {}",
                    crate::value::value_to_display_string(&v, self.heap)
                ));
            }

            Inst::Error { msg } => {
                return Err(self
                    .program
                    .get_string_constant(*msg)
                    .unwrap_or("Unknown error")
                    .to_string());
            }
            // The instruction set is now fully implemented — no catch-all, so a
            // future `Inst` variant is a compile error until the VM handles it.
        }
        Ok(StepResult::Continue)
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

    // -- calls ---------------------------------------------------------------

    /// Dispatch `callable(args...)`, writing the result into `dst` of frame `fi`
    /// (closures push a frame that writes `dst` on return; native/enum results
    /// are written immediately).
    fn do_call(
        &mut self,
        fi: usize,
        dst: Reg,
        callable: Value,
        args: &[Value],
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        match callable {
            Value::Closure(_) | Value::OverloadSet(_) => {
                let cid = calls::resolve_callable(
                    self.program,
                    self.closures,
                    self.overload_sets,
                    callable,
                    args.len(),
                )?;
                self.push_closure_frame(cid, args, Some(dst), call_site)?;
            }
            Value::NativeFunction(nid) => {
                let v = self.call_native_or_intrinsic(nid, args)?;
                self.set(fi, dst, v);
            }
            // Calling a fieldless enum variant yields the variant itself.
            Value::EnumVariant { .. } if args.is_empty() => self.set(fi, dst, callable),
            _ => return Err(format!("Cannot call {}", callable.type_name())),
        }
        Ok(())
    }

    /// Method-call syntax `recv.name(args...)`: a callable field on a record
    /// receiver, else a native function with `recv` prepended to the args.
    fn do_method_call(
        &mut self,
        fi: usize,
        dst: Reg,
        recv: Value,
        name_cid: crate::constant_table::ConstantId,
        args: &[Value],
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        let method_name = match self.program.get_string_constant(name_cid) {
            Some(s) => s.to_string(),
            None => return Err("Invalid method name".into()),
        };

        // 1) Callable field on a record receiver.
        if let Value::Map(map_id) = recv {
            let field_val = self.heap.get_map(map_id).get(&method_name).copied();
            if let Some(field_val) = field_val {
                match field_val {
                    Value::Closure(_) | Value::OverloadSet(_) => {
                        return self.do_call(fi, dst, field_val, args, call_site);
                    }
                    Value::NativeFunction(nid) => {
                        let v = self.call_native_fn(nid, args)?;
                        self.set(fi, dst, v);
                        return Ok(());
                    }
                    _ => {} // not callable — fall through to method lookup
                }
            }
        }

        // 2) Native function with `recv` prepended.
        if let Some(nid) = self.native_fns.lookup_name(&method_name) {
            let mut full_args = vec![recv];
            full_args.extend_from_slice(args);
            let v = self.call_native_or_intrinsic(nid, &full_args)?;
            self.set(fi, dst, v);
            Ok(())
        } else {
            let hint = match method_name.as_str() {
                "toString" => Some("use str() or the str() method instead"),
                "log" => Some("use print() instead of console.log()"),
                "indexOf" => Some("use contains() to check membership"),
                "concat" => Some("use the ++ operator to concatenate lists or strings"),
                _ => None,
            };
            Err(match hint {
                Some(hint) => format!(
                    "No method '{}' on type {} — {}",
                    method_name,
                    recv.type_name(),
                    hint
                ),
                None => format!("No method '{}' on type {}", method_name, recv.type_name()),
            })
        }
    }

    /// Static builtin call `name(args...)` (unshadowed builtin called directly).
    fn do_builtin_call(
        &mut self,
        fi: usize,
        dst: Reg,
        name_cid: crate::constant_table::ConstantId,
        args: &[Value],
        in_place: bool,
    ) -> Result<(), String> {
        let name = match self.program.get_string_constant(name_cid) {
            Some(s) => s.to_string(),
            None => return Err("BuiltinCall: invalid name constant".into()),
        };
        let nid = match self.native_fns.lookup_name(&name) {
            Some(id) => id,
            None => return Err(format!("Unknown builtin: {}", name)),
        };
        // Mutating builtins (`append`/`set`/…) are never intrinsics, so the
        // in-place flag only reaches `call_native_fn`.
        let v = if in_place {
            self.call_native_fn_in_place(nid, args)?
        } else {
            self.call_native_or_intrinsic(nid, args)?
        };
        self.set(fi, dst, v);
        Ok(())
    }

    /// Push a closure activation record onto the frame stack. Mirrors the graph
    /// engine's `build_closure_frame`, but sizes and populates the *flat*
    /// register file using the lowered function's binding metadata.
    fn push_closure_frame(
        &mut self,
        cid: ClosureId,
        args: &[Value],
        dst: Option<Reg>,
        call_site: Option<TermId>,
    ) -> Result<(), String> {
        let bc = self.bc;
        let program = self.program;
        let closure = &self.closures[cid.0 as usize];
        let fn_id = closure.function_id;
        let captures = closure.captures.clone();

        let bcfn = bc.function(fn_id);
        let func = &program.functions[fn_id.0 as usize];
        if args.len() != func.params.len() {
            let name = func.name.as_deref().unwrap_or("<anonymous>");
            return Err(format!(
                "{}() expected {} argument{}, got {}",
                name,
                func.params.len(),
                if func.params.len() == 1 { "" } else { "s" },
                args.len()
            ));
        }

        let mut frame = VmFrame::new(Some(fn_id), bcfn.reg_count, dst, call_site);
        for (i, &preg) in bcfn.param_regs.iter().enumerate() {
            if let Some(slot) = frame.regs.get_mut(preg as usize) {
                *slot = args[i];
            }
        }
        for (i, &creg) in bcfn.capture_regs.iter().enumerate() {
            if let (Some(slot), Some(cap)) = (frame.regs.get_mut(creg as usize), captures.get(i)) {
                *slot = *cap;
            }
        }
        if let Some(sreg) = bcfn.self_ref_reg {
            if let Some(slot) = frame.regs.get_mut(sreg as usize) {
                *slot = Value::Closure(cid);
            }
        }
        self.stack.vm_frames.push(frame);
        Ok(())
    }

    // -- native dispatch -----------------------------------------------------

    /// Dispatch a native function, handling the higher-order intrinsics
    /// specially (they call closures synchronously).
    fn call_native_or_intrinsic(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        let nf = self.native_fns;
        if nf.intrinsic_map == Some(nid) {
            self.builtin_map(args)
        } else if nf.intrinsic_filter == Some(nid) {
            self.builtin_filter(args)
        } else if nf.intrinsic_reduce == Some(nid) {
            self.builtin_reduce(args)
        } else if nf.intrinsic_for_each == Some(nid) {
            self.builtin_for_each(args)
        } else {
            self.call_native_fn(nid, args)
        }
    }

    /// Call a non-intrinsic native function via `PetalCxt` (clone-and-alloc).
    fn call_native_fn(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        self.call_native_fn_flagged(nid, args, false)
    }

    /// Call a non-intrinsic native function marked in-place: a mutating builtin
    /// (`append`/`set`/…) may reuse its container argument's backing store.
    /// Only reached when escape analysis proved the container unique +
    /// non-escaping (M4).
    fn call_native_fn_in_place(&mut self, nid: NativeFnId, args: &[Value]) -> Result<Value, String> {
        self.call_native_fn_flagged(nid, args, true)
    }

    fn call_native_fn_flagged(
        &mut self,
        nid: NativeFnId,
        args: &[Value],
        in_place: bool,
    ) -> Result<Value, String> {
        let func = self.native_fns.get_func(nid);
        let mut cxt = PetalCxt::new(
            args,
            self.heap,
            self.output,
            self.symbols,
            self.output_buffers,
            self.bindings,
            self.counters,
        );
        cxt.set_in_place(in_place);
        let count = func(&mut cxt)?;
        let results = cxt.take_results();
        Ok(if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        })
    }

    // -- higher-order intrinsics ---------------------------------------------

    /// Call a closure synchronously: push its frame, step until it pops, and
    /// return its result. Mirrors the graph engine's `call_closure_sync`.
    fn call_closure_sync(&mut self, callable: Value, call_args: &[Value]) -> Result<Value, String> {
        let cid = calls::resolve_callable(
            self.program,
            self.closures,
            self.overload_sets,
            callable,
            call_args.len(),
        )?;
        let target_depth = self.stack.vm_frames.len();
        self.push_closure_frame(cid, call_args, None, None)?;
        self.stack.last_pop_result = None;

        loop {
            if self.stack.vm_frames.len() <= target_depth {
                return Ok(self.stack.last_pop_result.take().unwrap_or(Value::Nil));
            }
            match self.step() {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v),
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    fn builtin_map(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("map() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("map() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut results = Vec::with_capacity(elements.len());
        for elem in elements {
            results.push(self.call_closure_sync(*func, &[elem])?);
        }
        Ok(Value::List(self.heap.alloc_list(results)))
    }

    fn builtin_filter(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("filter() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("filter() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut results = Vec::new();
        for elem in elements {
            if self.call_closure_sync(*func, &[elem])?.is_truthy() {
                results.push(elem);
            }
        }
        Ok(Value::List(self.heap.alloc_list(results)))
    }

    fn builtin_reduce(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, initial, func] = args else {
            return Err("reduce() expects 3 arguments (list, initial, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("reduce() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        let mut acc = *initial;
        for elem in elements {
            acc = self.call_closure_sync(*func, &[acc, elem])?;
        }
        Ok(acc)
    }

    fn builtin_for_each(&mut self, args: &[Value]) -> Result<Value, String> {
        let [list, func] = args else {
            return Err("forEach() expects 2 arguments (list, function)".into());
        };
        let Value::List(list_id) = *list else {
            return Err("forEach() expects a list as first argument".into());
        };
        let elements = self.heap.get_list(list_id).to_vec();
        for elem in elements {
            self.call_closure_sync(*func, &[elem])?;
        }
        Ok(Value::Nil)
    }
}

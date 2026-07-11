//! The executor core: `exec_inst` (the per-[`Inst`] dispatch match) and the
//! arithmetic helpers it calls.
//!
//! Split out of `vm/mod.rs`; see that module for the [`Vm`] struct and the
//! core step loop.

use super::*;

use super::super::isa::Inst;
use crate::backend::{calls, ops};
use crate::program::{ClosureId, TermOp};
use crate::stack::LoopKeyPart;

impl<'a> Vm<'a> {
    pub(super) fn exec_inst(
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
            Inst::JumpIfPresent { cond, to } => {
                if self.reg(fi, *cond).is_present() {
                    self.stack.vm_frames[fi].ip = *to as usize;
                }
            }
            Inst::JumpIfPending { cond, to } => {
                if matches!(self.reg(fi, *cond), Value::Pending(_)) {
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

            Inst::Add { dst, a, b } => self.binop(fi, TermOp::Add, *dst, *a, *b, origin)?,
            Inst::Sub { dst, a, b } => self.binop(fi, TermOp::Sub, *dst, *a, *b, origin)?,
            Inst::Mul { dst, a, b } => self.binop(fi, TermOp::Mul, *dst, *a, *b, origin)?,
            Inst::Div { dst, a, b } => self.binop(fi, TermOp::Div, *dst, *a, *b, origin)?,
            Inst::Mod { dst, a, b } => self.binop(fi, TermOp::Mod, *dst, *a, *b, origin)?,
            Inst::Neg { dst, a } => {
                let v = ops::negate(self.reg(fi, *a))?;
                self.note_absorption(v, origin);
                self.set(fi, *dst, v);
            }

            Inst::Eq { dst, a, b } => {
                let v = ops::eq(self.reg(fi, *a), self.reg(fi, *b), self.heap);
                self.note_absorption(v, origin);
                self.set(fi, *dst, v);
            }
            Inst::Ne { dst, a, b } => {
                let v = ops::ne(self.reg(fi, *a), self.reg(fi, *b), self.heap);
                self.note_absorption(v, origin);
                self.set(fi, *dst, v);
            }
            Inst::Lt { dst, a, b } => self.cmp(fi, TermOp::Lt, *dst, *a, *b, origin)?,
            Inst::Le { dst, a, b } => self.cmp(fi, TermOp::Le, *dst, *a, *b, origin)?,
            Inst::Gt { dst, a, b } => self.cmp(fi, TermOp::Gt, *dst, *a, *b, origin)?,
            Inst::Ge { dst, a, b } => self.cmp(fi, TermOp::Ge, *dst, *a, *b, origin)?,

            Inst::Not { dst, a } => {
                let v = ops::not(self.reg(fi, *a));
                self.note_absorption(v, origin);
                self.set(fi, *dst, v);
            }
            Inst::Concat { dst, a, b } => {
                let v = ops::concat(self.reg(fi, *a), self.reg(fi, *b), self.heap)?;
                self.note_absorption(v, origin);
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
                let base = self.reg(fi, *obj);
                let v = ops::get_field(self.program, self.heap, *field, base)?;
                // Absorption gates on the BASE: `pending.field` yields that same
                // Pending. A resolved record that merely holds a Pending field is
                // element-wise (not an absorption), so the result value can't be
                // the signal here.
                self.note_absorption(base, origin);
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
                let base = self.reg(fi, *obj);
                let v = ops::get_index(self.heap, base, self.reg(fi, *idx))?;
                // Absorption gates on the BASE (`pending[i]` is that Pending); a
                // resolved collection holding a Pending element is element-wise.
                // A Pending *index* is a hard error above and never reaches here.
                self.note_absorption(base, origin);
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
                let captures = self.regs(fi, caps).into_vec();
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
                self.do_builtin_call(fi, *dst, *name, &argv, *in_place, origin)?;
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
            Inst::StateWrite { dst, base, in_loop, val, key, init } => {
                let val_v = self.reg(fi, *val);
                let explicit = key.map(|r| self.reg(fi, r));
                let k = self.state_key(*base, *in_loop, explicit);
                self.stack.touched_state_keys.insert(k.clone());
                // A pending StateInit result is not committed: leave the slot
                // uninitialized so the init block re-runs next frame until it
                // resolves. Reads this frame still see the Pending (via `dst`).
                // Ordinary reassignments (`init = false`) commit any value.
                if !(*init && matches!(val_v, Value::Pending(_))) {
                    self.stack.state.insert(k, val_v);
                }
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

    fn binop(&mut self, fi: usize, op: TermOp, dst: Reg, a: Reg, b: Reg, origin: Option<TermId>) -> Result<(), String> {
        let v = ops::arithmetic(&op, self.reg(fi, a), self.reg(fi, b), self.heap)?;
        self.note_absorption(v, origin);
        self.set(fi, dst, v);
        Ok(())
    }

    fn cmp(&mut self, fi: usize, op: TermOp, dst: Reg, a: Reg, b: Reg, origin: Option<TermId>) -> Result<(), String> {
        let v = ops::comparison(&op, self.reg(fi, a), self.reg(fi, b), self.heap)?;
        self.note_absorption(v, origin);
        self.set(fi, dst, v);
        Ok(())
    }

    /// Record a strict-operator absorption: when an operator's result is a
    /// `Pending`, an operand was `Pending` and the op swallowed it, so bump the
    /// always-on `absorbed_count` (the plan's steady-state observability
    /// counter) and, when the debug-gated log is on, push `(origin, id)` to
    /// [`absorption_log`](Vm::absorption_log) — the span→resource pair a dataflow
    /// viz paints. A no-op on any resolved result. The value-producing ops in
    /// `ops.rs` (arithmetic, comparisons, `!`/negate, `++`) fabricate a `Pending`
    /// only by absorption, so gating on the result is exact for them; field /
    /// index access instead gate on the *base* (a resolved container may merely
    /// store a Pending element, which is not an absorption). `origin` is the
    /// absorbing instruction's source term, `None` when it has no source span.
    pub(super) fn note_absorption(&mut self, v: Value, origin: Option<TermId>) {
        if let Value::Pending(id) = v {
            self.resources.note_absorbed(id);
            if self.trace_pending {
                self.absorption_log.push((origin, id));
            }
        }
    }
}

//! The straight-line fast path: [`Vm::run_straight`] executes the common,
//! uneventful instructions of one frame in a tight loop.
//!
//! [`Vm::step_in`] is the general executor, and it pays for its generality on
//! every instruction: it re-reads the frame from `vm_frames` (a bounds check),
//! loads and stores `ip` through memory, looks up the instruction's source
//! origin, returns a `Result<StepResult, String>` and has the batch loop check
//! the GC budget. This loop instead keeps `ip`, the register file and the code
//! in locals for the whole run and handles only the *happy path* of the
//! hottest instructions — constants, moves, jumps, loop steps, number
//! arithmetic and comparisons, field and index reads.
//!
//! Anything else stops the run with `ip` still pointing at the instruction,
//! and the caller hands it to `step_in` unchanged. That covers every
//! instruction not listed here (calls, returns, allocation, loop setup,
//! state, cells) and every unusual case of one that is: a `Pending` operand (so
//! absorption is noted by `exec_inst`), an error (so it is annotated with the
//! origin, computed only then), an integer overflow, a register outside the
//! file. Since the fast arms have no side effects before they commit, running
//! the instruction again in `step_in` is exact: this loop never changes what a
//! program does, only how quickly the easy instructions retire.
//!
//! The GC budget is checked at backward jumps: the fast arms allocate rarely
//! (a string constant not yet interned, a vec3 or list result of arithmetic),
//! and straight-line code between back-edges is bounded by the code length.

use super::*;

use crate::backend::ops;
use crate::constant_table::ConstantValue;
use crate::program::TermOp;
use crate::stack::PathPart;

/// Why [`Vm::run_straight`] stopped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StraightStop {
    /// The instruction at `ip` needs the general executor (or `ip` is past the
    /// end of the code and the frame has to finish).
    Slow,
    /// The instruction budget ran out.
    Budget,
    /// The heap wants a collection.
    Collect,
}

impl<'a> Vm<'a> {
    /// Run the frame `fi` (whose function is `func`) through its fast
    /// instructions, at most `budget` of them. Returns why it stopped and how
    /// many instructions retired. Only valid with the per-instruction hooks
    /// off, since nothing here traces, observes or profiles.
    #[inline(never)]
    pub(super) fn run_straight(
        &mut self,
        fi: usize,
        func: &'a BytecodeFn,
        budget: u64,
    ) -> (StraightStop, u64) {
        let program = self.program;
        let code: &'a [Inst] = &func.code;
        let frame = &mut self.stack.vm_frames[fi];
        let mut ip = frame.ip;
        let regs: &mut [Value] = &mut frame.regs;
        let loops = &mut frame.loops;
        let path = &mut frame.path;
        let heap = &mut *self.heap;
        let mut n: u64 = 0;

        // Registers are read as `Nil` past the end of the file, like
        // `Vm::reg`; a write past the end stops the run so `Vm::set` can grow
        // the file.
        macro_rules! get {
            ($r:expr) => {
                match regs.get($r as usize) {
                    Some(v) => *v,
                    None => Value::Nil,
                }
            };
        }
        macro_rules! put {
            ($r:expr, $v:expr) => {{
                let v = $v;
                match regs.get_mut($r as usize) {
                    Some(slot) => *slot = v,
                    None => break StraightStop::Slow,
                }
            }};
        }

        let stop = loop {
            if n >= budget {
                break StraightStop::Budget;
            }
            let Some(inst) = code.get(ip) else {
                break StraightStop::Slow;
            };
            match *inst {
                Inst::LoadConst { dst, k } => {
                    let v = match program.constants.get(k) {
                        ConstantValue::Float(bits) => Value::Float(f64::from_bits(*bits)),
                        ConstantValue::Int(i) => Value::Int(*i),
                        ConstantValue::Bool(b) => Value::Bool(*b),
                        ConstantValue::Nil => Value::Nil,
                        ConstantValue::String(_) => ops::constant_to_value(program, heap, k),
                    };
                    put!(dst, v);
                }
                Inst::LoadNil { dst } => put!(dst, Value::Nil),
                Inst::LoadBool { dst, val } => put!(dst, Value::Bool(val)),
                Inst::Move { dst, src } => put!(dst, get!(src)),

                Inst::Jump { to } => {
                    n += 1;
                    let back = (to as usize) <= ip;
                    ip = to as usize;
                    if back && heap.should_collect() {
                        break StraightStop::Collect;
                    }
                    continue;
                }
                // `if` and `while` test their condition with a JumpIfPending
                // and then a JumpIfFalse on the same register: take both in
                // one dispatch, still counted as two instructions.
                Inst::JumpIfPending { cond, to } => {
                    let v = get!(cond);
                    if matches!(v, Value::Pending(_)) {
                        n += 1;
                        ip = to as usize;
                        continue;
                    }
                    if let Some(&Inst::JumpIfFalse { cond: c2, to: t2 }) = code.get(ip + 1)
                        && c2 == cond
                    {
                        n += 2;
                        ip = if v.is_truthy() { ip + 2 } else { t2 as usize };
                        continue;
                    }
                }
                Inst::JumpIfFalse { cond, to } => {
                    if !get!(cond).is_truthy() {
                        n += 1;
                        ip = to as usize;
                        continue;
                    }
                }
                Inst::JumpIfTrue { cond, to } => {
                    if get!(cond).is_truthy() {
                        n += 1;
                        ip = to as usize;
                        continue;
                    }
                }
                Inst::JumpIfPresent { cond, to } => {
                    if get!(cond).is_present() {
                        n += 1;
                        ip = to as usize;
                        continue;
                    }
                }

                // Loop cursors, as in `exec_inst`: the next element (or the
                // exit), and the innermost loop's `Index` path part.
                Inst::ForEachNext { slot, var, exit } => {
                    let Some(Some(LoopCursor::ForEach { elems, i, .. })) =
                        loops.get_mut(slot as usize)
                    else {
                        break StraightStop::Slow;
                    };
                    if *i >= elems.len() {
                        n += 1;
                        ip = exit as usize;
                        continue;
                    }
                    let (e, idx) = (elems[*i], *i);
                    *i += 1;
                    put!(var, e);
                    set_loop_idx(path, idx);
                }
                Inst::RangeNext { slot, var, exit } => {
                    let Some(Some(LoopCursor::Range { cur, end, iter, .. })) =
                        loops.get_mut(slot as usize)
                    else {
                        break StraightStop::Slow;
                    };
                    if *cur >= *end {
                        n += 1;
                        ip = exit as usize;
                        continue;
                    }
                    let (v, it) = (*cur, *iter);
                    *cur += 1;
                    *iter += 1;
                    put!(var, Value::Int(v));
                    set_loop_idx(path, it);
                }

                Inst::Add { dst, a, b } => {
                    let v = match (get!(a), get!(b)) {
                        (Value::Float(x), Value::Float(y)) => Value::Float(x + y),
                        (Value::Int(x), Value::Int(y)) => match x.checked_add(y) {
                            Some(z) => Value::Int(z),
                            None => break StraightStop::Slow,
                        },
                        (x, y) => match arith(TermOp::Add, x, y, heap) {
                            Some(v) => v,
                            None => break StraightStop::Slow,
                        },
                    };
                    put!(dst, v);
                }
                Inst::Sub { dst, a, b } => {
                    let v = match (get!(a), get!(b)) {
                        (Value::Float(x), Value::Float(y)) => Value::Float(x - y),
                        (Value::Int(x), Value::Int(y)) => match x.checked_sub(y) {
                            Some(z) => Value::Int(z),
                            None => break StraightStop::Slow,
                        },
                        (x, y) => match arith(TermOp::Sub, x, y, heap) {
                            Some(v) => v,
                            None => break StraightStop::Slow,
                        },
                    };
                    put!(dst, v);
                }
                Inst::Mul { dst, a, b } => {
                    let v = match (get!(a), get!(b)) {
                        (Value::Float(x), Value::Float(y)) => Value::Float(x * y),
                        (Value::Int(x), Value::Int(y)) => match x.checked_mul(y) {
                            Some(z) => Value::Int(z),
                            None => break StraightStop::Slow,
                        },
                        (x, y) => match arith(TermOp::Mul, x, y, heap) {
                            Some(v) => v,
                            None => break StraightStop::Slow,
                        },
                    };
                    put!(dst, v);
                }
                Inst::Neg { dst, a } => {
                    let v = match get!(a) {
                        Value::Float(x) => Value::Float(-x),
                        Value::Int(x) => match x.checked_neg() {
                            Some(z) => Value::Int(z),
                            None => break StraightStop::Slow,
                        },
                        _ => break StraightStop::Slow,
                    };
                    put!(dst, v);
                }
                Inst::Div { dst, a, b } => match arith(TermOp::Div, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },
                Inst::Mod { dst, a, b } => match arith(TermOp::Mod, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },

                Inst::Lt { dst, a, b } => match compare(TermOp::Lt, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },
                Inst::Le { dst, a, b } => match compare(TermOp::Le, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },
                Inst::Gt { dst, a, b } => match compare(TermOp::Gt, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },
                Inst::Ge { dst, a, b } => match compare(TermOp::Ge, get!(a), get!(b), heap) {
                    Some(v) => put!(dst, v),
                    None => break StraightStop::Slow,
                },
                Inst::Eq { dst, a, b } => {
                    let (x, y) = (get!(a), get!(b));
                    if is_pending(x) || is_pending(y) {
                        break StraightStop::Slow;
                    }
                    put!(dst, ops::eq(x, y, heap));
                }
                Inst::Ne { dst, a, b } => {
                    let (x, y) = (get!(a), get!(b));
                    if is_pending(x) || is_pending(y) {
                        break StraightStop::Slow;
                    }
                    put!(dst, ops::ne(x, y, heap));
                }
                Inst::Not { dst, a } => {
                    let x = get!(a);
                    if is_pending(x) {
                        break StraightStop::Slow;
                    }
                    put!(dst, Value::Bool(!x.is_truthy()));
                }

                Inst::GetField {
                    dst,
                    obj,
                    field,
                    opt,
                } => {
                    let base = get!(obj);
                    if is_pending(base) {
                        break StraightStop::Slow;
                    }
                    match ops::get_field(program, heap, field, base, opt) {
                        Ok(v) => put!(dst, v),
                        Err(_) => break StraightStop::Slow,
                    }
                }
                Inst::GetIndex { dst, obj, idx, opt } => {
                    let base = get!(obj);
                    if is_pending(base) {
                        break StraightStop::Slow;
                    }
                    match ops::get_index(heap, base, get!(idx), opt) {
                        Ok(v) => put!(dst, v),
                        Err(_) => break StraightStop::Slow,
                    }
                }

                _ => break StraightStop::Slow,
            }
            // A conditional jump that was not taken, or any non-jump arm.
            n += 1;
            ip += 1;
        };

        self.stack.vm_frames[fi].ip = ip;
        self.stack.insts += n;
        (stop, n)
    }
}

/// [`Vm::set_loop_idx_top`] on a borrowed path.
#[inline(always)]
fn set_loop_idx(path: &mut super::frame::LocalPath, idx: usize) {
    if let Some(last @ PathPart::Index(_)) = path.last_mut() {
        *last = PathPart::Index(idx);
    }
}

#[inline(always)]
fn is_pending(v: Value) -> bool {
    matches!(v, Value::Pending(_))
}

/// The general arithmetic for operands the inline arms did not take, when
/// neither is `Pending` (absorption belongs to `exec_inst`). `None` on an
/// error, which the general executor then reproduces and annotates.
#[inline(always)]
fn arith(op: TermOp, a: Value, b: Value, heap: &mut Heap) -> Option<Value> {
    if is_pending(a) || is_pending(b) {
        return None;
    }
    ops::arithmetic(&op, a, b, heap).ok()
}

/// Ordering comparisons, inline for two ints or two floats; otherwise the
/// general comparison when neither operand is `Pending`. `None` on an error.
#[inline(always)]
fn compare(op: TermOp, a: Value, b: Value, heap: &Heap) -> Option<Value> {
    let r = match (a, b) {
        (Value::Float(x), Value::Float(y)) => match op {
            TermOp::Lt => x < y,
            TermOp::Le => x <= y,
            TermOp::Gt => x > y,
            _ => x >= y,
        },
        (Value::Int(x), Value::Int(y)) => match op {
            TermOp::Lt => x < y,
            TermOp::Le => x <= y,
            TermOp::Gt => x > y,
            _ => x >= y,
        },
        _ => {
            if is_pending(a) || is_pending(b) {
                return None;
            }
            return ops::comparison(&op, a, b, heap).ok();
        }
    };
    Some(Value::Bool(r))
}

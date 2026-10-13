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
//! arithmetic and comparisons, field and index reads, in-place index writes,
//! and reads and writes of a `var` cell that memoization has no interest in.
//!
//! Anything else stops the run with `ip` still pointing at the instruction,
//! and the caller hands it to `step_in` unchanged. That covers every
//! instruction not listed here (calls, returns, allocation, loop setup,
//! state) and every unusual case of one that is: a `Pending` operand (so
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

use super::super::isa::{CellReadMode, CellWriteMode, IntrinsicOp};
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
        // A cell access is a memo dependency only while a scope is recording,
        // and then only for a cell the scope did not create itself (see
        // `memo_note_cell_read` and its siblings). Neither changes during a
        // straight run: scopes open and close at calls and cells are created
        // by `CellNew`, all of which stop it. `Some` holds the cells this run
        // may touch without telling the recorder; `None` means any.
        let memo_scope = if self.memo && self.stack.memo.recording() {
            self.stack.memo.open.last()
        } else {
            None
        };
        let memo_locals = memo_scope.map(|s| &s.local_cells);
        let recording = memo_locals.is_some();
        // A field read on a variant is a dependency on the stack's enum data,
        // which one entry covers for a whole run of reads (see
        // `memo_note_enum_read`). Only the read that has to add the entry
        // goes to the general path; nothing else is recorded during a
        // straight run, so the answer holds until this one stops.
        let enum_read_noted = memo_scope.is_none_or(|s| {
            s.effectful || matches!(s.deps.last(), Some(crate::memo::Dep::EnumData { .. }))
        });
        let frame = &mut self.stack.vm_frames[fi];
        let result_dropped = frame.result_dropped;
        let mut ip = frame.ip;
        let regs: &mut [Value] = &mut frame.regs;
        let loops = &mut frame.loops;
        let path = &mut frame.path;
        let heap = &mut *self.heap;
        let closures = &*self.closures;
        let run_deps = &mut self.stack.run_deps;
        let enums = &mut self.stack.enums;
        let bc = self.bc;
        // The two enum builtins this loop can retire. Publishing data is an
        // effect a recording scope has to note, so it is not offered then.
        let enum_builtins = EnumBuiltins {
            all: self.native_fns.intrinsic_enum_all.map(|id| id.0),
            data: match recording {
                false => self.native_fns.intrinsic_enum_data.map(|id| id.0),
                true => None,
            },
        };
        // An inline builtin is computed here only when nothing is watching
        // native calls: the two audits bracket each one, and take the
        // general path (the profiler and the trace never reach this loop).
        let intrinsics = !self.audit.enabled && !self.static_audit.enabled;
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
                // The enum work this loop retires is kept out of line (see
                // `straight_enum_variant` and its neighbours below): each is
                // a few loads behind a call, and inlined here they cost every
                // other instruction more than they save these.
                //
                // `Enum.variant` with no payload.
                Inst::MakeEnumVariant { dst, name, ref fields } if fields.is_empty() => {
                    let Some(v) = straight_enum_variant(bc, enums, heap, name) else {
                        break StraightStop::Slow;
                    };
                    put!(dst, v);
                }
                // `Enum.all` and `__enum_data`, in the cases that are a table
                // read or a table write with nothing to record. Every other
                // builtin call is the general path's.
                Inst::BuiltinCall {
                    dst,
                    name,
                    ref args,
                    in_place: false,
                    ref arg_names,
                } => {
                    let v = if arg_names.is_empty() {
                        straight_enum_builtin(bc, enums, heap, enum_builtins, name, args, regs)
                    } else {
                        None
                    };
                    let Some(v) = v else {
                        break StraightStop::Slow;
                    };
                    put!(dst, v);
                }
                // The trivial builtins, for the arguments that need no
                // native: see `straight_intrinsic`. The conversions are
                // matched here so `float(i)` is a load, a test and a store.
                Inst::Intrinsic { dst, op, args, .. } => {
                    if !intrinsics {
                        break StraightStop::Slow;
                    }
                    let a = get!(args[0]);
                    let v = match (op, a) {
                        (IntrinsicOp::Float, Value::Int(i)) => Value::Float(i as f64),
                        (IntrinsicOp::Float, Value::Float(_)) => a,
                        (IntrinsicOp::Int, Value::Int(_)) => a,
                        (IntrinsicOp::Int, Value::Float(f)) => Value::Int(f as i64),
                        _ => match straight_intrinsic(heap, op, a, get!(args[1]), get!(args[2])) {
                            Some(v) => v,
                            None => break StraightStop::Slow,
                        },
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
                    ref cache,
                } => {
                    let base = get!(obj);
                    if let Some(v) = ops::cached_field(program, heap, cache, field, base) {
                        put!(dst, v);
                        n += 1;
                        ip += 1;
                        continue;
                    }
                    // A variant's field may be constant data, which a
                    // recording scope notes as a dependency on the slow path.
                    if is_pending(base) {
                        break StraightStop::Slow;
                    }
                    if matches!(base, Value::EnumVariant { .. }) {
                        if !enum_read_noted {
                            break StraightStop::Slow;
                        }
                        if let Some(v) = straight_variant_field(program, heap, enums, cache, field, base) {
                            put!(dst, v);
                            n += 1;
                            ip += 1;
                            continue;
                        }
                    }
                    match ops::get_field(program, heap, enums, field, base, opt) {
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

                Inst::SetIndexInPlace { dst, obj, idx, val } => {
                    if dst as usize >= regs.len() {
                        break StraightStop::Slow;
                    }
                    let (obj_v, idx_v, val_v) = (get!(obj), get!(idx), get!(val));
                    // The frame gate's question, asked before the store (see
                    // `RunDeps::note_in_place_store`); nothing once the run is
                    // already unsettled.
                    let keeps = run_deps.wants_in_place_stores()
                        && ops::index_store_keeps(heap, closures, obj_v, idx_v, val_v);
                    run_deps.note_in_place_store(None);
                    // Writing the same element twice is harmless, so an error
                    // here (reported by the general executor, which runs the
                    // instruction again) has nothing to undo.
                    match ops::set_index_in_place(heap, obj_v, idx_v, val_v) {
                        Ok(v) => {
                            run_deps.note_in_place_store(keeps.then_some(v));
                            put!(dst, v)
                        }
                        Err(_) => break StraightStop::Slow,
                    }
                }

                // A `var` cell, when the recorder does not need to hear of it.
                // What is left to the general executor: a cell some open memo
                // scope depends on, a write that might carry a scope's own
                // cell out of it, and the fingerprint the frame gate wants
                // before a `state var` is first mutated in a run.
                Inst::CellRead { dst, cell, mode } => {
                    let Value::Cell(id) = get!(cell) else {
                        break StraightStop::Slow;
                    };
                    if memo_locals.is_some_and(|locals| !locals.contains(&id))
                        || dst as usize >= regs.len()
                    {
                        break StraightStop::Slow;
                    }
                    let v = match mode {
                        CellReadMode::Peek => heap.cell_read(id),
                        CellReadMode::Shared => heap.cell_read_shared(id),
                        CellReadMode::Take => {
                            if heap.cell_wants_fingerprint(id) {
                                break StraightStop::Slow;
                            }
                            heap.cell_take(id)
                        }
                    };
                    put!(dst, v);
                }
                Inst::CellWrite {
                    dst,
                    cell,
                    val,
                    mode,
                } => {
                    let Value::Cell(id) = get!(cell) else {
                        break StraightStop::Slow;
                    };
                    if dst as usize >= regs.len() {
                        break StraightStop::Slow;
                    }
                    let v = get!(val);
                    let keeps = match mode {
                        // The recorder inspects every value stored this way.
                        CellWriteMode::Shared => {
                            if recording {
                                break StraightStop::Slow;
                            }
                            heap.cell_write(id, v);
                            false
                        }
                        CellWriteMode::Put | CellWriteMode::PutTail => {
                            if memo_locals.is_some_and(|locals| !locals.contains(&id)) {
                                break StraightStop::Slow;
                            }
                            let keeps = mode == CellWriteMode::Put || result_dropped;
                            heap.cell_put(id, v, keeps);
                            keeps
                        }
                    };
                    // As in `exec_inst`, with observation known to be off.
                    put!(dst, if keeps { Value::Nil } else { v });
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

// ── Inline builtins ──────────────────────────────────────────────────────

/// What a trivial builtin returns for arguments it needs no native for:
/// numbers (`Int` and `Float` only), a list, a string or a float array for
/// `len`, and anything but a `Pending` for `type`. `None` is everything else
/// — a wrong type, a dual number, a string to convert, an overflow, a
/// `Pending` — which the general executor runs as the builtin call, so each
/// arm here has to agree with its native only where it answers. `b` and `c`
/// are the second and third arguments where the builtin has them.
///
/// The natives are `crate::builtins::math::{native_float, native_int,
/// native_abs, native_floor, native_min, native_max}`,
/// `creative_coding::native_clamp`, `collections::native_len` and
/// `io::native_type`; `core/tests/intrinsics.rs` holds the two to each other
/// over a matrix of values.
#[inline]
fn straight_intrinsic(
    heap: &mut Heap,
    op: IntrinsicOp,
    a: Value,
    b: Value,
    c: Value,
) -> Option<Value> {
    Some(match op {
        IntrinsicOp::Float => match a {
            Value::Int(i) => Value::Float(i as f64),
            Value::Float(_) => a,
            _ => return None,
        },
        IntrinsicOp::Int => match a {
            Value::Int(_) => a,
            Value::Float(f) => Value::Int(f as i64),
            _ => return None,
        },
        IntrinsicOp::Abs => match a {
            Value::Int(i) => Value::Int(i.checked_abs()?),
            Value::Float(f) => Value::Float(f.abs()),
            _ => return None,
        },
        IntrinsicOp::Floor => match a {
            Value::Int(_) => a,
            Value::Float(f) => Value::Float(f.floor()),
            _ => return None,
        },
        // The first argument on a tie and on an unordered (NaN) pair, and the
        // chosen argument as it was passed: `min(1, 2.0)` is the int 1.
        IntrinsicOp::Min => match (a, b) {
            (Value::Int(x), Value::Int(y)) => if x <= y { a } else { b },
            (Value::Float(x), Value::Float(y)) => if x > y { b } else { a },
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                match crate::value::compare_values(&a, &b, heap).ok()? {
                    std::cmp::Ordering::Greater => b,
                    _ => a,
                }
            }
            _ => return None,
        },
        IntrinsicOp::Max => match (a, b) {
            (Value::Int(x), Value::Int(y)) => if x >= y { a } else { b },
            (Value::Float(x), Value::Float(y)) => if x < y { b } else { a },
            (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
                match crate::value::compare_values(&a, &b, heap).ok()? {
                    std::cmp::Ordering::Less => b,
                    _ => a,
                }
            }
            _ => return None,
        },
        // An int only when all three are; one float makes it a float.
        IntrinsicOp::Clamp => {
            if let (Value::Int(v), Value::Int(lo), Value::Int(hi)) = (a, b, c) {
                return Some(Value::Int(v.max(lo).min(hi)));
            }
            let num = |v: Value| match v {
                Value::Float(f) => Some(f),
                Value::Int(i) => Some(i as f64),
                _ => None,
            };
            Value::Float(num(a)?.max(num(b)?).min(num(c)?))
        }
        IntrinsicOp::Len => Value::Int(match a {
            Value::List(id) => heap.list_len(id),
            Value::String(id) => heap.string_char_len(id),
            Value::F64Array(id) => heap.f64_array_len(id),
            _ => return None,
        } as i64),
        IntrinsicOp::Type => match a {
            Value::Pending(_) => return None,
            _ => Value::String(crate::builtins::type_name_id(heap, a)),
        },
    })
}

// ── Enums ────────────────────────────────────────────────────────────────
//
// What the straight-line loop does for enums, out of line. Each is the hit
// path of something the general executor also does (and does on a miss):
// `None` sends the instruction there.

/// The native ids of the enum builtins [`straight_enum_builtin`] may retire.
#[derive(Clone, Copy)]
struct EnumBuiltins {
    /// `__enum_all`.
    all: Option<u32>,
    /// `__enum_data`, unless a memo scope is recording.
    data: Option<u32>,
}

/// `Enum.variant` with no payload, for a variant the program declares: its
/// tag is in the stack's table, at the slot the name constant resolved to,
/// and its payload is the heap's one empty list.
#[inline(never)]
fn straight_enum_variant(
    bc: &BytecodeProgram,
    enums: &crate::enum_table::EnumTable,
    heap: &mut crate::heap::Heap,
    name: crate::constant_table::ConstantId,
) -> Option<Value> {
    let tag = enums.tag_at(bc.variant_slot(name)?)?;
    let data = heap.empty_payload();
    Some(Value::EnumVariant { tag, data })
}

/// A constant-data read on a variant ([`ops::cached_variant_field`]).
#[inline(never)]
fn straight_variant_field(
    program: &Program,
    heap: &crate::heap::Heap,
    enums: &crate::enum_table::EnumTable,
    cache: &crate::record::FieldCache,
    field: crate::constant_table::ConstantId,
    base: Value,
) -> Option<Value> {
    ops::cached_variant_field(program, heap, enums, cache, field, base)
}

/// `Enum.all` once the stack holds the enum's list (the first ask builds it,
/// on the general path), and `__enum_data` publishing what the run before
/// published, value for value — every variant of every run of a program
/// whose data is constant: the new record takes the slot and the stack's
/// enum-data revision stays where it is.
#[inline(never)]
fn straight_enum_builtin(
    bc: &BytecodeProgram,
    enums: &mut crate::enum_table::EnumTable,
    heap: &crate::heap::Heap,
    builtins: EnumBuiltins,
    name: crate::constant_table::ConstantId,
    args: &[Reg],
    regs: &[Value],
) -> Option<Value> {
    let id = bc.builtin_id(name)?;
    let reg = |r: Reg| regs.get(r as usize).copied().unwrap_or(Value::Nil);
    match *args {
        [name] if builtins.all == Some(id) => match reg(name) {
            Value::String(name) => enums.all_cached(name).map(Value::List),
            _ => None,
        },
        [tag, record] if builtins.data == Some(id) => match (reg(tag), reg(record)) {
            (Value::String(tag), Value::Map(record)) => {
                enums.republish(heap, tag, record).then_some(Value::Nil)
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod intrinsic_tests {
    use super::*;

    /// The inline arms answer for numbers (and `len`, `type`), so the calls
    /// they stand for are not made; `core/tests/intrinsics.rs` holds the
    /// answers to the natives'. This is the other half: that they do answer.
    #[test]
    fn numbers_are_answered_inline() {
        let mut heap = Heap::new();
        let nil = Value::Nil;
        let mut call = |op, a, b, c| straight_intrinsic(&mut heap, op, a, b, c);
        let (i, f) = (Value::Int, Value::Float);
        use IntrinsicOp::*;
        assert_eq!(call(Float, i(3), nil, nil), Some(f(3.0)));
        assert_eq!(call(Float, f(2.5), nil, nil), Some(f(2.5)));
        assert_eq!(call(Int, f(-2.9), nil, nil), Some(i(-2)));
        assert_eq!(call(Int, f(f64::NAN), nil, nil), Some(i(0)));
        assert_eq!(call(Int, f(1e19), nil, nil), Some(i(i64::MAX)));
        assert_eq!(call(Int, i(7), nil, nil), Some(i(7)));
        assert_eq!(call(Abs, i(-7), nil, nil), Some(i(7)));
        assert_eq!(call(Abs, f(-0.5), nil, nil), Some(f(0.5)));
        assert_eq!(call(Floor, i(7), nil, nil), Some(i(7)));
        assert_eq!(call(Floor, f(-0.5), nil, nil), Some(f(-1.0)));
        assert_eq!(call(Min, i(1), i(2), nil), Some(i(1)));
        assert_eq!(call(Min, f(2.0), i(1), nil), Some(i(1)));
        assert_eq!(call(Min, i(1), f(1.0), nil), Some(i(1)));
        assert_eq!(call(Max, i(1), f(1.0), nil), Some(i(1)));
        assert_eq!(call(Max, f(1.5), f(2.5), nil), Some(f(2.5)));
        // Exact where an int does not fit a float: 2^53 + 1 against 2^53.
        assert_eq!(call(Max, f(9007199254740992.0), i(9007199254740993), nil), Some(i(9007199254740993)));
        assert_eq!(call(Min, i(9007199254740993), f(9007199254740992.0), nil), Some(f(9007199254740992.0)));
        assert_eq!(call(Clamp, i(5), i(0), i(3)), Some(i(3)));
        assert_eq!(call(Clamp, i(5), f(0.0), i(3)), Some(f(3.0)));
        assert_eq!(call(Clamp, f(-1.0), f(0.0), f(1.0)), Some(f(0.0)));
    }

    #[test]
    fn len_and_type_are_answered_inline() {
        let mut heap = Heap::new();
        let list = Value::List(heap.alloc_list(vec![Value::Int(1), Value::Nil]));
        let text = Value::String(heap.intern_str("h\u{e9}llo"));
        let nil = Value::Nil;
        use IntrinsicOp::*;
        assert_eq!(straight_intrinsic(&mut heap, Len, list, nil, nil), Some(Value::Int(2)));
        assert_eq!(straight_intrinsic(&mut heap, Len, text, nil, nil), Some(Value::Int(5)));
        for (v, name) in [(list, "list"), (text, "string"), (nil, "nil"), (Value::Int(1), "int")] {
            let Some(Value::String(id)) = straight_intrinsic(&mut heap, Type, v, nil, nil) else {
                panic!("type({name}) was not answered");
            };
            assert_eq!(heap.get_string(id), name);
        }
    }

    /// What the natives have to see: errors, conversions, overflow, duals
    /// and `Pending`s.
    #[test]
    fn everything_else_is_left_to_the_native() {
        let mut heap = Heap::new();
        let text = Value::String(heap.intern_str("12"));
        let list = Value::List(heap.alloc_list(Vec::new()));
        let dual = Value::Dual { value: 1.0, derivative: 1.0 };
        let pending = Value::Pending(crate::value::PendingId(0));
        let nil = Value::Nil;
        let one = Value::Int(1);
        use IntrinsicOp::*;
        for op in [Float, Int, Abs, Floor] {
            for v in [text, list, dual, pending, nil, Value::Bool(true)] {
                assert_eq!(straight_intrinsic(&mut heap, op, v, nil, nil), None, "{op:?}");
            }
        }
        assert_eq!(straight_intrinsic(&mut heap, Abs, Value::Int(i64::MIN), nil, nil), None);
        for v in [text, list, dual, pending, nil] {
            for op in [Min, Max, Clamp] {
                assert_eq!(straight_intrinsic(&mut heap, op, v, one, one), None, "{op:?}");
                assert_eq!(straight_intrinsic(&mut heap, op, one, v, one), None, "{op:?}");
            }
            assert_eq!(straight_intrinsic(&mut heap, Clamp, one, one, v), None);
        }
        for v in [nil, one, dual, pending] {
            assert_eq!(straight_intrinsic(&mut heap, Len, v, nil, nil), None);
        }
        assert_eq!(straight_intrinsic(&mut heap, Type, pending, nil, nil), None);
    }
}

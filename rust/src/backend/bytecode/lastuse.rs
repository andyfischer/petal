//! Route-A straight-line uniqueness: a *last-use* rewrite pass over lowered
//! bytecode (M4 route A).
//!
//! Route B (`escape.rs`) proves loop-carried accumulators unique on the term
//! graph. This pass covers the other in-place case the plan names: a mutation
//! of a **freshly allocated container whose register is dead afterward** —
//! `let xs = [1, 2, 3]; xs[0] = v` builder code, per-iteration record/list
//! initialization, and read-then-mutate sequences (`len(xs)` then
//! `append(xs, …)`) that a single-static-consumer graph test would forbid.
//!
//! It runs **after** lowering (and after route B's opcode selection), because
//! the linear instruction stream has a total order per function, which makes
//! last-use a classic backward-reachability question instead of a phi-cycle
//! analysis.
//!
//! The lowering emits a `Move` for every variable *use* (each `Copy` term gets
//! its own register), so the container id typically lives in a small **alias
//! group** of registers: the fresh allocation's register plus every register
//! `Move`-loaded from it. `Move` is the only instruction that propagates an id
//! between registers unchanged — a clone-semantics mutation writes a *new* id
//! and `GetIndex`/`GetField` extract element values — so the group is exactly
//! the Move-closure of the allocation register. A candidate mutation `i` with
//! container register `c` is rewritten to its in-place form
//! (`SetIndexInPlace`/`SetFieldInPlace`/`BuiltinCall{in_place}`) iff:
//!
//! 1. **Fresh root.** Chasing `c` backward through `Move`s — each link having
//!    exactly one def in the function — lands on a fresh allocation
//!    (`Alloc*`) or an already-in-place mutation. The latter is the chain
//!    case: once `xs[0] = v` fires, its result register carries the same
//!    unique id, so `xs[1] = w` chains off it. Params, captures, self-refs,
//!    and pattern bindings have no def instruction, and any phi register has
//!    several (its init and carry-out `Move`s), so those all reject here —
//!    which is what keeps this pass out of route B's loop-phi territory.
//! 2. **Tracked, non-retaining alias group.** The group is closed over
//!    `Move`s: a `Move` from a member into a register with any other def
//!    (a phi/carry slot) rejects, since the id could then outlive tracking.
//!    Every group member's other reads must be *non-retaining*: pure
//!    observers (arithmetic/compare operands, jump conditions,
//!    `GetField`/`GetIndex` — which copy an element out, not the container
//!    id — whitelisted builtins like `len`/`print`/`str` that measure or
//!    format immediately, `ForEachInit`'s cursor snapshot) and the container
//!    slot of a clone-semantics mutation (which copies the backing store).
//!    Call arguments, closure captures, `Return`, state writes, storage into
//!    another container, and match subjects (pattern bindings can bind the
//!    subject itself) are retaining — any such read rejects the candidate no
//!    matter where it sits, because the reference it keeps would observe the
//!    in-place write.
//! 3. **Dead after the mutation.** For each group member `g`: no read of `g`
//!    is reachable from `i` (following the CFG through jumps, loop
//!    back-edges, `MatchArm`/`StateInit` side exits) without first passing
//!    `g`'s single def (the kill), and the function cannot fall off the end
//!    with `g` as its result register. Reads *before* the mutation stay fine
//!    while any path on which the stale id would be observed again rejects —
//!    including `i` re-reaching itself around a loop back-edge, which is
//!    exactly where in-place (mutating one id repeatedly) diverges from
//!    clone semantics (a fresh id per iteration). The kill is what lets the
//!    per-iteration builder fire (`let t = [0, 0]; t[0] = i` inside a loop:
//!    the back edge re-executes the alloc before any re-read).
//!
//! **Soundness.** The heap is immutable-by-construction, so the only way any
//! code observes the container is through a register holding its id or a
//! retained copy of that id. Condition 1 pins the id's birth to a single
//! instruction; condition 2 enumerates every register that can hold the id
//! (the Move-closure) and rules out every copy that could outlive `i`;
//! condition 3 rules out every later read of those registers. After `i`, the
//! sole live holder of the id is `i`'s destination register — the mutated
//! container, byte-identical to what the clone would have produced.
//!
//! Uncertain ⇒ decline; the mutation stays clone-and-alloc (always correct).
//! Gated behind `OptFlags::in_place_straight_line`; with the flag off this
//! pass never runs and the lowering is untouched.

use std::collections::HashMap;

use super::isa::{BytecodeFn, BytecodeProgram, Inst, Reg};
use crate::builtins::is_mutating_builtin;
use crate::program::{Program, TermId};

/// Rewrite every provably-safe straight-line mutation in `bc` to its in-place
/// form. Returns the number of instructions rewritten (diagnostics / tests).
pub fn apply(bc: &mut BytecodeProgram, program: &Program) -> usize {
    let mut n = apply_fn(&mut bc.root, &bc.match_binds, program);
    for f in &mut bc.fns {
        n += apply_fn(f, &bc.match_binds, program);
    }
    n
}

/// Builtins that read a container argument without retaining any reference to
/// it: they measure or format immediately (`len`, `last`, `print`, `str`,
/// `type`). Everything not listed is conservatively treated as retaining —
/// an unknown builtin (or a higher-order intrinsic whose closure could stash
/// the value) blocks the rewrite, it never breaks it.
fn is_pure_builtin(name: &str) -> bool {
    matches!(name, "len" | "last" | "print" | "str" | "type")
}

/// One function's rewrite pass. Re-scans until a fixpoint so chains convert
/// regardless of code order (each conversion can make the next candidate's
/// def "fresh").
fn apply_fn(
    f: &mut BytecodeFn,
    match_binds: &HashMap<(TermId, u16), Vec<(String, Reg)>>,
    program: &Program,
) -> usize {
    let mut converted = 0;
    loop {
        let mut changed = 0;
        for i in 0..f.code.len() {
            let Some(c) = candidate_container(&f.code[i], program) else {
                continue;
            };
            if route_a_fires(f, match_binds, program, i, c) {
                rewrite_in_place(&mut f.code[i]);
                changed += 1;
            }
        }
        if changed == 0 {
            return converted;
        }
        converted += changed;
    }
}

/// If `inst` is a clone-semantics mutation this pass can rewrite, its
/// container register.
fn candidate_container(inst: &Inst, program: &Program) -> Option<Reg> {
    match inst {
        Inst::SetIndex { obj, .. } | Inst::SetField { obj, .. } => Some(*obj),
        Inst::BuiltinCall { name, args, in_place: false, .. }
            if builtin_name(program, *name).is_some_and(is_mutating_builtin) =>
        {
            args.first().copied()
        }
        _ => None,
    }
}

fn builtin_name(program: &Program, cid: crate::constant_table::ConstantId) -> Option<&str> {
    program.get_string_constant(cid)
}

/// Swap a candidate for its in-place form. Operands (and therefore `origins`
/// and all jump targets) are unchanged.
fn rewrite_in_place(inst: &mut Inst) {
    match inst {
        Inst::SetIndex { dst, obj, idx, val } => {
            *inst = Inst::SetIndexInPlace { dst: *dst, obj: *obj, idx: *idx, val: *val };
        }
        Inst::SetField { dst, obj, field, val } => {
            *inst = Inst::SetFieldInPlace { dst: *dst, obj: *obj, field: *field, val: *val };
        }
        Inst::BuiltinCall { in_place, .. } => *in_place = true,
        other => unreachable!("not a route-A candidate: {other:?}"),
    }
}

/// A def whose result is a container id this pass may treat as unique: a
/// fresh allocation, or a mutation already rewritten in place (route A chain
/// or a route-B accumulator step) — its dst carries the same unique id.
fn is_fresh_def(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::AllocList { .. }
            | Inst::AllocMap { .. }
            | Inst::AllocMapSpread { .. }
            | Inst::AllocElement { .. }
            | Inst::SetIndexInPlace { .. }
            | Inst::SetFieldInPlace { .. }
            | Inst::BuiltinCall { in_place: true, .. }
    )
}

/// The full route-A test for candidate `i` with container register `c` (see
/// the module docs for the three conditions).
fn route_a_fires(
    f: &BytecodeFn,
    match_binds: &HashMap<(TermId, u16), Vec<(String, Reg)>>,
    program: &Program,
    i: usize,
    c: Reg,
) -> bool {
    // Def sites of every register (including the candidate's own dst write —
    // a candidate whose dst shares its container's register, as arm carry
    // slots do, must count as a second def and reject).
    let mut defs: HashMap<Reg, Vec<usize>> = HashMap::new();
    for (j, inst) in f.code.iter().enumerate() {
        inst.for_each_write(match_binds, |w| defs.entry(w).or_default().push(j));
    }
    let single_def = |r: Reg| -> Option<usize> {
        match defs.get(&r).map(Vec::as_slice) {
            Some(&[d]) => Some(d),
            _ => None, // no def (param/capture/binding) or several (phi)
        }
    };

    // 1. Fresh root: chase `c` back through single-def Moves.
    let mut cur = c;
    let mut hops = 0;
    loop {
        let Some(d) = single_def(cur) else { return false };
        match &f.code[d] {
            Inst::Move { src, .. } => {
                cur = *src;
                hops += 1;
                if hops > f.code.len() {
                    return false; // Move cycle — not a fresh chain
                }
            }
            inst if is_fresh_def(inst) => break,
            _ => return false,
        }
    }
    let root_reg = cur;

    // 2. Alias group: the Move-closure of the root register. A Move from a
    // member into a register with any other def (a phi/carry slot) means the
    // id escapes tracking — reject.
    let mut group: Vec<Reg> = vec![root_reg];
    let mut w = 0;
    while w < group.len() {
        let g = group[w];
        w += 1;
        for (j, inst) in f.code.iter().enumerate() {
            if let Inst::Move { dst, src } = inst {
                if *src == g && !group.contains(dst) {
                    if single_def(*dst) != Some(j) {
                        return false; // alias merged into an untracked register
                    }
                    group.push(*dst);
                }
            }
        }
    }
    debug_assert!(group.contains(&c), "container must be in its own alias group");

    // 3. No retaining reader of any member, anywhere in the function.
    // Group-internal Moves are the alias edges themselves (handled above);
    // the candidate's own container-slot read is non-retaining by
    // construction, while its *other* operands (an `append(xs, xs)` value
    // slot, say) reject here like any other retaining read.
    for inst in &f.code {
        if matches!(inst, Inst::Move { .. }) {
            continue;
        }
        let mut retained = false;
        for_each_read(inst, program, |r, retaining| {
            retained |= retaining && group.contains(&r);
        });
        if retained {
            return false;
        }
    }

    // 4. Every member is dead after the mutation: no read of `g` reachable
    // from `i` before `g`'s def re-executes (the kill), and `g` is not the
    // function's fall-off result. A `Move` re-loading a member after `i`
    // rejects via its *source* member's walk, so post-mutation aliases can't
    // sneak in through a kill.
    let exit = f.code.len(); // pseudo-node: running off the end
    for &g in &group {
        let d = single_def(g).expect("group members have single defs");
        let mut seen = vec![false; f.code.len() + 1];
        let mut stack: Vec<usize> = Vec::new();
        push_succs(&f.code[i], i, exit, &mut stack);
        while let Some(j) = stack.pop() {
            if seen[j] {
                continue;
            }
            seen[j] = true;
            if j == exit {
                if f.result_reg == Some(g) {
                    return false; // delivered as the function's result
                }
                continue;
            }
            let mut reads = false;
            for_each_read(&f.code[j], program, |r, _| reads |= r == g);
            if reads {
                return false; // the stale id would be observed post-mutation
            }
            if j == d {
                continue; // the def kills `g` — stop this path
            }
            push_succs(&f.code[j], j, exit, &mut stack);
        }
    }
    true
}

/// CFG successors of instruction `j` (instruction indices; `exit` is the
/// pseudo-node for running off the end of the function). Fall-through and the
/// explicit branch target are both sourced from [`Inst`]'s own metadata so this
/// can never disagree with lowering's backpatch set.
fn push_succs(inst: &Inst, j: usize, exit: usize, out: &mut Vec<usize>) {
    if inst.falls_through() {
        out.push((j + 1).min(exit));
    }
    if let Some(to) = inst.branch_target() {
        out.push(to as usize);
    }
}

/// Every register `inst` reads, with whether the read may *retain* a
/// reference to the value (aliasing it, storing it into another container /
/// closure / state, passing it to arbitrary code, or letting it escape the
/// function). Non-retaining reads observe the value and let it go:
///
/// - operands consumed by-value (arithmetic, comparisons, conditions,
///   formatting) — a container operand either errors or is rendered to a
///   string immediately;
/// - `GetField`/`GetIndex`, which copy an *element* `Value` out (interior
///   ids are distinct from the container's own id);
/// - `ForEachInit`, which snapshots the element vector into the loop cursor;
/// - whitelisted pure builtins ([`is_pure_builtin`]);
/// - the container slot of a **clone-semantics** mutation, which copies the
///   backing store into a fresh id. (The container slot of an *in-place*
///   mutation is retaining here: it mutates the id, so a second in-place
///   consumer of the same register must never fire.)
fn for_each_read(inst: &Inst, program: &Program, mut f: impl FnMut(Reg, bool)) {
    const PURE: bool = false;
    const RETAIN: bool = true;
    match inst {
        Inst::Move { src, .. } => f(*src, RETAIN), // an alias of the same id
        Inst::Add { a, b, .. }
        | Inst::Sub { a, b, .. }
        | Inst::Mul { a, b, .. }
        | Inst::Div { a, b, .. }
        | Inst::Mod { a, b, .. }
        | Inst::Eq { a, b, .. }
        | Inst::Ne { a, b, .. }
        | Inst::Lt { a, b, .. }
        | Inst::Le { a, b, .. }
        | Inst::Gt { a, b, .. }
        | Inst::Ge { a, b, .. }
        | Inst::Concat { a, b, .. } => {
            f(*a, PURE);
            f(*b, PURE);
        }
        Inst::Neg { a, .. } | Inst::Not { a, .. } => f(*a, PURE),
        Inst::JumpIfFalse { cond, .. } | Inst::JumpIfTrue { cond, .. } => f(*cond, PURE),
        // The tested register survives as the coalesce result when present, so
        // it must not be freed at this use.
        Inst::JumpIfPresent { cond, .. } => f(*cond, RETAIN),
        Inst::ForEachInit { iter, .. } => f(*iter, PURE), // snapshots elems
        Inst::RangeInit { start, end, .. } => {
            f(*start, PURE);
            f(*end, PURE);
        }
        Inst::Call { callee, args, .. } => {
            f(*callee, RETAIN);
            for a in args {
                f(*a, RETAIN); // arbitrary code may store/return it
            }
        }
        Inst::MethodCall { recv, args, .. } => {
            f(*recv, RETAIN);
            for a in args {
                f(*a, RETAIN);
            }
        }
        Inst::BuiltinCall { name, args, in_place, .. } => {
            match builtin_name(program, *name) {
                Some(n) if is_mutating_builtin(n) => {
                    // Container slot: clone semantics copy, in-place mutates.
                    if let Some(&c0) = args.first() {
                        f(c0, *in_place);
                    }
                    for a in &args[1..] {
                        f(*a, RETAIN); // stored into the (new) container
                    }
                }
                Some(n) if is_pure_builtin(n) => {
                    for a in args {
                        f(*a, PURE);
                    }
                }
                _ => {
                    for a in args {
                        f(*a, RETAIN); // unknown builtin / intrinsic
                    }
                }
            }
        }
        Inst::MakeClosure { caps, .. } => {
            for r in caps {
                f(*r, RETAIN); // captured for later
            }
        }
        Inst::MakeOverloadSet { closures, .. } => {
            for r in closures {
                f(*r, RETAIN);
            }
        }
        Inst::Return { val } => {
            if let Some(v) = val {
                f(*v, RETAIN); // escapes the function
            }
        }
        Inst::AllocList { elems, .. } => {
            for r in elems {
                f(*r, RETAIN); // stored into the new container
            }
        }
        Inst::AllocMap { vals, .. } => {
            for r in vals {
                f(*r, RETAIN);
            }
        }
        Inst::AllocMapSpread { ins, .. } | Inst::AllocElement { ins, .. } => {
            for r in ins {
                f(*r, RETAIN);
            }
        }
        Inst::MakeEnumVariant { fields, .. } => {
            for r in fields {
                f(*r, RETAIN);
            }
        }
        Inst::GetField { obj, .. } => f(*obj, PURE),
        Inst::GetIndex { obj, idx, .. } => {
            f(*obj, PURE);
            f(*idx, PURE);
        }
        Inst::SetField { obj, val, .. } => {
            f(*obj, PURE); // clone-semantics container copy
            f(*val, RETAIN);
        }
        Inst::SetIndex { obj, idx, val, .. } => {
            f(*obj, PURE);
            f(*idx, PURE);
            f(*val, RETAIN);
        }
        Inst::SetFieldInPlace { obj, val, .. } => {
            f(*obj, RETAIN); // mutates the id — no second consumer allowed
            f(*val, RETAIN);
        }
        Inst::SetIndexInPlace { obj, idx, val, .. } => {
            f(*obj, RETAIN);
            f(*idx, PURE);
            f(*val, RETAIN);
        }
        Inst::StateWrite { val, key, .. } => {
            f(*val, RETAIN); // persisted into the state map
            if let Some(k) = key {
                f(*k, RETAIN);
            }
        }
        Inst::StateInit { key, .. } => {
            if let Some(k) = key {
                f(*k, RETAIN);
            }
        }
        Inst::MatchArm { subject, .. } => f(*subject, RETAIN), // bindings may bind it
        Inst::MatchFail { subject } => f(*subject, PURE), // formats + raises
        Inst::LoadConst { .. }
        | Inst::LoadNil { .. }
        | Inst::LoadBool { .. }
        | Inst::Jump { .. }
        | Inst::ForEachNext { .. }
        | Inst::RangeNext { .. }
        | Inst::WhileInit { .. }
        | Inst::LoopBumpIdx { .. }
        | Inst::LoopPop { .. }
        | Inst::StateRead { .. }
        | Inst::Error { .. } => {}
    }
}

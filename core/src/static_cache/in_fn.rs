//! Static values inside function bodies.
//!
//! The parent module keeps top-level values. This one keeps the values a
//! function builds that could only ever come out the same: the
//! `{kind: "none", ml: 0}` a parser returns for "nothing here", the colour
//! literals handed to `color_lerp` once per cell of a heat map. Registers
//! are per call, so the value is kept the same way a top-level one is, in
//! a slot of the stack's table, and the function's code gets the same two
//! instructions around the code that builds it:
//!
//! ```text
//!       static_skip run 7 [r4 = s31] -> L     ; kept? load it into this frame, jump
//!       ...the instructions that build r4, as they lower without the cache...
//!       static_store [s31 = r4]
//!   L:  ...
//! ```
//!
//! No instruction is moved or removed. The first execution fills the slot,
//! whichever call that is in; every later one, in any call, loads it.
//!
//! # Which code
//!
//! Two things have to hold, and neither is enough alone.
//!
//! **The term is invariant** ([`FnPlan`]). The classifier calls a term
//! inside a function static *assuming the function's parameters are data*,
//! so `[a, 1]` is static there and a different list on every call. A term
//! is invariant when it is static and reads nothing that belongs to the
//! call: only constants, other invariant terms, the function itself, and
//! captures that are a kept top-level value or a function at every place
//! the closure is made. A parameter never is. A loop or pattern variable
//! never is on its own; a loop, branch or match is invariant as a whole
//! when everything under it is, given its own variables, and then
//! everything under it is kept *with it or not at all* (a value built from
//! the loop variable is one value per iteration). A name a loop or branch
//! rebinds is invariant behind it when the loop or branch is.
//!
//! **The instruction is clean.** Every register it reads is written only
//! by instructions that are themselves kept candidates, or is an invariant
//! capture or the function's own name; and every register it writes is
//! written by nothing else. This is the check on the lowering that the
//! parent module makes at the top level: a register a parameter arrives
//! in, a loop cursor writes, or two arms of a dynamic branch share is not
//! one a kept value may come out of.
//!
//! A maximal sequence of such instructions is a *stretch*. It is kept as
//! one run, storing every register it writes that anything outside it
//! reads, when control enters it only at its first instruction and leaves
//! it only at its end, it allocates something ([`MIN_WEIGHT`] and more
//! than a single call), and none of the registers it stores reaches a
//! write: the term-level rule of [`StaticPlan`](super::StaticPlan) (`let
//! out = []` in front of a loop that appends is the commonest invariant
//! allocation there is, and keeping it would cost a copy per call) and the
//! register-level one for in-place instructions.
//!
//! # What keeps a kept value from changing
//!
//! The immutable bit, as at the top level: `StaticStore` marks the value
//! and everything under it, and the heap copies such an object instead of
//! editing it. That is the whole guarantee for a value that leaves the
//! function, returned to a caller that writes to it or passed to a callee
//! that writes its parameter. The escape analysis is *not* told about
//! these values (it is about top-level ones): a caller that edits in place
//! the "fresh" list a function returns then pays one copy, the allocation
//! the slot saved, and edits the copy in place from there on. Telling it
//! would turn every such edit into a copy.
//!
//! # What is the same value
//!
//! Every call gets the same object where it got a new equal one. A program
//! cannot tell: it has no identity test, an in-place edit copies, and
//! `freeze` is not constant, so nothing invariant is frozen.
//!
//! A value that cannot be kept (it holds a handle or a pending result)
//! rejects its slots, and the code runs on every call as before.
//!
//! # Recursion, closures, memoized calls, forks
//!
//! The slot belongs to the place in the program, not to the call: a
//! recursive call that reaches the code while an outer call is building
//! the value builds it too, and the first to finish fills the slot (they
//! are the same value). A closure made per call shares its slots with
//! every other closure of that function, which is why a capture has to be
//! invariant at *every* place the closure is made. A memoized call that is
//! replayed does not reach the code at all; one that records fills or
//! reads the slot like any other. A fork starts with an empty table.
//!
//! # Reloads
//!
//! Every reload that patches a literal or recompiles empties these slots,
//! whatever the edit was: they are not followed (`env/static_follow.rs`).
//! The next call that reaches the code builds the value again, once. (A
//! reload that only moves text changes neither the code nor a constant,
//! and the table is left as it is.) They are emptied again, with the values
//! a paused drag rebuilds, when it pauses, since one may have been built
//! from a value the drag left stale.
//!
//! # Observations
//!
//! Not kept at all where the lowering preserves observations (Garden's
//! panels): a skipped stretch would have to report the names bound inside
//! it, per call. The top-level cache is unaffected.
//!
//! # Checking it
//!
//! [`StaticSlots::set_check`](super::StaticSlots::set_check) makes every
//! skip fall through: the code runs on every call, and the store compares
//! what it built with what the slot holds ([`identical`](super::identical)).
//! A difference is a value that was not invariant, and is reported
//! ([`StaticSlots::mismatches`](super::StaticSlots::mismatches)). The
//! fuzzers and the corpus differential run under it.

/// Keyed by term, block and register ids and asked several times per term:
/// the VM's fast hasher, not SipHash. Nothing is read in iteration order
/// where the order could show.
type HashMap<K, V> = std::collections::HashMap<K, V, crate::fxhash::FxBuildHasher>;
type HashSet<K> = std::collections::HashSet<K, crate::fxhash::FxBuildHasher>;

use super::{MIN_WEIGHT, SlotFollow, StaticPlan, StaticRun, StaticSlotInfo, skippable_inst, weight};
use crate::backend::bytecode::isa::{BytecodeFn, BytecodeProgram, Inst, Label, Reg};
use crate::native_fn::NativeFnTable;
use crate::program::{BlockId, Program, Term, TermId, TermOp};
use crate::static_classify::{StaticAnalysis, TermClass};

fn is_phantom(term: &Term) -> bool {
    matches!(term.op, TermOp::Copy) && term.inputs.is_empty() && term.name.is_some()
}

fn blocks_of(program: &Program, term: &Term) -> Vec<BlockId> {
    let is_match = matches!(term.op, TermOp::Match);
    if term.child_blocks.is_empty() && !is_match {
        return Vec::new();
    }
    let mut out: Vec<BlockId> = term.child_blocks.iter().copied().collect();
    if is_match && let Some(arms) = program.match_arms.get(&term.id) {
        for arm in arms {
            out.extend(arm.guard_block);
            out.push(arm.body_block);
        }
    }
    out.sort_by_key(|b| b.0);
    out.dedup();
    out
}

const NONE: u32 = u32::MAX;

/// Which terms inside function bodies are invariant (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct FnPlan {
    /// Per term: invariant on its own.
    inv: Vec<bool>,
    /// Per term: the nearest invariant loop, branch or match it is under,
    /// or [`NONE`].
    region: Vec<u32>,
    /// Per term: the function it is written in, or [`NONE`] for the root
    /// block.
    function: Vec<u32>,
    /// Per invariant term with blocks: every term under it.
    under: HashMap<TermId, Vec<TermId>>,
    /// Per function: which of its captures are invariant, in capture order.
    captures: Vec<Vec<bool>>,
    /// Per function: some invariant term of it allocates, calls or loops,
    /// so its code is worth looking at.
    worth: Vec<bool>,
}

impl FnPlan {
    /// Whether `t` is invariant on its own.
    pub fn invariant(&self, t: TermId) -> bool {
        self.inv.get(t.0 as usize).copied().unwrap_or(false)
    }

    /// The nearest invariant loop, branch or match `t` is under.
    pub fn region_of(&self, t: TermId) -> Option<TermId> {
        self.region.get(t.0 as usize).copied().filter(|r| *r != NONE).map(TermId)
    }

    /// The function `t` is written in (an index into `Program::functions`).
    pub fn function_of(&self, t: TermId) -> Option<u32> {
        self.function.get(t.0 as usize).copied().filter(|f| *f != NONE)
    }

    /// Every term under the invariant loop, branch or match `t`; empty for
    /// any other term.
    pub fn under(&self, t: TermId) -> &[TermId] {
        self.under.get(&t).map(|v| &v[..]).unwrap_or(&[])
    }

    /// Whether anything in a function is invariant at all.
    pub fn is_empty(&self) -> bool {
        !self.worth.contains(&true)
    }

    pub fn of(program: &Program, analysis: &StaticAnalysis, plan: &StaticPlan) -> FnPlan {
        let n = program.terms.len();
        let nblocks = program.blocks.len();

        // Structure: the terms of each block (phantoms included), each
        // block's parent term, the function a block belongs to.
        let mut in_block: Vec<Vec<TermId>> = vec![Vec::new(); nblocks];
        let mut parent: Vec<Option<TermId>> = program.blocks.iter().map(|b| b.parent_term_id).collect();
        parent.resize(nblocks, None);
        let mut term_blocks: HashMap<TermId, Vec<BlockId>> = HashMap::default();
        for term in &program.terms {
            if let Some(v) = in_block.get_mut(term.block_id.0 as usize) {
                v.push(term.id);
            }
            let blocks = blocks_of(program, term);
            for b in &blocks {
                if let Some(slot) = parent.get_mut(b.0 as usize) {
                    slot.get_or_insert(term.id);
                }
            }
            if !blocks.is_empty() {
                term_blocks.insert(term.id, blocks);
            }
        }
        let mut body_fn: HashMap<BlockId, u32> = HashMap::default();
        let mut closure_sites: Vec<Vec<TermId>> = vec![Vec::new(); program.functions.len()];
        for (i, f) in program.functions.iter().enumerate() {
            body_fn.insert(f.body_block, i as u32);
        }
        for term in &program.terms {
            if let TermOp::MakeClosure(f) = term.op
                && let Some(v) = closure_sites.get_mut(f.0 as usize)
            {
                v.push(term.id);
            }
        }
        let mut fn_of_block: Vec<u32> = vec![NONE; nblocks];
        for (b, slot) in fn_of_block.iter_mut().enumerate() {
            let mut cur = BlockId(b as u32);
            for _ in 0..=nblocks {
                if let Some(&f) = body_fn.get(&cur) {
                    *slot = f;
                    break;
                }
                if cur == program.root_block {
                    break;
                }
                match parent.get(cur.0 as usize).copied().flatten() {
                    Some(t) => cur = program.get_term(t).block_id,
                    None => break,
                }
            }
        }
        let function: Vec<u32> = program
            .terms
            .iter()
            .map(|t| fn_of_block.get(t.block_id.0 as usize).copied().unwrap_or(NONE))
            .collect();
        if program.terms.iter().enumerate().any(|(i, t)| t.id.0 as usize != i) {
            // Terms are looked up by id everywhere below.
            return FnPlan::default();
        }
        let mut carried: HashMap<TermId, Vec<TermId>> = HashMap::default();
        for block in &program.blocks {
            for po in &block.phi_outs {
                carried.entry(po.dest_term).or_default().push(po.src_term);
            }
        }
        // Every term under a term's blocks, nested ones included.
        let region = |t: TermId| -> Vec<TermId> {
            let mut out = Vec::new();
            let mut work: Vec<BlockId> = term_blocks.get(&t).cloned().unwrap_or_default();
            while let Some(b) = work.pop() {
                for &x in in_block.get(b.0 as usize).map(|v| &v[..]).unwrap_or(&[]) {
                    out.push(x);
                    work.extend(term_blocks.get(&x).into_iter().flatten().copied());
                }
            }
            out
        };
        // Ops that are never part of a kept value's code: a return leaves
        // it, a closure is made per run, and state and cells are not static
        // (the classifier says so too; this does not rest on it).
        let plain = |term: &Term| {
            !matches!(
                term.op,
                TermOp::Return
                    | TermOp::Break
                    | TermOp::Continue
                    | TermOp::MakeClosure(_)
                    | TermOp::MakeOverloadSet
                    | TermOp::StateInit
                    | TermOp::StateRead
                    | TermOp::StateWrite
                    | TermOp::CellNew
                    | TermOp::CellRead
                    | TermOp::CellWrite
                    | TermOp::Error(_)
            )
        };
        let class = |t: TermId| analysis.term_class(t);

        // The fixed point, from below: nothing is invariant until shown to
        // be, so a cycle (a name a loop rebinds from itself) never is by
        // assumption.
        let mut inv = vec![false; n];
        let ok = |inv: &[bool], t: TermId| -> bool {
            let Some(term) = program.terms.get(t.0 as usize) else { return false };
            if term.block_id == program.root_block {
                // A top-level value: the same on every run when it is one
                // the cache may keep (static data that reaches no write),
                // or a function.
                plan.roots.contains(&t) || class(t) == TermClass::Function
            } else {
                inv[t.0 as usize]
            }
        };
        // Whether everything under `t` is invariant given the variables of
        // `t`'s own blocks.
        let closed = |inv: &[bool], t: TermId| -> bool {
            let inside = region(t);
            if inside.is_empty() {
                return true;
            }
            if inside.iter().any(|x| {
                let xt = program.get_term(*x);
                class(*x) == TermClass::Dynamic || (!is_phantom(xt) && !plain(xt))
            }) {
                return false;
            }
            let set: HashSet<TermId> = inside.iter().copied().collect();
            let mut loc: HashSet<TermId> = HashSet::default();
            let ok_in = |loc: &HashSet<TermId>, y: TermId| -> bool {
                if set.contains(&y) {
                    return loc.contains(&y);
                }
                if ok(inv, y) {
                    return true;
                }
                // A name the region rebinds, read inside it: its value on
                // the way in, and after that only what the region gave it.
                let yt = program.get_term(y);
                matches!(yt.op, TermOp::Phi)
                    && yt.block_id == program.get_term(t).block_id
                    && yt.inputs.iter().all(|x| ok(inv, *x))
                    && carried.get(&y).is_some_and(|srcs| srcs.iter().all(|x| set.contains(x)))
            };
            loop {
                let mut changed = false;
                for &x in &inside {
                    if loc.contains(&x) {
                        continue;
                    }
                    let xt = program.get_term(x);
                    let now = if is_phantom(xt) {
                        // A variable of one of the region's blocks: what its
                        // loop iterates or its match inspects.
                        match parent.get(xt.block_id.0 as usize).copied().flatten() {
                            Some(pt) => {
                                let p = program.get_term(pt);
                                !p.inputs.is_empty() && p.inputs.iter().all(|i| ok_in(&loc, *i))
                            }
                            None => false,
                        }
                    } else {
                        // A name rebound under the region is taken to be
                        // what the region gives it. That is only an
                        // assumption until every term under the region is
                        // shown invariant, and the region is accepted whole
                        // or not at all; a run goes round a loop a
                        // well-founded number of times, so it then holds.
                        xt.inputs.iter().all(|i| ok_in(&loc, *i))
                            && carried
                                .get(&x)
                                .is_none_or(|srcs| srcs.iter().all(|i| set.contains(i) || ok_in(&loc, *i)))
                    };
                    if now {
                        loc.insert(x);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            loc.len() == set.len()
        };
        // Terms that can never be invariant, whatever the rest turns out to
        // be: asked once.
        let mut never: Vec<bool> = program
            .terms
            .iter()
            .map(|term| {
                term.block_id == program.root_block
                    || function[term.id.0 as usize] == NONE
                    || class(term.id) == TermClass::Dynamic
                    || (!is_phantom(term) && !plain(term))
            })
            .collect();
        for t in term_blocks.keys() {
            if never[t.0 as usize] {
                continue;
            }
            if region(*t).iter().any(|x| {
                let xt = program.get_term(*x);
                class(*x) == TermClass::Dynamic || (!is_phantom(xt) && !plain(xt))
            }) {
                never[t.0 as usize] = true;
            }
        }
        loop {
            let mut changed = false;
            for term in &program.terms {
                let i = term.id.0 as usize;
                if inv[i] || never[i] {
                    continue;
                }
                let now = if is_phantom(term) {
                    match body_fn.get(&term.block_id) {
                        Some(&fi) => {
                            let f = &program.functions[fi as usize];
                            if f.self_ref_register == Some(term.register) {
                                true
                            } else if let Some(slot) = f.capture_registers.iter().position(|r| *r == term.register) {
                                let sites = &closure_sites[fi as usize];
                                !sites.is_empty()
                                    && sites.iter().all(|s| {
                                        program.get_term(*s).inputs.get(slot).is_some_and(|outer| ok(&inv, *outer))
                                    })
                            } else {
                                // A parameter.
                                false
                            }
                        }
                        // A loop or pattern variable.
                        None => false,
                    }
                } else if !plain(term) {
                    false
                } else if matches!(term.op, TermOp::Phi) && carried.contains_key(&term.id) {
                    // A name a loop or branch rebinds: invariant behind it
                    // when it was in front of it and the loop or branch is.
                    term.inputs.iter().all(|x| ok(&inv, *x))
                        && carried[&term.id].iter().all(|src| {
                            // The term of the phi's block the source is under.
                            let mut b = program.get_term(*src).block_id;
                            let mut over = None;
                            for _ in 0..=nblocks {
                                if b == term.block_id {
                                    break;
                                }
                                match parent.get(b.0 as usize).copied().flatten() {
                                    Some(pt) => {
                                        over = Some(pt);
                                        b = program.get_term(pt).block_id;
                                    }
                                    None => return false,
                                }
                            }
                            b == term.block_id && over.is_some_and(|pt| inv[pt.0 as usize])
                        })
                } else {
                    term.inputs.iter().all(|x| ok(&inv, *x)) && closed(&inv, term.id)
                };
                if now {
                    inv[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // The nearest invariant region above each term: worked out per
        // block, which is what it depends on.
        let mut block_region = vec![NONE; nblocks];
        for (bi, slot) in block_region.iter_mut().enumerate() {
            let mut b = BlockId(bi as u32);
            for _ in 0..=nblocks {
                if body_fn.contains_key(&b) || b == program.root_block {
                    break;
                }
                match parent.get(b.0 as usize).copied().flatten() {
                    Some(pt) if inv[pt.0 as usize] => {
                        *slot = pt.0;
                        break;
                    }
                    Some(pt) => b = program.get_term(pt).block_id,
                    None => break,
                }
            }
        }
        let region_of: Vec<u32> = program
            .terms
            .iter()
            .map(|t| block_region.get(t.block_id.0 as usize).copied().unwrap_or(NONE))
            .collect();
        let under: HashMap<TermId, Vec<TermId>> = term_blocks
            .keys()
            .filter(|t| inv[t.0 as usize])
            .map(|t| (*t, region(*t)))
            .collect();
        let captures = program
            .functions
            .iter()
            .map(|f| {
                f.capture_registers
                    .iter()
                    .map(|reg| {
                        in_block
                            .get(f.body_block.0 as usize)
                            .into_iter()
                            .flatten()
                            .find(|t| {
                                let term = program.get_term(**t);
                                is_phantom(term) && term.register == *reg
                            })
                            .is_some_and(|t| inv[t.0 as usize])
                    })
                    .collect()
            })
            .collect();
        let mut worth = vec![false; program.functions.len()];
        for term in &program.terms {
            let i = term.id.0 as usize;
            if !inv[i] || function[i] == NONE {
                continue;
            }
            let heavy = matches!(
                term.op,
                TermOp::AllocList
                | TermOp::AllocMap { .. }
                | TermOp::AllocMapSpread { .. }
                | TermOp::AllocElement { .. }
                | TermOp::Concat
                | TermOp::SetField(_)
                | TermOp::SetIndex
                | TermOp::MakeEnumVariant(_)
                | TermOp::Call
                | TermOp::MethodCall { .. }
                | TermOp::BuiltinCall(_)
                | TermOp::ForLoop
                | TermOp::NumericForLoop
                | TermOp::WhileLoop
            );
            if heavy && let Some(w) = worth.get_mut(function[i] as usize) {
                *w = true;
            }
        }
        FnPlan { inv, region: region_of, function, under, captures, worth }
    }
}

/// One kept stretch of a function's code.
struct Stretch {
    start: usize,
    end: usize,
    /// The registers it writes that anything outside it reads.
    own: Vec<Reg>,
}

/// Find the stretches of `f` that may be kept. `mutated` are the terms
/// whose value reaches the container operand of a write.
#[allow(clippy::too_many_arguments)] // one caller; the pieces of one lowering
fn stretches(
    f: &BytecodeFn,
    bc: &BytecodeProgram,
    program: &Program,
    analysis: &StaticAnalysis,
    natives: &NativeFnTable,
    plan: &FnPlan,
    mutated: &std::collections::HashSet<TermId>,
    result_written: bool,
) -> Vec<Stretch> {
    let n = f.code.len();
    let Some(fid) = f.func_id else { return Vec::new() };
    if n == 0 {
        return Vec::new();
    }
    let origin = |ip: usize| f.origins.get(ip).copied().flatten();
    // Only code lowered from this function's own terms.
    let own_term = |t: TermId| plan.function_of(t) == Some(fid.0);
    if plan.worth.get(fid.0 as usize) != Some(&true) {
        return Vec::new();
    }
    let mut reads: Vec<Vec<Reg>> = Vec::with_capacity(n);
    let mut writes: Vec<Vec<Reg>> = Vec::with_capacity(n);
    for inst in &f.code {
        reads.push(inst.read_regs().to_vec());
        let mut w = Vec::new();
        inst.for_each_write(&bc.match_binds, |r| w.push(r));
        writes.push(w);
    }
    // Registers the frame arrives with: a parameter is never clean, a
    // capture is when it is invariant, the function's own name always is.
    let mut clean_entry: HashSet<Reg> = HashSet::default();
    let mut dirty_entry: HashSet<Reg> = f.param_regs.iter().copied().collect();
    let inv_captures = plan.captures.get(fid.0 as usize).map(|v| &v[..]).unwrap_or(&[]);
    for (i, r) in f.capture_regs.iter().enumerate() {
        if inv_captures.get(i) == Some(&true) {
            clean_entry.insert(*r);
        } else {
            dirty_entry.insert(*r);
        }
    }
    clean_entry.extend(f.self_ref_reg);
    for r in &dirty_entry {
        clean_entry.remove(r);
    }

    // The invariant loops, branches and matches with code here, and the
    // span of that code: each is kept whole or not at all.
    let mut span: HashMap<TermId, (usize, usize)> = HashMap::default();
    let mut unit_of: Vec<Option<TermId>> = vec![None; n];
    for ip in 0..n {
        let Some(t) = origin(ip).filter(|t| own_term(*t)) else { continue };
        let unit = if plan.invariant(t) && !plan.under(t).is_empty() {
            Some(t)
        } else if !plan.invariant(t) {
            plan.region_of(t)
        } else {
            None
        };
        // An invariant term is its own; one that is only invariant given a
        // loop variable belongs to the loop. Either may sit inside the code
        // of a larger invariant region, which is spanned below.
        unit_of[ip] = unit;
        let mut at = unit.or_else(|| plan.region_of(t));
        while let Some(r) = at {
            let e = span.entry(r).or_insert((ip, ip));
            e.0 = e.0.min(ip);
            e.1 = e.1.max(ip);
            at = plan.region_of(r);
        }
    }
    let mut unit_ok: HashMap<TermId, bool> = span.keys().map(|t| (*t, true)).collect();
    let mut demoted = vec![false; n];
    let mut cand = vec![false; n];
    loop {
        for ip in 0..n {
            let inst = &f.code[ip];
            let base = match origin(ip) {
                Some(t) => {
                    own_term(t)
                        && analysis.term_class(t) != TermClass::Dynamic
                        && match unit_of[ip] {
                            Some(u) => unit_ok.get(&u) == Some(&true),
                            None => plan.invariant(t),
                        }
                }
                // A loop's bookkeeping: with the region whose code it is in.
                None => span.iter().any(|(u, (a, b))| unit_ok[u] && (*a..=*b).contains(&ip)),
            };
            cand[ip] = base
                && !demoted[ip]
                && skippable_inst(inst, program, natives)
                && !matches!(inst, Inst::MakeClosure { .. });
        }
        let mut written: HashSet<Reg> = HashSet::default();
        let mut tainted: HashSet<Reg> = dirty_entry.clone();
        for ip in 0..n {
            written.extend(writes[ip].iter().copied());
            if !cand[ip] {
                tainted.extend(writes[ip].iter().copied());
            }
        }
        let mut changed = false;
        for ip in 0..n {
            if !cand[ip] {
                continue;
            }
            let dirty_read = reads[ip]
                .iter()
                .any(|r| tainted.contains(r) || (!written.contains(r) && !clean_entry.contains(r)));
            let shared_write = writes[ip].iter().any(|r| tainted.contains(r) || clean_entry.contains(r));
            if dirty_read || shared_write {
                demoted[ip] = true;
                cand[ip] = false;
                changed = true;
            }
        }
        for (u, (a, b)) in &span {
            if unit_ok[u] && (*a..=*b).any(|ip| !cand[ip]) {
                unit_ok.insert(*u, false);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Maximal sequences of candidates.
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut run_of: Vec<Option<usize>> = vec![None; n];
    let mut ip = 0;
    while ip < n {
        if !cand[ip] {
            ip += 1;
            continue;
        }
        let start = ip;
        while ip < n && cand[ip] {
            run_of[ip] = Some(bounds.len());
            ip += 1;
        }
        bounds.push((start, ip));
    }
    // What a stretch ends in that builds nothing (the constants of the
    // dynamic code behind it) is left outside: it would only be one more
    // value to store and load.
    let mut targeted = vec![false; n + 1];
    for inst in &f.code {
        for to in inst.targets() {
            targeted[(to as usize).min(n)] = true;
        }
    }
    let in_unit = |ip: usize| span.iter().any(|(u, (a, b))| unit_ok[u] && (*a..=*b).contains(&ip));
    for (start, end) in bounds.iter_mut() {
        while *end > *start {
            let last = *end - 1;
            let inst = &f.code[last];
            if weight(inst) != 0 || !inst.targets().is_empty() || targeted[last] || in_unit(last) {
                break;
            }
            run_of[last] = None;
            *end = last;
        }
    }
    // Control enters a stretch at its first instruction and leaves it at
    // its end.
    let mut broken = vec![false; bounds.len()];
    for ip in 0..n {
        for to in f.code[ip].targets() {
            let to = to as usize;
            if let Some(k) = run_of[ip] {
                let (s, e) = bounds[k];
                if to < s || to > e {
                    broken[k] = true;
                }
            }
            if to < n
                && let Some(j) = run_of[to]
                && run_of[ip] != Some(j)
                && to != bounds[j].0
            {
                broken[j] = true;
            }
        }
    }
    // A stretch the code can run off the end of the function from, or that
    // a `return` could sit in, is not one: `skippable_inst` refuses the
    // return, and falling off the end reads the result register, which is
    // counted as a reader below.

    // Registers that reach the container operand of an in-place
    // instruction, back through what carries a value unchanged.
    let mut in_place: HashSet<Reg> = HashSet::default();
    for inst in &f.code {
        match inst {
            Inst::SetFieldInPlace { obj, .. } | Inst::SetIndexInPlace { obj, .. } => {
                in_place.insert(*obj);
            }
            Inst::BuiltinCall { args, in_place: true, .. } => {
                in_place.extend(args.first().copied());
            }
            _ => {}
        }
    }
    loop {
        let before = in_place.len();
        for inst in &f.code {
            match inst {
                Inst::Move { dst, src } if in_place.contains(dst) => {
                    in_place.insert(*src);
                }
                Inst::MatchArm { subject, term, arm, .. } => {
                    let binds = bc.match_binds.get(&(*term, *arm));
                    if binds.is_some_and(|b| b.iter().any(|(_, r)| in_place.contains(r))) {
                        in_place.insert(*subject);
                    }
                }
                _ => {}
            }
        }
        if in_place.len() == before {
            break;
        }
    }
    let mut readers: HashMap<Reg, Vec<usize>> = HashMap::default();
    for (ip, rs) in reads.iter().enumerate() {
        for r in rs {
            readers.entry(*r).or_default().push(ip);
        }
    }
    let mut writer_terms: HashMap<Reg, Vec<TermId>> = HashMap::default();
    for ip in 0..n {
        if let Some(t) = origin(ip) {
            for r in &writes[ip] {
                writer_terms.entry(*r).or_default().push(t);
            }
        }
    }
    let direct_allocation = |inst: &Inst| {
        matches!(
            inst,
            Inst::AllocList { .. }
                | Inst::AllocMap { .. }
                | Inst::AllocMapSpread { .. }
                | Inst::AllocElement { .. }
                | Inst::Concat { .. }
        ) || matches!(inst, Inst::MakeEnumVariant { fields, .. } if !fields.is_empty())
    };
    // The registers the function's result may come out of, when some caller
    // writes to what the function returns (see `written_results`).
    let mut result_regs: HashSet<Reg> = HashSet::default();
    if result_written {
        result_regs.extend(f.result_reg);
        for inst in &f.code {
            if let Inst::Return { val: Some(r) } = inst {
                result_regs.insert(*r);
            }
        }
        loop {
            let before = result_regs.len();
            for inst in &f.code {
                if let Inst::Move { dst, src } = inst
                    && result_regs.contains(dst)
                {
                    result_regs.insert(*src);
                }
            }
            if result_regs.len() == before {
                break;
            }
        }
    }
    let mut out = Vec::new();
    for (k, &(start, end)) in bounds.iter().enumerate() {
        if broken[k] || start == end {
            continue;
        }
        let code = &f.code[start..end];
        // Worth a check per call: it allocates, or it is more than the one
        // call a check would cost as much as.
        if code.iter().map(weight).sum::<u32>() < MIN_WEIGHT
            || !(code.iter().any(direct_allocation) || code.len() >= 2)
        {
            continue;
        }
        let mut own: Vec<Reg> = (start..end).flat_map(|ip| writes[ip].iter().copied()).collect();
        own.sort_unstable();
        own.dedup();
        own.retain(|r| {
            f.result_reg == Some(*r)
                || readers.get(r).is_some_and(|at| at.iter().any(|ip| !(start..end).contains(ip)))
        });
        // Code nothing reads runs for what it raises.
        if own.is_empty() {
            continue;
        }
        let storable = |r: &Reg| {
            !in_place.contains(r)
                && !result_regs.contains(r)
                && writer_terms.get(r).is_some_and(|ts| ts.iter().all(|t| !mutated.contains(t)))
        };
        if !own.iter().all(storable) {
            continue;
        }
        out.push(Stretch { start, end, own });
    }
    out
}

/// The functions some caller writes to the result of: `let a = build()`,
/// then `a[i] = v`. Such a function hands back a container for its caller
/// to fill, and the escape analysis lets the caller fill it in place
/// because it is new. Kept, it would be copied by the first write of every
/// call: the same work as building it, a second copy of it held for good,
/// and a copy where the program had none. So the value such a function
/// returns is not kept.
///
/// By name, since that is what a call site has: every function with the
/// name a written-to call calls, and every method of that name. A call
/// whose callee has no name (a closure called where it is made) covers
/// every function.
struct WrittenResults {
    names: HashSet<String>,
    all: bool,
}

impl WrittenResults {
    fn covers(&self, function: Option<&str>) -> bool {
        if self.all {
            return true;
        }
        match function {
            // A closure is called through whatever name holds it.
            None => !self.names.is_empty(),
            Some(name) => {
                let base = crate::program::base_fn_name(name);
                let last = base.rsplit(['.', ':']).next().unwrap_or(base);
                self.names.contains(base) || self.names.contains(last)
            }
        }
    }
}

fn written_results(program: &Program, mutated: &std::collections::HashSet<TermId>) -> WrittenResults {
    let mut out = WrittenResults { names: HashSet::default(), all: false };
    for &t in mutated {
        let Some(term) = program.terms.get(t.0 as usize) else { continue };
        match &term.op {
            TermOp::Call => {
                // The callee, back through the names it was bound to.
                let mut at = term.inputs.first().copied();
                let mut name = None;
                for _ in 0..8 {
                    let Some(c) = at.and_then(|c| program.terms.get(c.0 as usize)) else { break };
                    if let TermOp::MakeClosure(f) = c.op {
                        name = program.functions.get(f.0 as usize).and_then(|f| f.name.clone());
                        break;
                    }
                    if c.name.is_some() {
                        name = c.name.clone();
                        break;
                    }
                    at = match c.op {
                        TermOp::Copy | TermOp::CellRead => c.inputs.first().copied(),
                        _ => None,
                    };
                }
                match name {
                    Some(n) => {
                        let base = crate::program::base_fn_name(&n);
                        out.names.insert(base.rsplit(['.', ':']).next().unwrap_or(base).to_string());
                        out.names.insert(base.to_string());
                    }
                    None => out.all = true,
                }
            }
            TermOp::MethodCall { name, .. } => match program.get_string_constant(*name) {
                Some(n) => {
                    out.names.insert(n.to_string());
                }
                None => out.all = true,
            },
            _ => {}
        }
    }
    out
}

/// Wrap the invariant stretches of every function of `bc` (see the module
/// docs), adding their runs and slots to `bc.statics` behind the top-level
/// ones. `slots` gets one entry per new slot, for [`follow`](super::follow).
pub(super) fn apply(
    bc: &mut BytecodeProgram,
    program: &Program,
    analysis: &StaticAnalysis,
    natives: &NativeFnTable,
    plan: &StaticPlan,
    slots: &mut Vec<SlotFollow>,
) {
    let fn_plan = FnPlan::of(program, analysis, plan);
    if fn_plan.is_empty() {
        return;
    }
    let mutated = &plan.mutated;
    let written = written_results(program, mutated);
    for i in 0..bc.fns.len() {
        let result_written = written.covers(bc.fns[i].name.as_deref());
        let kept = stretches(&bc.fns[i], bc, program, analysis, natives, &fn_plan, mutated, result_written);
        if kept.is_empty() {
            continue;
        }
        let name = bc.fns[i].name.clone();
        let first_run = bc.statics.runs.len() as u32;
        let mut first_slot: Vec<u32> = Vec::with_capacity(kept.len());
        for s in &kept {
            let f = &bc.fns[i];
            first_slot.push(bc.statics.slots.len() as u32);
            for r in &s.own {
                // The term that writes the register last inside the stretch.
                let term = (s.start..s.end)
                    .rev()
                    .find(|ip| {
                        let mut hit = false;
                        f.code[*ip].for_each_write(&bc.match_binds, |w| hit |= w == *r);
                        hit
                    })
                    .and_then(|ip| f.origins.get(ip).copied().flatten());
                bc.statics.slots.push(StaticSlotInfo {
                    name: None,
                    at: term.and_then(|t| crate::static_audit::position(program, t)),
                    term,
                    is_const: false,
                    function: Some(name.clone().unwrap_or_else(|| "a function".to_string())),
                });
                // No key and no dependency set: every reload rebuilds it.
                slots.push(SlotFollow::default());
            }
            let base = *first_slot.last().unwrap_or(&0);
            let stores: Vec<u32> = (0..s.own.len() as u32).map(|k| base + k).collect();
            let code = &f.code[s.start..s.end];
            bc.statics.runs.push(StaticRun {
                feeds: stores.clone(),
                stores,
                reads: Vec::new(),
                group: None,
                instructions: s.end - s.start,
                allocations: code
                    .iter()
                    .filter(|i| {
                        matches!(
                            i,
                            Inst::AllocList { .. }
                                | Inst::AllocMap { .. }
                                | Inst::AllocMapSpread { .. }
                                | Inst::AllocElement { .. }
                        )
                    })
                    .count(),
            });
        }
        wrap(&mut bc.fns[i], &kept, first_run, &first_slot);
    }
    bc.statics.index();
}

/// Put a `StaticSkip` in front of each stretch and a `StaticStore` behind
/// it, and move every label.
fn wrap(f: &mut BytecodeFn, kept: &[Stretch], first_run: u32, first_slot: &[u32]) {
    let n = f.code.len();
    let starts: HashMap<usize, usize> = kept.iter().enumerate().map(|(i, s)| (s.start, i)).collect();
    let ends: HashMap<usize, usize> = kept.iter().enumerate().map(|(i, s)| (s.end, i)).collect();
    let mut inside: Vec<Option<usize>> = vec![None; n];
    for (i, s) in kept.iter().enumerate() {
        inside[s.start..s.end].iter_mut().for_each(|x| *x = Some(i));
    }
    let slots_of = |i: usize| -> Box<[(u32, Reg)]> {
        kept[i].own.iter().enumerate().map(|(k, r)| (first_slot[i] + k as u32, *r)).collect()
    };
    let old_code = std::mem::take(&mut f.code);
    let old_origins = std::mem::take(&mut f.origins);
    let mut code: Vec<Inst> = Vec::with_capacity(n + 2 * kept.len());
    let mut origins: Vec<Option<TermId>> = Vec::with_capacity(code.capacity());
    let mut was: Vec<Option<usize>> = Vec::with_capacity(code.capacity());
    let mut new_pos = vec![0usize; n + 1];
    let mut skip_pos = vec![0usize; kept.len()];
    let mut store_pos = vec![0usize; kept.len()];
    let mut old = old_code.into_iter().zip(old_origins.into_iter().chain(std::iter::repeat(None)));
    for ip in 0..=n {
        if let Some(&i) = ends.get(&ip) {
            store_pos[i] = code.len();
            code.push(Inst::StaticStore { slots: slots_of(i) });
            origins.push(None);
            was.push(None);
        }
        if let Some(&i) = starts.get(&ip) {
            skip_pos[i] = code.len();
            code.push(Inst::StaticSkip { after: 0, run: first_run + i as u32, slots: slots_of(i) });
            origins.push(None);
            was.push(None);
        }
        new_pos[ip] = code.len();
        if let Some((inst, o)) = old.next() {
            code.push(inst);
            origins.push(o);
            was.push(Some(ip));
        }
    }
    let retarget = |from: usize, to: usize| -> Label {
        let from_in = inside.get(from).copied().flatten();
        // A jump out of a stretch to its end lands on the store.
        if let Some(i) = from_in
            && kept[i].end == to
        {
            return store_pos[i] as Label;
        }
        // A jump to a stretch's first instruction from outside lands on the
        // skip.
        if let Some(&i) = starts.get(&to)
            && from_in != Some(i)
        {
            return skip_pos[i] as Label;
        }
        new_pos[to.min(n)] as Label
    };
    for (inst, from) in code.iter_mut().zip(&was) {
        match (inst, from) {
            (Inst::StaticSkip { after, run, .. }, _) => {
                // Past the stretch and its store.
                let i = (*run - first_run) as usize;
                *after = new_pos[kept[i].end] as Label;
            }
            (other, Some(from)) => other.for_each_target_mut(|to| *to = retarget(*from, *to as usize)),
            _ => {}
        }
    }
    f.code = code;
    f.origins = origins;
}

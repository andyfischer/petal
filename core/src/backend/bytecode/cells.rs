//! In-place writes through `var` cells.
//!
//! `set xs[i] = v` compiles to three terms — read the cell, build the changed
//! container, write it back:
//!
//! ```text
//!   r = CellRead  [cell]
//!   m = SetIndex  [r, i, v]
//!   w = CellWrite [cell, m]
//! ```
//!
//! Neither in-place route can touch `m`. Both prove a container unique from
//! where it was *allocated* ([`super::escape`] on the term graph,
//! [`super::lastuse`] on the bytecode), and a cell's contents were allocated
//! somewhere else entirely: by another function, on another frame, or in an
//! earlier run. A cell is exactly the thing that is shared across function
//! boundaries, so no analysis of one function can say who else holds what is
//! in it. Every such write therefore copied its whole container.
//!
//! So uniqueness is tracked where it can be known: **at run time, per cell**.
//! [`Heap`](crate::heap::Heap) keeps an *owned* bit beside each cell's value,
//! set while the cell is the only holder of its container, and this pass
//! picks out the reads and writes that can keep it set:
//!
//! * **take / put** — the triple above. The read becomes a *take*: it yields
//!   the cell's own container when the cell owns it and a fresh copy when it
//!   does not, so the mutation after it is in place either way. The write
//!   becomes a *put*: it stores that container back and marks it owned. The
//!   first write after anything else got hold of the contents pays one copy;
//!   the writes after it pay none.
//! * **peek** — a read that is only looked into (`get xs[i]`, `get r.f`,
//!   `len(get xs)`). It hands the container to a register without clearing
//!   the bit, because nothing keeps it. Without this every read-modify-write
//!   (`set xs[i] = get xs[i] + 1`) would give the ownership straight back.
//!
//! Every other read hands the contents to code that may keep them, and every
//! other write stores a value that something else may hold; both clear the
//! bit ([`CellReadMode::Shared`] / [`CellWriteMode::Shared`], the forms with
//! no optimization at all). Value semantics is therefore preserved by
//! construction: `let snap = get xs` is a shared read, so the write after it
//! copies and `snap` keeps what it saw.
//!
//! ## What makes a rewrite safe
//! The rewritten registers hold the cell's container *without* the cell
//! knowing, so each one must be dead by the time anything else could run:
//!
//! 1. **A closed window.** The terms from the read to its last reader (for a
//!    take: to the write) are in one block and are all from a short list of
//!    operations that cannot run other code or touch a cell
//!    ([`window_op`]). A call in between could read the cell — and see a
//!    half-finished write, or have its own write overwritten — so it rejects.
//!    This is the same condition that makes the rewrite invisible to the
//!    clone-and-alloc oracle: with nothing between the read and the write,
//!    reading at the read and reading at the write are the same thing. A
//!    take's write also follows its mutation *directly*, so that no error can
//!    land between editing the cell's container and writing it back.
//! 2. **Every reader in the window.** The read's users are element reads of
//!    it (plus, for a take, the one mutation whose container it is), all
//!    inside the window; it is no phi's carry-out and no block's result.
//! 3. **Nothing reads the write.** A `set` is an expression: `fn put(i, v)
//!    set xs[i] = v end` returns the whole list. A put keeps the container to
//!    the cell, so its value must go nowhere ([`Analysis::result_use`]): no
//!    user, no carry-out, and not the result of a block whose value is read.
//!    The one case decided at run time is a write in *tail position*, whose
//!    value is its function's result: the caller knows whether it reads that,
//!    and says so on the callee's frame ([`ResultUse`], set on `Call`s by
//!    this same pass). `put(i, v)` as a statement drops it and the write
//!    stays in place; `let ys = put(i, v)` reads it and the write hands the
//!    list out like any other.
//!
//! A register that outlives its window still holds the id, and that is fine:
//! conditions 2 and 3 are precisely "it is never read again".
//!
//! ## Who else looks at a cell
//! Three things outside the script read a cell's contents and keep them
//! across a write, and each is handled where it lives:
//!
//! * a **memo record** keeps what a scope read, to validate against later. A
//!   peek does not clear ownership, so the value it recorded may be mutated
//!   under it; the record keeps the cell's mutation count beside the value
//!   and is invalid once that has moved (`Dep::CellRead`). A scope that
//!   *performs* an in-place write to a cell it did not create cannot be
//!   replayed at all — there is no before-value to re-apply — and is given up
//!   as effectful, as one that mutates a `state` slot in place already is. A
//!   call site that does so every time stops opening scopes
//!   (`MemoTable::note_unrecordable`): a helper called per contact, per
//!   frame, should cost a call.
//! * the **frame gate** snapshots every `state var` at the start of a run to
//!   tell whether the run changed one. It keeps the count too, and has the
//!   heap fingerprint the contents just before the first in-place write of
//!   the run, so that a script which stores the same values every frame is
//!   still seen to have settled (`Stack::cells_at_run_start`).
//! * the **observation buffer** shows a binding's last value, and for an
//!   in-place write that is the container itself, as it already is for the
//!   other two routes. The `explain` trace is a *history* of values, which an
//!   in-place write would rewrite, so this pass is off whenever the trace is
//!   on ([`OptFlags::preserve_trace`](crate::backend::OptFlags)).
//!
//! ## What does not fire
//! * `set xs = append(xs, f(x))` — the call sits between the read and the
//!   write. Bind the argument first.
//! * The inner containers of a nested write. `set g[i][j] = v` rewrites `g`
//!   in place and copies row `i`; `set ps[i].x = v` copies one record. The
//!   bit covers a cell's top-level container only, since an inner one is
//!   handed out by every `get g[i]`.
//! * A write whose value is read (`let ys = (set xs[i] = v)`), which has to
//!   hand the container out.

use std::collections::{HashMap, HashSet};

use super::isa::{CellReadMode, CellWriteMode, ResultUse};
use crate::program::{BlockId, Program, Term, TermId, TermOp};

/// The reads, writes, mutations and calls this pass rewrites, for
/// [`super::lower`] to consult. Empty means every cell access lowers to its
/// `Shared` form — the clone-and-alloc oracle.
#[derive(Debug, Default, Clone)]
pub struct CellPlan {
    reads: HashMap<TermId, CellReadMode>,
    writes: HashMap<TermId, CellWriteMode>,
    /// The mutation between a take and its put: lowered in place.
    mutations: HashSet<TermId>,
    /// `Call` terms whose result is not (or may not be) read.
    calls: HashMap<TermId, ResultUse>,
}

impl CellPlan {
    pub fn read_mode(&self, t: TermId) -> CellReadMode {
        self.reads.get(&t).copied().unwrap_or_default()
    }

    pub fn write_mode(&self, t: TermId) -> CellWriteMode {
        self.writes.get(&t).copied().unwrap_or_default()
    }

    /// Whether mutation term `t` sits between a take and its put.
    pub fn mutates_in_place(&self, t: TermId) -> bool {
        self.mutations.contains(&t)
    }

    pub fn result_use(&self, t: TermId) -> ResultUse {
        self.calls.get(&t).copied().unwrap_or_default()
    }

    /// Number of in-place writes found (diagnostics / tests).
    pub fn in_place_writes(&self) -> usize {
        self.writes.len()
    }

    /// Number of reads that leave the cell owning its contents (tests).
    pub fn peeks(&self) -> usize {
        self.reads
            .values()
            .filter(|m| **m == CellReadMode::Peek)
            .count()
    }
}

/// Find every cell access in `program` that can keep its cell's ownership.
pub fn analyze(program: &Program) -> CellPlan {
    let ctx = Analysis::build(program);
    let mut plan = CellPlan::default();
    for term in &program.terms {
        if !matches!(term.op, TermOp::CellWrite) {
            continue;
        }
        if let Some((read, mutation, mode)) = ctx.in_place_write(term) {
            plan.reads.insert(read, CellReadMode::Take);
            plan.mutations.insert(mutation);
            plan.writes.insert(term.id, mode);
        }
    }
    for term in &program.terms {
        match term.op {
            TermOp::CellRead if !plan.reads.contains_key(&term.id) && ctx.is_peek(term) => {
                plan.reads.insert(term.id, CellReadMode::Peek);
            }
            // A named call is a binding (`let ys = put(i, v)`): it has a
            // reader even with no user term, if only the observation buffer.
            TermOp::Call if term.name.is_none() => match ctx.result_use(term.id) {
                ResultUse::Read => {}
                dropped => {
                    plan.calls.insert(term.id, dropped);
                }
            },
            _ => {}
        }
    }
    plan
}

/// Whether an operation may sit inside a window (condition 1): it runs no
/// other code, and reads and writes no cell. `CellRead` is deliberately not
/// here; a peek's window admits it separately, since a read cannot disturb
/// another read.
fn window_op(program: &Program, term: &Term) -> bool {
    match &term.op {
        TermOp::Constant(_)
        | TermOp::Copy
        | TermOp::Add
        | TermOp::Sub
        | TermOp::Mul
        | TermOp::Div
        | TermOp::Mod
        | TermOp::Neg
        | TermOp::Eq
        | TermOp::Ne
        | TermOp::Lt
        | TermOp::Le
        | TermOp::Gt
        | TermOp::Ge
        | TermOp::Not
        | TermOp::Concat
        | TermOp::AllocList
        | TermOp::AllocMap { .. }
        | TermOp::AllocMapSpread { .. }
        | TermOp::GetField(_)
        | TermOp::GetFieldOpt(_)
        | TermOp::GetIndex
        | TermOp::GetIndexOpt
        | TermOp::SetField(_)
        | TermOp::SetIndex => term.child_blocks.is_empty(),
        TermOp::BuiltinCall(_) => is_mutating_call(program, term) || is_len_call(program, term),
        _ => false,
    }
}

fn builtin_name<'p>(program: &'p Program, term: &Term) -> Option<&'p str> {
    match &term.op {
        TermOp::BuiltinCall(cid) => program.get_string_constant(*cid),
        _ => None,
    }
}

/// A call to a mutating builtin (`append`, `drop_last`, …), written
/// positionally: its container is `inputs[0]` and its result is that
/// container, changed.
fn is_mutating_call(program: &Program, term: &Term) -> bool {
    builtin_name(program, term).is_some_and(crate::builtins::is_mutating_builtin)
        && term.arg_names.iter().all(Option::is_none)
}

/// `len(x)`: measures its argument and keeps nothing.
fn is_len_call(program: &Program, term: &Term) -> bool {
    builtin_name(program, term) == Some("len")
        && term.inputs.len() == 1
        && term.arg_names.iter().all(Option::is_none)
}

/// Whether `term` is a mutation whose container is `inputs[0]`.
fn is_mutation(program: &Program, term: &Term) -> bool {
    matches!(term.op, TermOp::SetIndex | TermOp::SetField(_)) || is_mutating_call(program, term)
}

/// Whether `user` only looks into `read` — takes an element out of it or
/// measures it — with `read` in the container position and nowhere else.
fn looks_into(program: &Program, user: &Term, read: TermId) -> bool {
    let in_container_slot_only =
        user.inputs.first() == Some(&read) && user.inputs[1..].iter().all(|&t| t != read);
    in_container_slot_only
        && (matches!(
            user.op,
            TermOp::GetField(_) | TermOp::GetFieldOpt(_) | TermOp::GetIndex | TermOp::GetIndexOpt
        ) || is_len_call(program, user))
}

/// Dataflow relations the checks ask about, built once per program.
struct Analysis<'p> {
    program: &'p Program,
    /// Reverse input edges: every term naming a term as any input.
    users: HashMap<TermId, Vec<TermId>>,
    /// Terms carried out of their block into a phi. That copy goes through
    /// the register, so it is not a user edge.
    phi_carry_srcs: HashSet<TermId>,
    /// Each term's position in its block's execution order. Phantoms (params,
    /// captures) execute nowhere and are absent.
    position: HashMap<TermId, usize>,
    /// Each block's terms in execution order.
    order: HashMap<BlockId, Vec<TermId>>,
    /// Function body blocks: their result is a function's return value.
    fn_bodies: HashSet<BlockId>,
}

impl<'p> Analysis<'p> {
    fn build(program: &'p Program) -> Analysis<'p> {
        let mut users: HashMap<TermId, Vec<TermId>> = HashMap::new();
        for term in &program.terms {
            for &inp in &term.inputs {
                users.entry(inp).or_default().push(term.id);
            }
        }
        let mut phi_carry_srcs = HashSet::new();
        let mut position = HashMap::new();
        let mut order: HashMap<BlockId, Vec<TermId>> = HashMap::new();
        for block in &program.blocks {
            for po in &block.phi_outs {
                phi_carry_srcs.insert(po.src_term);
            }
            let terms = order.entry(block.id).or_default();
            let mut cur = block.entry;
            // Bounded by the term count, so a malformed link cycle in
            // hand-written IR cannot hang the pass.
            while let Some(tid) = cur
                && terms.len() <= program.terms.len()
            {
                position.insert(tid, terms.len());
                terms.push(tid);
                cur = program.get_term(tid).block_next;
            }
        }
        Analysis {
            program,
            users,
            phi_carry_srcs,
            position,
            order,
            fn_bodies: program.functions.iter().map(|f| f.body_block).collect(),
        }
    }

    fn users_of(&self, t: TermId) -> &[TermId] {
        self.users.get(&t).map(Vec::as_slice).unwrap_or(&[])
    }

    fn is_block_result(&self, t: &Term) -> bool {
        self.order
            .get(&t.block_id)
            .and_then(|terms| terms.last())
            == Some(&t.id)
    }

    /// Whether every term strictly between positions `from` and `to` of
    /// `block` satisfies `ok`.
    fn window_is(&self, block: BlockId, from: usize, to: usize, ok: impl Fn(&Term) -> bool) -> bool {
        let Some(terms) = self.order.get(&block) else {
            return false;
        };
        terms
            .get(from + 1..to)
            .is_some_and(|w| w.iter().all(|&t| ok(self.program.get_term(t))))
    }

    /// If `write` is the last term of a take/put triple, the read, the
    /// mutation, and the mode the write lowers with (the module docs'
    /// conditions 1–3).
    fn in_place_write(&self, write: &Term) -> Option<(TermId, TermId, CellWriteMode)> {
        let program = self.program;
        let &[cell, value] = write.inputs.as_slice() else {
            return None;
        };
        // The written value is a mutation of what the same cell just held.
        let mutation = program.get_term(value);
        if !is_mutation(program, mutation) || mutation.name.is_some() {
            return None;
        }
        let read = program.get_term(*mutation.inputs.first()?);
        if !matches!(read.op, TermOp::CellRead) || read.inputs.as_slice() != [cell] {
            return None;
        }
        if read.name.is_some() || mutation.inputs[1..].contains(&read.id) || value == cell {
            return None;
        }

        // 1. One block, in order, with nothing in between that could run
        // other code or touch a cell.
        if read.block_id != write.block_id || mutation.block_id != write.block_id {
            return None;
        }
        let (r, m, w) = (
            *self.position.get(&read.id)?,
            *self.position.get(&mutation.id)?,
            *self.position.get(&write.id)?,
        );
        // The write follows its mutation directly: once the cell's own
        // container has been edited, nothing may fail before the write that
        // was supposed to make the edit visible (a `state var` outlives a run
        // that errors, and must hold what clone-and-alloc would have left).
        if !(r < m && w == m + 1)
            || !self.window_is(write.block_id, r, w, |t| window_op(program, t))
        {
            return None;
        }

        // 2. The read is looked into, mutated once, and nothing else.
        if self.phi_carry_srcs.contains(&read.id) {
            return None;
        }
        for &u in self.users_of(read.id) {
            let user = program.get_term(u);
            let in_window = user.block_id == write.block_id
                && self.position.get(&u).is_some_and(|&p| r < p && p <= m);
            if !in_window || !(u == mutation.id || looks_into(program, user, read.id)) {
                return None;
            }
        }
        // The mutated container goes to the write and nowhere else.
        if self.users_of(mutation.id) != [write.id] || self.phi_carry_srcs.contains(&mutation.id) {
            return None;
        }

        // 3. Nothing reads the write itself.
        match self.result_use(write.id) {
            ResultUse::Dropped => Some((read.id, mutation.id, CellWriteMode::Put)),
            ResultUse::Forwarded => Some((read.id, mutation.id, CellWriteMode::PutTail)),
            ResultUse::Read => None,
        }
    }

    /// Whether `read` (a `CellRead`) is only looked into, by terms that follow
    /// it with nothing in between that could write a cell.
    fn is_peek(&self, read: &Term) -> bool {
        let program = self.program;
        if read.name.is_some()
            || self.phi_carry_srcs.contains(&read.id)
            || self.is_block_result(read)
        {
            return false;
        }
        let Some(&r) = self.position.get(&read.id) else {
            return false;
        };
        let users = self.users_of(read.id);
        let mut last = r;
        for &u in users {
            let user = program.get_term(u);
            let Some(&p) = self.position.get(&u) else {
                return false;
            };
            if user.block_id != read.block_id || p <= r || !looks_into(program, user, read.id) {
                return false;
            }
            last = last.max(p);
        }
        // No user at all is a dead read: nothing to gain, nothing to check.
        !users.is_empty()
            && self.window_is(read.block_id, r, last, |t| {
                // Reads only: a mutation here could be the middle of a take
                // of this very cell.
                matches!(t.op, TermOp::CellRead)
                    || (window_op(program, t) && !is_mutation(program, t))
            })
    }

    /// Whether anything reads the value of `t`, as far as this function can
    /// tell: `Dropped` when nothing does, `Forwarded` when it is the
    /// function's own result (so the caller decides), `Read` otherwise.
    ///
    /// A value is read through a user term, through a phi carry-out, or by
    /// being the result of a block whose value is read. The last one walks
    /// outward: an `if` or `match` arm's result becomes the control term's
    /// value, a statement loop's body result is discarded, and everything
    /// else that takes a block's result (a collecting loop, a `while`
    /// condition, a match guard, `&&`/`||`/`??`, a `state` initializer) reads
    /// it.
    fn result_use(&self, t: TermId) -> ResultUse {
        let program = self.program;
        let mut cur = program.get_term(t);
        // Bounded by the nesting depth; the cap only guards malformed IR.
        for _ in 0..=program.blocks.len() {
            if !self.users_of(cur.id).is_empty() || self.phi_carry_srcs.contains(&cur.id) {
                return ResultUse::Read;
            }
            if !self.is_block_result(cur) {
                return ResultUse::Dropped;
            }
            let block = cur.block_id;
            if block == program.root_block || self.fn_bodies.contains(&block) {
                return ResultUse::Forwarded;
            }
            let Some(parent) = program.get_block(block).parent_term_id else {
                return ResultUse::Read;
            };
            let parent = program.get_term(parent);
            match parent.op {
                TermOp::Branch if parent.child_blocks.contains(&block) => {}
                TermOp::Match
                    if program
                        .match_arms
                        .get(&parent.id)
                        .is_some_and(|arms| arms.iter().any(|a| a.body_block == block)) => {}
                TermOp::ForLoop | TermOp::NumericForLoop
                    if !parent.collect && parent.child_blocks.first() == Some(&block) =>
                {
                    return ResultUse::Dropped;
                }
                TermOp::WhileLoop
                    if !parent.collect && parent.child_blocks.get(1) == Some(&block) =>
                {
                    return ResultUse::Dropped;
                }
                _ => return ResultUse::Read,
            }
            // The arm's result is the control term's value: ask about that. A
            // named control term is a binding, which has a reader.
            if parent.name.is_some() {
                return ResultUse::Read;
            }
            cur = parent;
        }
        ResultUse::Read
    }
}

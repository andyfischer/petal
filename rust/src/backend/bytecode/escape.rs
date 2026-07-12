//! Escape / uniqueness analysis over the term graph (M4).
//!
//! Determines which collection-mutating terms (`SetField`/`SetIndex` and the
//! mutating builtins `append`/`drop_last`/`remove`/`set`/`swap`) operate on a
//! container that is **statically unique + non-escaping** — so lowering may emit
//! an *in-place* opcode (or set the in-place flag on a builtin call) instead of
//! clone-and-alloc, without changing observable value semantics.
//!
//! ## What fires: the loop-carried accumulator (route B)
//! The dominant mutation pattern in real Petal code is the loop-carried
//! accumulator — `xs = append(xs, v)` inside a loop (`append.ptl`,
//! `game_of_life.ptl`, `particles.ptl`). The container is a loop-carried phi
//! whose value is rebuilt every iteration; a per-iteration clone is quadratic.
//! This analysis proves that each iteration holds the container *exclusively*,
//! so the mutation can grow it in place.
//!
//! The proof works over the container's **value-web**: the set of terms that
//! carry the same heap id, connected by `Copy` (alias), `Phi` (loop/branch
//! merge), and mutation (container-input → result) edges. A web is in-place-safe
//! when:
//!
//! 1. **Unique fresh root.** Exactly one web term is not a carrier — it is a
//!    fresh `Alloc*` in this function. (A param, capture, call result, or
//!    state-read root could alias something else, so it is rejected.)
//! 2. **One loop spine.** Exactly one web phi's init resolves to that root; it is
//!    the loop-carried phi `P_loop`, and its loop body block defines the
//!    *region* — the block subtree over which the accumulator is live.
//! 3. **All mutations in-region.** Every mutation in the web is inside the loop
//!    body. (A post-loop mutation of the finished value could alias a surviving
//!    reference, so it is rejected.)
//! 4. **Linear use inside the region.** Every web term's in-region readers are
//!    themselves web terms and *linear*: at most one, unless they sit in
//!    mutually-exclusive branch/match arms (which is how `game_of_life.ptl`'s
//!    `if cell == 1 then row = append(row, …) else row = append(row, …) end`
//!    lowers — two mutations, one per arm). A non-web in-region reader (an
//!    in-loop `len(xs)`, a store into another container, a closure capture, a
//!    state write) breaks uniqueness and rejects the web.
//!
//! Reads of the *final* value after the loop (`len(xs)`, `next = append(next,
//! row)`, `return xs`) are unrestricted: in-place mutation produces exactly the
//! same final list, so any downstream observation is unaffected.
//!
//! **Soundness.** The heap is immutable-by-construction, so a dataflow edge to a
//! container's producing term is the *only* way any code observes it. The web
//! enumerates every carrier; conditions 1–4 establish that within each iteration
//! the id in `P_loop` is referenced solely by that iteration's linear
//! mutation chain, and the back-edge writes the (identical) mutated id forward.
//! No live observer ever sees a pre-mutation state. Fork safety is automatic:
//! `Heap::fork` deep-copies the slot vectors, so a speculative child mutates its
//! own copy (see `docs/dev/speculative-execution-plan.md`).
//!
//! ## Companion pass: straight-line uniqueness (route A)
//! Straight-line last-use uniqueness (`let xs = […]; xs[0] = v` where `xs` is
//! dead after) is handled separately by [`super::lastuse`] — a rewrite pass
//! over the *lowered bytecode* (gated by `OptFlags::in_place_straight_line`),
//! where the linear instruction order makes last-use a reachability question.
//! This graph-side analysis stays focused on the loop-carried phi cycle, which
//! bytecode-level liveness cannot prove (the accumulator is live around the
//! back edge by construction).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::program::{BlockId, Program, Term, TermId, TermOp};

/// Terms whose container input is provably unique + non-escaping, and may
/// therefore be lowered to an in-place mutation.
#[derive(Debug, Default, Clone)]
pub struct InPlaceSet {
    terms: HashSet<TermId>,
}

impl InPlaceSet {
    /// Whether the mutation term `t` may be lowered in place.
    pub fn allows(&self, t: TermId) -> bool {
        self.terms.contains(&t)
    }

    /// Number of terms proven in-place-safe (diagnostics / tests).
    pub fn len(&self) -> usize {
        self.terms.len()
    }

    /// Whether no mutation was proven in-place-safe.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// Analyze a program and return the set of in-place-eligible mutation terms.
pub fn analyze(program: &Program) -> InPlaceSet {
    let ctx = Analysis::build(program);
    let mut terms = HashSet::new();
    for term in &program.terms {
        if ctx.is_mutation(term) && ctx.route_b_ok(term.id) {
            terms.insert(term.id);
        }
    }
    InPlaceSet { terms }
}

/// Precomputed dataflow relations for the analysis, built once per program.
struct Analysis<'p> {
    program: &'p Program,
    /// For each phi term, the `phi_out` back-edge source terms (dest == phi).
    phi_srcs: HashMap<TermId, Vec<TermId>>,
    /// Reverse "read" edges: for each term `w`, the terms that read `w` as a
    /// *carried* input — a `Copy` source, a mutation's container, or a phi's
    /// init. Excludes phi back-edge sources (a carry-forward, not a live read).
    read_consumers: HashMap<TermId, Vec<TermId>>,
    /// Reverse direct-input edges: for each term, every term naming it as *any*
    /// input (used to catch a non-carrier reader observing the container).
    users: HashMap<TermId, Vec<TermId>>,
    /// Block-subtree membership cache, filled lazily per region root.
    block_children: HashMap<BlockId, Vec<BlockId>>,
}

impl<'p> Analysis<'p> {
    fn build(program: &'p Program) -> Analysis<'p> {
        let mut phi_srcs: HashMap<TermId, Vec<TermId>> = HashMap::new();
        for block in &program.blocks {
            for po in &block.phi_outs {
                phi_srcs.entry(po.dest_term).or_default().push(po.src_term);
            }
        }

        // Direct child blocks of each block (via its terms' child_blocks and
        // match-arm blocks) — the parent→children edges for region subtrees.
        let mut block_children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for term in &program.terms {
            for &cb in &term.child_blocks {
                block_children.entry(term.block_id).or_default().push(cb);
            }
            if matches!(term.op, TermOp::Match) {
                if let Some(arms) = program.match_arms.get(&term.id) {
                    for arm in arms {
                        let e = block_children.entry(term.block_id).or_default();
                        e.push(arm.body_block);
                        if let Some(g) = arm.guard_block {
                            e.push(g);
                        }
                    }
                }
            }
        }

        let mut ctx = Analysis {
            program,
            phi_srcs,
            read_consumers: HashMap::new(),
            users: HashMap::new(),
            block_children,
        };

        // Build reverse edges now that `ctx` can classify carried inputs.
        let mut read_consumers: HashMap<TermId, Vec<TermId>> = HashMap::new();
        let mut users: HashMap<TermId, Vec<TermId>> = HashMap::new();
        for term in &program.terms {
            for w in ctx.read_inputs(term) {
                read_consumers.entry(w).or_default().push(term.id);
            }
            for &inp in &term.inputs {
                users.entry(inp).or_default().push(term.id);
            }
        }
        ctx.read_consumers = read_consumers;
        ctx.users = users;
        ctx
    }

    /// The container input of a mutation term (`inputs[0]` for every kind), or
    /// `None` if `term` is not a mutation.
    fn is_mutation(&self, term: &Term) -> bool {
        match &term.op {
            TermOp::SetIndex | TermOp::SetField(_) => true,
            TermOp::BuiltinCall(cid) => self
                .program
                .get_string_constant(*cid)
                .is_some_and(crate::builtins::is_mutating_builtin),
            _ => false,
        }
    }

    /// Terms `t` reads as a *carried alias input* (the value flows through
    /// unchanged): a `Copy` source, a mutation's container, or a phi's init.
    /// These are the edges whose reverse gives "who reads this value".
    fn read_inputs(&self, t: &Term) -> Vec<TermId> {
        match &t.op {
            TermOp::Copy => t.inputs.first().copied().into_iter().collect(),
            TermOp::Phi => t.inputs.first().copied().into_iter().collect(),
            _ if self.is_mutation(t) => t.inputs.first().copied().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// All *carrier* neighbors of `t` for the undirected value-web traversal:
    /// its carried inputs (read inputs) plus, for a phi, its back-edge sources.
    fn carrier_inputs(&self, t: &Term) -> Vec<TermId> {
        let mut v = self.read_inputs(t);
        if matches!(t.op, TermOp::Phi) {
            if let Some(srcs) = self.phi_srcs.get(&t.id) {
                v.extend(srcs.iter().copied());
            }
        }
        v
    }

    /// A term carries a value through unchanged (`Copy`/`Phi`/mutation); the sole
    /// non-carrier in a valid web is the fresh-alloc root.
    fn is_carrier(&self, term: &Term) -> bool {
        matches!(term.op, TermOp::Copy | TermOp::Phi) || self.is_mutation(term)
    }

    fn is_fresh_alloc(term: &Term) -> bool {
        matches!(
            term.op,
            TermOp::AllocList
                | TermOp::AllocMap { .. }
                | TermOp::AllocMapSpread { .. }
                | TermOp::AllocElement { .. }
        )
    }

    /// Follow `Copy` (and single-input `Phi`… no) chains backward to the first
    /// non-`Copy` term.
    fn strip_copies(&self, mut t: TermId) -> TermId {
        loop {
            let term = self.program.get_term(t);
            match &term.op {
                TermOp::Copy => t = term.inputs[0],
                _ => return t,
            }
        }
    }

    /// Backward cone of `seed` over *container* carrier inputs (copy sources,
    /// mutation containers, phi inits, and phi back-edge sources). Forward
    /// consumers are deliberately excluded, so the value is not followed once it
    /// escapes the accumulator's loop — that keeps two independent accumulators
    /// (`next` and the `particles` it feeds) from merging into one web.
    fn backward_cone(&self, seed: TermId) -> HashSet<TermId> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(seed);
        queue.push_back(seed);
        while let Some(w) = queue.pop_front() {
            for n in self.carrier_inputs(self.program.get_term(w)) {
                if seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        seen
    }

    /// All terms reachable backward from `seeds` over carrier inputs — the value
    /// sources that feed into `seeds`. Used to test spine membership: whether a
    /// mutation's result flows back into a loop phi's back edge.
    fn backward_carrier_closure(&self, seeds: &[TermId]) -> HashSet<TermId> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<TermId> = VecDeque::new();
        for &s in seeds {
            if seen.insert(s) {
                queue.push_back(s);
            }
        }
        while let Some(w) = queue.pop_front() {
            for n in self.carrier_inputs(self.program.get_term(w)) {
                if seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        seen
    }

    /// A phi whose back-edge lives in a loop body block (as opposed to a branch
    /// or match arm) — the loop-carried spine of an accumulator.
    fn is_loop_phi(&self, phi: TermId) -> bool {
        if !matches!(self.program.get_term(phi).op, TermOp::Phi) {
            return false;
        }
        match self.body_block_of(phi) {
            Some(b) => match self.program.get_block(b).parent_term_id {
                Some(l) => matches!(
                    self.program.get_term(l).op,
                    TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop
                ),
                None => false,
            },
            None => false,
        }
    }

    /// Route B: is `seed` (a mutation term) a safe loop-carried accumulator?
    fn route_b_ok(&self, seed: TermId) -> bool {
        // Locate the single loop-carried phi behind the mutation via a backward
        // (producer-only) walk, so post-loop escapes never enter the picture.
        let cone = self.backward_cone(seed);
        let mut p_loop: Option<TermId> = None;
        for &t in &cone {
            if self.is_loop_phi(t) {
                if p_loop.replace(t).is_some() {
                    return false; // more than one loop spine — ambiguous
                }
            }
        }
        let Some(p_loop) = p_loop else { return false };

        // The root is P_loop's init, which must be a fresh, uniquely-owned alloc
        // (a param/capture/call/state root could already be aliased).
        let root = self.strip_copies(self.program.get_term(p_loop).inputs[0]);
        if !Self::is_fresh_alloc(self.program.get_term(root)) {
            return false;
        }

        // `seed` must be *on the spine*: its result has to flow back into
        // `p_loop`'s back edge. A mutation whose result is discarded — e.g.
        // `let ys = xs; ys = append(ys, v)` inside the loop, where `xs` is the
        // real carried value and `ys` a throwaway alias — is NOT the
        // accumulator; mutating it in place would corrupt the aliased `xs`.
        let back_srcs: Vec<TermId> = self.phi_srcs.get(&p_loop).cloned().unwrap_or_default();
        let spine = self.backward_carrier_closure(&back_srcs);
        if !spine.contains(&seed) {
            return false;
        }

        // The region is the loop body subtree: the window over which the
        // accumulator is live and being mutated.
        let Some(body_block) = self.body_block_of(p_loop) else {
            return false;
        };
        let region = self.block_subtree(body_block);

        // Build the region-confined value-web: carriers connected to `seed`
        // within the region, plus the boundary spine (`p_loop`, `root`). Only
        // carriers (and the root) are ever added — a non-carrier in-region
        // reader is caught during validation, not folded into the web.
        let web = self.build_confined_web(seed, p_loop, root, &region);

        // (1) Unique fresh root: the only non-carrier in the web is `root`.
        for &t in &web {
            let term = self.program.get_term(t);
            if !self.is_carrier(term) && t != root {
                return false;
            }
        }

        // (1b) The fresh root must flow *only* into the accumulator spine. A
        // reference taken before the loop (`let ys = xs`) aliases the initial
        // container and would observe every in-place mutation, so any root user
        // outside the web rejects the whole accumulator. (Post-loop reads are
        // reads of the *final* value and stay safe; those never read the root.)
        for &u in self.users.get(&root).into_iter().flatten() {
            if !web.contains(&u) {
                return false;
            }
        }

        // (2) Closed phi sources: every phi in the web draws its init and every
        // back-edge only from web terms — so no foreign value merges into the
        // spine (e.g. a re-`let xs = []` inside the loop).
        for &t in &web {
            let term = self.program.get_term(t);
            if matches!(term.op, TermOp::Phi) {
                for c in self.carrier_inputs(term) {
                    if !web.contains(&c) {
                        return false;
                    }
                }
            }
        }

        // (3) & (4): all mutations in-region; in-region observers are web
        // carriers and linear (≤1, or in mutually-exclusive branch/match arms).
        for &t in &web {
            let term = self.program.get_term(t);
            if self.is_mutation(term) && !region.contains(&term.block_id) {
                return false; // a post-loop mutation of the finished value
            }

            let in_region_users: Vec<TermId> = self
                .users
                .get(&t)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&u| region.contains(&self.program.get_term(u).block_id))
                .collect();
            for &u in &in_region_users {
                if !web.contains(&u) {
                    return false; // a non-carrier observes the container mid-build
                }
            }
            // Linearity over the carrier readers (a phi's back-edge write is a
            // carry-forward, not a competing read, so it is excluded here).
            let readers: Vec<TermId> = in_region_users
                .into_iter()
                .filter(|u| self.read_consumers.get(&t).is_some_and(|rc| rc.contains(u)))
                .collect();
            if readers.len() > 1 && !self.all_mutually_exclusive(&readers) {
                return false;
            }
        }
        true
    }

    /// BFS the region-confined carrier web from `seed`, always including the
    /// spine boundary `p_loop`/`root`. Expansion visits carrier inputs, carrier
    /// readers, and phis fed on a back edge, but only *adds* a term when it is a
    /// carrier (or the root) that is in-region or is the spine boundary.
    fn build_confined_web(
        &self,
        seed: TermId,
        p_loop: TermId,
        root: TermId,
        region: &HashSet<BlockId>,
    ) -> HashSet<TermId> {
        let mut web = HashSet::new();
        let mut queue = VecDeque::new();
        for t in [seed, p_loop, root] {
            if web.insert(t) {
                queue.push_back(t);
            }
        }
        while let Some(w) = queue.pop_front() {
            let mut neighbors = self.carrier_inputs(self.program.get_term(w));
            if let Some(rc) = self.read_consumers.get(&w) {
                neighbors.extend(rc.iter().copied()); // carrier readers
            }
            for (&phi, srcs) in &self.phi_srcs {
                if srcs.contains(&w) {
                    neighbors.push(phi); // phis w feeds on a back edge
                }
            }
            for n in neighbors {
                let term = self.program.get_term(n);
                let allowed = region.contains(&term.block_id) || n == p_loop || n == root;
                let is_member = self.is_carrier(term) || n == root;
                if allowed && is_member && web.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        web
    }

    /// The block whose `phi_outs` carry a value back into `phi` (its loop body).
    fn body_block_of(&self, phi: TermId) -> Option<BlockId> {
        for block in &self.program.blocks {
            if block.phi_outs.iter().any(|po| po.dest_term == phi) {
                return Some(block.id);
            }
        }
        None
    }

    /// All blocks in the subtree rooted at `block` (inclusive), via child blocks.
    fn block_subtree(&self, block: BlockId) -> HashSet<BlockId> {
        let mut out = HashSet::new();
        let mut stack = vec![block];
        while let Some(b) = stack.pop() {
            if !out.insert(b) {
                continue;
            }
            if let Some(children) = self.block_children.get(&b) {
                stack.extend(children.iter().copied());
            }
        }
        out
    }

    /// Whether the given consumer terms are pairwise mutually exclusive — each
    /// pair diverges at a common `Branch`/`Match` into distinct arms.
    fn all_mutually_exclusive(&self, terms: &[TermId]) -> bool {
        for (i, &a) in terms.iter().enumerate() {
            for &b in &terms[i + 1..] {
                if !self.blocks_exclusive(
                    self.program.get_term(a).block_id,
                    self.program.get_term(b).block_id,
                ) {
                    return false;
                }
            }
        }
        true
    }

    /// Two blocks are mutually exclusive if their ancestor arm-paths share a
    /// `Branch`/`Match` control term but enter it through different arms.
    fn blocks_exclusive(&self, b1: BlockId, b2: BlockId) -> bool {
        if b1 == b2 {
            return false;
        }
        let p1 = self.arm_path(b1);
        let p2 = self.arm_path(b2);
        for (&l, &arm1) in &p1 {
            if let Some(&arm2) = p2.get(&l) {
                if arm1 != arm2
                    && matches!(self.program.get_term(l).op, TermOp::Branch | TermOp::Match)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Map from each enclosing control term to the arm block on the path from the
    /// program root down to `block`.
    fn arm_path(&self, block: BlockId) -> HashMap<TermId, BlockId> {
        let mut map = HashMap::new();
        let mut cur = block;
        loop {
            let blk = self.program.get_block(cur);
            let Some(l) = blk.parent_term_id else { break };
            map.insert(l, cur);
            cur = self.program.get_term(l).block_id;
        }
        map
    }
}

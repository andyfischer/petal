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
//!    *fresh* value produced in this function: an `Alloc*` term, or a call whose
//!    result is provably a brand-new unaliased container (see
//!    [`Analysis::is_fresh_root`]). A param, capture, or state read could alias
//!    something else, so it is rejected.
//! 2. **One spine.** Every phi the seed reads back through — loop phis *and* the
//!    merge phis of any `if`/`match` the loop sits in — must hang off the root:
//!    each phi's init resolves, through `Copy` carriers, to the root or to
//!    another spine phi. That makes the spine a *tree* (a loop nested in another
//!    loop's body is a child, and so is a loop that runs after it), and together
//!    with the root and the connecting copies it is the accumulator's
//!    *backbone*. Each phi's region is the subtree of the blocks that carry a
//!    value back into it — the loop body, or every arm of the merge — and their
//!    union is the *region*, the window over which the accumulator is live.
//!
//!    A connecting `Copy` is a **snapshot** of the value where it stands, while
//!    a phi's register is kept current by its construct's carry-outs. So a copy
//!    is only a valid link when no mutation lands between it and the phi reading
//!    it: `let a = xs` before a loop that mutates `xs`, with `a` then driving a
//!    second loop, is two spines over one id and both would mutate it.
//! 3. **All mutations in-region.** Every mutation in the web is inside the
//!    region. (A post-loop mutation of the finished value could alias a
//!    surviving reference, so it is rejected.)
//! 4. **Linear use inside the region.** Every web term's in-region readers are
//!    themselves web terms and *linear*: at most one, unless they sit in
//!    mutually-exclusive branch/match arms (which is how `game_of_life.ptl`'s
//!    `if cell == 1 then row = append(row, …) else row = append(row, …) end`
//!    lowers — two mutations, one per arm). A non-web in-region user is allowed
//!    only when it *observes* the container rather than *retaining* it — `xs[i]`,
//!    `r.f`, `len(xs)` — and does so before the id is rewritten; see
//!    [`Analysis::observation_index`] and [`Analysis::first_mutation_index_after`],
//!    which carry the argument. Anything that keeps the id (a store into another
//!    container, a closure capture, a state write, a user-function argument)
//!    breaks uniqueness and rejects the web.
//! 5. **Closed backbone interior.** Every backbone term *except the exempt phis*
//!    — the root, the connecting copies, and each non-exempt phi — has all of
//!    its users inside the web (or observing it, per condition 4, before the id
//!    is rewritten). Those terms hold a *mid-build* id that a later region still
//!    mutates, so a retained outside reference (`let ys = xs` between two loops,
//!    or before the first) would observe the in-place writes. A phi is exempt —
//!    its readers see the finished value — when every spine phi after it in
//!    execution order has a region *inside* its own, which is exactly what makes
//!    a guard's merge phi (whose arms contain the loop) readable after the `if`.
//! 6. **Live spines stay separate.** Two spine phis with the same parent must
//!    not have overlapping regions. A tree is how one container's spine branches
//!    into a nested loop and a later one — regions disjoint in time — whereas two
//!    phis over the same parent value whose regions overlap are two accumulators
//!    aliasing one id (`let al = xs` at the top of a loop, then both appended in
//!    it), and both would mutate it. Relatedly, a spine phi must *receive* the
//!    mutations of any arm that performs them: an arm that mutates an alias and
//!    carries the untouched original back into the merge leaves that merge a
//!    live pre-mutation holder. Both conditions came from the fuzzer.
//! 7. **No escape without an input edge.** Two value flows have no input edge to
//!    show for them, and both are checked explicitly: a **block result** (an
//!    `if` arm's value, a `collect` loop's element, a function's return) and a
//!    **phi carry-out** into a phi that is not on the spine. The second is how a
//!    branch smuggles an alias past the analysis — `if i == 1 then keep = xs
//!    else xs = append(xs, i) end` copies the accumulator into `keep`'s phi,
//!    which then watches later iterations mutate it. The one carry-out that is
//!    not a second holder is the merge phi of the *state variable this web owns*
//!    (`a = f64_array(n)` inside a guard, with the loop's result leaving in `a`'s
//!    own register); that one is allowed, and what it carries the value on to is
//!    checked in turn.
//!
//! Reads of the *final* value after the last loop (`len(xs)`, `next =
//! append(next, row)`, `return xs`) are unrestricted: in-place mutation produces
//! exactly the same final list, so any downstream observation is unaffected.
//!
//! ## The `state`-backed accumulator
//! A frame-loop simulation keeps its arrays in `state` so they survive between
//! runs, so the root is a `StateInit`/`StateRead`, not a fresh alloc — and the
//! slot's id *outlives the run*, which is a strictly stronger obligation than
//! the conditions above discharge. A state-rooted web additionally requires
//! (see [`Analysis::state_web_ok`]):
//!
//! * **One slot, one reader.** The key must name a single runtime slot, and a
//!   base key only does that when its **path is statically empty**. A
//!   `RuntimeStateKey` is `(declaration id, path)`, and the path is composed
//!   from *live* context: a `Call` part per frame on the way in, an `Index`
//!   part per enclosing loop iteration, or a single `Key` part hashed from an
//!   explicit `state(expr)` value. Only a declaration at module scope, outside
//!   every loop, runs on the empty path and therefore owns exactly one slot for
//!   the whole program. A declaration *inside a function* is a slot per
//!   callsite chain that reaches it, one inside a loop is a slot per iteration,
//!   and an explicit key is a slot per runtime value — in each case the base key
//!   alone cannot say which slot a read filled or a write commits to, so no web
//!   may root on one (plan §3.7). Accesses are checked against the same rule
//!   rather than assumed: a `StateWrite` nested deeper in loops than its
//!   declaration is fine, because its `path_pop` drops exactly those `Index`
//!   parts and lands it back on the declaration's slot — that is the top-level
//!   accumulator (`state xs = []` plus `xs = append(xs, i)` in a `for`), which
//!   is the shape this whole section exists to optimize. The key must also have
//!   exactly one `StateInit`/`StateRead` term — this web's root — so no second
//!   read hands the id elsewhere.
//!
//!   This is deliberately the coarse v1 rule. Winning back the in-function
//!   cases needs a "the path is statically fixed at this site" analysis (a
//!   declaration whose every access provably shares one live path), which is
//!   out of scope until profiles ask for it; post-migration the accumulator
//!   patterns live at top level anyway.
//! * **Immediate commit.** Every mutation in the web feeds a `StateWrite` of
//!   that same key directly. The slot therefore holds the mutated id at every
//!   instruction boundary, which is exactly what value semantics commits there
//!   too — so a run that errors partway leaves the same state behind either way.
//! * **Unique writers.** Every value written into the key (each `StateWrite`
//!   input and the `StateInit` init-block result) is either this web's own
//!   accumulator or a value freshly allocated *at the write site* and aliased
//!   nowhere — `state b = a` would otherwise put one id into two slots that both
//!   outlive the run, and a hoisted allocation assigned inside a loop would put
//!   one id into a slot the loop keeps overwriting.
//! * **No retention past the region.** Out-of-region users of web terms must
//!   observe rather than retain: `push_output(s, xs)` parks the id where the
//!   host drains it *after* the run, and a second state slot or a closure keeps
//!   it across runs. Plain reads of the finished value stay fine.
//!
//! The host side is an **API contract**, not an analysis: a `Value` from
//! `Env::get_state`/`get_all_state` is a snapshot that must not be held across a
//! run. `Env::fork_execution` is unaffected — it deep-copies the heap, so a
//! fork mutates its own slots — and `Env::transfer_state` keeps the one stack it
//! reshapes, so it never duplicates a slot into a second live stack.
//!
//! **Soundness.** The heap is immutable-by-construction, so a dataflow edge to a
//! container's producing term is the *only* way any code observes it. The web
//! enumerates every carrier; conditions 1–6 establish that within each iteration
//! the id in the governing phi is referenced solely by that iteration's linear
//! mutation chain, and the back-edge writes the (identical) mutated id forward.
//! No live observer ever sees a pre-mutation state. Fork safety is automatic:
//! `Heap::fork` deep-copies the slot vectors, so a speculative child mutates its
//! own copy (see `docs/dev/bytecode-future-ideas.md`, "Hazards" section).
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

use crate::program::{BlockId, Program, StateKey, Term, TermId, TermOp};

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

/// Whether a state term addresses the **empty state path** — the one slot a
/// declaration id owns all by itself.
///
/// A `RuntimeStateKey` is `(declaration id, path)`, and every path part comes
/// from live context: a `Call` part per frame entered on the way in, an `Index`
/// part per enclosing loop iteration. So the base key names one fixed slot only
/// for a term at module scope, outside every loop.
///
/// The one exception is [`Term::path_pop`](crate::program::Term::path_pop): a
/// `StateWrite`/`StateRead` nested `path_pop` loop levels deeper than its
/// declaration drops exactly that many innermost `Index` parts at runtime, so it
/// lands back on the declaration's slot. Such an access still counts as empty —
/// that is what keeps the top-level accumulator (`state xs = []` with
/// `xs = append(xs, i)` inside a `for`) eligible.
///
/// Walks the block tree upward, spending one `path_pop` per loop crossed: a
/// loop's body block hangs off its loop term, a function body block has no
/// parent term at all — so the walk ends either at the root block (empty iff
/// every crossed loop was paid for) or at a parentless block that is not the
/// root, which is a function body and only ever reached through a call.
fn addresses_the_empty_path(program: &Program, term: &Term) -> bool {
    let mut cur = term.block_id;
    let mut pops = term.path_pop;
    loop {
        if cur == program.root_block {
            return pops == 0;
        }
        let Some(parent_term) = program.blocks[cur.0 as usize].parent_term_id else {
            // A function body block: reached only through a call, and no
            // `path_pop` can drop a `Call` part.
            return false;
        };
        let parent = program.get_term(parent_term);
        if matches!(
            parent.op,
            TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop
        ) {
            if pops == 0 {
                return false;
            }
            pops -= 1;
        }
        cur = parent.block_id;
    }
}

/// The accumulator's *backbone*: the fresh root, the loop-carried phis that
/// carry it, and the `Copy` aliases that connect them. Everything else in the
/// value-web hangs off this inside the loop regions.
struct Backbone {
    /// The unique fresh value the accumulator starts from.
    root: TermId,
    /// The spine's phis in execution order. Each one's init resolves to `root`
    /// or to another spine phi, so together with `root` they form a tree: a
    /// loop nested in another's body is a child, and so is a loop that runs
    /// after it. Condition 5 uses the order plus region containment to decide
    /// which of them may be read from outside.
    phis: Vec<TermId>,
    /// `root` ∪ `phis` ∪ the connecting `Copy` carriers.
    terms: HashSet<TermId>,
    /// Each spine phi paired with the member its init resolves to (`root` or
    /// another phi) — the tree's parent links.
    parents: Vec<(TermId, TermId)>,
    /// Each connecting `Copy` paired with the spine phi whose init reads it.
    /// A copy is a *snapshot* of the value at its own position, so it is only a
    /// valid spine link when no mutation lands between the two.
    copy_links: Vec<(TermId, TermId)>,
}

/// Precomputed dataflow relations for the analysis, built once per program.
struct Analysis<'p> {
    program: &'p Program,
    /// For each phi term, the `phi_out` back-edge source terms (dest == phi).
    phi_srcs: HashMap<TermId, Vec<TermId>>,
    /// Every term that is the source of some `phi_out` — the terms whose value
    /// leaves their block through a phi carry-out. Flattened from `phi_srcs`
    /// because the escape checks ask the question per term.
    phi_carry_srcs: HashSet<TermId>,
    /// Reverse `phi_outs` edges: for each term, the phis it is carried into on a
    /// block pop. Read through [`Analysis::phi_out_targets`], which is named for
    /// the question rather than the storage.
    phi_outs_by_src: HashMap<TermId, Vec<TermId>>,
    /// For each phi, the blocks that carry a value back into it on pop (see
    /// [`Analysis::body_blocks_of`]).
    phi_body_blocks: HashMap<TermId, Vec<BlockId>>,
    /// Reverse "read" edges: for each term `w`, the terms that read `w` as a
    /// *carried* input — a `Copy` source, a mutation's container, or a phi's
    /// init. Excludes phi back-edge sources (a carry-forward, not a live read).
    read_consumers: HashMap<TermId, Vec<TermId>>,
    /// Reverse direct-input edges: for each term, every term naming it as *any*
    /// input (used to catch a non-carrier reader observing the container).
    users: HashMap<TermId, Vec<TermId>>,
    /// Block-subtree membership cache, filled lazily per region root.
    block_children: HashMap<BlockId, Vec<BlockId>>,
    /// Each block's last term — its result value. Precomputed because the
    /// escape checks ask for it once per web term.
    block_last: HashMap<BlockId, TermId>,
    /// Every value written into a base state key, as `(value term, home block)`.
    /// A `StateWrite` contributes its input and its own block; a `StateInit`
    /// contributes its init block's result and that block. `home` is the block
    /// whose execution performs one write, so an allocation *in* it produces a
    /// distinct id per write (see [`Analysis::writer_is_uniquely_fresh`]).
    state_writers: HashMap<StateKey, Vec<(TermId, BlockId)>>,
    /// How many `StateInit`/`StateRead` terms each base state key has. A
    /// state-rooted web requires exactly one — its own root.
    state_readers: HashMap<StateKey, usize>,
    /// For each term, the phis whose *init* resolves to it through `Copy`
    /// carriers — the forward direction of a spine link. Lets a state-rooted
    /// backbone be extended past the seed's backward cone.
    phi_by_init: HashMap<TermId, Vec<TermId>>,
    /// Base state keys that do **not** address one fixed runtime slot: some
    /// state term for them sits off the statically empty path (see
    /// [`addresses_the_empty_path`] — inside a function body, so its
    /// `RuntimeStateKey` picks up a `Call` part per callsite chain, or inside a
    /// loop it does not pop back out of, so it picks up an `Index` part per
    /// iteration) or carries an explicit `state(expr)` key (a `Key` part hashed
    /// from a runtime value). A base key alone does not identify the slot for
    /// these, so no web may root on one.
    multi_slot_keys: HashSet<StateKey>,
    /// Structural execution order: each term's position in a depth-first walk of
    /// the block tree (a term, then the child blocks it introduces, then the next
    /// term). Within one pass over a block this is exactly the order the VM runs
    /// the terms in — the ordering [`Analysis::observation_index`] needs. Phantom
    /// terms sit in no block's list and are absent, which reads as "unknown" and
    /// rejects.
    exec_index: HashMap<TermId, usize>,
}

impl<'p> Analysis<'p> {
    fn build(program: &'p Program) -> Analysis<'p> {
        let mut phi_srcs: HashMap<TermId, Vec<TermId>> = HashMap::new();
        let mut phi_carry_srcs: HashSet<TermId> = HashSet::new();
        let mut phi_outs_by_src: HashMap<TermId, Vec<TermId>> = HashMap::new();
        let mut phi_body_blocks: HashMap<TermId, Vec<BlockId>> = HashMap::new();
        for block in &program.blocks {
            for po in &block.phi_outs {
                phi_srcs.entry(po.dest_term).or_default().push(po.src_term);
                phi_carry_srcs.insert(po.src_term);
                phi_outs_by_src
                    .entry(po.src_term)
                    .or_default()
                    .push(po.dest_term);
                let blocks = phi_body_blocks.entry(po.dest_term).or_default();
                if blocks.last() != Some(&block.id) {
                    blocks.push(block.id);
                }
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
            phi_carry_srcs,
            phi_outs_by_src,
            phi_body_blocks,
            read_consumers: HashMap::new(),
            users: HashMap::new(),
            block_children,
            block_last: Self::build_block_last(program),
            exec_index: Self::build_exec_index(program),
            state_writers: HashMap::new(),
            state_readers: HashMap::new(),
            multi_slot_keys: HashSet::new(),
            phi_by_init: HashMap::new(),
        };

        let mut phi_by_init: HashMap<TermId, Vec<TermId>> = HashMap::new();
        for term in &program.terms {
            if matches!(term.op, TermOp::Phi) {
                if let Some(&init) = term.inputs.first() {
                    phi_by_init
                        .entry(ctx.strip_copies(init))
                        .or_default()
                        .push(term.id);
                }
            }
        }
        ctx.phi_by_init = phi_by_init;

        // State-slot traffic, keyed by base key (needs `block_last` above).
        let mut state_writers: HashMap<StateKey, Vec<(TermId, BlockId)>> = HashMap::new();
        let mut state_readers: HashMap<StateKey, usize> = HashMap::new();
        let mut multi_slot_keys: HashSet<StateKey> = HashSet::new();
        for term in &program.terms {
            let Some(key) = term.state_key else { continue };
            // A non-empty runtime path mixes live context — the callsite chain
            // and the loop iterations reaching this term — into the runtime key,
            // and an explicit `state(expr)` key hashes a runtime value into it.
            // Either way the base key no longer names one slot, and a write
            // executed under a different path than the read commits to a
            // different slot than the one it just mutated. Only a declaration
            // whose path is statically empty — a top-level `state` outside every
            // loop — addresses exactly one slot (plan §3.7).
            //
            // `Copy` carriers also inherit the key (so a chain of reassignments
            // still resolves to the `StateInit`), but they touch no slot and
            // carry no `path_pop`, so only the three real state ops are judged.
            let explicit_key = match term.op {
                TermOp::StateInit => !term.inputs.is_empty(),
                TermOp::StateWrite => term.inputs.len() > 1,
                _ => false,
            };
            let off_the_empty_path = matches!(
                term.op,
                TermOp::StateInit | TermOp::StateRead | TermOp::StateWrite
            ) && !addresses_the_empty_path(program, term);
            if explicit_key || off_the_empty_path {
                multi_slot_keys.insert(key);
            }
            match term.op {
                TermOp::StateWrite => {
                    if let Some(&v) = term.inputs.first() {
                        state_writers
                            .entry(key)
                            .or_default()
                            .push((v, term.block_id));
                    }
                }
                TermOp::StateInit => {
                    *state_readers.entry(key).or_default() += 1;
                    // The init block's result is committed to the slot on a
                    // cache miss (the lowering emits the write for it).
                    if let Some(&init_block) = term.child_blocks.first() {
                        if let Some(v) = ctx.last_term_of(init_block) {
                            state_writers.entry(key).or_default().push((v, init_block));
                        }
                    }
                }
                TermOp::StateRead => {
                    *state_readers.entry(key).or_default() += 1;
                }
                _ => {}
            }
        }
        ctx.state_writers = state_writers;
        ctx.state_readers = state_readers;
        ctx.multi_slot_keys = multi_slot_keys;

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

    /// Whether `t` is carried out of its block into a phi on pop. That write
    /// happens through the *register*, so it never shows up as a user edge and
    /// has to be asked about separately.
    fn is_phi_carry_source(&self, t: TermId) -> bool {
        self.phi_carry_srcs.contains(&t)
    }

    /// Whether `term` is a collection-mutating term — a `SetIndex`/`SetField`,
    /// or a call to one of the mutating builtins. Its container input is
    /// `inputs[0]` for every kind.
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

    /// The last term of each non-empty block, following `entry` → `block_next`.
    fn build_block_last(program: &Program) -> HashMap<BlockId, TermId> {
        let mut out = HashMap::new();
        for block in &program.blocks {
            let mut cur = block.entry;
            while let Some(t) = cur {
                out.insert(block.id, t);
                cur = program.get_term(t).block_next;
            }
        }
        out
    }

    /// Number every term by its position in a depth-first walk of the block tree
    /// (see [`Analysis::exec_index`]). The root block is walked first, then each
    /// function body; a web never spans two of those, so only same-tree
    /// comparisons are ever made.
    fn build_exec_index(program: &Program) -> HashMap<TermId, usize> {
        fn visit(
            program: &Program,
            block: BlockId,
            next: &mut usize,
            out: &mut HashMap<TermId, usize>,
        ) {
            let mut cur = program.get_block(block).entry;
            while let Some(t) = cur {
                out.insert(t, *next);
                *next += 1;
                let term = program.get_term(t);
                // A term's child blocks run at the term's position, so they take
                // the indices between it and the block's next term.
                for &cb in &term.child_blocks {
                    visit(program, cb, next, out);
                }
                if matches!(term.op, TermOp::Match) {
                    if let Some(arms) = program.match_arms.get(&t) {
                        for arm in arms {
                            if let Some(g) = arm.guard_block {
                                visit(program, g, next, out);
                            }
                            visit(program, arm.body_block, next, out);
                        }
                    }
                }
                cur = term.block_next;
            }
        }

        let mut out = HashMap::new();
        let mut next = 0;
        visit(program, program.root_block, &mut next, &mut out);
        for f in &program.functions {
            visit(program, f.body_block, &mut next, &mut out);
        }
        out
    }

    /// The execution index at which `user` finishes reading `observed`'s id, if
    /// `user` merely *observes* the container rather than retaining a reference
    /// to it — otherwise `None`.
    ///
    /// `xs[i]` / `r.f` yield the element's or field's own id, `len(xs)` an int,
    /// `get(a, i)` a float, `sort(xs)` a fresh list — an in-place write replaces
    /// a slot in the container's store and cannot reach any of those. A `Copy`
    /// whose own users are all observations is one too (the read of `a[i]` in
    /// `a[i] = a[i] + 1` lowers as `Copy` → `GetIndex`); its index is the *last*
    /// of those reads, since that is when it last touches the id. What is not an
    /// observation, and stays rejected: storing the id into another container,
    /// capturing it in a closure, writing it to state, returning it, feeding it
    /// to a phi carry-out, or handing it to a user function, which could stash it
    /// anywhere. `MethodCall` is rejected too — it may dispatch to a record field
    /// holding an arbitrary closure.
    ///
    /// The index is what the caller needs to enforce ordering: an observation is
    /// a snapshot taken where it runs, so it is only equivalent to the
    /// value-semantics read if it runs *before* the mutation that rewrites that
    /// id. See [`Self::first_mutation_index_after`].
    fn observation_index(&self, user: TermId, observed: TermId) -> Option<usize> {
        let term = self.program.get_term(user);
        let here = self.exec_index.get(&user).copied();
        match &term.op {
            TermOp::GetField(_) | TermOp::GetFieldOpt(_)
                if term.inputs.first() == Some(&observed) =>
            {
                here
            }
            TermOp::GetIndex | TermOp::GetIndexOpt
                if term.inputs.first() == Some(&observed)
                    && term.inputs.get(1) != Some(&observed) =>
            {
                here
            }
            TermOp::BuiltinCall(cid)
                if self
                    .program
                    .get_string_constant(*cid)
                    .is_some_and(crate::builtins::retains_no_reference) =>
            {
                here
            }
            TermOp::Copy if term.inputs.first() == Some(&observed) => {
                // Two retentions the `users` map cannot see, because both read
                // the copy's *register* from the parent frame rather than naming
                // it as an input. A phi carry-out writes it into the parent's phi
                // slot on pop; and a block's last term is that block's result —
                // the value of an `if`/`match` arm, a `collect` loop's element, a
                // function's return. Either hands the id to code the web does not
                // enumerate, so neither is an observation.
                if self.is_phi_carry_source(user) || self.escapes_as_block_result(user) {
                    return None;
                }
                let mut last = here?;
                for &u in self.users.get(&user).into_iter().flatten() {
                    last = last.max(self.observation_index(u, user)?);
                }
                Some(last)
            }
            _ => None,
        }
    }

    /// Whether `t`'s value leaves its block through the **block-result
    /// register** — the one value flow the term graph does not spell out as an
    /// input edge. A block's last term is its result: the value of an `if`/`match`
    /// arm, one element of a `collect` loop, a function's return, the program's
    /// result. If something reads that value, the id has escaped the web without
    /// any user edge to show for it.
    ///
    /// A block result that nobody reads is not an escape, which is what keeps the
    /// ordinary shapes alive: a statement `if` inside a loop body still ends its
    /// arm with the accumulator's carry-`Copy`, and a plain (non-`collect`) loop
    /// body's result is discarded.
    fn escapes_as_block_result(&self, t: TermId) -> bool {
        let block = self.program.get_term(t).block_id;
        self.last_term_of(block) == Some(t) && self.block_result_is_read(block)
    }

    /// Whether anything reads `block`'s result value. Walks outward: the root
    /// block (and a function body) yields to its caller; an arm block yields to
    /// its control term, which is itself only observed if *its* value is read.
    fn block_result_is_read(&self, block: BlockId) -> bool {
        let Some(parent) = self.program.get_block(block).parent_term_id else {
            return true; // program result / function return value
        };
        let term = self.program.get_term(parent);
        if matches!(
            term.op,
            TermOp::ForLoop | TermOp::NumericForLoop | TermOp::WhileLoop
        ) && !term.collect
        {
            return false; // a statement loop discards each iteration's value
        }
        self.value_is_read(parent)
    }

    /// Whether `t`'s value reaches any reader at all — a direct input edge, a phi
    /// carry-out, or its own block's result.
    fn value_is_read(&self, t: TermId) -> bool {
        if self.users.get(&t).is_some_and(|u| !u.is_empty()) {
            return true;
        }
        if self.is_phi_carry_source(t) {
            return true;
        }
        self.escapes_as_block_result(t)
    }

    /// Whether `user` only observes `observed`, and does so before `mutated_at`
    /// (the first in-pass mutation of that id, if any) — the combination that
    /// makes it invisible to the uniqueness argument.
    fn observes_before_mutation(
        &self,
        user: TermId,
        observed: TermId,
        mutated_at: Option<usize>,
    ) -> bool {
        match self.observation_index(user, observed) {
            Some(at) => mutated_at.is_none_or(|m| at < m),
            None => false,
        }
    }

    /// The earliest execution index at which the id held by `t` is rewritten in
    /// place, or `None` if it never is. An observation of `t` is only equivalent
    /// to the value-semantics read when it finishes strictly before this.
    ///
    /// Forward carrier edges (`Copy` source, mutation container, phi init) are
    /// followed from `t`, since they are exactly the edges that hand `t`'s id to
    /// something that may mutate it:
    ///
    /// * a mutation contributes its own index;
    /// * a **phi** contributes the phi's index and stops the walk. Reaching a phi
    ///   means `t` is that phi's *init*, i.e. `t`'s id is what enters the loop or
    ///   branch the phi heads — so every mutation inside it rewrites `t`'s id,
    ///   and the earliest point that can happen is where the phi sits (the VM
    ///   emits it immediately before its control term). Charging the whole
    ///   construct to that one index is what makes `let ys = xs` before a
    ///   mutating loop, read after it, come out unsound-and-rejected.
    ///
    /// Phi *back* edges are not read edges, so the walk never runs forward into
    /// the next iteration: within one pass each term reached here executes at
    /// most once, which is what makes an index comparison meaningful.
    fn first_mutation_index_after(&self, t: TermId) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut seen = HashSet::from([t]);
        let mut queue = VecDeque::from([t]);
        let note = |best: &mut Option<usize>, n: TermId| {
            let idx = self.exec_index.get(&n).copied().unwrap_or(0);
            *best = Some(best.map_or(idx, |b: usize| b.min(idx)));
        };
        while let Some(w) = queue.pop_front() {
            for &n in self.read_consumers.get(&w).into_iter().flatten() {
                if !seen.insert(n) {
                    continue;
                }
                let term = self.program.get_term(n);
                if matches!(term.op, TermOp::Phi) {
                    note(&mut best, n);
                    continue;
                }
                if self.is_mutation(term) {
                    note(&mut best, n);
                }
                queue.push_back(n);
            }
        }
        best
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

    /// The base state key `t` reads, when `t` is a state slot read — the root of
    /// a web whose container lives in `state` and outlives the run.
    fn state_root_key(&self, t: TermId) -> Option<StateKey> {
        let term = self.program.get_term(t);
        match term.op {
            TermOp::StateInit | TermOp::StateRead => term.state_key,
            _ => None,
        }
    }

    /// Every `(block, register)` the mutated value can occupy, following the web
    /// forward: carrier reads move it into another term's register, and a phi
    /// carry-out moves it into the enclosing frame's phi register. Used to ask
    /// whether an arm's mutations reach the register the arm carries out.
    fn web_value_locations(
        &self,
        from: TermId,
        web: &HashSet<TermId>,
    ) -> HashSet<(BlockId, crate::program::RegisterIndex)> {
        let mut out = HashSet::new();
        let mut seen = HashSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(t) = queue.pop_front() {
            let term = self.program.get_term(t);
            out.insert((term.block_id, term.register));
            // Carrier reads move the value into another term's register…
            let mut onward: Vec<TermId> = self
                .read_consumers
                .get(&t)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            // …and a block pop copies *this register* into the parent's phi.
            // Matching by register (not by `src_term`) is what the VM does, and
            // is required here: a block's carry-out names the term that first
            // bound the register, while the loop phi that later rewrote it is a
            // different term with the same register.
            for po in &self.program.get_block(term.block_id).phi_outs {
                if self.program.get_term(po.src_term).register == term.register {
                    onward.push(po.dest_term);
                }
            }
            for n in onward {
                if web.contains(&n) && seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        out
    }

    /// Whether `p` is the merge phi of the state variable this web owns: its
    /// init is a read of slot `key`, so `p` is where that variable's value lives
    /// on both sides of the branch. Carrying our id into it is the container
    /// leaving the construct in the register it already occupies, not a second
    /// live holder.
    fn phi_continues_state_slot(&self, p: TermId, key: StateKey) -> bool {
        let term = self.program.get_term(p);
        if !matches!(term.op, TermOp::Phi) {
            return false;
        }
        let Some(&init) = term.inputs.first() else {
            return false;
        };
        self.state_root_key(self.strip_copies(init)) == Some(key)
    }

    /// Whether the value leaving through continuation phi `p` is only *read*
    /// from there on. Letting a carry-out reach `p` (see
    /// [`Self::phi_continues_state_slot`]) would otherwise be a blind spot: the
    /// id is live in `p`'s register, and `state_web_ok`'s retention check only
    /// walks users of *web* terms, which `p` is not.
    fn continuation_is_confined(&self, p: TermId, key: StateKey, web: &HashSet<TermId>) -> bool {
        for &u in self.users.get(&p).into_iter().flatten() {
            if web.contains(&u) || self.writes_state_key(u, key) {
                continue;
            }
            // An arm's "unchanged" carry reads `p` only to hand the same value
            // straight back into it — the same variable, not a second holder.
            if self.phi_out_targets(u).contains(&p) {
                continue;
            }
            if self.observation_index(u, p).is_none() {
                return false;
            }
        }
        // And it must not be carried on into yet another holder.
        self.phi_out_targets(p)
            .iter()
            .all(|&q| web.contains(&q) || self.phi_continues_state_slot(q, key))
    }

    /// Whether `u` commits a value into base state key `key`.
    fn writes_state_key(&self, u: TermId, key: StateKey) -> bool {
        let term = self.program.get_term(u);
        matches!(term.op, TermOp::StateWrite) && term.state_key == Some(key)
    }

    /// The extra obligations a **state-rooted** web carries, on top of
    /// conditions 1–5. The slot's id outlives the run, so uniqueness has to hold
    /// across runs, not just within one: the value the slot hands us at the top
    /// of *this* run must be unaliased, and nothing may keep it once the run
    /// ends. See the module header for the four rules; each is one block below.
    fn state_web_ok(
        &self,
        key: StateKey,
        root: TermId,
        web: &HashSet<TermId>,
        region: &HashSet<BlockId>,
    ) -> bool {
        // (a) One slot, one reader. The key must name a single runtime slot
        // (see `multi_slot_keys`), and read it in exactly one place — this web's
        // root. A second `StateInit`/`StateRead` would be another term holding
        // the same id, outside this web's reasoning.
        if self.multi_slot_keys.contains(&key) {
            return false;
        }
        if self.state_readers.get(&key).copied().unwrap_or(0) != 1 {
            return false;
        }
        // The root either *is* this slot's read, or is a fresh value the web
        // commits into it; a root reading some *other* slot is not our owner.
        if self.state_root_key(root).is_some_and(|k| k != key) {
            return false;
        }

        // (b) Immediate commit. Every mutation hands its result straight to a
        // `StateWrite` of this key, so the slot and the value-semantics slot
        // agree at every instruction boundary — including the boundary a
        // mid-run error stops at.
        for &t in web {
            if !self.is_mutation(self.program.get_term(t)) {
                continue;
            }
            let committed = self
                .users
                .get(&t)
                .into_iter()
                .flatten()
                .any(|&u| self.writes_state_key(u, key));
            if !committed {
                return false;
            }
        }

        // (c) Unique writers. Anything else that lands in this slot must be
        // freshly allocated where it is written and aliased nowhere, so the id
        // we inherit next run is ours alone.
        for &(value, home) in self.state_writers.get(&key).into_iter().flatten() {
            if web.contains(&value) {
                continue;
            }
            if !self.writer_is_uniquely_fresh(value, home, key, web) {
                return false;
            }
        }

        // (d) No retention past the region. Out-of-region users read the
        // finished value, which is fine — but only if they *read* it. Anything
        // holding the id when the run ends (an output buffer the host drains, a
        // second slot, a closure kept in `functions`) would see the next run
        // mutate it. `observation_index` recurses through copies, so a copy that
        // is later retained is not an observation and rejects here.
        for &t in web {
            for &u in self.users.get(&t).into_iter().flatten() {
                if region.contains(&self.program.get_term(u).block_id) {
                    continue; // condition 4 governs in-region users
                }
                if web.contains(&u) || self.writes_state_key(u, key) {
                    continue;
                }
                if self.observation_index(u, t).is_none() {
                    return false;
                }
            }
        }
        true
    }

    /// Whether `value` is a container this program allocated *at the write site*
    /// and handed to nothing else — the standard a value must meet to enter a
    /// state slot that a web then mutates in place.
    ///
    /// `home` is the block whose execution performs one write. Requiring the
    /// allocation to live in that same block is what separates `state c = [0,0]`
    /// inside a loop (a fresh list per per-iteration slot) from `let shared =
    /// [0,0]` hoisted above it and assigned to every slot — one id in many slots.
    fn writer_is_uniquely_fresh(
        &self,
        value: TermId,
        home: BlockId,
        key: StateKey,
        web: &HashSet<TermId>,
    ) -> bool {
        let (alloc, copies) = self.copy_chain(value);
        if !self.is_fresh_root(alloc) {
            return false;
        }
        if self.program.get_term(alloc).block_id != home {
            return false;
        }
        let chain: HashSet<TermId> = copies.iter().copied().chain([alloc]).collect();
        for &t in &chain {
            if self.is_phi_carry_source(t) {
                return false;
            }
            for &u in self.users.get(&t).into_iter().flatten() {
                if !chain.contains(&u) && !self.writes_state_key(u, key) && !web.contains(&u) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether `t` may root a unique value-web: it produces a container that no
    /// other live value can reference.
    ///
    /// An `Alloc*` term is fresh by construction. A *call* is fresh only when the
    /// callee is known to hand back sole ownership: a builtin on the
    /// [`crate::builtins::returns_fresh_container`] list, or a user function whose
    /// result is a container it allocated and let nothing else observe
    /// ([`Self::function_returns_fresh`]). Everything else — a param, a capture, a
    /// state read, an unknown call — could alias a value the caller still holds.
    fn is_fresh_root(&self, t: TermId) -> bool {
        let term = self.program.get_term(t);
        if Self::is_fresh_alloc(term) {
            return true;
        }
        self.call_returns_fresh(term)
    }

    /// [`Self::is_fresh_root`] for the call forms, split out so the recursion
    /// into a callee body is explicitly one level deep (the callee's own result
    /// must be an alloc or a fresh builtin, never another user call).
    fn call_returns_fresh(&self, term: &Term) -> bool {
        match &term.op {
            TermOp::BuiltinCall(_) => self.call_returns_fresh_builtin(term),
            TermOp::Call => {
                let Some(&callee) = term.inputs.first() else {
                    return false;
                };
                // Only a directly-named function qualifies: a dynamic callable
                // (a param, an overload set, a captured closure variable) could
                // be any function at all.
                let TermOp::MakeClosure(fid) = self.program.get_term(self.strip_copies(callee)).op
                else {
                    return false;
                };
                self.program
                    .functions
                    .iter()
                    .find(|f| f.id == fid)
                    .is_some_and(|def| self.function_returns_fresh(def))
            }
            _ => false,
        }
    }

    /// Whether every call of `def` returns a container it freshly allocated and
    /// leaked nowhere — a conservative intraprocedural check.
    ///
    /// The function's result is its body block's last term. That term's `Copy`
    /// chain must bottom out in an `Alloc*` or a fresh builtin call *inside the
    /// body*, and every user of every term on that chain must itself be on the
    /// chain (no store into another container, no closure capture, no state
    /// write, no phi carrying it into an enclosing scope). That makes the
    /// allocation flow linearly to the `return` and nowhere else, so the caller
    /// receives the sole reference. Any explicit `return` in the body rejects the
    /// function outright: an early return could yield a different, possibly
    /// aliased, value than the tail expression the check inspected.
    fn function_returns_fresh(&self, def: &crate::program::FunctionDef) -> bool {
        let body = self.block_subtree(def.body_block);
        if self
            .program
            .terms
            .iter()
            .any(|t| body.contains(&t.block_id) && matches!(t.op, TermOp::Return))
        {
            return false;
        }
        let Some(result) = self.last_term_of(def.body_block) else {
            return false;
        };
        let (alloc, copies) = self.copy_chain(result);
        let alloc_term = self.program.get_term(alloc);
        if !Self::is_fresh_alloc(alloc_term) && !self.call_returns_fresh_builtin(alloc_term) {
            return false;
        }
        if !body.contains(&alloc_term.block_id) {
            return false;
        }
        let chain: HashSet<TermId> = copies.iter().copied().chain([alloc]).collect();
        for &t in &chain {
            for &u in self.users.get(&t).into_iter().flatten() {
                if !chain.contains(&u) {
                    return false;
                }
            }
        }
        // A phi carry-out is a write into an enclosing frame's register, not a
        // `users` edge, so it has to be excluded separately.
        !chain.iter().any(|&t| self.is_phi_carry_source(t))
    }

    /// The builtin half of [`Self::call_returns_fresh`] — used where recursing
    /// into another user function would not be sound to assume.
    fn call_returns_fresh_builtin(&self, term: &Term) -> bool {
        matches!(&term.op, TermOp::BuiltinCall(cid) if self
            .program
            .get_string_constant(*cid)
            .is_some_and(crate::builtins::returns_fresh_container))
    }

    /// The last term of `block` in execution order (`entry` → `block_next`) —
    /// the block's result value, mirroring the lowering's `block_result_reg`.
    fn last_term_of(&self, block: BlockId) -> Option<TermId> {
        self.block_last.get(&block).copied()
    }

    /// Follow `Copy` chains backward to the first non-`Copy` term.
    fn strip_copies(&self, t: TermId) -> TermId {
        self.copy_chain(t).0
    }

    /// Like [`Self::strip_copies`], but also returns the `Copy` terms traversed
    /// (nearest-first). Those copies are pure aliases of the same id, so they
    /// belong to the value-web alongside the term they resolve to.
    fn copy_chain(&self, mut t: TermId) -> (TermId, Vec<TermId>) {
        let mut copies = Vec::new();
        loop {
            let term = self.program.get_term(t);
            // A capture / function-parameter placeholder lowers as an
            // *input-less* `Copy`: its value comes from the frame, not from a
            // dataflow edge, so the chain ends there (and it is not a fresh
            // root, which is what rejects a returned capture).
            match (&term.op, term.inputs.first()) {
                (TermOp::Copy, Some(&src)) => {
                    copies.push(t);
                    t = src;
                }
                _ => return (t, copies),
            }
        }
    }

    /// All terms reachable backward from `seeds` over *container* carrier inputs
    /// (copy sources, mutation containers, phi inits, and phi back-edge sources)
    /// — the value sources that feed into `seeds`. Forward consumers are
    /// deliberately excluded, so the value is not followed once it escapes the
    /// accumulator's loop — that keeps two independent accumulators (`next` and
    /// the `particles` it feeds) from merging into one web.
    ///
    /// Seeded with the mutation it gives that mutation's cone; seeded with a
    /// phi's back-edge sources it tests spine membership, i.e. whether a
    /// mutation's result flows back into that loop phi's back edge.
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
        // Locate the loop-carried phis behind the mutation via a backward
        // (producer-only) walk, so post-loop escapes never enter the picture,
        // and resolve them into a single accumulator backbone.
        let cone = self.backward_carrier_closure(&[seed]);
        let Some(backbone) = self.build_backbone(&cone) else {
            return false;
        };
        let root = backbone.root;

        // The root must be a uniquely-owned value: either freshly produced in
        // this run (a param or capture root could alias anything, and is
        // rejected), or a `state` slot this program alone owns — checked in full
        // by `state_web_ok` once the web and region are known.
        let root_key = self.state_root_key(root);
        if root_key.is_none() && !self.is_fresh_root(root) {
            return false;
        }

        // `seed` must be *on a spine*: its result has to flow back into some
        // backbone phi's back edge. A mutation whose result is discarded — e.g.
        // `let ys = xs; ys = append(ys, v)` inside the loop, where `xs` is the
        // real carried value and `ys` a throwaway alias — is NOT the
        // accumulator; mutating it in place would corrupt the aliased `xs`.
        let on_spine = backbone.phis.iter().any(|p| {
            let back_srcs: Vec<TermId> = self.phi_srcs.get(p).cloned().unwrap_or_default();
            self.backward_carrier_closure(&back_srcs).contains(&seed)
        });
        if !on_spine {
            return false;
        }

        // The region is the union of the backbone loops' body subtrees: the
        // window over which the accumulator is live and being mutated.
        let mut per_phi: Vec<HashSet<BlockId>> = Vec::with_capacity(backbone.phis.len());
        let mut region: HashSet<BlockId> = HashSet::new();
        for &p in &backbone.phis {
            let blocks = self.body_blocks_of(p);
            if blocks.is_empty() {
                return false;
            }
            // A merge phi has one carry-in block per arm that rebinds the name;
            // its region is all of them, which is what makes it *enclose* the
            // loop nested inside one arm (and so exempt under condition 5).
            let mut sub = HashSet::new();
            for b in blocks {
                sub.extend(self.block_subtree(b));
            }
            region.extend(sub.iter().copied());
            per_phi.push(sub);
        }

        // Two spine phis with the *same parent* must not be live at once. A tree
        // is how one container's spine branches into a nested loop and a later
        // one — those regions are disjoint in time. Two phis over the same
        // parent value whose regions *overlap* are two accumulators aliasing one
        // id (`let al = xs` at the top of a loop, then both `xs` and `al`
        // appended in it), and both would mutate it. (Fuzzer seed 132768.)
        for (i, &(p, pp)) in backbone.parents.iter().enumerate() {
            for &(q, qp) in &backbone.parents[i + 1..] {
                if pp != qp {
                    continue;
                }
                let (Some(pi), Some(qi)) = (
                    backbone.phis.iter().position(|&x| x == p),
                    backbone.phis.iter().position(|&x| x == q),
                ) else {
                    return false;
                };
                if !per_phi[pi].is_disjoint(&per_phi[qi]) {
                    return false;
                }
            }
        }

        // Build the region-confined value-web: carriers connected to `seed`
        // within the region, plus the backbone itself. Only carriers (and the
        // root) are ever added — a non-carrier in-region reader is caught during
        // validation, not folded into the web.
        let web = self.build_confined_web(seed, &backbone, &region, &cone);

        // The web's `state` anchor: the slot it reads from, the slot it commits
        // into, or both — and they must agree. A container allocated fresh but
        // then parked in a slot (`a = f64_array(n)` inside a guard) outlives the
        // run just as a state-read root does, so it carries the same
        // obligations; a web committing into *two* slots is two owners and is
        // rejected outright.
        let mut anchor = root_key;
        for &t in &web {
            for &u in self.users.get(&t).into_iter().flatten() {
                let user = self.program.get_term(u);
                if !matches!(user.op, TermOp::StateWrite) {
                    continue;
                }
                match (anchor, user.state_key) {
                    (_, None) => return false,
                    (None, k) => anchor = k,
                    (Some(a), Some(k)) if a == k => {}
                    _ => return false,
                }
            }
        }

        // (1) Unique fresh root: the only non-carrier in the web is `root`.
        for &t in &web {
            let term = self.program.get_term(t);
            if !self.is_carrier(term) && t != root {
                return false;
            }
        }

        // (5) The backbone's *interior* holds a mid-build id that a later region
        // still mutates in place, so it must flow only into the accumulator: a
        // reference taken before the first loop (`let ys = xs`) or between two
        // sequential loops would observe those mutations, and rejects the whole
        // accumulator.
        //
        // A phi is exempt from that rule — its out-of-region readers see the
        // *finished* value, which in-place mutation leaves value-identical —
        // exactly when every later loop in the chain runs *inside* its own loop.
        // Chain order is execution order (each phi's init reads its
        // predecessor's value), so once `phis[i]`'s loop exits, every mutation
        // from an earlier loop has already happened and every later one happened
        // nested within it. The last phi is always exempt (vacuously); with
        // sequential loops it is the *only* one, since a value read between them
        // is still due to be rewritten.
        let exempt: HashSet<TermId> = backbone
            .phis
            .iter()
            .enumerate()
            .filter(|&(i, _)| {
                per_phi[i + 1..]
                    .iter()
                    .all(|later| later.is_subset(&per_phi[i]))
            })
            .map(|(_, &p)| p)
            .collect();
        for &t in &backbone.terms {
            if exempt.contains(&t) {
                continue;
            }
            let mutated_at = self.first_mutation_index_after(t);
            for &u in self.users.get(&t).into_iter().flatten() {
                if web.contains(&u) || anchor.is_some_and(|k| self.writes_state_key(u, k)) {
                    continue;
                }
                if !self.observes_before_mutation(u, t, mutated_at) {
                    return false;
                }
            }
        }

        // (5b) No stale spine link. A `Copy` on a spine link is a *snapshot* of
        // the value where it stands, whereas a phi's register is kept current by
        // its construct's carry-outs. So a copy is only a valid link when no
        // mutation lands between it and the phi that reads it: `let a = xs`
        // before a loop that mutates `xs`, with `a` then driving a second loop,
        // is two spines over one id, not one spine — and both would mutate it.
        for &(c, q) in &backbone.copy_links {
            let (Some(&ci), Some(&qi)) = (self.exec_index.get(&c), self.exec_index.get(&q)) else {
                return false;
            };
            for &t in &web {
                if !self.is_mutation(self.program.get_term(t)) {
                    continue;
                }
                let mi = self.exec_index.get(&t).copied().unwrap_or(0);
                if ci < mi && mi < qi {
                    return false;
                }
            }
        }

        // (5c) A spine phi must actually *receive* the mutations of any arm that
        // performs them. Resolving the spine through merge phis makes the phi of
        // an enclosing `if` a spine member, and condition 5 then treats it as
        // holding the finished value — true only when the accumulator's mutated
        // id flows back into it. `if c then let al = xs; <mutate al> end` mutates
        // an *alias* and carries the untouched `xs` back into the merge, so the
        // merge stays a live pre-mutation holder that the code after the `if`
        // reads. (Fuzzer seed 113278.)
        for &p in &backbone.phis {
            for b in self.body_blocks_of(p) {
                let sub = self.block_subtree(b);
                let arm_mutations: Vec<TermId> = web
                    .iter()
                    .copied()
                    .filter(|&t| {
                        let term = self.program.get_term(t);
                        self.is_mutation(term) && sub.contains(&term.block_id)
                    })
                    .collect();
                if arm_mutations.is_empty() {
                    continue; // an arm that changes nothing carries the value through
                }
                for po in &self.program.get_block(b).phi_outs {
                    if po.dest_term != p {
                        continue;
                    }
                    // A carry-out copies a *register*, not a term: what leaves
                    // the arm is that register's last write. So ask whether any
                    // of the arm's mutations actually lands there.
                    let src = self.program.get_term(po.src_term);
                    let landed = arm_mutations.iter().any(|&m| {
                        self.web_value_locations(m, &web)
                            .contains(&(src.block_id, src.register))
                    });
                    if !landed {
                        return false;
                    }
                }
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

            // The mid-build id must not leave through a block-result register:
            // `let snaps = for i in … do xs = append(xs, i); xs end` collects a
            // reference to the accumulator on every iteration, and no input edge
            // records it.
            if region.contains(&term.block_id) && self.escapes_as_block_result(t) {
                return false;
            }

            // Nor through a *phi carry-out* into another variable's phi that
            // lives inside the region — the other escape with no input edge.
            // `if i == 1 then keep = xs else xs = append(xs, i) end` inside a
            // loop copies the accumulator into `keep`'s in-loop phi, which then
            // watches later iterations mutate it. A carry-out to a phi *outside*
            // the region is the value simply leaving the construct (the `if`'s
            // merge, the loop's result) and is the finished value, not a
            // mid-build snapshot.
            for p in self.phi_out_targets(t) {
                if web.contains(&p) {
                    continue;
                }
                // The one carry-out that is not a second holder: the merge phi
                // of the very state variable this web owns. `a = f64_array(n)`
                // inside a guard makes the loop's result leave the guard in
                // `a`'s own register, which already tracks the slot we commit
                // to. Any other phi is a different variable capturing our id
                // (`if … then <mutate a> else keep = a end`) and rejects.
                if anchor.is_some_and(|k| self.phi_continues_state_slot(p, k))
                    && anchor.is_some_and(|k| self.continuation_is_confined(p, k, &web))
                {
                    continue;
                }
                return false;
            }

            // Users that merely observe `t` — and finish doing so before any
            // mutation rewrites its id — neither escape it nor compete for it,
            // so they are dropped before both checks below. Everything else
            // *retains* the id and has to be a web term.
            let mutated_at = self.first_mutation_index_after(t);
            let in_region_users: Vec<TermId> = self
                .users
                .get(&t)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&u| region.contains(&self.program.get_term(u).block_id))
                .filter(|&u| !self.observes_before_mutation(u, t, mutated_at))
                // A commit back into the web's own state slot is the
                // accumulator's write-back, not an escape (see `state_web_ok`).
                .filter(|&u| !anchor.is_some_and(|k| self.writes_state_key(u, k)))
                // Nor is the *next* spine phi reading this value as its init a
                // competing reader: that is the hand-off from one loop of the
                // chain to the next, which happens once, after this stage is
                // finished. (Without an enclosing guard the same edge simply
                // fell outside the region and was never counted.)
                .filter(|&u| u == t || !backbone.phis.contains(&u))
                .collect();
            for &u in &in_region_users {
                if !web.contains(&u) {
                    return false; // a non-carrier *retains* the container mid-build
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

        // A `state`-backed container outlives the run; prove the slot is ours.
        if let Some(key) = anchor {
            if !self.state_web_ok(key, root, &web, &region) {
                return false;
            }
        }
        true
    }

    /// Resolve the loop-carried phis in `cone` into a single accumulator
    /// backbone, or `None` when they do not form one chain.
    ///
    /// Each phi's init is followed through its `Copy` carriers; the result is
    /// either the chain's root (a value produced outside every backbone loop) or
    /// the phi's predecessor in the chain. Exactly one phi may have a non-phi
    /// init, the predecessor edges must be injective, and following them from
    /// that first phi must reach every phi — so the container is built by one
    /// linear succession of loops (`for … end; for … end`, or an inner loop
    /// carrying an outer loop's accumulator) rather than by two independent
    /// spines that merge, which no per-region argument would cover.
    fn build_backbone(&self, cone: &HashSet<TermId>) -> Option<Backbone> {
        // Every phi the seed reads back through, not just the loop-carried
        // ones: a mutation loop inside an `if`/`match` has the arm's *merge* phi
        // between the root and the loop phi, and that merge is a carrier on the
        // same spine. At least one loop phi must still be present — route B is
        // about loop-carried accumulators; straight-line uniqueness is route A's.
        let mut phis: Vec<TermId> = cone
            .iter()
            .copied()
            .filter(|&t| matches!(self.program.get_term(t).op, TermOp::Phi))
            .collect();
        if !phis.iter().any(|&p| self.is_loop_phi(p)) {
            return None;
        }

        // The root is the one init target that is not itself a spine phi.
        let mut root: Option<TermId> = None;
        {
            let set: HashSet<TermId> = phis.iter().copied().collect();
            for &p in &phis {
                let init = *self.program.get_term(p).inputs.first()?;
                let (target, _) = self.copy_chain(init);
                if set.contains(&target) {
                    continue;
                }
                if root.replace(target).is_some_and(|r| r != target) {
                    return None; // two independent roots — not one spine
                }
            }
        }
        let root = root?;

        // The cone only sees what feeds the *seed*, so with two mutation loops
        // over one container the first loop's analysis never learns about the
        // second and rejects its phi as an outside holder. For a `state` root
        // the whole slot has to be reasoned about anyway (its id outlives the
        // run), so extend the spine forward past the seed: every phi whose init
        // resolves to a member joins it. Nested and sequential loops both show
        // up here (`for i … for j … end; for k …` gives one parent two
        // children), so the spine is a *tree*, not a chain.
        if self.state_root_key(root).is_some() {
            let mut seen: HashSet<TermId> = phis.iter().copied().collect();
            let mut queue: VecDeque<TermId> = phis.iter().copied().collect();
            queue.push_back(root);
            while let Some(parent) = queue.pop_front() {
                for &child in self.phi_by_init.get(&parent).into_iter().flatten() {
                    if seen.insert(child) {
                        phis.push(child);
                        queue.push_back(child);
                    }
                }
            }
        }

        // Every phi must hang off the root through spine members, and each link
        // records the `Copy` carriers it passes through — those copies are
        // snapshots, and `route_b_ok` checks none of them goes stale.
        let set: HashSet<TermId> = phis.iter().copied().collect();
        let mut copies: Vec<TermId> = Vec::new();
        let mut copy_links: Vec<(TermId, TermId)> = Vec::new();
        let mut parents: Vec<(TermId, TermId)> = Vec::new();
        for &p in &phis {
            let init = *self.program.get_term(p).inputs.first()?;
            let (target, chain) = self.copy_chain(init);
            if target != root && !set.contains(&target) {
                return None; // a phi merging in a value from outside the spine
            }
            parents.push((p, target));
            for c in chain {
                copies.push(c);
                copy_links.push((c, p));
            }
        }

        // Execution order makes the containment test in condition 5 meaningful.
        phis.sort_by_key(|p| self.exec_index.get(p).copied().unwrap_or(usize::MAX));

        let mut terms: HashSet<TermId> = phis.iter().copied().collect();
        terms.extend(copies);
        terms.insert(root);
        Some(Backbone {
            root,
            phis,
            terms,
            parents,
            copy_links,
        })
    }

    /// BFS the region-confined carrier web from `seed`, always including the
    /// whole backbone. Expansion visits carrier inputs, carrier readers, and
    /// phis fed on a back edge, but only *adds* a term when it is a carrier (or
    /// the root) that is in-region or on the backbone.
    fn build_confined_web(
        &self,
        seed: TermId,
        backbone: &Backbone,
        region: &HashSet<BlockId>,
        cone: &HashSet<TermId>,
    ) -> HashSet<TermId> {
        let root = backbone.root;
        let mut web = HashSet::new();
        let mut queue = VecDeque::new();
        for t in std::iter::once(seed).chain(backbone.terms.iter().copied()) {
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
                // A phi `w` is carried into on a block pop — but only the
                // spine's own phis (those the seed reads back, or the backbone)
                // belong to this web. Absorbing any phi fed by a web term would
                // pull a *different variable* that merely aliased the container
                // into the web, and with it the very escape condition 4 exists
                // to catch (`if … then keep = xs else xs = append(xs, i) end`).
                if srcs.contains(&w) && (cone.contains(&phi) || backbone.terms.contains(&phi)) {
                    neighbors.push(phi);
                }
            }
            for n in neighbors {
                let term = self.program.get_term(n);
                let allowed = region.contains(&term.block_id) || backbone.terms.contains(&n);
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
        self.body_blocks_of(phi).into_iter().next()
    }

    /// Every block that carries a value back into `phi` on pop. A loop phi has
    /// one (the loop body); a branch/match merge phi has one per arm that
    /// rebinds the name.
    fn body_blocks_of(&self, phi: TermId) -> Vec<BlockId> {
        self.phi_body_blocks.get(&phi).cloned().unwrap_or_default()
    }

    /// The phis `t` is carried into on a block pop (`phi_outs`), which is a
    /// value flow with no input edge to show for it.
    fn phi_out_targets(&self, t: TermId) -> Vec<TermId> {
        self.phi_outs_by_src.get(&t).cloned().unwrap_or_default()
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

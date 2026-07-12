//! Lowering: term-graph IR (`crate::program::Program`) → linear [`BytecodeProgram`].
//!
//! One [`BytecodeFn`] is produced per `FunctionDef`, plus one for the program's
//! root block. Within a function, *all* of its blocks (its body plus every
//! transitively-nested control-flow `child_block` — but never a called
//! function's body, which is reached via `MakeClosure`, not `child_blocks`) are
//! flattened into a single instruction stream over one flat register file.
//!
//! Flat register assignment reuses the compiler's existing per-block allocation:
//! each block is given a base offset (the running sum of preceding blocks'
//! `register_count`), and a term's flat register is `base[block] + term.register`.
//! This keeps phantom terms (parameters, captures, loop variables) — which carry
//! a register but never appear in an execution list — correctly mapped.
//!
//! ## Milestone status
//! M0 lowers the straight-line op set (constants, arithmetic/compare/logical,
//! `Copy`, and the data-structure allocators / field & index access). Control
//! flow, calls, closures, and state return an `unlowered op` error until M1–M3.

use std::collections::HashMap;

use smallvec::SmallVec;

use super::escape::InPlaceSet;
use super::isa::{BytecodeFn, BytecodeProgram, Inst, LoopSlot, Reg};
use crate::program::{BlockId, FunctionDef, Program, Term, TermId, TermOp};

/// Break/continue backpatch targets for one active loop during lowering.
struct LoopCtx {
    /// The loop's body block — the outermost region whose phi carry-outs a
    /// break/continue must run before leaving the loop.
    body_block: BlockId,
    /// Indices of `Jump` placeholders emitted by `continue`, patched to the
    /// loop's continue target (the `*Next` for counted loops, the index bump
    /// for `while`).
    continue_jumps: Vec<usize>,
    /// Indices of `Jump` placeholders emitted by `break`, patched to the loop
    /// exit (`LoopPop`).
    break_jumps: Vec<usize>,
}

/// Lower a whole program to bytecode with no optimizations (the correctness
/// baseline / oracle). Equivalent to `lower_program_opt` with an empty
/// in-place set.
pub fn lower_program(program: &Program) -> Result<BytecodeProgram, String> {
    lower_program_opt(program, &InPlaceSet::default())
}

/// Lower a whole program to bytecode, emitting in-place mutation opcodes for the
/// terms `in_place` proved unique + non-escaping (M4). Returns an error naming
/// the first op that cannot yet be lowered (so `ShowBytecode` surfaces progress
/// honestly). With `InPlaceSet::default()` (empty) the output is identical to
/// the un-optimized backend, so "bytecode with opts off" stays a clean oracle.
pub fn lower_program_opt(
    program: &Program,
    in_place: &InPlaceSet,
) -> Result<BytecodeProgram, String> {
    let mut match_binds = HashMap::new();
    let (root, root_binds) = FnLowerer::new(program, None, program.root_block, in_place).lower()?;
    match_binds.extend(root_binds);
    let mut fns = Vec::with_capacity(program.functions.len());
    for func in &program.functions {
        let (bf, binds) = FnLowerer::for_function(program, func, in_place).lower()?;
        match_binds.extend(binds);
        fns.push(bf);
    }
    Ok(BytecodeProgram {
        root,
        fns,
        match_binds,
    })
}

/// Lowers one function (root block or a `FunctionDef` body) into a [`BytecodeFn`].
struct FnLowerer<'p> {
    program: &'p Program,
    func: Option<&'p FunctionDef>,
    /// Mutation terms escape analysis proved safe to lower in place (M4).
    in_place: &'p InPlaceSet,
    /// The function's entry block (root block, or the def's `body_block`).
    entry_block: BlockId,
    /// Blocks belonging to this function, in discovery order.
    blocks: Vec<BlockId>,
    /// Flat base register offset for each block.
    base: HashMap<BlockId, u16>,
    /// Total flat registers needed.
    reg_count: u16,
    code: Vec<Inst>,
    /// The stack of blocks currently being emitted (outermost-last). A
    /// break/continue walks this from the top down to its loop body to emit the
    /// enclosing phi carry-outs it exits through.
    region_stack: Vec<BlockId>,
    /// Active loops, innermost last — supplies break/continue targets.
    loop_stack: Vec<LoopCtx>,
    /// Next free loop-cursor slot; also the loop-slot count for this function.
    next_slot: LoopSlot,
    /// Per-arm pattern-binding registers accumulated while lowering `Match`
    /// terms in this function; merged into the `BytecodeProgram`.
    match_binds: HashMap<(TermId, u16), Vec<(String, Reg)>>,
    /// Source term of each emitted instruction, parallel to `code` (for error
    /// annotation).
    origins: Vec<Option<TermId>>,
    /// The source term currently being lowered — recorded as the origin of each
    /// instruction pushed. Set as `emit_block` walks the term list.
    cur_origin: Option<TermId>,
}

impl<'p> FnLowerer<'p> {
    fn new(
        program: &'p Program,
        func: Option<&'p FunctionDef>,
        entry_block: BlockId,
        in_place: &'p InPlaceSet,
    ) -> Self {
        FnLowerer {
            program,
            func,
            in_place,
            entry_block,
            blocks: Vec::new(),
            base: HashMap::new(),
            reg_count: 0,
            code: Vec::new(),
            region_stack: Vec::new(),
            loop_stack: Vec::new(),
            next_slot: 0,
            match_binds: HashMap::new(),
            origins: Vec::new(),
            cur_origin: None,
        }
    }

    /// Push an instruction, recording the current source term as its origin.
    /// Returns the instruction's index (for jump backpatching).
    fn push(&mut self, inst: Inst) -> usize {
        self.origins.push(self.cur_origin);
        self.code.push(inst);
        self.code.len() - 1
    }

    fn for_function(program: &'p Program, func: &'p FunctionDef, in_place: &'p InPlaceSet) -> Self {
        Self::new(program, Some(func), func.body_block, in_place)
    }

    /// Flat register for a term (in any block belonging to this function).
    ///
    /// A term whose block is not part of this function signals a malformed IR
    /// graph — an input edge crossing a function boundary (only captures may do
    /// that, and they resolve to the closure's own registers). Rather than
    /// panic, surface it as a lowering error so it flows through the same
    /// `Result` path as every other lowering failure (see `lower_program_opt`).
    fn flat(&self, tid: TermId) -> Result<Reg, String> {
        let term = self.program.get_term(tid);
        let base = self.base.get(&term.block_id).copied().ok_or_else(|| {
            format!(
                "term t{} in block b{} not in this function",
                tid.0, term.block_id.0
            )
        })?;
        Ok(base + term.register.0)
    }

    /// Flat register for a raw `(block, register)` pair (params/captures/loop
    /// variables). Unlike [`flat`](Self::flat), `block` is always one this
    /// function owns (a function body or a loop's child block), never a foreign
    /// term reference — so the boundary-crossing failure `flat` guards against
    /// cannot arise here, and the base lookup is infallible.
    fn flat_reg(&self, block: BlockId, reg: u16) -> Reg {
        self.base[&block] + reg
    }

    fn lower(mut self) -> Result<(BytecodeFn, HashMap<(TermId, u16), Vec<(String, Reg)>>), String> {
        self.collect_blocks();
        self.assign_registers();

        // Emit the entry block; control-flow terms recurse into their child
        // blocks inline (one flat instruction stream over one register file).
        self.emit_block(self.entry_block)?;

        let (param_regs, capture_regs, self_ref_reg) = self.binding_regs();
        let result_reg = self.entry_result_reg()?;
        let bf = BytecodeFn {
            func_id: self.func.map(|f| f.id),
            name: self.func.and_then(|f| f.name.clone()),
            code: self.code,
            reg_count: self.reg_count,
            param_regs,
            capture_regs,
            self_ref_reg,
            loop_slots: self.next_slot,
            result_reg,
            origins: self.origins,
        };
        Ok((bf, self.match_binds))
    }

    /// Discover every block reachable from the entry via control-flow
    /// `child_blocks`. Called function bodies are *not* reached this way (they
    /// hang off `MakeClosure`), so this stays within one function.
    fn collect_blocks(&mut self) {
        let mut stack = vec![self.entry_block];
        let mut seen = std::collections::HashSet::new();
        while let Some(b) = stack.pop() {
            if !seen.insert(b) {
                continue;
            }
            self.blocks.push(b);
            for tid in self.all_block_terms(b) {
                let term = self.program.get_term(tid);
                for &cb in &term.child_blocks {
                    stack.push(cb);
                }
                // Match guard blocks live in `match_arms`, not `child_blocks`.
                if matches!(term.op, TermOp::Match) {
                    if let Some(arms) = self.program.match_arms.get(&tid) {
                        for arm in arms {
                            stack.push(arm.body_block);
                            if let Some(g) = arm.guard_block {
                                stack.push(g);
                            }
                        }
                    }
                }
            }
        }
    }

    fn assign_registers(&mut self) {
        let mut offset = 0u16;
        for &b in &self.blocks {
            self.base.insert(b, offset);
            offset = offset
                .checked_add(self.program.get_block(b).register_count)
                .expect("flat register file overflow (u16)");
        }
        self.reg_count = offset;
    }

    /// Param/capture/self-ref flat registers for a function body (empty for root).
    fn binding_regs(&self) -> (Vec<Reg>, Vec<Reg>, Option<Reg>) {
        let Some(func) = self.func else {
            return (Vec::new(), Vec::new(), None);
        };
        let body = func.body_block;
        // Parameters occupy body-block registers 0..N (see build_closure_frame).
        let param_regs = (0..func.params.len() as u16)
            .map(|i| self.flat_reg(body, i))
            .collect();
        let capture_regs = func
            .capture_registers
            .iter()
            .map(|r| self.flat_reg(body, r.0))
            .collect();
        let self_ref_reg = func.self_ref_register.map(|r| self.flat_reg(body, r.0));
        (param_regs, capture_regs, self_ref_reg)
    }

    /// Flat register of the entry block's last term — the function's result
    /// register (mirrors the graph engine's `block_result`). `None` for an
    /// empty entry block.
    fn entry_result_reg(&self) -> Result<Option<Reg>, String> {
        self.block_result_reg(self.entry_block)
    }

    /// Terms in a block in execution order (entry → `block_next`).
    fn block_terms_in_order(&self, block: BlockId) -> Vec<TermId> {
        let mut out = Vec::new();
        let mut cur = self.program.get_block(block).entry;
        while let Some(tid) = cur {
            out.push(tid);
            cur = self.program.get_term(tid).block_next;
        }
        out
    }

    /// All terms recorded for a block (including phantoms), from the program's
    /// prebuilt `block_terms` index.
    fn all_block_terms(&self, block: BlockId) -> Vec<TermId> {
        self.program
            .block_terms
            .get(&block)
            .cloned()
            .unwrap_or_default()
    }

    fn regs(&self, inputs: &[TermId]) -> Result<SmallVec<[Reg; 4]>, String> {
        inputs.iter().map(|&t| self.flat(t)).collect()
    }

    /// Lower a single non-control-flow term to one instruction. Control-flow
    /// terms (Branch/And/Or/loops/Match/Break/Continue) are handled by
    /// [`emit_block`](Self::emit_block) and never reach here.
    fn lower_term_inst(&self, term: &Term) -> Result<Inst, String> {
        let dst = self.flat(term.id)?;
        let ins = &term.inputs;
        let inst = match &term.op {
            TermOp::Constant(k) => Inst::LoadConst { dst, k: *k },
            TermOp::Error(msg) => Inst::Error { msg: *msg },
            TermOp::Copy => Inst::Move {
                dst,
                src: self.flat(ins[0])?,
            },
            // A Phi initializes its register from the pre-control-flow value;
            // child regions overwrite it via phi_outs (also lowered to Move).
            TermOp::Phi => Inst::Move {
                dst,
                src: self.flat(ins[0])?,
            },

            // State reads/writes are single instructions; StateInit is
            // multi-instruction (inline init block) and handled in emit_block.
            TermOp::StateRead => Inst::StateRead {
                dst,
                base: term.state_key.expect("StateRead without state_key"),
                in_loop: term.in_loop,
            },
            TermOp::StateWrite => Inst::StateWrite {
                dst,
                base: term.state_key.expect("StateWrite without state_key"),
                in_loop: term.in_loop,
                val: self.flat(ins[0])?,
                // Inputs are [value] or [value, explicit_key]; the key is last.
                key: (ins.len() > 1)
                    .then(|| self.flat(ins[ins.len() - 1]))
                    .transpose()?,
                // A plain reassignment commits whatever value it is given,
                // including a Pending (see the StateInit no-commit rule).
                init: false,
            },

            TermOp::Add => Inst::Add {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Sub => Inst::Sub {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Mul => Inst::Mul {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Div => Inst::Div {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Mod => Inst::Mod {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Neg => Inst::Neg {
                dst,
                a: self.flat(ins[0])?,
            },

            TermOp::Eq => Inst::Eq {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Ne => Inst::Ne {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Lt => Inst::Lt {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Le => Inst::Le {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Gt => Inst::Gt {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },
            TermOp::Ge => Inst::Ge {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },

            TermOp::Not => Inst::Not {
                dst,
                a: self.flat(ins[0])?,
            },
            TermOp::Concat => Inst::Concat {
                dst,
                a: self.flat(ins[0])?,
                b: self.flat(ins[1])?,
            },

            TermOp::AllocList => Inst::AllocList {
                dst,
                elems: self.regs(ins)?,
            },
            TermOp::AllocMap { fields } => Inst::AllocMap {
                dst,
                fields: fields.clone(),
                vals: self.regs(ins)?,
            },
            TermOp::AllocMapSpread { entries } => Inst::AllocMapSpread {
                dst,
                entries: entries.clone(),
                ins: self.regs(ins)?,
            },
            TermOp::AllocElement { tag, prop_keys } => Inst::AllocElement {
                dst,
                tag: *tag,
                prop_keys: prop_keys.clone(),
                ins: self.regs(ins)?,
            },
            TermOp::MakeEnumVariant(name) => Inst::MakeEnumVariant {
                dst,
                name: *name,
                fields: self.regs(ins)?,
            },
            TermOp::GetField(field) => Inst::GetField {
                dst,
                obj: self.flat(ins[0])?,
                field: *field,
            },
            TermOp::SetField(field) if self.in_place.allows(term.id) => Inst::SetFieldInPlace {
                dst,
                obj: self.flat(ins[0])?,
                field: *field,
                val: self.flat(ins[1])?,
            },
            TermOp::SetField(field) => Inst::SetField {
                dst,
                obj: self.flat(ins[0])?,
                field: *field,
                val: self.flat(ins[1])?,
            },
            TermOp::GetIndex => Inst::GetIndex {
                dst,
                obj: self.flat(ins[0])?,
                idx: self.flat(ins[1])?,
            },
            TermOp::SetIndex if self.in_place.allows(term.id) => Inst::SetIndexInPlace {
                dst,
                obj: self.flat(ins[0])?,
                idx: self.flat(ins[1])?,
                val: self.flat(ins[2])?,
            },
            TermOp::SetIndex => Inst::SetIndex {
                dst,
                obj: self.flat(ins[0])?,
                idx: self.flat(ins[1])?,
                val: self.flat(ins[2])?,
            },

            // --- calls / closures (M1c) ---
            // A `Call` term is [callable, args...]; a `MethodCall` is
            // [receiver, args...]; a `BuiltinCall` carries only args.
            TermOp::Call => Inst::Call {
                dst,
                callee: self.flat(ins[0])?,
                args: self.regs(&ins[1..])?,
            },
            TermOp::MethodCall(name) => Inst::MethodCall {
                dst,
                recv: self.flat(ins[0])?,
                name: *name,
                args: self.regs(&ins[1..])?,
            },
            TermOp::BuiltinCall(name) => Inst::BuiltinCall {
                dst,
                name: *name,
                args: self.regs(ins)?,
                in_place: self.in_place.allows(term.id),
            },
            TermOp::MakeClosure(fn_id) => Inst::MakeClosure {
                dst,
                func: *fn_id,
                caps: self.regs(ins)?,
            },
            TermOp::MakeOverloadSet => Inst::MakeOverloadSet {
                dst,
                closures: self.regs(ins)?,
            },
            TermOp::Return => Inst::Return {
                val: ins.first().map(|&t| self.flat(t)).transpose()?,
            },

            other => {
                return Err(format!(
                    "unlowered op: {other:?} (arrives in a later milestone)"
                ));
            }
        };
        Ok(inst)
    }

    // -- block emission ------------------------------------------------------

    /// Emit a block's terms in order. Straight-line and call terms lower to one
    /// instruction each; control-flow terms recurse into their child regions.
    fn emit_block(&mut self, block: BlockId) -> Result<(), String> {
        self.region_stack.push(block);
        for tid in self.block_terms_in_order(block) {
            let term = self.program.get_term(tid);
            // Record this term as the origin of the instructions it emits (used
            // for runtime-error source annotation). Nested control-flow handlers
            // update it as they recurse into child blocks.
            self.cur_origin = Some(tid);
            match &term.op {
                TermOp::Branch => self.emit_branch(term)?,
                TermOp::And => self.emit_short_circuit(term, true)?,
                TermOp::Or => self.emit_short_circuit(term, false)?,
                TermOp::Coalesce => self.emit_coalesce(term)?,
                TermOp::ForLoop => self.emit_for_each(term)?,
                TermOp::NumericForLoop => self.emit_range(term)?,
                TermOp::WhileLoop => self.emit_while(term)?,
                TermOp::Break => self.emit_break_or_continue(true)?,
                TermOp::Continue => self.emit_break_or_continue(false)?,
                TermOp::Match => self.emit_match(term)?,
                TermOp::StateInit => self.emit_state_init(term)?,
                _ => {
                    let inst = self.lower_term_inst(term)?;
                    self.push(inst);
                }
            }
        }
        self.region_stack.pop();
        Ok(())
    }

    /// `for x in <list> do <body> end` →
    /// ```text
    ///   foreach_init iter, slot
    /// cont: foreach_next slot -> var else -> exit
    ///   <body>; <body.phi_outs>; jump -> cont
    /// exit: loop_pop slot
    /// ```
    fn emit_for_each(&mut self, term: &Term) -> Result<(), String> {
        let slot = self.alloc_slot();
        let iter = self.flat(term.inputs[0])?;
        let body_block = term.child_blocks[0];
        let var = self.flat_reg(body_block, 0);
        // A Pending iterable absorbs: skip the loop entirely (zero iterations)
        // and yield the Pending as the loop value.
        let jpend = self.emit_placeholder(Inst::JumpIfPending { cond: iter, to: 0 });
        self.push(Inst::ForEachInit {
            iter,
            slot,
            idx_ctx: true,
        });
        let cont = self.here();
        let next = self.emit_placeholder(Inst::ForEachNext { slot, var, exit: 0 });
        self.emit_counted_loop(body_block, slot, cont, next)?;
        let jend = self.emit_placeholder(Inst::Jump { to: 0 });

        let pend_label = self.here();
        self.patch(jpend, pend_label);
        let dst = self.flat(term.id)?;
        self.push(Inst::Move { dst, src: iter });

        let end = self.here();
        self.patch(jend, end);
        Ok(())
    }

    /// `for i in range(a, b) do <body> end` — like [`emit_for_each`](Self::emit_for_each)
    /// but with an integer-range cursor and no list allocation.
    fn emit_range(&mut self, term: &Term) -> Result<(), String> {
        let slot = self.alloc_slot();
        let start = self.flat(term.inputs[0])?;
        let end = self.flat(term.inputs[1])?;
        let body_block = term.child_blocks[0];
        let var = self.flat_reg(body_block, 0);
        self.push(Inst::RangeInit {
            start,
            end,
            slot,
            idx_ctx: true,
        });
        let cont = self.here();
        let next = self.emit_placeholder(Inst::RangeNext { slot, var, exit: 0 });
        self.emit_counted_loop(body_block, slot, cont, next)
    }

    /// Shared tail of the counted loops (`for-each` / range): emit the body with
    /// an active loop context, close the back-edge, and patch the exit and the
    /// break/continue jumps. `next_idx` is the `*Next` placeholder whose exit is
    /// patched to the loop's `LoopPop`.
    fn emit_counted_loop(
        &mut self,
        body_block: BlockId,
        slot: LoopSlot,
        cont: u32,
        next_idx: usize,
    ) -> Result<(), String> {
        self.loop_stack.push(LoopCtx {
            body_block,
            continue_jumps: Vec::new(),
            break_jumps: Vec::new(),
        });
        self.emit_block(body_block)?;
        self.emit_phi_outs(body_block)?; // normal-path carry-outs
        self.push(Inst::Jump { to: cont }); // back-edge

        let ctx = self.loop_stack.pop().unwrap();
        let exit = self.here();
        self.patch(next_idx, exit);
        for j in ctx.break_jumps {
            self.patch(j, exit);
        }
        for j in ctx.continue_jumps {
            self.patch(j, cont);
        }
        self.push(Inst::LoopPop { slot });
        Ok(())
    }

    /// `while <cond> do <body> end` →
    /// ```text
    ///   while_init slot
    /// top: <cond>; <cond.phi_outs>; jump_if_false cond_result -> exit
    ///   <body>; <body.phi_outs>
    /// bump: loop_bump_idx slot; jump -> top
    /// exit: loop_pop slot
    /// ```
    /// `continue` re-evaluates the condition for the next iteration (so it bumps
    /// the index), matching the graph engine.
    fn emit_while(&mut self, term: &Term) -> Result<(), String> {
        let slot = self.alloc_slot();
        let dst = self.flat(term.id)?;
        let cond_block = term.child_blocks[0];
        let body_block = term.child_blocks[1];
        self.push(Inst::WhileInit { slot });
        let top = self.here();
        self.emit_block(cond_block)?;
        self.emit_phi_outs(cond_block)?;
        let cond = self
            .block_result_reg(cond_block)?
            .ok_or("while loop has an empty condition block")?;
        // A Pending condition (this or any later iteration) stops the loop and
        // makes the whole `while` evaluate to that Pending.
        let jpend = self.emit_placeholder(Inst::JumpIfPending { cond, to: 0 });
        let jexit = self.emit_placeholder(Inst::JumpIfFalse { cond, to: 0 });

        self.loop_stack.push(LoopCtx {
            body_block,
            continue_jumps: Vec::new(),
            break_jumps: Vec::new(),
        });
        self.emit_block(body_block)?;
        self.emit_phi_outs(body_block)?; // normal-path carry-outs
        let bump = self.here();
        self.push(Inst::LoopBumpIdx { slot });
        self.push(Inst::Jump { to: top }); // back-edge

        let ctx = self.loop_stack.pop().unwrap();
        // Pending arm: yield the Pending as the loop value, then share the exit
        // (so the loop-index context is still popped).
        let pend_label = self.here();
        self.patch(jpend, pend_label);
        self.push(Inst::Move { dst, src: cond });

        let exit = self.here();
        self.patch(jexit, exit);
        for j in ctx.break_jumps {
            self.patch(j, exit);
        }
        for j in ctx.continue_jumps {
            self.patch(j, bump);
        }
        self.push(Inst::LoopPop { slot });
        Ok(())
    }

    /// `match subject when P1 -> B1 … end` →
    /// ```text
    /// arm0: match_arm subject, term, 0, next=arm1   // bind on match, else -> arm1
    ///       [<guard0>; jump_if_false gres -> arm1]
    ///       <B0>; <B0.phi_outs>; dst = <B0 result>; jump -> end
    /// arm1: …
    /// fail: match_fail subject                       // no arm matched
    /// end:
    /// ```
    /// `MatchArm` runs the shared `match_pattern` and writes captures into the
    /// precomputed flat registers; a present guard block is emitted inline and
    /// its result drives a `JumpIfFalse` to the next arm.
    fn emit_match(&mut self, term: &Term) -> Result<(), String> {
        let subject = self.flat(term.inputs[0])?;
        let dst = self.flat(term.id)?;
        // A Pending subject absorbs: no arm (not even a wildcard) is tested and
        // the match evaluates to the Pending.
        let jpend = self.emit_placeholder(Inst::JumpIfPending {
            cond: subject,
            to: 0,
        });
        let arms: Vec<(BlockId, Option<BlockId>)> = self
            .program
            .match_arms
            .get(&term.id)
            .ok_or("Match: no arm metadata")?
            .iter()
            .map(|a| (a.body_block, a.guard_block))
            .collect();

        // Jumps to the *next* arm (from a failed pattern test or guard) awaiting
        // the next arm's label; and jumps to `end` from each taken arm.
        let mut to_next: Vec<usize> = Vec::new();
        let mut to_end: Vec<usize> = Vec::new();

        for (k, (body_block, guard_block)) in arms.iter().enumerate() {
            let arm_label = self.here();
            for j in to_next.drain(..) {
                self.patch(j, arm_label);
            }
            let ma = self.emit_placeholder(Inst::MatchArm {
                subject,
                term: term.id,
                arm: k as u16,
                next: 0,
                dst,
            });
            to_next.push(ma);
            // Bindings must reach both the guard block and the body block —
            // each has its own registers for the captured names (the graph
            // engine applies bindings to each block separately).
            let mut binds = self.arm_bind_regs(*body_block)?;
            if let Some(gb) = guard_block {
                binds.extend(self.arm_bind_regs(*gb)?);
            }
            self.match_binds.insert((term.id, k as u16), binds);

            if let Some(gb) = guard_block {
                self.emit_block(*gb)?;
                self.emit_phi_outs(*gb)?;
                let gres = self
                    .block_result_reg(*gb)?
                    .ok_or("match guard has an empty block")?;
                let jf = self.emit_placeholder(Inst::JumpIfFalse { cond: gres, to: 0 });
                to_next.push(jf);
            }

            self.emit_arm(*body_block, dst)?;
            to_end.push(self.emit_placeholder(Inst::Jump { to: 0 }));
        }

        let fail_label = self.here();
        for j in to_next.drain(..) {
            self.patch(j, fail_label);
        }
        // Recursion into arm bodies moved cur_origin; restore it so a no-match
        // error points at the match term.
        self.cur_origin = Some(term.id);
        self.push(Inst::MatchFail { subject });

        // Pending arm: yield the Pending subject as the match value. Reached
        // only via `jpend`; `MatchFail` above diverges, so control never falls
        // into it from the fail path.
        let pend_label = self.here();
        self.patch(jpend, pend_label);
        self.push(Inst::Move { dst, src: subject });

        let end = self.here();
        for j in to_end {
            self.patch(j, end);
        }
        Ok(())
    }

    /// `state name = <init>` (optionally `state(key) name`) →
    /// ```text
    ///   state_init dst, base, after=<after>, key   // cache hit: dst=slot; -> after
    ///   <init block>; <init.phi_outs>              // cache miss: compute init
    ///   state_write dst, base, val=<init result>, key   // commit slot + dst
    /// after:
    /// ```
    /// The init block is inlined and reached only on a cache miss; on a hit the
    /// `StateInit` op loads the slot and jumps past it.
    fn emit_state_init(&mut self, term: &Term) -> Result<(), String> {
        let dst = self.flat(term.id)?;
        let base = term.state_key.expect("StateInit without state_key");
        let in_loop = term.in_loop;
        // The explicit `state(expr)` key, if any, is the only input.
        let key = term.inputs.first().map(|&t| self.flat(t)).transpose()?;

        let si = self.emit_placeholder(Inst::StateInit {
            dst,
            base,
            in_loop,
            after: 0,
            key,
        });
        match term.child_blocks.first() {
            Some(&init_block) => {
                self.emit_block(init_block)?;
                self.emit_phi_outs(init_block)?;
                let init_res = self
                    .block_result_reg(init_block)?
                    .ok_or("state init has an empty init block")?;
                // Recursion moved cur_origin; restore it for the commit write.
                self.cur_origin = Some(term.id);
                // `init: true` — a Pending init result is not committed, so the
                // slot re-initializes next frame until the value resolves.
                self.push(Inst::StateWrite {
                    dst,
                    base,
                    in_loop,
                    val: init_res,
                    key,
                    init: true,
                });
            }
            None => {
                // No init block (synthetic StateInit): seed nil.
                self.push(Inst::LoadNil { dst });
                self.push(Inst::StateWrite {
                    dst,
                    base,
                    in_loop,
                    val: dst,
                    key,
                    init: true,
                });
            }
        }
        let after = self.here();
        self.patch(si, after);
        Ok(())
    }

    /// Precompute the flat registers a match arm's body binds pattern variables
    /// into — the flat-register form of the graph engine's
    /// `apply_pattern_bindings` name→register scan over the body block's terms.
    fn arm_bind_regs(&self, body_block: BlockId) -> Result<Vec<(String, Reg)>, String> {
        let mut out = Vec::new();
        if let Some(tids) = self.program.block_terms.get(&body_block) {
            for &tid in tids {
                let term = self.program.get_term(tid);
                if let Some(name) = &term.name {
                    out.push((name.clone(), self.flat(tid)?));
                }
            }
        }
        Ok(out)
    }

    /// `break` / `continue`: emit the phi carry-outs of every region from the
    /// current point up to (and including) the loop body — replicating the
    /// per-block phi propagation the graph engine performs as it pops each
    /// enclosing frame — then a placeholder `Jump` recorded for backpatching to
    /// the loop exit (break) or continue target.
    fn emit_break_or_continue(&mut self, is_break: bool) -> Result<(), String> {
        let body_block = self
            .loop_stack
            .last()
            .ok_or("break/continue outside a loop")?
            .body_block;
        self.emit_exit_phi_chain(body_block)?;
        let j = self.emit_placeholder(Inst::Jump { to: 0 });
        let ctx = self.loop_stack.last_mut().unwrap();
        if is_break {
            ctx.break_jumps.push(j);
        } else {
            ctx.continue_jumps.push(j);
        }
        Ok(())
    }

    /// Emit phi carry-outs for the active regions from the innermost down to
    /// `body_block` (inclusive), innermost first — the order in which the graph
    /// engine runs them as it pops frames on a break/continue.
    fn emit_exit_phi_chain(&mut self, body_block: BlockId) -> Result<(), String> {
        let k = self
            .region_stack
            .iter()
            .rposition(|&b| b == body_block)
            .expect("loop body must be on the region stack");
        let chain: Vec<BlockId> = self.region_stack[k..].iter().rev().copied().collect();
        for blk in chain {
            self.emit_phi_outs(blk)?;
        }
        Ok(())
    }

    /// Allocate a fresh loop-cursor slot for this function.
    fn alloc_slot(&mut self) -> LoopSlot {
        let s = self.next_slot;
        self.next_slot += 1;
        s
    }

    /// `if cond then A else B end` →
    /// ```text
    ///   dst = nil
    ///   jump_if_false cond -> else
    ///   <A>; <A.phi_outs>; dst = <A result>; jump -> end
    /// else:
    ///   <B>; <B.phi_outs>; dst = <B result>
    /// end:
    /// ```
    /// `dst = nil` up front makes an empty/untaken arm yield `nil` (matching the
    /// graph's `block_result`) and clears any stale value from a prior iteration
    /// when the branch sits in a loop body.
    fn emit_branch(&mut self, term: &Term) -> Result<(), String> {
        let dst = self.flat(term.id)?;
        let cond = self.flat(term.inputs[0])?;
        self.push(Inst::LoadNil { dst });
        // A Pending condition absorbs: run no arm and yield the Pending itself.
        let jpend = self.emit_placeholder(Inst::JumpIfPending { cond, to: 0 });
        let jif = self.emit_placeholder(Inst::JumpIfFalse { cond, to: 0 });

        self.emit_arm(term.child_blocks[0], dst)?;
        let jend = self.emit_placeholder(Inst::Jump { to: 0 });

        let else_label = self.here();
        self.patch(jif, else_label);
        if let Some(&else_block) = term.child_blocks.get(1) {
            self.emit_arm(else_block, dst)?;
        }
        // Both real arms jump over the pending arm to `end`.
        let jelse = self.emit_placeholder(Inst::Jump { to: 0 });

        let pend_label = self.here();
        self.patch(jpend, pend_label);
        self.push(Inst::Move { dst, src: cond });

        let end_label = self.here();
        self.patch(jend, end_label);
        self.patch(jelse, end_label);
        Ok(())
    }

    /// `a && b` (`is_and` true) / `a || b` (false) →
    /// ```text
    ///   <short-circuit>: dst = false/true ; jump -> end
    ///   <rhs>: <B>; <B.phi_outs>; dst = <B result>
    /// end:
    /// ```
    /// `&&` runs the rhs when the left is truthy and short-circuits to `false`;
    /// `||` runs the rhs when the left is falsy and short-circuits to `true`.
    fn emit_short_circuit(&mut self, term: &Term, is_and: bool) -> Result<(), String> {
        let dst = self.flat(term.id)?;
        let left = self.flat(term.inputs[0])?;
        let to_rhs = if is_and {
            self.emit_placeholder(Inst::JumpIfTrue { cond: left, to: 0 })
        } else {
            self.emit_placeholder(Inst::JumpIfFalse { cond: left, to: 0 })
        };
        self.push(Inst::LoadBool { dst, val: !is_and });
        let jend = self.emit_placeholder(Inst::Jump { to: 0 });

        let rhs_label = self.here();
        self.patch(to_rhs, rhs_label);
        self.emit_arm(term.child_blocks[0], dst)?;

        let end_label = self.here();
        self.patch(jend, end_label);
        Ok(())
    }

    /// Lower `x ?? y`:
    /// ```text
    ///   dst = <left>
    ///   JumpIfPresent dst -> end   // present LHS wins, skip the RHS
    ///   <rhs>: dst = <rhs result>  // absent LHS: evaluate the fallback
    /// end:
    /// ```
    /// The RHS arm runs only when the LHS is absent (`Nil` or `Pending`), so a
    /// present LHS short-circuits any RHS side effect.
    fn emit_coalesce(&mut self, term: &Term) -> Result<(), String> {
        let dst = self.flat(term.id)?;
        let left = self.flat(term.inputs[0])?;
        self.push(Inst::Move { dst, src: left });
        let to_end = self.emit_placeholder(Inst::JumpIfPresent { cond: dst, to: 0 });
        self.emit_arm(term.child_blocks[0], dst)?;
        let end_label = self.here();
        self.patch(to_end, end_label);
        Ok(())
    }

    /// Emit a child region and join its result: the block's instructions, its
    /// phi carry-outs, then `dst = <block result>` (the control term's value).
    fn emit_arm(&mut self, block: BlockId, dst: Reg) -> Result<(), String> {
        self.emit_block(block)?;
        self.emit_phi_outs(block)?;
        if let Some(src) = self.block_result_reg(block)? {
            self.push(Inst::Move { dst, src });
        }
        Ok(())
    }

    /// Emit a block's phi carry-outs as `Move dest <- src` at the region's exit
    /// edge. In the flat register file, the child's `src` and the parent's
    /// `dest` are distinct registers, so the graph's cross-frame copy becomes a
    /// plain intra-file move.
    fn emit_phi_outs(&mut self, block: BlockId) -> Result<(), String> {
        let outs: Vec<(TermId, TermId)> = self
            .program
            .get_block(block)
            .phi_outs
            .iter()
            .map(|p| (p.src_term, p.dest_term))
            .collect();
        for (src, dest) in outs {
            let dst = self.flat(dest)?;
            let src = self.flat(src)?;
            self.push(Inst::Move { dst, src });
        }
        Ok(())
    }

    /// Flat register of a block's result (its last term), or `None` if empty.
    fn block_result_reg(&self, block: BlockId) -> Result<Option<Reg>, String> {
        self.block_terms_in_order(block)
            .last()
            .map(|&t| self.flat(t))
            .transpose()
    }

    /// Push an instruction that carries a jump target, returning its index for
    /// later [`patch`](Self::patch)ing once the target position is known.
    fn emit_placeholder(&mut self, inst: Inst) -> usize {
        self.push(inst)
    }

    /// The index of the next instruction to be emitted — a forward jump label.
    fn here(&self) -> u32 {
        self.code.len() as u32
    }

    /// Backpatch the jump/exit target of the instruction at `at`.
    fn patch(&mut self, at: usize, target: u32) {
        match self.code[at].branch_target_mut() {
            Some(to) => *to = target,
            None => panic!("patch: not a patchable instruction: {:?}", self.code[at]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use crate::native_fn::NativeFnTable;
    use crate::program::ProgramId;
    use crate::source_map::ENTRY_FILE;

    fn compile(src: &str) -> Program {
        let (_, stmts) = crate::cst::parse_source(src, ENTRY_FILE).expect("parse");
        let mut natives = NativeFnTable::new();
        crate::builtins::register_builtins(&mut natives);
        Compiler::new().compile(&stmts, src.to_string(), ProgramId(0), &natives)
    }

    #[test]
    fn lowers_straight_line_arithmetic() {
        let bc = lower_program(&compile("let x = 1 + 2 * 3")).expect("lower");
        // `let x = <expr>` names the result term directly, so no trailing Copy:
        // const 1, const 2, const 3, mul, add.
        assert!(bc.fns.is_empty(), "no user functions expected");
        let kinds: Vec<&str> = bc
            .root
            .code
            .iter()
            .map(|i| match i {
                Inst::LoadConst { .. } => "const",
                Inst::Add { .. } => "add",
                Inst::Mul { .. } => "mul",
                Inst::Move { .. } => "move",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["const", "const", "const", "mul", "add"]);
    }

    #[test]
    fn lowers_containers_and_access() {
        let bc =
            lower_program(&compile("let p = { a: 1, b: [2, 3] }\nlet y = p.b[1]")).expect("lower");
        let has = |pred: fn(&Inst) -> bool| bc.root.code.iter().any(pred);
        assert!(has(|i| matches!(i, Inst::AllocMap { .. })));
        assert!(has(|i| matches!(i, Inst::AllocList { .. })));
        assert!(has(|i| matches!(i, Inst::GetField { .. })));
        assert!(has(|i| matches!(i, Inst::GetIndex { .. })));
    }

    #[test]
    fn conditionals_lower_to_jumps() {
        // A conditional lowers to a JumpIfFalse over the arm(s).
        let bc = lower_program(&compile("let x = 1\nif x > 0 then x = 2 end")).expect("lower");
        assert!(
            bc.root
                .code
                .iter()
                .any(|i| matches!(i, Inst::JumpIfFalse { .. })),
            "expected a JumpIfFalse in the lowered conditional"
        );
    }

    #[test]
    fn loops_lower_to_cursor_ops() {
        let bc = lower_program(&compile("for i in range(3) do print(i) end")).expect("lower");
        assert!(
            bc.root
                .code
                .iter()
                .any(|i| matches!(i, Inst::RangeInit { .. } | Inst::RangeNext { .. })),
            "expected range cursor ops in the lowered loop"
        );
        assert_eq!(bc.root.loop_slots, 1, "one loop slot expected");
    }

    #[test]
    fn match_lowers_to_arm_ops() {
        let bc = lower_program(&compile(
            "let x = match 1\n  when 1 -> \"a\"\n  when _ -> \"b\"\nend",
        ))
        .expect("lower");
        assert!(
            bc.root
                .code
                .iter()
                .any(|i| matches!(i, Inst::MatchArm { .. })),
            "expected MatchArm ops in the lowered match"
        );
    }

    #[test]
    fn state_lowers_to_state_ops() {
        let bc = lower_program(&compile("state n = 0\nn = n + 1")).expect("lower");
        assert!(
            bc.root
                .code
                .iter()
                .any(|i| matches!(i, Inst::StateInit { .. })),
            "expected a StateInit op"
        );
        assert!(
            bc.root
                .code
                .iter()
                .any(|i| matches!(i, Inst::StateWrite { .. })),
            "expected a StateWrite op"
        );
    }

    #[test]
    fn whole_program_lowers_with_all_op_families() {
        // A program mixing calls, control flow, loops, match, and state now
        // lowers end-to-end (no unlowered ops remain).
        let src = "\
            state total = 0\n\
            fn score(n)\n  match n\n    when 0 -> 0\n    when x if x > 10 -> 100\n    when x -> x\n  end\nend\n\
            for i in range(5) do\n  if i > 0 then total = total + score(i * 3) end\nend\n\
            print(total)";
        lower_program(&compile(src)).expect("whole program should lower");
    }
}

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

use super::isa::{BytecodeFn, BytecodeProgram, Inst, Reg};
use crate::program::{BlockId, FunctionDef, Program, Term, TermId, TermOp};

/// Lower a whole program to bytecode. Returns an error naming the first op that
/// cannot yet be lowered (so `ShowBytecode` surfaces progress honestly).
pub fn lower_program(program: &Program) -> Result<BytecodeProgram, String> {
    let root = FnLowerer::new(program, None, program.root_block).lower()?;
    let mut fns = Vec::with_capacity(program.functions.len());
    for func in &program.functions {
        fns.push(FnLowerer::for_function(program, func).lower()?);
    }
    Ok(BytecodeProgram { root, fns })
}

/// Lowers one function (root block or a `FunctionDef` body) into a [`BytecodeFn`].
struct FnLowerer<'p> {
    program: &'p Program,
    func: Option<&'p FunctionDef>,
    /// The function's entry block (root block, or the def's `body_block`).
    entry_block: BlockId,
    /// Blocks belonging to this function, in discovery order.
    blocks: Vec<BlockId>,
    /// Flat base register offset for each block.
    base: HashMap<BlockId, u16>,
    /// Total flat registers needed.
    reg_count: u16,
    code: Vec<Inst>,
}

impl<'p> FnLowerer<'p> {
    fn new(program: &'p Program, func: Option<&'p FunctionDef>, entry_block: BlockId) -> Self {
        FnLowerer {
            program,
            func,
            entry_block,
            blocks: Vec::new(),
            base: HashMap::new(),
            reg_count: 0,
            code: Vec::new(),
        }
    }

    fn for_function(program: &'p Program, func: &'p FunctionDef) -> Self {
        Self::new(program, Some(func), func.body_block)
    }

    /// Flat register for a term (in any block belonging to this function).
    fn flat(&self, tid: TermId) -> Reg {
        let term = self.program.get_term(tid);
        let base = self
            .base
            .get(&term.block_id)
            .copied()
            .unwrap_or_else(|| panic!("term t{} in block b{} not in this function",
                tid.0, term.block_id.0));
        base + term.register.0
    }

    /// Flat register for a raw `(block, register)` pair (params/captures).
    fn flat_reg(&self, block: BlockId, reg: u16) -> Reg {
        self.base[&block] + reg
    }

    fn lower(mut self) -> Result<BytecodeFn, String> {
        self.collect_blocks();
        self.assign_registers();

        // M0: only the entry block's straight-line terms are emitted. Child
        // blocks (control flow) are collected for register assignment but their
        // lowering arrives in M2.
        for tid in self.block_terms_in_order(self.entry_block) {
            let term = self.program.get_term(tid);
            self.lower_term(term)?;
        }

        let (param_regs, capture_regs, self_ref_reg) = self.binding_regs();
        Ok(BytecodeFn {
            func_id: self.func.map(|f| f.id),
            name: self.func.and_then(|f| f.name.clone()),
            code: self.code,
            reg_count: self.reg_count,
            param_regs,
            capture_regs,
            self_ref_reg,
            loop_slots: 0,
        })
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
                for &cb in &self.program.get_term(tid).child_blocks {
                    stack.push(cb);
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

    fn regs(&self, inputs: &[TermId]) -> SmallVec<[Reg; 4]> {
        inputs.iter().map(|&t| self.flat(t)).collect()
    }

    fn lower_term(&mut self, term: &Term) -> Result<(), String> {
        let dst = self.flat(term.id);
        let ins = &term.inputs;
        let inst = match &term.op {
            TermOp::Constant(k) => Inst::LoadConst { dst, k: *k },
            TermOp::Error(msg) => Inst::Error { msg: *msg },
            TermOp::Copy => Inst::Move { dst, src: self.flat(ins[0]) },

            TermOp::Add => Inst::Add { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Sub => Inst::Sub { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Mul => Inst::Mul { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Div => Inst::Div { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Mod => Inst::Mod { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Neg => Inst::Neg { dst, a: self.flat(ins[0]) },

            TermOp::Eq => Inst::Eq { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Ne => Inst::Ne { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Lt => Inst::Lt { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Le => Inst::Le { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Gt => Inst::Gt { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },
            TermOp::Ge => Inst::Ge { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },

            TermOp::Not => Inst::Not { dst, a: self.flat(ins[0]) },
            TermOp::Concat => Inst::Concat { dst, a: self.flat(ins[0]), b: self.flat(ins[1]) },

            TermOp::AllocList => Inst::AllocList { dst, elems: self.regs(ins) },
            TermOp::AllocMap { fields } => {
                Inst::AllocMap { dst, fields: fields.clone(), vals: self.regs(ins) }
            }
            TermOp::AllocMapSpread { entries } => {
                Inst::AllocMapSpread { dst, entries: entries.clone(), ins: self.regs(ins) }
            }
            TermOp::AllocElement { tag, prop_keys } => Inst::AllocElement {
                dst,
                tag: *tag,
                prop_keys: prop_keys.clone(),
                ins: self.regs(ins),
            },
            TermOp::MakeEnumVariant(name) => {
                Inst::MakeEnumVariant { dst, name: *name, fields: self.regs(ins) }
            }
            TermOp::GetField(field) => Inst::GetField { dst, obj: self.flat(ins[0]), field: *field },
            TermOp::SetField(field) => Inst::SetField {
                dst,
                obj: self.flat(ins[0]),
                field: *field,
                val: self.flat(ins[1]),
            },
            TermOp::GetIndex => {
                Inst::GetIndex { dst, obj: self.flat(ins[0]), idx: self.flat(ins[1]) }
            }
            TermOp::SetIndex => Inst::SetIndex {
                dst,
                obj: self.flat(ins[0]),
                idx: self.flat(ins[1]),
                val: self.flat(ins[2]),
            },

            other => {
                return Err(format!("unlowered op: {} (arrives in a later milestone)", op_name(other)));
            }
        };
        self.code.push(inst);
        Ok(())
    }
}

/// Short display name for a `TermOp`, for `unlowered op` diagnostics.
fn op_name(op: &TermOp) -> &'static str {
    match op {
        TermOp::And => "And",
        TermOp::Or => "Or",
        TermOp::Phi => "Phi",
        TermOp::Branch => "Branch",
        TermOp::ForLoop => "ForLoop",
        TermOp::NumericForLoop => "NumericForLoop",
        TermOp::WhileLoop => "WhileLoop",
        TermOp::Break => "Break",
        TermOp::Continue => "Continue",
        TermOp::Return => "Return",
        TermOp::MakeClosure(_) => "MakeClosure",
        TermOp::MakeOverloadSet => "MakeOverloadSet",
        TermOp::Call => "Call",
        TermOp::MethodCall(_) => "MethodCall",
        TermOp::BuiltinCall(_) => "BuiltinCall",
        TermOp::StateInit => "StateInit",
        TermOp::StateRead => "StateRead",
        TermOp::StateWrite => "StateWrite",
        TermOp::Match => "Match",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use crate::lexer::Lexer;
    use crate::native_fn::NativeFnTable;
    use crate::parse::Parser;
    use crate::program::ProgramId;

    fn compile(src: &str) -> Program {
        let mut lexer = Lexer::new(src);
        lexer.tokenize().expect("tokenize");
        let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
        let stmts = parser.parse_program().expect("parse");
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
        let bc = lower_program(&compile(
            "let p = { a: 1, b: [2, 3] }\nlet y = p.b[1]",
        ))
        .expect("lower");
        let has = |pred: fn(&Inst) -> bool| bc.root.code.iter().any(pred);
        assert!(has(|i| matches!(i, Inst::AllocMap { .. })));
        assert!(has(|i| matches!(i, Inst::AllocList { .. })));
        assert!(has(|i| matches!(i, Inst::GetField { .. })));
        assert!(has(|i| matches!(i, Inst::GetIndex { .. })));
    }

    #[test]
    fn control_flow_is_unlowered_for_now() {
        // A conditional emits Phi; M0 lowering reports it honestly rather than
        // producing wrong code.
        let err = lower_program(&compile("let x = 1\nif x > 0 then x = 2 end"))
            .expect_err("control flow should be unlowered in M0");
        assert!(err.contains("unlowered op"), "got: {err}");
    }
}

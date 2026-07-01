//! Per-term dispatch for the graph engine.
//!
//! Most value-producing ops (arithmetic, comparison, allocation, field/index
//! access) delegate to the backend-shared handlers in [`crate::backend::ops`];
//! this file keeps the graph-specific control-flow ops (branches, loops, calls,
//! state, match) that push frames or signal break/continue/return.

use super::*;
use crate::backend::ops;

impl<'a> Evaluator<'a> {
    /// Execute a single term. Most ops compute a value and finish via
    /// `produce` (write result, advance); control-flow ops push frames or
    /// signal break/continue/return instead.
    pub(super) fn exec_term(&mut self, term: &Term, inputs: &[Value]) -> ControlFlow {
        match &term.op {
            TermOp::Constant(cid) => {
                let val = ops::constant_to_value(self.program, self.heap, *cid);
                self.produce(term, val)
            }

            TermOp::Error(cid) => {
                let msg = self
                    .program
                    .get_string_constant(*cid)
                    .unwrap_or("Unknown error")
                    .to_string();
                ControlFlow::Error(msg)
            }

            // Identity / variable reference
            TermOp::Copy => self.produce(term, inputs.first().copied().unwrap_or(Value::Nil)),

            // Initialize the phi's register from inputs[0] — the
            // pre-control-flow value of the name being joined. When a
            // child frame later rebinds the name, its pop's phi_outs
            // overwrite this register; branches that don't rebind leave
            // the init value in place.
            TermOp::Phi => self.produce(term, inputs.first().copied().unwrap_or(Value::Nil)),

            TermOp::Add | TermOp::Sub | TermOp::Mul | TermOp::Div | TermOp::Mod => {
                self.numeric_binop(term, inputs)
            }
            TermOp::Neg => match ops::negate(inputs.first().copied().unwrap_or(Value::Nil)) {
                Ok(val) => self.produce(term, val),
                Err(e) => ControlFlow::Error(e),
            },

            TermOp::Not => {
                let val = ops::not(inputs.first().copied().unwrap_or(Value::Nil));
                self.produce(term, val)
            }

            TermOp::Eq => {
                let val = Value::Bool(ops::equals(inputs[0], inputs[1], self.heap));
                self.produce(term, val)
            }
            TermOp::Ne => {
                let val = Value::Bool(!ops::equals(inputs[0], inputs[1], self.heap));
                self.produce(term, val)
            }
            TermOp::Lt | TermOp::Le | TermOp::Gt | TermOp::Ge => self.comparison_op(term, inputs),

            TermOp::Concat => match ops::concat(inputs[0], inputs[1], self.heap) {
                Ok(val) => self.produce(term, val),
                Err(e) => ControlFlow::Error(e),
            },

            // Short-circuit: when the left side decides the answer, produce
            // it; otherwise run the RHS block.
            TermOp::And => {
                if !inputs[0].is_truthy() {
                    self.produce(term, Value::Bool(false))
                } else {
                    self.push_child_frame(term.child_blocks[0], term);
                    ControlFlow::FramePushed
                }
            }
            TermOp::Or => {
                if inputs[0].is_truthy() {
                    self.produce(term, Value::Bool(true))
                } else {
                    self.push_child_frame(term.child_blocks[0], term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Branch => {
                let block_idx = if inputs[0].is_truthy() { 0 } else { 1 };
                self.push_child_frame(term.child_blocks[block_idx], term);
                ControlFlow::FramePushed
            }

            TermOp::ForLoop => self.exec_for_loop(term, inputs),
            TermOp::NumericForLoop => self.exec_numeric_for_loop(term, inputs),
            TermOp::WhileLoop => self.exec_while_loop(term),

            TermOp::Break => ControlFlow::Break,
            TermOp::Continue => ControlFlow::Continue,
            TermOp::Return => ControlFlow::Return(inputs.first().copied().unwrap_or(Value::Nil)),

            TermOp::MakeOverloadSet => self.exec_make_overload_set(term, inputs),
            TermOp::Call => self.exec_call(term, inputs),
            TermOp::MethodCall(method_cid) => self.exec_method_call(*method_cid, term, inputs),
            TermOp::BuiltinCall(name_cid) => self.exec_builtin_call(*name_cid, term, inputs),

            TermOp::MakeClosure(fn_id) => {
                let closure_id = ClosureId(self.closures.len() as u32);
                self.closures.push(RuntimeClosure {
                    function_id: *fn_id,
                    captures: inputs.to_vec(),
                });
                self.produce(term, Value::Closure(closure_id))
            }

            TermOp::StateInit => self.exec_state_init(term, inputs),
            TermOp::StateRead => self.exec_state_read(term),
            TermOp::StateWrite => self.exec_state_write(term, inputs),

            TermOp::AllocList => {
                let val = ops::alloc_list(self.heap, inputs);
                self.produce(term, val)
            }
            TermOp::AllocMap { fields } => {
                match ops::alloc_map(self.program, self.heap, fields, inputs) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }
            TermOp::AllocMapSpread { entries } => {
                match ops::alloc_map_spread(self.program, self.heap, entries, inputs) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }
            TermOp::AllocElement { tag, prop_keys } => {
                match ops::alloc_element(self.program, self.heap, *tag, prop_keys, inputs) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }

            TermOp::GetField(field_cid) => {
                match ops::get_field(self.program, self.heap, *field_cid, inputs[0]) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }
            TermOp::SetField(field_cid) => {
                match ops::set_field(self.program, self.heap, *field_cid, inputs[0], inputs[1]) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }
            TermOp::GetIndex => match ops::get_index(self.heap, inputs[0], inputs[1]) {
                Ok(val) => self.produce(term, val),
                Err(e) => ControlFlow::Error(e),
            },
            TermOp::SetIndex => {
                match ops::set_index(self.heap, inputs[0], inputs[1], inputs[2]) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }

            TermOp::MakeEnumVariant(name_cid) => {
                match ops::make_enum_variant(self.program, self.heap, *name_cid, inputs) {
                    Ok(val) => self.produce(term, val),
                    Err(e) => ControlFlow::Error(e),
                }
            }

            TermOp::Match => self.exec_match(term, inputs),
        }
    }
}

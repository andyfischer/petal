//! Eval - Step-based IR evaluator.
//!
//! Executes a program by walking the term graph one term at a time.

use std::collections::BTreeMap;

use crate::ast::*;
use crate::builtins;
use crate::constant_table::{ConstantId, ConstantValue};
use crate::heap::Heap;
use crate::native_fn::{NativeFnTable, PetalState};
use crate::program::*;
use crate::stack::{Frame, LoopState, Stack};
use crate::value::{self, Value};

/// Result of a single evaluation step.
#[derive(Debug)]
pub enum StepResult {
    Continue,
    Complete(Value),
    Error(String),
}

/// Signal for control flow within the evaluator.
enum ControlFlow {
    /// Normal — advance to next term
    Advance,
    /// Frame was pushed — don't advance, execute new frame
    FramePushed,
    /// Return from function
    Return(Value),
    /// Break from loop
    Break,
    /// Continue to next iteration
    Continue,
    /// Fatal error
    Error(String),
}

/// The evaluator operates on Env's data.
pub struct Evaluator;

impl Evaluator {
    /// Execute one step: evaluate the current term and advance.
    pub fn step(
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> StepResult {
        let frame_idx = match stack.frames.len().checked_sub(1) {
            Some(idx) => idx,
            None => return StepResult::Complete(Value::Nil),
        };

        let current_term_id = match stack.frames[frame_idx].current_term {
            Some(tid) => tid,
            None => {
                // Block is done — pop frame
                return Self::pop_frame(program, stack, heap);
            }
        };

        // If break_flag or continue_flag is set and the current frame is a direct loop body,
        // skip remaining terms and pop immediately so the parent loop term
        // can handle the break/continue on its next execution.
        if (stack.break_flag || stack.continue_flag) && stack.frames[frame_idx].is_loop_body {
            return Self::pop_frame(program, stack, heap);
        }

        let term = program.get_term(current_term_id);

        // Read input values
        let input_values: Vec<Value> = term
            .inputs
            .iter()
            .map(|&input_tid| Self::read_register(program, stack, input_tid))
            .collect();

        // Execute the term
        let result = Self::exec_term(
            term,
            &input_values,
            program,
            stack,
            heap,
            closures,
            native_fns,
            output,
        );

        match result {
            ControlFlow::Advance => {
                // Store result and advance
                Self::advance(stack, term);
                StepResult::Continue
            }
            ControlFlow::FramePushed => {
                // New frame was pushed — continue executing it
                StepResult::Continue
            }
            ControlFlow::Return(val) => {
                // Pop frames to function boundary
                Self::handle_return(program, stack, val);
                StepResult::Continue
            }
            ControlFlow::Break => {
                stack.break_flag = true;
                // Advance past the break term then the loop handler will catch it
                Self::advance(stack, term);
                StepResult::Continue
            }
            ControlFlow::Continue => {
                stack.continue_flag = true;
                Self::advance(stack, term);
                StepResult::Continue
            }
            ControlFlow::Error(msg) => {
                // Annotate error with source position from the term that failed
                if let Some(span) = program.source_map.get(current_term_id) {
                    if span.start.line > 0 {
                        StepResult::Error(format!(
                            "{} [line {}, column {}]",
                            msg, span.start.line, span.start.column
                        ))
                    } else {
                        StepResult::Error(msg)
                    }
                } else {
                    StepResult::Error(msg)
                }
            }
        }
    }

    /// Read a value from the register of the term that produced it.
    fn read_register(program: &Program, stack: &Stack, term_id: TermId) -> Value {
        let term = program.get_term(term_id);
        let term_block = term.block_id;
        let reg_idx = term.register.0 as usize;

        // Search from current frame upward through parent_frame links
        let mut frame_idx = stack.frames.len() - 1;
        loop {
            let frame = &stack.frames[frame_idx];
            if frame.block_id == term_block {
                return if reg_idx < frame.registers.len() {
                    frame.registers[reg_idx]
                } else {
                    Value::Nil
                };
            }
            match frame.parent_frame {
                Some(parent) => frame_idx = parent,
                None => return Value::Nil, // not found
            }
        }
    }

    /// Store result in current frame's register and advance to next term.
    fn advance(stack: &mut Stack, term: &Term) {
        if let Some(frame) = stack.frames.last_mut() {
            let reg = term.register.0 as usize;
            // Ensure register file is large enough
            if reg >= frame.registers.len() {
                frame.registers.resize(reg + 1, Value::Nil);
            }
            // Note: result was already written by exec_term for most ops
            frame.current_term = term.block_next;
        }
    }

    /// Write a value to the current term's register.
    fn write_register(stack: &mut Stack, term: &Term, value: Value) {
        if let Some(frame) = stack.frames.last_mut() {
            let reg = term.register.0 as usize;
            if reg >= frame.registers.len() {
                frame.registers.resize(reg + 1, Value::Nil);
            }
            frame.registers[reg] = value;
        }
    }

    /// Pop the current frame and handle the result.
    fn pop_frame(program: &Program, stack: &mut Stack, _heap: &Heap) -> StepResult {
        let frame = match stack.pop_frame() {
            Some(f) => f,
            None => return StepResult::Complete(Value::Nil),
        };

        // When a loop body pops due to continue, clear the flag immediately
        // so it doesn't propagate to outer loop bodies.
        if stack.continue_flag && frame.is_loop_body {
            stack.continue_flag = false;
        }

        // Get the last term's value as the block result
        let block = program.get_block(frame.block_id);
        let result = Self::get_last_register_value(&frame, block, program);

        // Always store the result for synchronous closure callers
        stack.last_pop_result = Some(result);

        if stack.frames.is_empty() {
            // Program complete
            return StepResult::Complete(result);
        }

        // Write result to the parent term's register
        if let Some(return_term) = frame.return_term {
            let parent_term = program.get_term(return_term);
            let reg = parent_term.register.0 as usize;
            if let Some(parent_frame) = stack.frames.last_mut() {
                if reg >= parent_frame.registers.len() {
                    parent_frame.registers.resize(reg + 1, Value::Nil);
                }
                parent_frame.registers[reg] = result;
            }
        }

        StepResult::Continue
    }

    fn get_last_register_value(frame: &Frame, block: &Block, program: &Program) -> Value {
        // Find the last term in this block and read its register
        let mut current = block.entry;
        let mut last_tid = None;
        while let Some(tid) = current {
            last_tid = Some(tid);
            current = program.get_term(tid).block_next;
        }
        if let Some(tid) = last_tid {
            let term = program.get_term(tid);
            let reg = term.register.0 as usize;
            if reg < frame.registers.len() {
                return frame.registers[reg];
            }
        }
        Value::Nil
    }

    fn handle_return(program: &Program, stack: &mut Stack, value: Value) {
        // Pop frames until we find a function call frame (parent_frame == None)
        loop {
            let frame = match stack.pop_frame() {
                Some(f) => f,
                None => return,
            };
            if frame.parent_frame.is_none() {
                // Store for synchronous closure callers
                stack.last_pop_result = Some(value);
                // This was a function frame — write return value to caller
                if let Some(return_term) = frame.return_term {
                    let parent_term = program.get_term(return_term);
                    let reg = parent_term.register.0 as usize;
                    if let Some(caller_frame) = stack.frames.last_mut() {
                        if reg >= caller_frame.registers.len() {
                            caller_frame.registers.resize(reg + 1, Value::Nil);
                        }
                        caller_frame.registers[reg] = value;
                        // Advance past the Call term
                        caller_frame.current_term = parent_term.block_next;
                    }
                }
                return;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Term execution
    // -----------------------------------------------------------------------

    fn exec_term(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> ControlFlow {
        match &term.op {
            TermOp::Constant(cid) => {
                let val = Self::constant_to_value(*cid, program, heap);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Error(cid) => {
                let msg = match program.constants.get(*cid) {
                    ConstantValue::String(s) => s.clone(),
                    _ => "Unknown error".to_string(),
                };
                ControlFlow::Error(msg)
            }

            TermOp::Copy => {
                // Identity / variable reference
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Assign(target_tid) => {
                // Write value to the target term's register in its frame
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                let target_term = program.get_term(*target_tid);
                let target_block = target_term.block_id;
                let target_reg = target_term.register.0 as usize;

                // Walk parent_frame links to find the frame holding the target block
                let mut frame_idx = stack.frames.len() - 1;
                loop {
                    if stack.frames[frame_idx].block_id == target_block {
                        if target_reg < stack.frames[frame_idx].registers.len() {
                            stack.frames[frame_idx].registers[target_reg] = val;
                        }
                        break;
                    }
                    match stack.frames[frame_idx].parent_frame {
                        Some(parent) => frame_idx = parent,
                        None => break,
                    }
                }

                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Add => Self::numeric_binop(term, inputs, stack, heap,
                |a, b| Value::Int(a + b), |a, b| Value::Float(a + b)),
            TermOp::Sub => Self::numeric_binop(term, inputs, stack, heap,
                |a, b| Value::Int(a - b), |a, b| Value::Float(a - b)),
            TermOp::Mul => Self::numeric_binop(term, inputs, stack, heap,
                |a, b| Value::Int(a * b), |a, b| Value::Float(a * b)),
            TermOp::Div => {
                if inputs.len() < 2 {
                    return ControlFlow::Error("Div: missing inputs".into());
                }
                match (&inputs[0], &inputs[1]) {
                    (_, Value::Int(0)) => return ControlFlow::Error("Division by zero".into()),
                    (_, Value::Float(f)) if *f == 0.0 => return ControlFlow::Error("Division by zero".into()),
                    _ => {}
                }
                Self::numeric_binop(term, inputs, stack, heap,
                    |a, b| Value::Int(a / b), |a, b| Value::Float(a / b))
            }
            TermOp::Mod => {
                if inputs.len() < 2 {
                    return ControlFlow::Error("Mod: missing inputs".into());
                }
                Self::numeric_binop(term, inputs, stack, heap,
                    |a, b| Value::Int(a % b), |a, b| Value::Float(a % b))
            }

            TermOp::Neg => {
                let val = match inputs.first() {
                    Some(Value::Int(n)) => Value::Int(-n),
                    Some(Value::Float(f)) => Value::Float(-f),
                    Some(v) => return ControlFlow::Error(format!("Cannot negate {}", v.type_name())),
                    None => return ControlFlow::Error("Neg: missing input".into()),
                };
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Not => {
                let val = match inputs.first() {
                    Some(v) => Value::Bool(!v.is_truthy()),
                    None => return ControlFlow::Error("Not: missing input".into()),
                };
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::Eq => {
                let val = Value::Bool(value::values_equal(&inputs[0], &inputs[1], heap));
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            TermOp::Ne => {
                let val = Value::Bool(!value::values_equal(&inputs[0], &inputs[1], heap));
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            TermOp::Lt => Self::comparison_op(term, inputs, stack, heap, |ord| ord == std::cmp::Ordering::Less),
            TermOp::Le => Self::comparison_op(term, inputs, stack, heap, |ord| ord != std::cmp::Ordering::Greater),
            TermOp::Gt => Self::comparison_op(term, inputs, stack, heap, |ord| ord == std::cmp::Ordering::Greater),
            TermOp::Ge => Self::comparison_op(term, inputs, stack, heap, |ord| ord != std::cmp::Ordering::Less),

            TermOp::Concat => {
                match (inputs[0], inputs[1]) {
                    (Value::List(a), Value::List(b)) => {
                        let mut combined = heap.get_list(a).to_vec();
                        combined.extend_from_slice(heap.get_list(b));
                        let id = heap.alloc_list(combined);
                        Self::write_register(stack, term, Value::List(id));
                    }
                    _ => {
                        let l = value::value_to_display_string(&inputs[0], heap);
                        let r = value::value_to_display_string(&inputs[1], heap);
                        let s = format!("{}{}", l, r);
                        let sid = heap.alloc_string(s);
                        Self::write_register(stack, term, Value::String(sid));
                    }
                }
                ControlFlow::Advance
            }

            TermOp::And => {
                let left = inputs[0];
                if !left.is_truthy() {
                    Self::write_register(stack, term, Value::Bool(false));
                    ControlFlow::Advance
                } else {
                    // Push frame for RHS block
                    let rhs_block = term.child_blocks[0];
                    Self::push_child_frame(program, stack, rhs_block, term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Or => {
                let left = inputs[0];
                if left.is_truthy() {
                    Self::write_register(stack, term, Value::Bool(true));
                    ControlFlow::Advance
                } else {
                    let rhs_block = term.child_blocks[0];
                    Self::push_child_frame(program, stack, rhs_block, term);
                    ControlFlow::FramePushed
                }
            }

            TermOp::Branch => {
                let cond = inputs[0];
                let block_idx = if cond.is_truthy() { 0 } else { 1 };
                let target_block = term.child_blocks[block_idx];
                Self::push_child_frame(program, stack, target_block, term);
                ControlFlow::FramePushed
            }

            TermOp::ForLoop => {
                Self::exec_for_loop(term, inputs, program, stack, heap)
            }

            TermOp::WhileLoop => {
                Self::exec_while_loop(term, program, stack)
            }

            TermOp::Break => ControlFlow::Break,
            TermOp::Continue => ControlFlow::Continue,

            TermOp::Return => {
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                ControlFlow::Return(val)
            }

            TermOp::Call => {
                Self::exec_call(term, inputs, program, stack, heap, closures, native_fns, output)
            }

            TermOp::MethodCall(method_cid) => {
                Self::exec_method_call(*method_cid, term, inputs, program, stack, heap, closures, native_fns, output)
            }

            TermOp::MakeClosure(fn_id) => {
                let captures: Vec<Value> = inputs.to_vec();
                let closure_id = ClosureId(closures.len() as u32);
                closures.push(RuntimeClosure {
                    function_id: *fn_id,
                    captures,
                });
                Self::write_register(stack, term, Value::Closure(closure_id));
                ControlFlow::Advance
            }

            TermOp::StateInit => {
                let state_key = term.state_key.unwrap();
                if !stack.state.contains_key(&state_key) {
                    let init_val = inputs.first().copied().unwrap_or(Value::Nil);
                    stack.state.insert(state_key, init_val);
                }
                let val = stack.state[&state_key];
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::StateRead => {
                let state_key = term.state_key.unwrap();
                let val = stack.state.get(&state_key).copied().unwrap_or(Value::Nil);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::StateWrite => {
                let state_key = term.state_key.unwrap();
                let val = inputs.first().copied().unwrap_or(Value::Nil);
                stack.state.insert(state_key, val);
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }

            TermOp::AllocList => {
                let list_id = heap.alloc_list(inputs.to_vec());
                Self::write_register(stack, term, Value::List(list_id));
                ControlFlow::Advance
            }

            TermOp::AllocMap { fields } => {
                let mut map = BTreeMap::new();
                for (i, field_cid) in fields.iter().enumerate() {
                    if let ConstantValue::String(key) = program.constants.get(*field_cid) {
                        let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                        map.insert(key.clone(), val);
                    }
                }
                let map_id = heap.alloc_map(map);
                Self::write_register(stack, term, Value::Map(map_id));
                ControlFlow::Advance
            }

            TermOp::AllocElement { tag, prop_keys } => {
                let tag_str = match program.constants.get(*tag) {
                    ConstantValue::String(s) => s.clone(),
                    _ => return ControlFlow::Error("AllocElement: invalid tag".into()),
                };
                let tag_id = heap.alloc_string(tag_str);

                let num_props = prop_keys.len();
                let mut map = BTreeMap::new();
                for (i, key_cid) in prop_keys.iter().enumerate() {
                    if let ConstantValue::String(key) = program.constants.get(*key_cid) {
                        let val = inputs.get(i).copied().unwrap_or(Value::Nil);
                        map.insert(key.clone(), val);
                    }
                }
                let props_id = heap.alloc_map(map);

                let children_id = heap.alloc_list(inputs[num_props..].to_vec());

                let elem_id = heap.alloc_element(tag_id, props_id, children_id);
                Self::write_register(stack, term, Value::Element(elem_id));
                ControlFlow::Advance
            }

            TermOp::GetField(field_cid) => {
                let obj = inputs[0];
                match obj {
                    Value::Map(map_id) => {
                        let field_name = match program.constants.get(*field_cid) {
                            ConstantValue::String(s) => s.as_str(),
                            _ => return ControlFlow::Error("GetField: invalid field name".into()),
                        };
                        let map = heap.get_map(map_id);
                        let val = map
                            .get(field_name)
                            .copied()
                            .ok_or_else(|| format!("No field '{}' on record", field_name));
                        match val {
                            Ok(v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            Err(e) => ControlFlow::Error(e),
                        }
                    }
                    Value::Element(elem_id) => {
                        let field_name = match program.constants.get(*field_cid) {
                            ConstantValue::String(s) => s.as_str(),
                            _ => return ControlFlow::Error("GetField: invalid field name".into()),
                        };
                        let val = match field_name {
                            "tag" => {
                                let tag_id = heap.get_element_tag(elem_id);
                                Value::String(tag_id)
                            }
                            "props" => Value::Map(heap.get_element_props(elem_id)),
                            "children" => Value::List(heap.get_element_children(elem_id)),
                            _ => {
                                return ControlFlow::Error(format!(
                                    "No field '{}' on element",
                                    field_name
                                ))
                            }
                        };
                        Self::write_register(stack, term, val);
                        ControlFlow::Advance
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot access field on {}",
                        obj.type_name()
                    )),
                }
            }

            TermOp::SetField(field_cid) => {
                let obj = inputs[0];
                let val = inputs[1];
                match obj {
                    Value::Map(map_id) => {
                        let field_name = match program.constants.get(*field_cid) {
                            ConstantValue::String(s) => s.clone(),
                            _ => return ControlFlow::Error("SetField: invalid field name".into()),
                        };
                        heap.get_map_mut(map_id).insert(field_name, val);
                        Self::write_register(stack, term, Value::Nil);
                        ControlFlow::Advance
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot set field on {}",
                        obj.type_name()
                    )),
                }
            }

            TermOp::GetIndex => {
                let obj = inputs[0];
                let idx = inputs[1];
                match (obj, idx) {
                    (Value::List(list_id), Value::Int(i)) => {
                        let list = heap.get_list(list_id);
                        let index = if i < 0 {
                            (list.len() as i64 + i) as usize
                        } else {
                            i as usize
                        };
                        match list.get(index) {
                            Some(&v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            None => ControlFlow::Error(format!(
                                "Index {} out of bounds (len {})",
                                i,
                                list.len()
                            )),
                        }
                    }
                    (Value::Map(map_id), Value::String(key_id)) => {
                        let key = heap.get_string(key_id).to_string();
                        let map = heap.get_map(map_id);
                        match map.get(&key) {
                            Some(&v) => {
                                Self::write_register(stack, term, v);
                                ControlFlow::Advance
                            }
                            None => ControlFlow::Error(format!("No key '{}' on record", key)),
                        }
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot index {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    )),
                }
            }

            TermOp::SetIndex => {
                let obj = inputs[0];
                let idx = inputs[1];
                let val = inputs[2];
                match (obj, idx) {
                    (Value::List(list_id), Value::Int(i)) => {
                        let list = heap.get_list_mut(list_id);
                        let index = i as usize;
                        if index < list.len() {
                            list[index] = val;
                            Self::write_register(stack, term, Value::Nil);
                            ControlFlow::Advance
                        } else {
                            ControlFlow::Error(format!(
                                "Index {} out of bounds (len {})",
                                i,
                                list.len()
                            ))
                        }
                    }
                    _ => ControlFlow::Error(format!(
                        "Cannot index-assign {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    )),
                }
            }

            TermOp::MakeEnumVariant(name_cid) => {
                let name_str = match program.constants.get(*name_cid) {
                    ConstantValue::String(s) => s.clone(),
                    _ => return ControlFlow::Error("MakeEnumVariant: invalid name".into()),
                };
                let tag = heap.alloc_string(name_str);
                let data = heap.alloc_list(inputs.to_vec());
                Self::write_register(stack, term, Value::EnumVariant { tag, data });
                ControlFlow::Advance
            }

            TermOp::Match => {
                Self::exec_match(term, inputs, program, stack, heap, closures, native_fns, output)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Extracted term handlers
    // -----------------------------------------------------------------------

    fn exec_for_loop(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
    ) -> ControlFlow {
        // Handle break from a completed iteration.
        if stack.break_flag {
            stack.break_flag = false;
            if let Some(frame) = stack.frames.last_mut() {
                frame.loop_states.remove(&term.id);
            }
            Self::write_register(stack, term, Value::Nil);
            return ControlFlow::Advance;
        }

        // Handle continue — just clear the flag and proceed to next iteration.
        if stack.continue_flag {
            stack.continue_flag = false;
        }

        let body_block = term.child_blocks[0];

        // Initialize loop state on the first visit.
        let needs_init = stack.frames.last()
            .map(|f| !f.loop_states.contains_key(&term.id))
            .unwrap_or(false);
        if needs_init {
            match inputs[0] {
                Value::List(list_id) => {
                    let elements = heap.get_list(list_id).to_vec();
                    if let Some(frame) = stack.frames.last_mut() {
                        frame.loop_states.insert(
                            term.id,
                            LoopState::For { elements, index: 0 },
                        );
                    }
                }
                other => {
                    return ControlFlow::Error(format!(
                        "Cannot iterate over {}",
                        other.type_name()
                    ))
                }
            }
        }

        // Get the next element (or detect loop completion).
        let maybe_elem: Option<Value> = {
            let frame = stack.frames.last_mut().unwrap();
            match frame.loop_states.get_mut(&term.id) {
                Some(LoopState::For { elements, index }) => {
                    if *index < elements.len() {
                        let elem = elements[*index];
                        *index += 1;
                        Some(elem)
                    } else {
                        frame.loop_states.remove(&term.id);
                        None
                    }
                }
                _ => None,
            }
        };

        match maybe_elem {
            Some(elem) => {
                // Push body frame for this iteration.
                let block = program.get_block(body_block);
                let parent_frame_idx = stack.frames.len() - 1;
                stack.push_frame(
                    Frame::new(body_block, block.entry, block.register_count as usize,
                        Some(term.id), Some(parent_frame_idx))
                    .as_loop_body()
                );
                // Set the loop variable in the first register.
                if let Some(frame) = stack.frames.last_mut() {
                    if !frame.registers.is_empty() {
                        frame.registers[0] = elem;
                    }
                }
                ControlFlow::FramePushed
            }
            None => {
                // All iterations complete.
                Self::write_register(stack, term, Value::Nil);
                ControlFlow::Advance
            }
        }
    }

    fn exec_while_loop(
        term: &Term,
        program: &Program,
        stack: &mut Stack,
    ) -> ControlFlow {
        // Handle break from the body.
        if stack.break_flag {
            stack.break_flag = false;
            if let Some(frame) = stack.frames.last_mut() {
                frame.loop_states.remove(&term.id);
            }
            Self::write_register(stack, term, Value::Nil);
            return ControlFlow::Advance;
        }

        // Handle continue — clear the flag and re-evaluate condition.
        if stack.continue_flag {
            stack.continue_flag = false;
        }

        let cond_block = term.child_blocks[0];
        let body_block = term.child_blocks[1];

        let is_awaiting_cond = stack.frames.last()
            .map(|f| matches!(f.loop_states.get(&term.id), Some(LoopState::WhileCondition)))
            .unwrap_or(false);

        if is_awaiting_cond {
            // The condition block just returned; its result was written to
            // this term's register by pop_frame.
            let cond_val = Self::read_register(program, stack, term.id);
            if let Some(frame) = stack.frames.last_mut() {
                frame.loop_states.remove(&term.id);
            }

            if !cond_val.is_truthy() {
                // Condition false — loop done.
                Self::write_register(stack, term, Value::Nil);
                return ControlFlow::Advance;
            }

            // Push body frame.
            let block = program.get_block(body_block);
            let parent_frame_idx = stack.frames.len() - 1;
            stack.push_frame(
                Frame::new(body_block, block.entry, block.register_count as usize,
                    Some(term.id), Some(parent_frame_idx))
                .as_loop_body()
            );
            return ControlFlow::FramePushed;
        }

        // Fresh start or body just returned — push condition block.
        let block = program.get_block(cond_block);
        let parent_frame_idx = stack.frames.len() - 1;
        stack.push_frame(Frame::new(
            cond_block, block.entry, block.register_count as usize,
            Some(term.id), Some(parent_frame_idx),
        ));
        if let Some(frame) = stack.frames.get_mut(parent_frame_idx) {
            frame.loop_states.insert(term.id, LoopState::WhileCondition);
        }
        ControlFlow::FramePushed
    }

    fn exec_call(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> ControlFlow {
        let callable = inputs[0];
        let args = &inputs[1..];

        match callable {
            Value::Closure(_) => {
                match Self::build_closure_frame(
                    callable, args, program, closures, Some(term.id),
                ) {
                    Ok(frame) => {
                        // Advance caller past the Call term before pushing
                        if let Some(caller_frame) = stack.frames.last_mut() {
                            caller_frame.current_term = term.block_next;
                        }
                        stack.push_frame(frame);
                        ControlFlow::FramePushed
                    }
                    Err(e) => ControlFlow::Error(e),
                }
            }

            Value::NativeFunction(native_id) => {
                Self::call_native_or_intrinsic(
                    native_id, args, term, program, stack, heap, closures, native_fns, output,
                )
            }

            Value::EnumVariant { .. } if args.is_empty() => {
                // Calling a fieldless variant returns itself
                Self::write_register(stack, term, callable);
                ControlFlow::Advance
            }

            _ => ControlFlow::Error(format!("Cannot call {}", callable.type_name())),
        }
    }

    fn exec_method_call(
        method_cid: ConstantId,
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> ControlFlow {
        let obj = inputs[0];
        let args = &inputs[1..];
        let method_name = match program.constants.get(method_cid) {
            ConstantValue::String(s) => s.clone(),
            _ => return ControlFlow::Error("Invalid method name".into()),
        };

        // 1) If obj is a map, check for a callable field first
        if let Value::Map(map_id) = obj {
            let map = heap.get_map(map_id);
            if let Some(&field_val) = map.get(&method_name) {
                match field_val {
                    Value::Closure(_) => {
                        match Self::build_closure_frame(
                            field_val, args, program, closures, Some(term.id),
                        ) {
                            Ok(frame) => {
                                if let Some(caller_frame) = stack.frames.last_mut() {
                                    caller_frame.current_term = term.block_next;
                                }
                                stack.push_frame(frame);
                                return ControlFlow::FramePushed;
                            }
                            Err(e) => return ControlFlow::Error(e),
                        }
                    }
                    Value::NativeFunction(native_id) => {
                        match Self::call_native_fn(native_id, args, native_fns, heap, output) {
                            Ok(val) => {
                                Self::write_register(stack, term, val);
                                return ControlFlow::Advance;
                            }
                            Err(e) => return ControlFlow::Error(e),
                        }
                    }
                    _ => {} // not callable, fall through to method lookup
                }
            }
        }

        // 2) Look up method as a native function, calling with obj prepended to args
        if let Some(native_id) = native_fns.lookup_name(&method_name) {
            let mut full_args = vec![obj];
            full_args.extend_from_slice(args);
            Self::call_native_or_intrinsic(
                native_id, &full_args, term, program, stack, heap, closures, native_fns, output,
            )
        } else {
            ControlFlow::Error(format!(
                "No method '{}' on type {}",
                method_name,
                obj.type_name()
            ))
        }
    }

    fn exec_match(
        term: &Term,
        inputs: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> ControlFlow {
        let subject = inputs[0];
        let arm_metas = match program.match_arms.get(&term.id) {
            Some(arms) => arms,
            None => return ControlFlow::Error("Match: no arm metadata".into()),
        };

        for arm_meta in arm_metas {
            // Try to match pattern
            let mut bindings = Vec::new();
            if Self::match_pattern(&arm_meta.pattern, subject, heap, &mut bindings) {
                // Check guard if present (with pattern bindings available)
                if let Some(guard_block) = arm_meta.guard_block {
                    // Push guard frame with pattern bindings
                    let gb = program.get_block(guard_block);
                    let gb_reg_count = gb.register_count as usize;
                    let parent_idx = stack.frames.len() - 1;
                    stack.push_frame(Frame::new(
                        guard_block, gb.entry, gb_reg_count,
                        Some(term.id), Some(parent_idx),
                    ));
                    if let Some(frame) = stack.frames.last_mut() {
                        Self::apply_pattern_bindings(program, guard_block, &bindings, frame);
                    }
                    // Run guard to completion
                    let target_depth = parent_idx + 1;
                    let mut guard_result = Value::Bool(false);
                    loop {
                        if stack.frames.len() <= target_depth {
                            if let Some(frame) = stack.frames.last() {
                                let reg = term.register.0 as usize;
                                if reg < frame.registers.len() {
                                    guard_result = frame.registers[reg];
                                }
                            }
                            break;
                        }
                        match Self::step(program, stack, heap, closures, native_fns, output) {
                            StepResult::Continue => {}
                            StepResult::Complete(v) => { guard_result = v; break; }
                            StepResult::Error(e) => return ControlFlow::Error(e),
                        }
                    }
                    if !guard_result.is_truthy() {
                        continue;
                    }
                }

                // Advance parent frame past the Match term
                if let Some(parent_frame) = stack.frames.last_mut() {
                    parent_frame.current_term = term.block_next;
                }

                // Execute body block with bindings
                let body_block_id = arm_meta.body_block;
                let block = program.get_block(body_block_id);
                let reg_count = block.register_count as usize;
                let parent_frame_idx = stack.frames.len() - 1;

                stack.push_frame(Frame::new(
                    body_block_id, block.entry, reg_count,
                    Some(term.id), Some(parent_frame_idx),
                ));

                // Apply pattern bindings to the body frame's registers
                if let Some(frame) = stack.frames.last_mut() {
                    Self::apply_pattern_bindings(program, body_block_id, &bindings, frame);
                }

                return ControlFlow::FramePushed;
            }
        }

        ControlFlow::Error(format!(
            "No matching pattern for value: {}",
            value::value_to_display_string(&subject, heap)
        ))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn constant_to_value(cid: ConstantId, program: &Program, heap: &mut Heap) -> Value {
        match program.constants.get(cid) {
            ConstantValue::Nil => Value::Nil,
            ConstantValue::Bool(b) => Value::Bool(*b),
            ConstantValue::Int(n) => Value::Int(*n),
            ConstantValue::Float(bits) => Value::Float(f64::from_bits(*bits)),
            ConstantValue::String(s) => {
                let sid = heap.alloc_string(s.clone());
                Value::String(sid)
            }
        }
    }

    fn numeric_binop(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
        _heap: &Heap,
        int_op: impl Fn(i64, i64) -> Value,
        float_op: impl Fn(f64, f64) -> Value,
    ) -> ControlFlow {
        if inputs.len() < 2 {
            return ControlFlow::Error("Binary op: missing inputs".into());
        }
        let val = match (&inputs[0], &inputs[1]) {
            (Value::Int(a), Value::Int(b)) => int_op(*a, *b),
            (Value::Float(a), Value::Float(b)) => float_op(*a, *b),
            (Value::Int(a), Value::Float(b)) => float_op(*a as f64, *b),
            (Value::Float(a), Value::Int(b)) => float_op(*a, *b as f64),
            _ => {
                return ControlFlow::Error(format!(
                    "Cannot perform arithmetic on {} and {}",
                    inputs[0].type_name(),
                    inputs[1].type_name()
                ))
            }
        };
        Self::write_register(stack, term, val);
        ControlFlow::Advance
    }

    fn comparison_op(
        term: &Term,
        inputs: &[Value],
        stack: &mut Stack,
        heap: &Heap,
        pred: impl Fn(std::cmp::Ordering) -> bool,
    ) -> ControlFlow {
        match builtins::compare_values(&inputs[0], &inputs[1], heap) {
            Ok(ord) => {
                Self::write_register(stack, term, Value::Bool(pred(ord)));
                ControlFlow::Advance
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    fn push_child_frame(
        program: &Program,
        stack: &mut Stack,
        block_id: BlockId,
        parent_term: &Term,
    ) {
        let block = program.get_block(block_id);
        let reg_count = block.register_count as usize;
        let parent_frame_idx = stack.frames.len() - 1;

        // Advance parent frame past the control flow term
        if let Some(parent_frame) = stack.frames.last_mut() {
            parent_frame.current_term = parent_term.block_next;
        }

        stack.push_frame(Frame::new(
            block_id, block.entry, reg_count,
            Some(parent_term.id), Some(parent_frame_idx),
        ));
    }

    // -----------------------------------------------------------------------
    // Native function dispatch
    // -----------------------------------------------------------------------

    /// Call a native function (non-intrinsic) via PetalState, returning the result value.
    fn call_native_fn(
        native_id: crate::native_fn::NativeFnId,
        args: &[Value],
        native_fns: &NativeFnTable,
        heap: &mut Heap,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        let func = native_fns.get_func(native_id);
        let mut state = PetalState::new(args, heap, output);
        let count = func(&mut state)?;
        let results = state.take_results();
        let val = if count > 0 && !results.is_empty() {
            results[0]
        } else {
            Value::Nil
        };
        Ok(val)
    }

    /// Dispatch a native function call, handling higher-order intrinsics (map, filter, reduce)
    /// specially since they need evaluator context to call closures.
    fn call_native_or_intrinsic(
        native_id: crate::native_fn::NativeFnId,
        args: &[Value],
        term: &Term,
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> ControlFlow {
        let result = if native_fns.intrinsic_map == Some(native_id) {
            Self::builtin_map(args, program, stack, heap, closures, native_fns, output)
        } else if native_fns.intrinsic_filter == Some(native_id) {
            Self::builtin_filter(args, program, stack, heap, closures, native_fns, output)
        } else if native_fns.intrinsic_reduce == Some(native_id) {
            Self::builtin_reduce(args, program, stack, heap, closures, native_fns, output)
        } else {
            Self::call_native_fn(native_id, args, native_fns, heap, output)
        };

        match result {
            Ok(val) => {
                Self::write_register(stack, term, val);
                ControlFlow::Advance
            }
            Err(e) => ControlFlow::Error(e),
        }
    }

    // -----------------------------------------------------------------------
    // Closure call helpers
    // -----------------------------------------------------------------------

    /// Build a Frame for calling a closure with the given arguments.
    /// Handles parameter binding, capture registers, and self-reference.
    fn build_closure_frame(
        callable: Value,
        args: &[Value],
        program: &Program,
        closures: &[RuntimeClosure],
        return_term: Option<TermId>,
    ) -> Result<Frame, String> {
        let closure_id = match callable {
            Value::Closure(id) => id,
            _ => return Err(format!("Expected a function, got {}", callable.type_name())),
        };

        let closure = &closures[closure_id.0 as usize];
        let func = &program.functions[closure.function_id.0 as usize];
        let body_block = func.body_block;
        let block = program.get_block(body_block);

        if args.len() != func.params.len() {
            return Err(format!(
                "Expected {} arguments, got {}",
                func.params.len(),
                args.len()
            ));
        }

        let reg_count = block.register_count as usize;
        let mut registers = vec![Value::Nil; reg_count];

        // Set parameter registers
        for (i, arg) in args.iter().enumerate() {
            if i < registers.len() {
                registers[i] = *arg;
            }
        }

        // Set capture registers
        for (i, cap) in closure.captures.iter().enumerate() {
            if i < func.capture_registers.len() {
                let reg_idx = func.capture_registers[i].0 as usize;
                if reg_idx < registers.len() {
                    registers[reg_idx] = *cap;
                }
            }
        }

        // Self-reference for recursion
        if let Some(self_reg) = func.self_ref_register {
            let reg_idx = self_reg.0 as usize;
            if reg_idx < registers.len() {
                registers[reg_idx] = callable;
            }
        }

        let mut frame = Frame::new(
            body_block, block.entry, 0, return_term, None,
        );
        frame.registers = registers;
        Ok(frame)
    }

    // -----------------------------------------------------------------------
    // Higher-order builtin helpers
    // -----------------------------------------------------------------------

    /// Call a closure synchronously with the given arguments, returning the result.
    fn call_closure_sync(
        callable: Value,
        call_args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        let frame = Self::build_closure_frame(callable, call_args, program, closures, None)?;
        let target_depth = stack.frames.len();
        stack.push_frame(frame);

        stack.last_pop_result = None;

        loop {
            if stack.frames.len() <= target_depth {
                // Frame was popped — retrieve the result
                return Ok(stack.last_pop_result.take().unwrap_or(Value::Nil));
            }

            let step = Self::step(program, stack, heap, closures, native_fns, output);
            match step {
                StepResult::Continue => {}
                StepResult::Complete(v) => return Ok(v),
                StepResult::Error(e) => return Err(e),
            }
        }
    }

    fn builtin_map(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("map() expects 2 arguments (list, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("map() expects a list as first argument".into()),
        };
        let func = args[1];
        let elements = heap.get_list(list_id).to_vec();

        let mut results = Vec::with_capacity(elements.len());
        for elem in elements {
            let result = Self::call_closure_sync(
                func, &[elem], program, stack, heap, closures, native_fns, output,
            )?;
            results.push(result);
        }

        let result_id = heap.alloc_list(results);
        Ok(Value::List(result_id))
    }

    fn builtin_filter(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        if args.len() != 2 {
            return Err("filter() expects 2 arguments (list, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("filter() expects a list as first argument".into()),
        };
        let func = args[1];
        let elements = heap.get_list(list_id).to_vec();

        let mut results = Vec::new();
        for elem in elements {
            let keep = Self::call_closure_sync(
                func, &[elem], program, stack, heap, closures, native_fns, output,
            )?;
            if keep.is_truthy() {
                results.push(elem);
            }
        }

        let result_id = heap.alloc_list(results);
        Ok(Value::List(result_id))
    }

    fn builtin_reduce(
        args: &[Value],
        program: &Program,
        stack: &mut Stack,
        heap: &mut Heap,
        closures: &mut Vec<RuntimeClosure>,
        native_fns: &NativeFnTable,
        output: &mut Vec<String>,
    ) -> Result<Value, String> {
        if args.len() != 3 {
            return Err("reduce() expects 3 arguments (list, initial, function)".into());
        }
        let list_id = match args[0] {
            Value::List(id) => id,
            _ => return Err("reduce() expects a list as first argument".into()),
        };
        let func = args[2];
        let elements = heap.get_list(list_id).to_vec();
        let mut acc = args[1];

        for elem in elements {
            acc = Self::call_closure_sync(
                func, &[acc, elem], program, stack, heap, closures, native_fns, output,
            )?;
        }

        Ok(acc)
    }

    // -----------------------------------------------------------------------
    // Pattern matching (runtime)
    // -----------------------------------------------------------------------

    fn match_pattern(
        pattern: &Pattern,
        value: Value,
        heap: &mut Heap,
        bindings: &mut Vec<(String, Value)>,
    ) -> bool {
        match pattern {
            Pattern::Wildcard => true,

            Pattern::Literal(lit) => {
                match (lit, value) {
                    (Literal::Nil, Value::Nil) => true,
                    (Literal::Bool(a), Value::Bool(b)) => *a == b,
                    (Literal::Int(a), Value::Int(b)) => *a == b,
                    (Literal::Float(a), Value::Float(b)) => *a == b,
                    (Literal::String(a), Value::String(sid)) => a == heap.get_string(sid),
                    _ => false,
                }
            }

            Pattern::Variable(name) => {
                // Pure variable binding — always matches and captures the value.
                // (Known enum variant names are resolved to Pattern::Variant by the compiler.)
                bindings.push((name.clone(), value));
                true
            }

            Pattern::Variant { name, fields } => {
                if let Value::EnumVariant { tag, data } = value {
                    let variant_name = heap.get_string(tag);
                    if variant_name != name {
                        return false;
                    }
                    let data_fields = heap.get_list(data);
                    if data_fields.len() != fields.len() {
                        return false;
                    }
                    let data_copy: Vec<Value> = data_fields.to_vec();
                    for (pat, val) in fields.iter().zip(data_copy.iter()) {
                        if !Self::match_pattern(pat, *val, heap, bindings) {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }

            Pattern::List { elements, rest } => {
                if let Value::List(list_id) = value {
                    let list = heap.get_list(list_id);
                    if let Some(rest_name) = rest {
                        if list.len() < elements.len() {
                            return false;
                        }
                        let list_copy: Vec<Value> = list.to_vec();
                        for (pat, val) in elements.iter().zip(list_copy.iter()) {
                            if !Self::match_pattern(pat, *val, heap, bindings) {
                                return false;
                            }
                        }
                        let rest_vals: Vec<Value> = list_copy[elements.len()..].to_vec();
                        let rest_list = Value::List(heap.alloc_list(rest_vals));
                        bindings.push((rest_name.clone(), rest_list));
                        true
                    } else {
                        if list.len() != elements.len() {
                            return false;
                        }
                        let list_copy: Vec<Value> = list.to_vec();
                        for (pat, val) in elements.iter().zip(list_copy.iter()) {
                            if !Self::match_pattern(pat, *val, heap, bindings) {
                                return false;
                            }
                        }
                        true
                    }
                } else {
                    false
                }
            }

            Pattern::Record(fields) => {
                if let Value::Map(map_id) = value {
                    // Copy relevant entries out before recursive matching
                    let entries: Vec<(String, Value)> = {
                        let map = heap.get_map(map_id);
                        fields
                            .iter()
                            .filter_map(|(key, _)| {
                                map.get(key).map(|&val| (key.clone(), val))
                            })
                            .collect()
                    };
                    if entries.len() != fields.len() {
                        return false; // Some fields missing
                    }
                    for ((_, pat), (_, val)) in fields.iter().zip(entries.iter()) {
                        if !Self::match_pattern(pat, *val, heap, bindings) {
                            return false;
                        }
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Apply pattern bindings to a frame's registers by matching names to
    /// terms in the block (including phantom terms not in the linked list).
    /// Uses the precomputed block_terms index for O(B) lookup instead of O(N)
    /// where B is the number of terms in the block and N is total program terms.
    fn apply_pattern_bindings(
        program: &Program,
        block_id: BlockId,
        bindings: &[(String, Value)],
        frame: &mut Frame,
    ) {
        if let Some(term_ids) = program.block_terms.get(&block_id) {
            for tid in term_ids {
                let term = program.get_term(*tid);
                if let Some(ref term_name) = term.name {
                    for (bind_name, bind_val) in bindings {
                        if term_name == bind_name {
                            let reg = term.register.0 as usize;
                            if reg < frame.registers.len() {
                                frame.registers[reg] = *bind_val;
                            }
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Match pattern needs mutable heap for rest-pattern list allocation
    // -----------------------------------------------------------------------
}

/// Runtime closure — captures + function reference.
pub struct RuntimeClosure {
    pub function_id: FunctionId,
    pub captures: Vec<Value>,
}

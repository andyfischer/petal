//! Evaluator for Petal programs
//!
//! Executes terms and manages the runtime state

use std::collections::HashMap;
use slotmap::new_key_type;

use crate::{
    BuiltinFn, Error, Program, StepResult, Value,
};
use crate::parser::{
    ConstantValue, Term, TermId, TermOp,
};
use crate::heap::{Heap, StringId};

new_key_type! {
    pub struct StackKey;
}

/// Runtime stack frame
#[derive(Clone, Debug)]
pub struct Frame {
    /// Current term being executed
    pub current_term: TermId,
    /// Local variables
    pub locals: HashMap<String, Value>,
    /// Return address
    pub return_term: Option<TermId>,
    /// Whether to return from this frame
    pub should_return: bool,
    /// Return value
    pub return_value: Option<Value>,
    /// Loop control
    pub in_loop: bool,
    pub should_break: bool,
    pub should_continue: bool,
}

impl Frame {
    pub fn new(entry_term: TermId) -> Self {
        Self {
            current_term: entry_term,
            locals: HashMap::new(),
            return_term: None,
            should_return: false,
            return_value: None,
            in_loop: false,
            should_break: false,
            should_continue: false,
        }
    }
}

/// Runtime execution stack
pub struct Stack {
    program_id: crate::ProgramKey,
    pub frames: Vec<Frame>,
    /// Persistent state storage (for `state` keyword)
    pub state_storage: HashMap<String, Value>,
    /// State initialization flags
    pub state_initialized: HashMap<String, bool>,
    /// Current control flow term
    pub current_term: TermId,
    /// Call stack depth (for overflow protection)
    pub call_depth: usize,
}

impl Stack {
    pub fn new(program_id: crate::ProgramKey, entry_term: TermId) -> Self {
        let initial_frame = Frame::new(entry_term);
        Self {
            program_id,
            frames: vec![initial_frame],
            state_storage: HashMap::new(),
            state_initialized: HashMap::new(),
            current_term: entry_term,
            call_depth: 0,
        }
    }

    pub fn program_id(&self) -> crate::ProgramKey {
        self.program_id
    }

    pub fn current_frame(&self) -> &Frame {
        self.frames.last().expect("Stack should always have at least one frame")
    }

    pub fn current_frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("Stack should always have at least one frame")
    }

    pub fn set_local(&mut self, name: String, value: Value) {
        self.current_frame_mut().locals.insert(name, value);
    }

    pub fn get_local(&self, name: &str) -> Option<&Value> {
        // Search from top of frame stack down
        for frame in self.frames.iter().rev() {
            if let Some(value) = frame.locals.get(name) {
                return Some(value);
            }
        }
        None
    }

    pub fn get_state(&self, name: &str) -> Option<&Value> {
        self.state_storage.get(name)
    }

    pub fn set_state(&mut self, name: String, value: Value) {
        self.state_storage.insert(name, value);
    }

    pub fn is_state_initialized(&self, name: &str) -> bool {
        self.state_initialized.get(name).copied().unwrap_or(false)
    }

    pub fn mark_state_initialized(&mut self, name: String) {
        self.state_initialized.insert(name, true);
    }
}

/// Execute a single step
pub fn step(
    stack: &mut Stack,
    program: &Program,
    heap: &mut Heap,
    builtins: &HashMap<String, BuiltinFn>,
) -> Result<StepResult, Error> {
    // Check call depth
    if stack.call_depth > 1000 {
        return Err(Error::StackOverflow);
    }

    let current_term_id = stack.current_frame().current_term;
    let term = program.get_term(current_term_id)
        .ok_or(Error::RuntimeError(format!("Invalid term id: {:?}", current_term_id)))?;

    // Evaluate the term
    let result = evaluate_term(stack, program, heap, builtins, term)?;

    // Determine next term
    let next_term = if let Some(next) = term.control_flow_next {
        next
    } else {
        // End of program
        return Ok(StepResult::Complete(result));
    };

    stack.current_frame_mut().current_term = next_term;
    stack.current_term = next_term;

    Ok(StepResult::Continue)
}

/// Evaluate a single term
fn evaluate_term(
    stack: &mut Stack,
    program: &Program,
    heap: &mut Heap,
    builtins: &HashMap<String, BuiltinFn>,
    term: &Term,
) -> Result<Value, Error> {
    match &term.op {
        TermOp::Constant(const_id) => {
            let const_value = program.get_constant(*const_id)
                .ok_or(Error::RuntimeError("Invalid constant".to_string()))?;
            Ok(constant_to_value(const_value, heap))
        }

        TermOp::Error(msg) => {
            Err(Error::RuntimeError(msg.clone()))
        }

        TermOp::Let { name } => {
            let value = if let Some(input_term) = term.inputs.first() {
                let input_term = program.get_term(*input_term)
                    .ok_or(Error::RuntimeError("Invalid input term".to_string()))?;
                evaluate_term(stack, program, heap, builtins, input_term)?
            } else {
                Value::Nil
            };
            stack.set_local(name.clone(), value);
            Ok(Value::Nil)
        }

        TermOp::StateInit { name, initial_value } => {
            if !stack.is_state_initialized(name) {
                let init_term = program.get_term(*initial_value)
                    .ok_or(Error::RuntimeError("Invalid state init term".to_string()))?;
                let value = evaluate_term(stack, program, heap, builtins, init_term)?;
                stack.set_state(name.clone(), value);
                stack.mark_state_initialized(name.clone());
            }
            Ok(Value::Nil)
        }

        TermOp::StateRead { name } => {
            if let Some(value) = stack.get_state(name) {
                Ok(value.clone())
            } else {
                Ok(Value::Nil)
            }
        }

        TermOp::GetVariable(name) => {
            // First check local variables, then state storage
            if let Some(value) = stack.get_local(name) {
                Ok(value.clone())
            } else if let Some(value) = stack.get_state(name) {
                Ok(value.clone())
            } else {
                Ok(Value::Nil)
            }
        }

        TermOp::Add => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.add(&right, heap)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Sub => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.sub(&right)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Mul => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.mul(&right)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Div => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.div(&right)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Mod => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.modulo(&right)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Pow => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            left.pow(&right)
                .map_err(|e| Error::RuntimeError(e))
        }

        TermOp::Eq => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left == right))
        }

        TermOp::Ne => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left != right))
        }

        TermOp::Lt => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left.compare(&right).map(|o| o == std::cmp::Ordering::Less).unwrap_or(false)))
        }

        TermOp::Le => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left.compare(&right).map(|o| o != std::cmp::Ordering::Greater).unwrap_or(false)))
        }

        TermOp::Gt => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left.compare(&right).map(|o| o == std::cmp::Ordering::Greater).unwrap_or(false)))
        }

        TermOp::Ge => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(Value::Bool(left.compare(&right).map(|o| o != std::cmp::Ordering::Less).unwrap_or(false)))
        }

        TermOp::And => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(left.and(&right))
        }

        TermOp::Or => {
            let left = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let right = evaluate_input(stack, program, heap, builtins, term, 1)?;
            Ok(left.or(&right))
        }

        TermOp::Not => {
            let operand = evaluate_input(stack, program, heap, builtins, term, 0)?;
            Ok(operand.not())
        }

        TermOp::Branch { then_term, else_term } => {
            let condition = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let target = if condition.is_truthy() { *then_term } else { *else_term };
            stack.current_frame_mut().current_term = target;

            // Execute the branch
            let target_term = program.get_term(target)
                .ok_or(Error::RuntimeError("Invalid branch target".to_string()))?;
            evaluate_term(stack, program, heap, builtins, target_term)
        }

        TermOp::Return => {
            let value = if let Some(input_term) = term.inputs.first() {
                let input_term = program.get_term(*input_term)
                    .ok_or(Error::RuntimeError("Invalid return term".to_string()))?;
                evaluate_term(stack, program, heap, builtins, input_term)?
            } else {
                Value::Nil
            };

            // Pop frame if we have one to return to
            if stack.frames.len() > 1 {
                stack.frames.pop();
                stack.call_depth -= 1;
            }

            Ok(value)
        }

        TermOp::CallBuiltin { name } => {
            let mut args = Vec::new();
            for input_id in &term.inputs[1..] {
                let input_term = program.get_term(*input_id)
                    .ok_or(Error::RuntimeError("Invalid call argument".to_string()))?;
                args.push(evaluate_term(stack, program, heap, builtins, input_term)?);
            }

            if let Some(builtin) = builtins.get(name) {
                Ok(builtin(heap, args))
            } else {
                Err(Error::RuntimeError(format!("Unknown builtin: {}", name)))
            }
        }

        TermOp::Call { function: _ } => {
            // For now, handle as builtin if it's a variable reference
            let mut args = Vec::new();
            for input_id in &term.inputs[1..] {
                let input_term = program.get_term(*input_id)
                    .ok_or(Error::RuntimeError("Invalid call argument".to_string()))?;
                args.push(evaluate_term(stack, program, heap, builtins, input_term)?);
            }

            // Try to get function name from first input
            if let Some(first_input) = term.inputs.first() {
                let first_term = program.get_term(*first_input)
                    .ok_or(Error::RuntimeError("Invalid function term".to_string()))?;
                if let TermOp::GetVariable(name) = &first_term.op {
                    if let Some(builtin) = builtins.get(name) {
                        return Ok(builtin(heap, args));
                    }
                }
            }

            Ok(Value::Nil)
        }

        TermOp::ForLoop { var, iter_expr: _, body_start, end_term } => {
            let iter_value = evaluate_input(stack, program, heap, builtins, term, 0)?;

            // Get the list to iterate over
            let list_id = match iter_value {
                Value::List(id) => id,
                _ => return Ok(Value::Nil),
            };

            let list = heap.get_list(list_id).cloned().unwrap_or_default();

            for item in list {
                stack.set_local(var.clone(), item);
                stack.current_frame_mut().current_term = *body_start;

                // Execute body
                let mut current = *body_start;
                loop {
                    let body_term = program.get_term(current)
                        .ok_or(Error::RuntimeError("Invalid body term".to_string()))?;

                    if let TermOp::Break = body_term.op {
                        break;
                    }
                    if let TermOp::Continue = body_term.op {
                        break;
                    }

                    evaluate_term(stack, program, heap, builtins, body_term)?;

                    if let Some(next) = body_term.control_flow_next {
                        current = next;
                    } else {
                        break;
                    }
                }
            }

            stack.current_frame_mut().current_term = *end_term;
            Ok(Value::Nil)
        }

        TermOp::WhileLoop { condition: _, body_start, end_term } => {
            loop {
                // Re-evaluate condition
                let cond_value = evaluate_input(stack, program, heap, builtins, term, 0)?;
                if !cond_value.is_truthy() {
                    break;
                }

                stack.current_frame_mut().current_term = *body_start;

                // Execute body
                let mut current = *body_start;
                let mut should_break = false;

                loop {
                    let body_term = program.get_term(current)
                        .ok_or(Error::RuntimeError("Invalid body term".to_string()))?;

                    if let TermOp::Break = body_term.op {
                        should_break = true;
                        break;
                    }
                    if let TermOp::Continue = body_term.op {
                        break;
                    }

                    evaluate_term(stack, program, heap, builtins, body_term)?;

                    if let Some(next) = body_term.control_flow_next {
                        current = next;
                    } else {
                        break;
                    }
                }

                if should_break {
                    break;
                }
            }

            stack.current_frame_mut().current_term = *end_term;
            Ok(Value::Nil)
        }

        TermOp::Break => Ok(Value::Nil),
        TermOp::Continue => Ok(Value::Nil),

        TermOp::CreateList => {
            let list_id = heap.alloc_list();
            for input_id in &term.inputs {
                let input_term = program.get_term(*input_id)
                    .ok_or(Error::RuntimeError("Invalid list element".to_string()))?;
                let value = evaluate_term(stack, program, heap, builtins, input_term)?;
                heap.push_to_list(list_id, value);
            }
            Ok(Value::List(list_id))
        }

        TermOp::CreateMap => {
            let map_id = heap.alloc_map();
            // Process key-value pairs
            let mut i = 0;
            while i + 1 < term.inputs.len() {
                let key_term = program.get_term(term.inputs[i])
                    .ok_or(Error::RuntimeError("Invalid map key".to_string()))?;
                let val_term = program.get_term(term.inputs[i + 1])
                    .ok_or(Error::RuntimeError("Invalid map value".to_string()))?;

                let key = evaluate_term(stack, program, heap, builtins, key_term)?;
                let value = evaluate_term(stack, program, heap, builtins, val_term)?;

                // Extract string key if needed
                let key = match key {
                    Value::String(s) => {
                        if let Some(s) = heap.get_string(s).map(|s| s.to_string()) {
                            Value::String(heap.alloc_string(&s))
                        } else {
                            key
                        }
                    }
                    _ => key,
                };

                heap.set_in_map(map_id, key, value);
                i += 2;
            }
            Ok(Value::Map(map_id))
        }

        TermOp::GetIndex => {
            let container = evaluate_input(stack, program, heap, builtins, term, 0)?;
            let index = evaluate_input(stack, program, heap, builtins, term, 1)?;

            match (container, index) {
                (Value::List(list_id), Value::Int(n)) => {
                    if let Some(list) = heap.get_list(list_id) {
                        if n >= 0 && n < list.len() as i64 {
                            Ok(list[n as usize].clone())
                        } else {
                            Ok(Value::Nil)
                        }
                    } else {
                        Ok(Value::Nil)
                    }
                }
                (Value::String(s), Value::Int(n)) => {
                    if let Some(s) = heap.get_string(s) {
                        if n >= 0 && n < s.len() as i64 {
                            let ch = s.chars().nth(n as usize).unwrap_or(' ');
                            Ok(Value::String(heap.alloc_string(&ch.to_string())))
                        } else {
                            Ok(Value::Nil)
                        }
                    } else {
                        Ok(Value::Nil)
                    }
                }
                _ => Ok(Value::Nil),
            }
        }

        TermOp::GetField { field } => {
            let container = evaluate_input(stack, program, heap, builtins, term, 0)?;

            match container {
                Value::Map(map_id) => {
                    if let Some(map) = heap.get_map(map_id) {
                        // Look for a key with matching string content
                        for (key, value) in map.iter() {
                            if let Value::String(key_id) = key {
                                if let Some(key_str) = heap.get_string(*key_id) {
                                    if key_str == field {
                                        return Ok(value.clone());
                                    }
                                }
                            }
                        }
                        Ok(Value::Nil)
                    } else {
                        Ok(Value::Nil)
                    }
                }
                _ => Ok(Value::Nil),
            }
        }

        TermOp::SetField { field: _ } => {
            Ok(Value::Nil)
        }

        TermOp::SetIndex => {
            Ok(Value::Nil)
        }

        TermOp::Assign { name } => {
            if let Some(input_term) = term.inputs.first() {
                let input_term = program.get_term(*input_term)
                    .ok_or(Error::RuntimeError("Invalid assignment value".to_string()))?;
                let value = evaluate_term(stack, program, heap, builtins, input_term)?;
                // If this variable exists in state, update state; otherwise update locals
                if stack.get_state(name).is_some() {
                    stack.set_state(name.clone(), value.clone());
                } else {
                    stack.set_local(name.clone(), value.clone());
                }
                Ok(value)
            } else {
                Ok(Value::Nil)
            }
        }

        _ => Ok(Value::Nil),
    }
}

/// Evaluate a specific input of a term
fn evaluate_input(
    stack: &mut Stack,
    program: &Program,
    heap: &mut Heap,
    builtins: &HashMap<String, BuiltinFn>,
    term: &Term,
    index: usize,
) -> Result<Value, Error> {
    let input_id = term.inputs.get(index)
        .ok_or(Error::RuntimeError(format!("Missing input {}", index)))?;
    let input_term = program.get_term(*input_id)
        .ok_or(Error::RuntimeError("Invalid input term".to_string()))?;
    evaluate_term(stack, program, heap, builtins, input_term)
}

/// Convert a constant to a runtime value
fn constant_to_value(constant: &ConstantValue, heap: &mut Heap) -> Value {
    match constant {
        ConstantValue::Nil => Value::Nil,
        ConstantValue::Bool(b) => Value::Bool(*b),
        ConstantValue::Int(n) => Value::Int(*n),
        ConstantValue::Float(f) => Value::Float(*f),
        ConstantValue::String(s) => {
            Value::String(heap.alloc_string(s))
        }
    }
}

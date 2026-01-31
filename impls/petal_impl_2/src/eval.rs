use crate::env::Env;
use crate::error::Error;
use crate::stack::{Frame, StackKey};
use crate::term::{StateKey, TermId, TermOp};
use crate::value::Value;
use crate::{Result, StepResult};
use std::collections::HashMap;

pub struct Evaluator<'a> {
    env: &'a mut Env,
}

impl<'a> Evaluator<'a> {
    pub fn new(env: &'a mut Env) -> Self {
        Self { env }
    }

    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult> {
        let stack = self
            .env
            .get_stack_mut(stack_id)
            .ok_or(Error::InvalidStackKey)?;

        if stack.completed {
            return Ok(StepResult::Complete);
        }

        let current_frame = stack.current_frame().ok_or(Error::StackUnderflow)?;
        let current_term = current_frame.current_term;
        let program_id = stack.program_id;

        let program = self
            .env
            .get_program(program_id)
            .ok_or(Error::InvalidProgramKey)?
            .clone();

        let term = program
            .get_term(current_term)
            .ok_or(Error::RuntimeError("Invalid term ID".to_string()))?
            .clone();

        let result = self.eval_term(&term, stack_id)?;

        // Move to next term in control flow or complete
        let stack = self.env.get_stack_mut(stack_id).unwrap();

        if let Some(next) = term.control_flow_next {
            if let Some(frame) = stack.current_frame_mut() {
                frame.current_term = next;
            }
        } else {
            // No explicit next term - execution completes
            stack.completed = true;
            stack.result = result;
            return Ok(StepResult::Complete);
        }

        Ok(StepResult::Continue)
    }

    fn eval_term(&mut self, term: &crate::term::Term, stack_id: StackKey) -> Result<Value> {
        match &term.op {
            TermOp::Constant(v) => Ok(v.clone()),

            TermOp::Error(msg) => Err(Error::ParseError(msg.clone())),

            TermOp::LoadVar(name) => self.load_var(name, stack_id),

            TermOp::StoreVar(name) => {
                let value = self.eval_input(term, 0, stack_id)?;
                let stack = self.env.get_stack_mut(stack_id).unwrap();

                // Check if this is a state variable
                if let Some(state_key) = stack.get_state_key_for_variable(name) {
                    // This is a state variable - update the state storage
                    stack.set_state(state_key, value.clone());
                    // Also update the variable for consistency
                    if stack.frames.len() == 1 {
                        stack.set_global(name.clone(), value.clone());
                    } else {
                        stack.set_variable(name.clone(), value.clone());
                    }
                } else {
                    // Regular variable - store in globals if at top level, otherwise in locals
                    if stack.frames.len() == 1 {
                        stack.set_global(name.clone(), value.clone());
                    } else {
                        stack.set_variable(name.clone(), value.clone());
                    }
                }
                Ok(value)
            }

            TermOp::Add => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.add_values(left, right)
            }

            TermOp::Sub => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.sub_values(left, right)
            }

            TermOp::Mul => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.mul_values(left, right)
            }

            TermOp::Div => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.div_values(left, right)
            }

            TermOp::Mod => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.mod_values(left, right)
            }

            TermOp::Neg => {
                let value = self.eval_input(term, 0, stack_id)?;
                match value {
                    Value::Int(i) => Ok(Value::Int(-i)),
                    Value::Float(f) => Ok(Value::Float(-f)),
                    _ => Err(Error::TypeError("Cannot negate non-number".to_string())),
                }
            }

            TermOp::Eq => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                Ok(Value::Bool(self.values_equal(&left, &right)))
            }

            TermOp::NotEq => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                Ok(Value::Bool(!self.values_equal(&left, &right)))
            }

            TermOp::Lt => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.compare_lt(left, right)
            }

            TermOp::Gt => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                self.compare_gt(left, right)
            }

            TermOp::LtEq => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                let lt = self.compare_lt(left.clone(), right.clone())?;
                let eq = Value::Bool(self.values_equal(&left, &right));
                match (lt, eq) {
                    (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                    _ => unreachable!(),
                }
            }

            TermOp::GtEq => {
                let left = self.eval_input(term, 0, stack_id)?;
                let right = self.eval_input(term, 1, stack_id)?;
                let gt = self.compare_gt(left.clone(), right.clone())?;
                let eq = Value::Bool(self.values_equal(&left, &right));
                match (gt, eq) {
                    (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                    _ => unreachable!(),
                }
            }

            TermOp::And => {
                let left = self.eval_input(term, 0, stack_id)?;
                if !left.is_truthy() {
                    return Ok(Value::Bool(false));
                }
                let right = self.eval_input(term, 1, stack_id)?;
                Ok(Value::Bool(right.is_truthy()))
            }

            TermOp::Or => {
                let left = self.eval_input(term, 0, stack_id)?;
                if left.is_truthy() {
                    return Ok(Value::Bool(true));
                }
                let right = self.eval_input(term, 1, stack_id)?;
                Ok(Value::Bool(right.is_truthy()))
            }

            TermOp::Not => {
                let value = self.eval_input(term, 0, stack_id)?;
                Ok(Value::Bool(!value.is_truthy()))
            }

            TermOp::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let cond_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*condition)
                    .ok_or(Error::RuntimeError("Invalid condition term".to_string()))?
                    .clone();

                let cond_value = self.eval_term(&cond_term, stack_id)?;

                let block = if cond_value.is_truthy() {
                    then_block
                } else {
                    else_block
                };

                let mut result = Value::Nil;
                for term_id in block {
                    let t = self
                        .env
                        .get_program(
                            self.env
                                .get_stack(stack_id)
                                .ok_or(Error::InvalidStackKey)?
                                .program_id,
                        )
                        .ok_or(Error::InvalidProgramKey)?
                        .get_term(*term_id)
                        .ok_or(Error::RuntimeError("Invalid term in block".to_string()))?
                        .clone();
                    result = self.eval_term(&t, stack_id)?;
                }

                Ok(result)
            }

            TermOp::ForLoop {
                var_name,
                iterable,
                body,
            } => {
                // Evaluate the iterable
                let iterable_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*iterable)
                    .ok_or(Error::RuntimeError("Invalid iterable term".to_string()))?
                    .clone();

                let iterable_value = self.eval_term(&iterable_term, stack_id)?;

                match iterable_value {
                    Value::List(items) => {
                        let mut result = Value::Nil;
                        let mut should_break = false;

                        for item in items {
                            if should_break {
                                break;
                            }

                            // Set loop variable in current frame's locals
                            let stack = self.env.get_stack_mut(stack_id).unwrap();
                            if let Some(frame) = stack.current_frame_mut() {
                                frame.locals.insert(var_name.clone(), item);
                            }

                            // Execute body
                            let program = self
                                .env
                                .get_program(
                                    self.env
                                        .get_stack(stack_id)
                                        .ok_or(Error::InvalidStackKey)?
                                        .program_id,
                                )
                                .ok_or(Error::InvalidProgramKey)?
                                .clone();

                            for term_id in body {
                                let t = program
                                    .get_term(*term_id)
                                    .ok_or(Error::RuntimeError(
                                        "Invalid term in loop".to_string(),
                                    ))?
                                    .clone();

                                match self.eval_term(&t, stack_id) {
                                    Ok(v) => result = v,
                                    Err(Error::LoopBreak) => {
                                        should_break = true;
                                        result = Value::Nil;  // Reset result on break
                                        break;
                                    }
                                    Err(Error::LoopContinue) => {
                                        result = Value::Nil;  // Reset result on continue
                                        break;
                                    }
                                    Err(e) => return Err(e),
                                }
                            }
                        }
                        Ok(result)
                    }
                    _ => Err(Error::TypeError(
                        "For loop requires iterable value".to_string(),
                    )),
                }
            }

            TermOp::WhileLoop { condition, body } => {
                let mut result = Value::Nil;

                loop {
                    // Evaluate condition
                    let program = self
                        .env
                        .get_program(
                            self.env
                                .get_stack(stack_id)
                                .ok_or(Error::InvalidStackKey)?
                                .program_id,
                        )
                        .ok_or(Error::InvalidProgramKey)?
                        .clone();

                    let cond_term = program
                        .get_term(*condition)
                        .ok_or(Error::RuntimeError("Invalid condition term".to_string()))?
                        .clone();

                    let cond_value = self.eval_term(&cond_term, stack_id)?;

                    if !cond_value.is_truthy() {
                        break;
                    }

                    // Execute body
                    let mut should_break = false;
                    for term_id in body {
                        let t = program
                            .get_term(*term_id)
                            .ok_or(Error::RuntimeError("Invalid term in loop".to_string()))?
                            .clone();

                        match self.eval_term(&t, stack_id) {
                            Ok(v) => result = v,
                            Err(Error::LoopBreak) => {
                                should_break = true;
                                result = Value::Nil;  // Reset result on break
                                break;
                            }
                            Err(Error::LoopContinue) => {
                                result = Value::Nil;  // Reset result on continue
                                break;
                            }
                            Err(e) => return Err(e),
                        }
                    }

                    if should_break {
                        break;
                    }
                }

                Ok(result)
            }

            TermOp::Return => {
                let value = self.eval_input(term, 0, stack_id)?;
                let stack = self.env.get_stack_mut(stack_id).unwrap();
                stack.result = value.clone();
                stack.completed = true;
                Ok(value)
            }

            TermOp::Break => Err(Error::LoopBreak),

            TermOp::Continue => Err(Error::LoopContinue),

            TermOp::StateRead(key) => {
                let stack = self.env.get_stack(stack_id).ok_or(Error::InvalidStackKey)?;
                Ok(stack.get_state(*key).cloned().unwrap_or(Value::Nil))
            }

            TermOp::StateWrite(key) => {
                let value = self.eval_input(term, 0, stack_id)?;
                let stack = self.env.get_stack_mut(stack_id).unwrap();
                stack.set_state(*key, value.clone());
                Ok(value)
            }

            TermOp::StateInit(key) => {
                // Only initialize if state doesn't already exist
                let stack = self.env.get_stack(stack_id).ok_or(Error::InvalidStackKey)?;
                if stack.get_state(*key).is_none() {
                    // State doesn't exist, evaluate and set it
                    let value = self.eval_input(term, 0, stack_id)?;
                    let stack = self.env.get_stack_mut(stack_id).unwrap();
                    stack.set_state(*key, value.clone());
                    Ok(value)
                } else {
                    // State already exists, just return the existing value
                    Ok(stack.get_state(*key).cloned().unwrap())
                }
            }

            TermOp::StateDeclare {
                state_key,
                var_name,
            } => {
                // Initialize state if it doesn't exist
                let stack = self.env.get_stack(stack_id).ok_or(Error::InvalidStackKey)?;
                let value = if stack.get_state(*state_key).is_none() {
                    // State doesn't exist, evaluate init value and set it
                    let init_value = self.eval_input(term, 0, stack_id)?;
                    let stack = self.env.get_stack_mut(stack_id).unwrap();
                    stack.set_state(*state_key, init_value.clone());
                    init_value
                } else {
                    // State already exists, use existing value
                    stack.get_state(*state_key).cloned().unwrap()
                };

                // Store the value in the variable
                let stack = self.env.get_stack_mut(stack_id).unwrap();
                if stack.frames.len() == 1 {
                    stack.set_global(var_name.clone(), value.clone());
                } else {
                    stack.set_variable(var_name.clone(), value.clone());
                }

                // Register this variable as state-backed
                stack.register_state_variable(var_name.clone(), *state_key);

                Ok(value)
            }

            TermOp::Call { function, args } => {
                let func_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*function)
                    .ok_or(Error::RuntimeError("Invalid function term".to_string()))?
                    .clone();

                let func_value = self.eval_term(&func_term, stack_id)?;

                let arg_values: Result<Vec<Value>> = args
                    .iter()
                    .map(|arg_id| {
                        let t = self
                            .env
                            .get_program(
                                self.env
                                    .get_stack(stack_id)
                                    .ok_or(Error::InvalidStackKey)?
                                    .program_id,
                            )
                            .ok_or(Error::InvalidProgramKey)?
                            .get_term(*arg_id)
                            .ok_or(Error::RuntimeError("Invalid argument term".to_string()))?
                            .clone();
                        self.eval_term(&t, stack_id)
                    })
                    .collect();

                let arg_values = arg_values?;

                match func_value {
                    Value::BuiltinFunction(name) => self.env.call_builtin(&name, &arg_values),
                    Value::Function(idx) => {
                        let program = self
                            .env
                            .get_program(
                                self.env
                                    .get_stack(stack_id)
                                    .ok_or(Error::InvalidStackKey)?
                                    .program_id,
                            )
                            .ok_or(Error::InvalidProgramKey)?
                            .clone();

                        let func_def = program
                            .get_function(idx)
                            .ok_or(Error::RuntimeError("Invalid function index".to_string()))?
                            .clone();

                        // Create new frame for function call
                        let mut locals = HashMap::new();
                        for (param, arg) in func_def.params.iter().zip(arg_values.iter()) {
                            locals.insert(param.clone(), arg.clone());
                        }

                        let frame = Frame {
                            current_term: func_def.entry,
                            locals,
                            return_term: None,
                            loop_context: None,
                        };

                        let stack = self.env.get_stack_mut(stack_id).unwrap();
                        stack.push_frame(frame);

                        // Execute function body
                        let mut result = Value::Nil;
                        for term_id in &func_def.body {
                            let t = program
                                .get_term(*term_id)
                                .ok_or(Error::RuntimeError("Invalid term in function".to_string()))?
                                .clone();
                            result = self.eval_term(&t, stack_id)?;

                            // Check if function returned early
                            let stack = self.env.get_stack(stack_id).unwrap();
                            if stack.completed {
                                result = stack.result.clone();
                                break;
                            }
                        }

                        // Pop frame
                        let stack = self.env.get_stack_mut(stack_id).unwrap();
                        stack.pop_frame();
                        stack.completed = false; // Reset for outer context

                        Ok(result)
                    }
                    _ => Err(Error::TypeError("Value is not callable".to_string())),
                }
            }

            TermOp::DefineFunction { .. } => {
                // Function definitions don't produce runtime values
                Ok(Value::Nil)
            }

            TermOp::Index { target, index } => {
                let target_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*target)
                    .ok_or(Error::RuntimeError("Invalid target term".to_string()))?
                    .clone();

                let target_value = self.eval_term(&target_term, stack_id)?;

                let index_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*index)
                    .ok_or(Error::RuntimeError("Invalid index term".to_string()))?
                    .clone();

                let index_value = self.eval_term(&index_term, stack_id)?;

                match (target_value, index_value) {
                    (Value::List(list), Value::Int(i)) => {
                        if i < 0 || i >= list.len() as i64 {
                            return Err(Error::OutOfBounds);
                        }
                        Ok(list[i as usize].clone())
                    }
                    _ => Err(Error::TypeError("Invalid index operation".to_string())),
                }
            }

            TermOp::FieldAccess { target, field } => {
                let target_term = self
                    .env
                    .get_program(
                        self.env
                            .get_stack(stack_id)
                            .ok_or(Error::InvalidStackKey)?
                            .program_id,
                    )
                    .ok_or(Error::InvalidProgramKey)?
                    .get_term(*target)
                    .ok_or(Error::RuntimeError("Invalid target term".to_string()))?
                    .clone();

                let target_value = self.eval_term(&target_term, stack_id)?;

                match target_value {
                    Value::Map(map) => Ok(map.get(field).cloned().unwrap_or(Value::Nil)),
                    _ => Err(Error::TypeError("Field access on non-map".to_string())),
                }
            }

            TermOp::MakeList(elements) => {
                let values: Result<Vec<Value>> = elements
                    .iter()
                    .map(|elem_id| {
                        let t = self
                            .env
                            .get_program(
                                self.env
                                    .get_stack(stack_id)
                                    .ok_or(Error::InvalidStackKey)?
                                    .program_id,
                            )
                            .ok_or(Error::InvalidProgramKey)?
                            .get_term(*elem_id)
                            .ok_or(Error::RuntimeError("Invalid element term".to_string()))?
                            .clone();
                        self.eval_term(&t, stack_id)
                    })
                    .collect();

                Ok(Value::List(values?))
            }

            TermOp::MakeMap(fields) => {
                let mut map = HashMap::new();
                for (key, value_id) in fields {
                    let t = self
                        .env
                        .get_program(
                            self.env
                                .get_stack(stack_id)
                                .ok_or(Error::InvalidStackKey)?
                                .program_id,
                        )
                        .ok_or(Error::InvalidProgramKey)?
                        .get_term(*value_id)
                        .ok_or(Error::RuntimeError("Invalid value term".to_string()))?
                        .clone();
                    let value = self.eval_term(&t, stack_id)?;
                    map.insert(key.clone(), value);
                }
                Ok(Value::Map(map))
            }

            TermOp::Nop => Ok(Value::Nil),
        }
    }

    fn eval_input(&mut self, term: &crate::term::Term, index: usize, stack_id: StackKey) -> Result<Value> {
        let input_id = term
            .inputs
            .get(index)
            .ok_or(Error::RuntimeError("Missing input".to_string()))?;

        let program_id = self
            .env
            .get_stack(stack_id)
            .ok_or(Error::InvalidStackKey)?
            .program_id;

        let input_term = self
            .env
            .get_program(program_id)
            .ok_or(Error::InvalidProgramKey)?
            .get_term(*input_id)
            .ok_or(Error::RuntimeError("Invalid input term".to_string()))?
            .clone();

        self.eval_term(&input_term, stack_id)
    }

    fn load_var(&self, name: &str, stack_id: StackKey) -> Result<Value> {
        let stack = self.env.get_stack(stack_id).ok_or(Error::InvalidStackKey)?;

        // Check if it's a builtin function
        if self.env.builtins.contains_key(name) {
            return Ok(Value::BuiltinFunction(name.to_string()));
        }

        stack
            .get_variable(name)
            .cloned()
            .ok_or_else(|| Error::UnknownVariable(name.to_string()))
    }

    fn add_values(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
            (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
            _ => Err(Error::TypeError("Cannot add these types".to_string())),
        }
    }

    fn sub_values(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
            _ => Err(Error::TypeError("Cannot subtract these types".to_string())),
        }
    }

    fn mul_values(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
            _ => Err(Error::TypeError("Cannot multiply these types".to_string())),
        }
    }

    fn div_values(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Int(a / b))
                }
            }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 / b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a / b as f64)),
            _ => Err(Error::TypeError("Cannot divide these types".to_string())),
        }
    }

    fn mod_values(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Int(a % b))
                }
            }
            _ => Err(Error::TypeError("Modulo requires integers".to_string())),
        }
    }

    fn compare_lt(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((a as f64) < b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(a < (b as f64))),
            _ => Err(Error::TypeError("Cannot compare these types".to_string())),
        }
    }

    fn compare_gt(&self, left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((a as f64) > b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(a > (b as f64))),
            _ => Err(Error::TypeError("Cannot compare these types".to_string())),
        }
    }

    fn values_equal(&self, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            _ => false,
        }
    }
}

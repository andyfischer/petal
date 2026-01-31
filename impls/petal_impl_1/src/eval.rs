//! Evaluator/Interpreter for Petal

use std::collections::HashMap;

use crate::env::{value_to_string, Env};
use crate::error::{Error, Result};
use crate::program::{ConstantValue, Term, TermId, TermOp};
use crate::provenance::ExecutionTrace;
use crate::stack::{StackKey, StepResult};
use crate::value::Value;

/// The evaluator executes Petal programs
pub struct Evaluator {
    /// Memoized term values for current execution
    term_values: HashMap<TermId, Value>,
    /// Execution trace for provenance tracking
    trace: ExecutionTrace,
    /// Whether tracing is enabled
    trace_enabled: bool,
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl Evaluator {
    pub fn new() -> Self {
        Self {
            term_values: HashMap::new(),
            trace: ExecutionTrace::new(),
            trace_enabled: false,
        }
    }

    /// Create an evaluator with tracing enabled
    pub fn with_tracing() -> Self {
        Self {
            term_values: HashMap::new(),
            trace: ExecutionTrace::new(),
            trace_enabled: true,
        }
    }

    /// Enable or disable tracing
    pub fn set_tracing(&mut self, enabled: bool) {
        self.trace_enabled = enabled;
        if !enabled {
            self.trace.clear();
        }
    }

    /// Get the execution trace
    pub fn trace(&self) -> &ExecutionTrace {
        &self.trace
    }

    /// Get mutable access to the trace
    pub fn trace_mut(&mut self) -> &mut ExecutionTrace {
        &mut self.trace
    }

    /// Run a program to completion
    pub fn run(&mut self, env: &mut Env, stack_key: StackKey) -> Result<Value> {
        let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
        let program_key = stack.program_id;
        let program = env.get_program(program_key).ok_or(Error::InvalidProgramKey)?;
        let entry = program.entry();

        self.eval_term(env, stack_key, entry)
    }

    /// Step through execution
    pub fn step(&mut self, env: &mut Env, stack_key: StackKey) -> Result<StepResult> {
        let result = self.run(env, stack_key)?;
        Ok(StepResult::Complete(result))
    }

    /// Evaluate a term
    fn eval_term(&mut self, env: &mut Env, stack_key: StackKey, term_id: TermId) -> Result<Value> {
        // Check memoized value
        if let Some(value) = self.term_values.get(&term_id) {
            return Ok(value.clone());
        }

        let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
        let program_key = stack.program_id;
        let program = env.get_program(program_key).ok_or(Error::InvalidProgramKey)?;
        let term = program.get_term(term_id).ok_or(Error::InvalidTermId(term_id))?.clone();

        // Get input term IDs for tracing
        let input_ids: Vec<TermId> = term.inputs.iter().copied().collect();

        let result = self.eval_term_op(env, stack_key, &term)?;

        // Record trace step if tracing is enabled
        if self.trace_enabled {
            self.trace.record(term_id, input_ids, result.clone());
        }

        // Memoize
        self.term_values.insert(term_id, result.clone());

        Ok(result)
    }

    fn eval_term_op(&mut self, env: &mut Env, stack_key: StackKey, term: &Term) -> Result<Value> {
        match &term.op {
            TermOp::Constant(const_id) => {
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                let program = env.get_program(stack.program_id).ok_or(Error::InvalidProgramKey)?;
                let constant = program.constants.get(*const_id).ok_or_else(|| Error::Runtime {
                    message: "Invalid constant".to_string(),
                })?;

                Ok(match constant {
                    ConstantValue::Nil => Value::Nil,
                    ConstantValue::Bool(b) => Value::Bool(*b),
                    ConstantValue::Int(n) => Value::Int(*n),
                    ConstantValue::Float(f) => Value::Float(*f),
                    ConstantValue::String(s) => {
                        let id = env.heap.alloc_string(s.clone());
                        Value::String(id)
                    }
                })
            }

            TermOp::Error(msg) => Err(Error::Runtime { message: msg.clone() }),

            TermOp::Var(name) => {
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                let program = env.get_program(stack.program_id).ok_or(Error::InvalidProgramKey)?;

                // Check if it's a state variable FIRST (state takes precedence)
                for term in program.terms() {
                    if let TermOp::StateDecl { name: state_name, key } = &term.op {
                        if state_name == name {
                            if let Some(value) = stack.get_state(*key) {
                                return Ok(value.clone());
                            }
                        }
                    }
                }

                // Then check locals
                if let Some(value) = stack.get_local(name) {
                    return Ok(value.clone());
                }

                // Check if it's a builtin function
                if env.get_builtin(name).is_some() {
                    return Ok(Value::NativeFunction(name.clone()));
                }

                // Check if it's a user function
                if let Some(program) = env.get_program(stack.program_id) {
                    if program.find_function(name).is_some() {
                        return Ok(Value::NativeFunction(name.clone()));
                    }
                }

                Err(Error::UndefinedVariable { name: name.clone() })
            }

            TermOp::Let { name } => {
                let value = self.eval_term(env, stack_key, term.inputs[0])?;
                let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                stack.set_local(name.clone(), value.clone());
                Ok(value)
            }

            TermOp::Assign { name } => {
                let value = self.eval_term(env, stack_key, term.inputs[0])?;

                // Check if it's a state variable first
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                let program = env.get_program(stack.program_id).ok_or(Error::InvalidProgramKey)?;

                // Collect state key if found
                let state_key_opt = program.terms()
                    .find_map(|t| {
                        if let TermOp::StateDecl { name: state_name, key } = &t.op {
                            if state_name == name {
                                return Some(*key);
                            }
                        }
                        None
                    });

                if let Some(key) = state_key_opt {
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.set_state(key, value.clone());
                    return Ok(value);
                }

                // Regular variable assignment
                let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                stack.set_local(name.clone(), value.clone());
                Ok(value)
            }

            TermOp::StateDecl { name, key } => {
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;

                // First time seeing this state? Bind it at current loop depth
                if !stack.is_state_bound(*key) {
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.bind_state(*key);
                }

                // Check if state already exists (with proper iteration context)
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                if let Some(value) = stack.get_state(*key) {
                    return Ok(value.clone());
                }

                // Initialize state with the default value
                let init_value = self.eval_term(env, stack_key, term.inputs[0])?;
                let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                stack.set_state(*key, init_value.clone());
                stack.set_local(name.clone(), init_value.clone());
                Ok(init_value)
            }

            TermOp::StateRead { name: _, key } => {
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                Ok(stack.get_state(*key).cloned().unwrap_or(Value::Nil))
            }

            TermOp::StateWrite { name: _, key } => {
                let value = self.eval_term(env, stack_key, term.inputs[0])?;
                let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                stack.set_state(*key, value.clone());
                Ok(value)
            }

            // Arithmetic
            TermOp::Add => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.add(env, &left, &right)
            }

            TermOp::Sub => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.sub(&left, &right)
            }

            TermOp::Mul => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.mul(&left, &right)
            }

            TermOp::Div => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.div(&left, &right)
            }

            TermOp::Mod => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.modulo(&left, &right)
            }

            TermOp::Neg => {
                let operand = self.eval_term(env, stack_key, term.inputs[0])?;
                match operand {
                    Value::Int(n) => Ok(Value::Int(-n)),
                    Value::Float(f) => Ok(Value::Float(-f)),
                    _ => Err(Error::Type {
                        expected: "number".to_string(),
                        got: operand.type_name().to_string(),
                    }),
                }
            }

            // Comparison
            TermOp::Eq => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                Ok(Value::Bool(self.values_equal(env, &left, &right)))
            }

            TermOp::NotEq => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                Ok(Value::Bool(!self.values_equal(env, &left, &right)))
            }

            TermOp::Lt => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.compare_lt(&left, &right)
            }

            TermOp::LtEq => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.compare_lteq(&left, &right)
            }

            TermOp::Gt => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.compare_gt(&left, &right)
            }

            TermOp::GtEq => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                let right = self.eval_term(env, stack_key, term.inputs[1])?;
                self.compare_gteq(&left, &right)
            }

            // Logical
            TermOp::And => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                if !left.is_truthy() {
                    return Ok(left);
                }
                self.eval_term(env, stack_key, term.inputs[1])
            }

            TermOp::Or => {
                let left = self.eval_term(env, stack_key, term.inputs[0])?;
                if left.is_truthy() {
                    return Ok(left);
                }
                self.eval_term(env, stack_key, term.inputs[1])
            }

            TermOp::Not => {
                let operand = self.eval_term(env, stack_key, term.inputs[0])?;
                Ok(Value::Bool(!operand.is_truthy()))
            }

            // Control flow
            TermOp::If => {
                let condition = self.eval_term(env, stack_key, term.inputs[0])?;
                if condition.is_truthy() {
                    self.eval_term(env, stack_key, term.inputs[1])
                } else {
                    self.eval_term(env, stack_key, term.inputs[2])
                }
            }

            TermOp::Block => {
                let mut result = Value::Nil;
                for &input in &term.inputs {
                    result = self.eval_term(env, stack_key, input)?;
                }
                Ok(result)
            }

            TermOp::Return => {
                self.eval_term(env, stack_key, term.inputs[0])
            }

            TermOp::Loop => {
                // Generic loop - not typically used directly
                Ok(Value::Nil)
            }

            // Functions
            TermOp::FnDef { name: _, params: _, body: _ } => {
                // Function definitions are handled at parse time
                Ok(Value::Nil)
            }

            TermOp::Call { function, arg_count } => {
                // Evaluate arguments
                let mut args = Vec::with_capacity(*arg_count);
                for i in 0..*arg_count {
                    args.push(self.eval_term(env, stack_key, term.inputs[i])?);
                }

                // Check for builtin
                if let Some(builtin) = env.get_builtin(function).cloned() {
                    return builtin(env, &args);
                }

                // Check for user function
                let stack = env.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
                let program = env.get_program(stack.program_id).ok_or(Error::InvalidProgramKey)?;

                if let Some((_, func)) = program.find_function(function) {
                    let params = func.params.clone();
                    let body = func.body;

                    if args.len() != params.len() {
                        return Err(Error::ArityMismatch {
                            expected: params.len(),
                            got: args.len(),
                        });
                    }

                    // Create new frame
                    let program_id = stack.program_id;
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.push_frame(program_id);

                    // Bind parameters
                    for (param, arg) in params.iter().zip(args) {
                        stack.set_local(param.clone(), arg);
                    }

                    // Clear memoization for new frame
                    let old_memo = std::mem::take(&mut self.term_values);

                    // Evaluate body
                    let result = self.eval_term(env, stack_key, body);

                    // Restore memoization
                    self.term_values = old_memo;

                    // Pop frame
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.pop_frame();

                    return result;
                }

                Err(Error::UndefinedFunction { name: function.clone() })
            }

            // Data structures
            TermOp::List => {
                let mut elements = Vec::with_capacity(term.inputs.len());
                for &input in &term.inputs {
                    elements.push(self.eval_term(env, stack_key, input)?);
                }
                let id = env.heap.alloc_list(elements);
                Ok(Value::List(id))
            }

            TermOp::Map => {
                let mut entries = HashMap::new();
                // Inputs are key-value pairs
                let mut i = 0;
                while i + 1 < term.inputs.len() {
                    let key_val = self.eval_term(env, stack_key, term.inputs[i])?;
                    let value = self.eval_term(env, stack_key, term.inputs[i + 1])?;

                    let key = match key_val {
                        Value::String(id) => env.heap.get_string(id).unwrap_or("").to_string(),
                        _ => value_to_string(env, &key_val),
                    };

                    entries.insert(key, value);
                    i += 2;
                }
                let id = env.heap.alloc_map(entries);
                Ok(Value::Map(id))
            }

            TermOp::Index => {
                let collection = self.eval_term(env, stack_key, term.inputs[0])?;
                let index = self.eval_term(env, stack_key, term.inputs[1])?;

                match collection {
                    Value::List(id) => {
                        let idx = index.as_int().ok_or_else(|| Error::Type {
                            expected: "int".to_string(),
                            got: index.type_name().to_string(),
                        })?;

                        let list = env.heap.get_list(id).ok_or_else(|| Error::Runtime {
                            message: "Invalid list".to_string(),
                        })?;

                        if idx < 0 || idx as usize >= list.len() {
                            return Err(Error::IndexOutOfBounds {
                                index: idx,
                                length: list.len(),
                            });
                        }

                        Ok(list[idx as usize].clone())
                    }
                    Value::Map(id) => {
                        let key = match &index {
                            Value::String(sid) => env.heap.get_string(*sid).unwrap_or("").to_string(),
                            _ => value_to_string(env, &index),
                        };

                        let map = env.heap.get_map(id).ok_or_else(|| Error::Runtime {
                            message: "Invalid map".to_string(),
                        })?;

                        Ok(map.get(&key).cloned().unwrap_or(Value::Nil))
                    }
                    Value::String(id) => {
                        let idx = index.as_int().ok_or_else(|| Error::Type {
                            expected: "int".to_string(),
                            got: index.type_name().to_string(),
                        })?;

                        let s = env.heap.get_string(id).ok_or_else(|| Error::Runtime {
                            message: "Invalid string".to_string(),
                        })?;

                        if idx < 0 || idx as usize >= s.len() {
                            return Err(Error::IndexOutOfBounds {
                                index: idx,
                                length: s.len(),
                            });
                        }

                        let ch = s.chars().nth(idx as usize).unwrap();
                        let ch_id = env.heap.alloc_string(ch.to_string());
                        Ok(Value::String(ch_id))
                    }
                    _ => Err(Error::Type {
                        expected: "indexable".to_string(),
                        got: collection.type_name().to_string(),
                    }),
                }
            }

            TermOp::Field { name } => {
                let obj = self.eval_term(env, stack_key, term.inputs[0])?;

                match obj {
                    Value::Map(id) => {
                        let map = env.heap.get_map(id).ok_or_else(|| Error::Runtime {
                            message: "Invalid map".to_string(),
                        })?;
                        Ok(map.get(name).cloned().unwrap_or(Value::Nil))
                    }
                    _ => Err(Error::Type {
                        expected: "map".to_string(),
                        got: obj.type_name().to_string(),
                    }),
                }
            }

            TermOp::SetField { name } => {
                let obj = self.eval_term(env, stack_key, term.inputs[0])?;
                let value = self.eval_term(env, stack_key, term.inputs[1])?;

                match obj {
                    Value::Map(id) => {
                        if let Some(map) = env.heap.get_map_mut(id) {
                            map.insert(name.clone(), value.clone());
                        }
                        Ok(value)
                    }
                    _ => Err(Error::Type {
                        expected: "map".to_string(),
                        got: obj.type_name().to_string(),
                    }),
                }
            }

            // Iteration
            TermOp::Range => {
                let start = self.eval_term(env, stack_key, term.inputs[0])?;
                let end = self.eval_term(env, stack_key, term.inputs[1])?;

                let start = start.as_int().ok_or_else(|| Error::Type {
                    expected: "int".to_string(),
                    got: start.type_name().to_string(),
                })?;

                let end = end.as_int().ok_or_else(|| Error::Type {
                    expected: "int".to_string(),
                    got: end.type_name().to_string(),
                })?;

                Ok(Value::Range { start, end })
            }

            TermOp::ForLoop { var_name, body } => {
                let iterator = self.eval_term(env, stack_key, term.inputs[0])?;
                let mut results = Vec::new();

                // Push loop context for iteration-aware state keys
                let loop_term_id = term.id;

                match iterator {
                    Value::Range { start, end } => {
                        for i in start..end {
                            let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                            // Push iteration context with current index
                            stack.push_loop_iteration(loop_term_id, i);
                            stack.set_local(var_name.clone(), Value::Int(i));

                            // Clear all memoization for fresh evaluation each iteration
                            self.term_values.clear();

                            let result = self.eval_term(env, stack_key, *body)?;
                            results.push(result);

                            // Pop iteration context
                            let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                            stack.pop_loop_iteration();
                        }
                    }
                    Value::List(id) => {
                        let elements = env.heap.get_list(id)
                            .ok_or_else(|| Error::Runtime { message: "Invalid list".to_string() })?
                            .to_vec();

                        for (idx, elem) in elements.into_iter().enumerate() {
                            let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                            // Push iteration context with current index
                            stack.push_loop_iteration(loop_term_id, idx as i64);
                            stack.set_local(var_name.clone(), elem);

                            // Clear all memoization for fresh evaluation each iteration
                            self.term_values.clear();

                            let result = self.eval_term(env, stack_key, *body)?;
                            results.push(result);

                            // Pop iteration context
                            let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                            stack.pop_loop_iteration();
                        }
                    }
                    _ => {
                        return Err(Error::Type {
                            expected: "iterable".to_string(),
                            got: iterator.type_name().to_string(),
                        });
                    }
                }

                // Return the last result or nil
                Ok(results.pop().unwrap_or(Value::Nil))
            }

            TermOp::WhileLoop { body } => {
                let condition_term = term.inputs[0];
                let loop_term_id = term.id;
                let mut last_result = Value::Nil;
                let mut iterations: i64 = 0;
                const MAX_ITERATIONS: i64 = 1_000_000;

                loop {
                    if iterations >= MAX_ITERATIONS {
                        return Err(Error::Runtime {
                            message: "Infinite loop detected".to_string(),
                        });
                    }

                    // Clear all memoization for fresh evaluation each iteration
                    self.term_values.clear();

                    let condition = self.eval_term(env, stack_key, condition_term)?;
                    if !condition.is_truthy() {
                        break;
                    }

                    // Push iteration context
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.push_loop_iteration(loop_term_id, iterations);

                    last_result = self.eval_term(env, stack_key, *body)?;

                    // Pop iteration context
                    let stack = env.get_stack_mut(stack_key).ok_or(Error::InvalidStackKey)?;
                    stack.pop_loop_iteration();

                    iterations += 1;
                }

                Ok(last_result)
            }

            TermOp::Print => {
                let mut parts = Vec::new();
                for &input in &term.inputs {
                    let value = self.eval_term(env, stack_key, input)?;
                    parts.push(value_to_string(env, &value));
                }
                println!("{}", parts.join(" "));
                Ok(Value::Nil)
            }
        }
    }

    fn add(&self, env: &mut Env, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
            (Value::String(a), Value::String(b)) => {
                let s1 = env.heap.get_string(*a).unwrap_or("");
                let s2 = env.heap.get_string(*b).unwrap_or("");
                let combined = format!("{}{}", s1, s2);
                let id = env.heap.alloc_string(combined);
                Ok(Value::String(id))
            }
            _ => Err(Error::Type {
                expected: "numbers or strings".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn sub(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - *b as f64)),
            _ => Err(Error::Type {
                expected: "numbers".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn mul(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * *b as f64)),
            _ => Err(Error::Type {
                expected: "numbers".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn div(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Int(a / b))
            }
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Float(a / b))
            }
            (Value::Int(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Float(*a as f64 / b))
            }
            (Value::Float(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Float(a / *b as f64))
            }
            _ => Err(Error::Type {
                expected: "numbers".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn modulo(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Int(a % b))
            }
            (Value::Float(a), Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(Error::DivisionByZero);
                }
                Ok(Value::Float(a % b))
            }
            _ => Err(Error::Type {
                expected: "numbers".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn values_equal(&self, env: &Env, left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::String(a), Value::String(b)) => {
                let s1 = env.heap.get_string(*a).unwrap_or("");
                let s2 = env.heap.get_string(*b).unwrap_or("");
                s1 == s2
            }
            _ => false,
        }
    }

    fn compare_lt(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) < *b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(*a < (*b as f64))),
            _ => Err(Error::Type {
                expected: "comparable values".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn compare_lteq(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) <= *b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(*a <= (*b as f64))),
            _ => Err(Error::Type {
                expected: "comparable values".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn compare_gt(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) > *b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(*a > (*b as f64))),
            _ => Err(Error::Type {
                expected: "comparable values".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn compare_gteq(&self, left: &Value, right: &Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) >= *b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(*a >= (*b as f64))),
            _ => Err(Error::Type {
                expected: "comparable values".to_string(),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }
}

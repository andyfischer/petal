//! Interpreter/evaluator - executes terms

use crate::{
    Error, Result,
    program::{Program, Term, TermOp, ConstantId, ConstantValue},
    stack::{Stack, StackKey, Frame, StepResult, LoopContext},
    value::Value,
    env::Env,
};

pub struct Evaluator<'a> {
    env: &'a mut Env,
}

impl<'a> Evaluator<'a> {
    pub fn new(env: &'a mut Env) -> Self {
        Self { env }
    }

    /// Execute one step of execution
    pub fn step(&mut self, stack_key: StackKey) -> Result<StepResult> {
        // Get stack and program
        let stack = self.env.get_stack(stack_key)
            .ok_or_else(|| Error::Runtime("Stack not found".to_string()))?;
        let program_key = stack.program_id;
        let program = self.env.get_program(program_key)
            .ok_or_else(|| Error::Runtime("Program not found".to_string()))?;

        // Get current term or start from entry
        let current_term = if let Some(term_id) = stack.current_term {
            term_id
        } else {
            program.entry
        };

        // Find the term
        let term = program.get_term(current_term)
            .ok_or_else(|| Error::Runtime(format!("Term {:?} not found", current_term)))?;

        // Evaluate the term
        match self.eval_term(term, stack_key, program_key)? {
            StepResult::Continue => {
                // Move to next term in control flow
                let next = term.control_flow_next.unwrap_or(current_term);
                self.env.get_stack_mut(stack_key).unwrap().current_term = Some(next);
                Ok(StepResult::Continue)
            }
            other => Ok(other),
        }
    }

    /// Evaluate a single term
    fn eval_term(
        &mut self,
        term: &Term,
        stack_key: StackKey,
        program_key: crate::ProgramKey,
    ) -> Result<StepResult> {
        let value = match &term.op {
            TermOp::NoOp => Value::Nil,

            TermOp::Constant(const_id) => self.eval_constant(*const_id, program_key)?,

            TermOp::Add => self.eval_add(term, stack_key, program_key)?,

            TermOp::Sub => self.eval_sub(term, stack_key, program_key)?,

            TermOp::Mul => self.eval_mul(term, stack_key, program_key)?,

            TermOp::Div => self.eval_div(term, stack_key, program_key)?,

            TermOp::Print => {
                // TODO: Implement print
                Value::Nil
            }

            TermOp::Error(msg_id) => {
                let msg = self.eval_constant(*msg_id, program_key)?;
                return Err(Error::Parse(ParseError::InvalidSyntax(format!("{:?}", msg))));
            }

            _ => {
                // For now, return nil for unimplemented operations
                Value::Nil
            }
        };

        // Store result in the current frame
        self.env.get_stack_mut(stack_key)
            .unwrap()
            .write_register(term.id.0 as usize, value.clone());

        Ok(StepResult::Continue)
    }

    fn eval_constant(&self, const_id: ConstantId, program_key: crate::ProgramKey) -> Result<Value> {
        let program = self.env.get_program(program_key)
            .ok_or_else(|| Error::Runtime("Program not found".to_string()))?;

        match program.constants.get(const_id) {
            Some(ConstantValue::Int(i)) => Ok(Value::Int(*i)),
            Some(ConstantValue::Float(f)) => Ok(Value::Float(*f)),
            Some(ConstantValue::String(s)) => Ok(Value::Nil), // TODO: Handle strings
            None => Err(Error::Runtime("Constant not found".to_string())),
        }
    }

    fn eval_add(&self, term: &Term, stack_key: StackKey, program_key: crate::ProgramKey) -> Result<Value> {
        if term.inputs.len() != 2 {
            return Err(Error::InvalidOperation("Add requires 2 inputs".to_string()));
        }

        let left_val = self.read_value(term.inputs[0], stack_key, program_key)?;
        let right_val = self.read_value(term.inputs[1], stack_key, program_key)?;

        match (left_val, right_val) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 + b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + b as f64)),
            _ => Err(Error::InvalidOperation("Cannot add these types".to_string())),
        }
    }

    fn eval_sub(&self, term: &Term, stack_key: StackKey, program_key: crate::ProgramKey) -> Result<Value> {
        if term.inputs.len() != 2 {
            return Err(Error::InvalidOperation("Sub requires 2 inputs".to_string()));
        }

        let left_val = self.read_value(term.inputs[0], stack_key, program_key)?;
        let right_val = self.read_value(term.inputs[1], stack_key, program_key)?;

        match (left_val, right_val) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 - b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - b as f64)),
            _ => Err(Error::InvalidOperation("Cannot subtract these types".to_string())),
        }
    }

    fn eval_mul(&self, term: &Term, stack_key: StackKey, program_key: crate::ProgramKey) -> Result<Value> {
        if term.inputs.len() != 2 {
            return Err(Error::InvalidOperation("Mul requires 2 inputs".to_string()));
        }

        let left_val = self.read_value(term.inputs[0], stack_key, program_key)?;
        let right_val = self.read_value(term.inputs[1], stack_key, program_key)?;

        match (left_val, right_val) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(a as f64 * b)),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * b as f64)),
            _ => Err(Error::InvalidOperation("Cannot multiply these types".to_string())),
        }
    }

    fn eval_div(&self, term: &Term, stack_key: StackKey, program_key: crate::ProgramKey) -> Result<Value> {
        if term.inputs.len() != 2 {
            return Err(Error::InvalidOperation("Div requires 2 inputs".to_string()));
        }

        let left_val = self.read_value(term.inputs[0], stack_key, program_key)?;
        let right_val = self.read_value(term.inputs[1], stack_key, program_key)?;

        match (left_val, right_val) {
            (Value::Int(a), Value::Int(b)) => {
                if b == 0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Int(a / b))
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if b == 0.0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Float(a / b))
                }
            }
            (Value::Int(a), Value::Float(b)) => {
                if b == 0.0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Float(a as f64 / b))
                }
            }
            (Value::Float(a), Value::Int(b)) => {
                if b == 0 {
                    Err(Error::DivisionByZero)
                } else {
                    Ok(Value::Float(a / b as f64))
                }
            }
            _ => Err(Error::InvalidOperation("Cannot divide these types".to_string())),
        }
    }

    fn read_value(
        &self,
        term_id: TermId,
        stack_key: StackKey,
        program_key: crate::ProgramKey,
    ) -> Result<&Value> {
        let stack = self.env.get_stack(stack_key)
            .ok_or_else(|| Error::Runtime("Stack not found".to_string()))?;

        stack.read_register(term_id.0 as usize)
            .ok_or_else(|| Error::Runtime(format!("No value for term {:?}", term_id)))
    }
}

use crate::parse::ParseError;

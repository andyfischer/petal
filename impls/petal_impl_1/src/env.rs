//! Environment - the foundational data structure

use slotmap::SlotMap;
use std::collections::HashMap;

use crate::differentiate::{backpropagate, Gradients};
use crate::error::{Error, Result};
use crate::eval::Evaluator;
use crate::heap::Heap;
use crate::live_edit::{migrate_state, SourceEdit, StateReconciliation, StateSchema};
use crate::parse::Parser;
use crate::program::{Program, ProgramKey, TermId};
use crate::projection::Projection;
use crate::provenance::ExecutionTrace;
use crate::stack::{Stack, StackKey, StepResult};
use crate::value::Value;

/// Built-in function type
pub type BuiltinFn = fn(&mut Env, &[Value]) -> Result<Value>;

/// The foundational environment
pub struct Env {
    /// Programs stored with generational indices
    programs: SlotMap<ProgramKey, Program>,
    /// Stacks stored with generational indices
    stacks: SlotMap<StackKey, Stack>,
    /// Garbage-collected heap
    pub heap: Heap,
    /// Built-in functions
    builtins: HashMap<String, BuiltinFn>,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// Create a new environment
    pub fn new() -> Self {
        let mut env = Self {
            programs: SlotMap::with_key(),
            stacks: SlotMap::with_key(),
            heap: Heap::new(),
            builtins: HashMap::new(),
        };
        env.register_default_builtins();
        env
    }

    fn register_default_builtins(&mut self) {
        // print - handled specially in evaluator
        self.builtins.insert("print".to_string(), builtin_print);

        // range
        self.builtins.insert("range".to_string(), builtin_range);

        // len
        self.builtins.insert("len".to_string(), builtin_len);

        // Math functions
        self.builtins.insert("sqrt".to_string(), builtin_sqrt);
        self.builtins.insert("sin".to_string(), builtin_sin);
        self.builtins.insert("cos".to_string(), builtin_cos);
        self.builtins.insert("floor".to_string(), builtin_floor);
        self.builtins.insert("ceil".to_string(), builtin_ceil);
        self.builtins.insert("abs".to_string(), builtin_abs);
        self.builtins.insert("random".to_string(), builtin_random);

        // Type conversions
        self.builtins.insert("int".to_string(), builtin_int);
        self.builtins.insert("float".to_string(), builtin_float);
        self.builtins.insert("str".to_string(), builtin_str);

        // List operations
        self.builtins.insert("push".to_string(), builtin_push);
        self.builtins.insert("pop".to_string(), builtin_pop);
    }

    /// Register a built-in function
    pub fn register_builtin(&mut self, name: &str, func: BuiltinFn) {
        self.builtins.insert(name.to_string(), func);
    }

    /// Get a built-in function
    pub fn get_builtin(&self, name: &str) -> Option<&BuiltinFn> {
        self.builtins.get(name)
    }

    /// Load a program from source
    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey> {
        let key = self.programs.insert_with_key(|key| Program::new(key));
        let mut parser = Parser::new(source, key);
        let program = parser.parse()?;

        if let Some(p) = self.programs.get_mut(key) {
            *p = program;
        }

        Ok(key)
    }

    /// Get a program by key
    pub fn get_program(&self, key: ProgramKey) -> Option<&Program> {
        self.programs.get(key)
    }

    /// Get a mutable program by key
    pub fn get_program_mut(&mut self, key: ProgramKey) -> Option<&mut Program> {
        self.programs.get_mut(key)
    }

    /// Create a new execution stack
    pub fn create_stack(&mut self, program_key: ProgramKey) -> Result<StackKey> {
        if !self.programs.contains_key(program_key) {
            return Err(Error::InvalidProgramKey);
        }

        let key = self.stacks.insert_with_key(|key| Stack::new(key, program_key));
        Ok(key)
    }

    /// Get a stack by key
    pub fn get_stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(key)
    }

    /// Get a mutable stack by key
    pub fn get_stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(key)
    }

    /// Destroy a stack
    pub fn destroy_stack(&mut self, key: StackKey) {
        self.stacks.remove(key);
    }

    /// Run a program to completion
    pub fn run(&mut self, stack_key: StackKey) -> Result<Value> {
        let mut evaluator = Evaluator::new();
        evaluator.run(self, stack_key)
    }

    /// Run a program with tracing enabled
    /// Returns the result and the execution trace
    pub fn run_with_tracing(
        &mut self,
        stack_key: StackKey,
    ) -> Result<(Value, crate::provenance::ExecutionTrace)> {
        let mut evaluator = Evaluator::with_tracing();
        let result = evaluator.run(self, stack_key)?;
        let trace = evaluator.trace().clone();
        Ok((result, trace))
    }

    /// Step through execution one term at a time
    pub fn step(&mut self, stack_key: StackKey) -> Result<StepResult> {
        let mut evaluator = Evaluator::new();
        evaluator.step(self, stack_key)
    }

    /// Reset a stack to run again (preserving state)
    pub fn reset_stack(&mut self, stack_key: StackKey) -> Result<()> {
        let stack = self.stacks.get_mut(stack_key).ok_or(Error::InvalidStackKey)?;
        stack.reset();
        Ok(())
    }

    /// Create a backward slice projection (what influences the target?)
    pub fn backward_slice(&self, program_key: ProgramKey, target: TermId) -> Result<Projection> {
        let program = self.get_program(program_key).ok_or(Error::InvalidProgramKey)?;
        Ok(Projection::backward_slice(program, target))
    }

    /// Create a forward slice projection (what does the source influence?)
    pub fn forward_slice(&self, program_key: ProgramKey, source: TermId) -> Result<Projection> {
        let program = self.get_program(program_key).ok_or(Error::InvalidProgramKey)?;
        Ok(Projection::forward_slice(program, source))
    }

    /// Create a dynamic slice based on execution trace
    pub fn dynamic_slice(
        &self,
        trace: &crate::provenance::ExecutionTrace,
        target: TermId,
    ) -> Projection {
        Projection::dynamic_slice(trace, target)
    }

    /// Apply a live edit to a program
    /// Returns the new program key and state reconciliation info
    pub fn live_edit(
        &mut self,
        old_program_key: ProgramKey,
        stack_key: StackKey,
        source: &str,
        edit: &SourceEdit,
    ) -> Result<(ProgramKey, StateReconciliation)> {
        // Apply the edit to get new source
        let new_source = edit.apply(source);

        // Parse the new source
        let new_program_key = self.load_program(&new_source)?;

        // Get old and new programs
        let old_program = self
            .get_program(old_program_key)
            .ok_or(Error::InvalidProgramKey)?
            .clone();
        let new_program = self
            .get_program(new_program_key)
            .ok_or(Error::InvalidProgramKey)?
            .clone();

        // Build schemas and reconcile
        let old_schema = StateSchema::from_program(&old_program);
        let new_schema = StateSchema::from_program(&new_program);
        let reconciliation = old_schema.reconcile(&new_schema);

        // Migrate state
        let stack = self.get_stack(stack_key).ok_or(Error::InvalidStackKey)?;
        let migrated_state = migrate_state(stack, &old_program, &new_program);

        // Update stack to use new program and migrated state
        if let Some(stack) = self.get_stack_mut(stack_key) {
            stack.program_id = new_program_key;
            stack.state_storage = migrated_state;
            stack.reset();
        }

        Ok((new_program_key, reconciliation))
    }

    /// Apply a live edit and continue execution
    pub fn live_edit_and_run(
        &mut self,
        old_program_key: ProgramKey,
        stack_key: StackKey,
        source: &str,
        edit: &SourceEdit,
    ) -> Result<(Value, StateReconciliation)> {
        let (_, reconciliation) = self.live_edit(old_program_key, stack_key, source, edit)?;
        let result = self.run(stack_key)?;
        Ok((result, reconciliation))
    }

    /// Run with tracing and compute gradients via backpropagation
    pub fn run_with_gradients(
        &mut self,
        stack_key: StackKey,
        seed_gradient: f64,
    ) -> Result<(Value, ExecutionTrace, Gradients)> {
        let (result, trace) = self.run_with_tracing(stack_key)?;

        // Find the final term
        let output_term = trace
            .all_steps()
            .last()
            .map(|s| s.term_id)
            .unwrap_or(TermId(0));

        // Compute gradients
        let gradients = backpropagate(&trace, output_term, seed_gradient);

        Ok((result, trace, gradients))
    }

    /// Compute gradients for a specific output term
    pub fn backpropagate(
        &self,
        trace: &ExecutionTrace,
        output_term: TermId,
        seed_gradient: f64,
    ) -> Gradients {
        backpropagate(trace, output_term, seed_gradient)
    }
}

// Built-in function implementations

fn builtin_print(env: &mut Env, args: &[Value]) -> Result<Value> {
    let parts: Vec<String> = args
        .iter()
        .map(|v| value_to_string(env, v))
        .collect();
    println!("{}", parts.join(" "));
    Ok(Value::Nil)
}

fn builtin_range(_env: &mut Env, args: &[Value]) -> Result<Value> {
    let (start, end) = match args.len() {
        1 => (0, args[0].as_int().ok_or_else(|| Error::Type {
            expected: "int".to_string(),
            got: args[0].type_name().to_string(),
        })?),
        2 => {
            let start = args[0].as_int().ok_or_else(|| Error::Type {
                expected: "int".to_string(),
                got: args[0].type_name().to_string(),
            })?;
            let end = args[1].as_int().ok_or_else(|| Error::Type {
                expected: "int".to_string(),
                got: args[1].type_name().to_string(),
            })?;
            (start, end)
        }
        _ => return Err(Error::ArityMismatch { expected: 2, got: args.len() }),
    };
    Ok(Value::Range { start, end })
}

fn builtin_len(env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    match &args[0] {
        Value::List(id) => {
            let len = env.heap.get_list(*id).map(|l| l.len()).unwrap_or(0);
            Ok(Value::Int(len as i64))
        }
        Value::String(id) => {
            let len = env.heap.get_string(*id).map(|s| s.len()).unwrap_or(0);
            Ok(Value::Int(len as i64))
        }
        _ => Err(Error::Type {
            expected: "list or string".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

fn builtin_sqrt(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let n = args[0].as_float().ok_or_else(|| Error::Type {
        expected: "number".to_string(),
        got: args[0].type_name().to_string(),
    })?;
    Ok(Value::Float(n.sqrt()))
}

fn builtin_sin(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let n = args[0].as_float().ok_or_else(|| Error::Type {
        expected: "number".to_string(),
        got: args[0].type_name().to_string(),
    })?;
    Ok(Value::Float(n.sin()))
}

fn builtin_cos(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let n = args[0].as_float().ok_or_else(|| Error::Type {
        expected: "number".to_string(),
        got: args[0].type_name().to_string(),
    })?;
    Ok(Value::Float(n.cos()))
}

fn builtin_floor(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let n = args[0].as_float().ok_or_else(|| Error::Type {
        expected: "number".to_string(),
        got: args[0].type_name().to_string(),
    })?;
    Ok(Value::Float(n.floor()))
}

fn builtin_ceil(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let n = args[0].as_float().ok_or_else(|| Error::Type {
        expected: "number".to_string(),
        got: args[0].type_name().to_string(),
    })?;
    Ok(Value::Float(n.ceil()))
}

fn builtin_abs(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(n.abs())),
        Value::Float(n) => Ok(Value::Float(n.abs())),
        _ => Err(Error::Type {
            expected: "number".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

fn builtin_random(_env: &mut Env, args: &[Value]) -> Result<Value> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let (min, max) = match args.len() {
        0 => (0.0, 1.0),
        2 => {
            let min = args[0].as_float().ok_or_else(|| Error::Type {
                expected: "number".to_string(),
                got: args[0].type_name().to_string(),
            })?;
            let max = args[1].as_float().ok_or_else(|| Error::Type {
                expected: "number".to_string(),
                got: args[1].type_name().to_string(),
            })?;
            (min, max)
        }
        _ => return Err(Error::ArityMismatch { expected: 2, got: args.len() }),
    };

    // Simple pseudo-random using time
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as f64;
    let rand = (nanos / 1_000_000_000.0).fract();
    Ok(Value::Float(min + rand * (max - min)))
}

fn builtin_int(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(n) => Ok(Value::Int(*n as i64)),
        Value::Bool(b) => Ok(Value::Int(if *b { 1 } else { 0 })),
        _ => Err(Error::Type {
            expected: "number or bool".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

fn builtin_float(_env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    match &args[0] {
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        Value::Float(n) => Ok(Value::Float(*n)),
        _ => Err(Error::Type {
            expected: "number".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

fn builtin_str(env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    let s = value_to_string(env, &args[0]);
    let id = env.heap.alloc_string(s);
    Ok(Value::String(id))
}

fn builtin_push(env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 2 {
        return Err(Error::ArityMismatch { expected: 2, got: args.len() });
    }
    match &args[0] {
        Value::List(id) => {
            if let Some(list) = env.heap.get_list_mut(*id) {
                list.push(args[1].clone());
            }
            Ok(args[0].clone())
        }
        _ => Err(Error::Type {
            expected: "list".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

fn builtin_pop(env: &mut Env, args: &[Value]) -> Result<Value> {
    if args.len() != 1 {
        return Err(Error::ArityMismatch { expected: 1, got: args.len() });
    }
    match &args[0] {
        Value::List(id) => {
            if let Some(list) = env.heap.get_list_mut(*id) {
                Ok(list.pop().unwrap_or(Value::Nil))
            } else {
                Ok(Value::Nil)
            }
        }
        _ => Err(Error::Type {
            expected: "list".to_string(),
            got: args[0].type_name().to_string(),
        }),
    }
}

/// Helper to convert a value to string for printing
pub fn value_to_string(env: &Env, value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => {
            if n.fract() == 0.0 {
                format!("{:.1}", n)
            } else {
                n.to_string()
            }
        }
        Value::String(id) => {
            env.heap.get_string(*id).unwrap_or("").to_string()
        }
        Value::List(id) => {
            if let Some(elements) = env.heap.get_list(*id) {
                let parts: Vec<String> = elements.iter().map(|e| value_to_string(env, e)).collect();
                format!("[{}]", parts.join(", "))
            } else {
                "[]".to_string()
            }
        }
        Value::Map(id) => {
            if let Some(entries) = env.heap.get_map(*id) {
                let parts: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, value_to_string(env, v)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            } else {
                "{}".to_string()
            }
        }
        Value::Function(id) => format!("<function:{}>", id.0),
        Value::NativeFunction(name) => format!("<native:{}>", name),
        Value::Range { start, end } => format!("{}..{}", start, end),
    }
}

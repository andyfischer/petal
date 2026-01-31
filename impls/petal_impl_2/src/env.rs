use crate::error::Error;
use crate::eval::Evaluator;
use crate::parse::Parser;
use crate::program::{Program, ProgramKey};
use crate::stack::{Stack, StackKey};
use crate::value::Value;
use crate::{Result, StepResult};

pub struct Env {
    programs: Vec<Program>,
    stacks: Vec<Stack>,
    next_program_id: usize,
    next_stack_id: usize,
    pub builtins: std::collections::HashMap<String, Box<dyn Fn(&[Value]) -> Result<Value>>>,
}

impl Env {
    pub fn new() -> Self {
        let mut env = Self {
            programs: Vec::new(),
            stacks: Vec::new(),
            next_program_id: 0,
            next_stack_id: 0,
            builtins: std::collections::HashMap::new(),
        };

        // Register default builtins
        env.register_builtins();
        env
    }

    fn register_builtins(&mut self) {
        // Print
        self.register_builtin("print", |args| {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{}", arg);
            }
            println!();
            Ok(Value::Nil)
        });

        // Range
        self.register_builtin("range", |args| {
            if args.len() != 2 {
                return Err(Error::RuntimeError(
                    "range expects 2 arguments".to_string(),
                ));
            }

            let start = match &args[0] {
                Value::Int(i) => *i,
                _ => {
                    return Err(Error::TypeError(
                        "range start must be an integer".to_string(),
                    ))
                }
            };

            let end = match &args[1] {
                Value::Int(i) => *i,
                _ => {
                    return Err(Error::TypeError(
                        "range end must be an integer".to_string(),
                    ))
                }
            };

            let values: Vec<Value> = (start..end).map(Value::Int).collect();
            Ok(Value::List(values))
        });

        // Random
        self.register_builtin("random", |args| {
            use rand::Rng;

            if args.len() != 2 {
                return Err(Error::RuntimeError(
                    "random expects 2 arguments".to_string(),
                ));
            }

            let min = match &args[0] {
                Value::Float(f) => *f,
                Value::Int(i) => *i as f64,
                _ => {
                    return Err(Error::TypeError(
                        "random min must be a number".to_string(),
                    ))
                }
            };

            let max = match &args[1] {
                Value::Float(f) => *f,
                Value::Int(i) => *i as f64,
                _ => {
                    return Err(Error::TypeError(
                        "random max must be a number".to_string(),
                    ))
                }
            };

            let mut rng = rand::thread_rng();
            let value = rng.gen_range(min..max);
            Ok(Value::Float(value))
        });

        // Math functions
        self.register_builtin("sqrt", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("sqrt expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.sqrt())),
                Value::Int(i) => Ok(Value::Float((*i as f64).sqrt())),
                _ => Err(Error::TypeError("sqrt requires a number".to_string())),
            }
        });

        self.register_builtin("sin", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("sin expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.sin())),
                Value::Int(i) => Ok(Value::Float((*i as f64).sin())),
                _ => Err(Error::TypeError("sin requires a number".to_string())),
            }
        });

        self.register_builtin("cos", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("cos expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::Float(f) => Ok(Value::Float(f.cos())),
                Value::Int(i) => Ok(Value::Float((*i as f64).cos())),
                _ => Err(Error::TypeError("cos requires a number".to_string())),
            }
        });

        self.register_builtin("floor", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("floor expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
                Value::Int(i) => Ok(Value::Int(*i)),
                _ => Err(Error::TypeError("floor requires a number".to_string())),
            }
        });

        self.register_builtin("ceil", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("ceil expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
                Value::Int(i) => Ok(Value::Int(*i)),
                _ => Err(Error::TypeError("ceil requires a number".to_string())),
            }
        });

        self.register_builtin("len", |args| {
            if args.len() != 1 {
                return Err(Error::RuntimeError("len expects 1 argument".to_string()));
            }

            match &args[0] {
                Value::List(list) => Ok(Value::Int(list.len() as i64)),
                Value::String(s) => Ok(Value::Int(s.len() as i64)),
                _ => Err(Error::TypeError(
                    "len requires a list or string".to_string(),
                )),
            }
        });
    }

    pub fn register_builtin<F>(&mut self, name: &str, func: F)
    where
        F: Fn(&[Value]) -> Result<Value> + 'static,
    {
        self.builtins.insert(name.to_string(), Box::new(func));
    }

    pub fn call_builtin(&self, name: &str, args: &[Value]) -> Result<Value> {
        if let Some(func) = self.builtins.get(name) {
            func(args)
        } else {
            Err(Error::UnknownFunction(name.to_string()))
        }
    }

    pub fn load_program(&mut self, source: &str) -> Result<ProgramKey> {
        let program_key = ProgramKey(self.next_program_id);
        self.next_program_id += 1;

        let mut parser = Parser::new(source, program_key);
        let program = parser.parse()?;

        self.programs.push(program);
        Ok(program_key)
    }

    pub fn create_stack(&mut self, program_id: ProgramKey) -> Result<StackKey> {
        let program = self
            .programs
            .get(program_id.0)
            .ok_or(Error::InvalidProgramKey)?;

        let stack_key = StackKey(self.next_stack_id);
        self.next_stack_id += 1;

        let stack = Stack::new(stack_key, program_id, program.entry);
        self.stacks.push(stack);

        Ok(stack_key)
    }

    pub fn get_program(&self, key: ProgramKey) -> Option<&Program> {
        self.programs.get(key.0)
    }

    pub fn get_program_mut(&mut self, key: ProgramKey) -> Option<&mut Program> {
        self.programs.get_mut(key.0)
    }

    pub fn get_stack(&self, key: StackKey) -> Option<&Stack> {
        self.stacks.get(key.0)
    }

    pub fn get_stack_mut(&mut self, key: StackKey) -> Option<&mut Stack> {
        self.stacks.get_mut(key.0)
    }

    pub fn step(&mut self, stack_id: StackKey) -> Result<StepResult> {
        let mut evaluator = Evaluator::new(self);
        evaluator.step(stack_id)
    }

    pub fn run(&mut self, stack_id: StackKey) -> Result<Value> {
        loop {
            match self.step(stack_id)? {
                StepResult::Continue => {}
                StepResult::Complete => {
                    let stack = self.get_stack(stack_id).ok_or(Error::InvalidStackKey)?;
                    return Ok(stack.result.clone());
                }
                StepResult::Error => {
                    return Err(Error::RuntimeError("Execution error".to_string()))
                }
            }
        }
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

use crate::ast::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum Value {
    Integer(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Null,
    Symbol(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
    Function {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
        closure: Rc<RefCell<Environment>>,
    },
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
        closure: Rc<RefCell<Environment>>,
    },
    BuiltinFunction(String),
    Range {
        start: i64,
        end: i64,
        step: i64,
    },
    EnumVariant {
        enum_name: String,
        variant: String,
        values: Vec<Value>,
    },
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Integer(n) => write!(f, "{}", n),
            Value::Float(n) => write!(f, "{}", n),
            Value::String(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Symbol(s) => write!(f, ":{}", s),
            Value::Array(arr) => {
                write!(f, "[")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", v)?;
                }
                write!(f, "]")
            }
            Value::Object(obj) => {
                write!(f, "{{")?;
                for (i, (k, v)) in obj.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Function { name, .. } => write!(f, "<function {}>", name),
            Value::Lambda { .. } => write!(f, "<lambda>"),
            Value::BuiltinFunction(name) => write!(f, "<builtin {}>", name),
            Value::Range { start, end, step } => write!(f, "range({}, {}, {})", start, end, step),
            Value::EnumVariant { enum_name, variant, values } => {
                if values.is_empty() {
                    write!(f, "{}::{}", enum_name, variant)
                } else {
                    write!(f, "{}::{}(", enum_name, variant)?;
                    for (i, v) in values.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", v)?;
                    }
                    write!(f, ")")
                }
            }
        }
    }
}

impl Value {
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Null => false,
            Value::Integer(n) => *n != 0,
            Value::Float(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(arr) => !arr.is_empty(),
            _ => true,
        }
    }
}

#[derive(Debug)]
pub struct Environment {
    values: HashMap<String, Value>,
    parent: Option<Rc<RefCell<Environment>>>,
    state: HashMap<String, Value>, // For state variables
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            values: HashMap::new(),
            parent: None,
            state: HashMap::new(),
        }
    }

    pub fn with_parent(parent: Rc<RefCell<Environment>>) -> Self {
        Environment {
            values: HashMap::new(),
            parent: Some(parent),
            state: HashMap::new(),
        }
    }

    pub fn define(&mut self, name: String, value: Value) {
        self.values.insert(name, value);
    }

    pub fn define_state(&mut self, name: String, value: Value) {
        self.state.insert(name, value);
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.values.get(name) {
            Some(value.clone())
        } else if let Some(value) = self.state.get(name) {
            Some(value.clone())
        } else if let Some(parent) = &self.parent {
            parent.borrow().get(name)
        } else {
            None
        }
    }

    pub fn get_state(&self, name: &str) -> Option<Value> {
        if let Some(value) = self.state.get(name) {
            Some(value.clone())
        } else if let Some(parent) = &self.parent {
            parent.borrow().get_state(name)
        } else {
            None
        }
    }

    pub fn set(&mut self, name: &str, value: Value) -> bool {
        if self.values.contains_key(name) {
            self.values.insert(name.to_string(), value);
            true
        } else if self.state.contains_key(name) {
            self.state.insert(name.to_string(), value);
            true
        } else if let Some(parent) = &self.parent {
            parent.borrow_mut().set(name, value)
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    Error(String),
    Return(Value),
    Break,
    Continue,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Error(msg) => write!(f, "Runtime error: {}", msg),
            RuntimeError::Return(_) => write!(f, "Unexpected return"),
            RuntimeError::Break => write!(f, "Unexpected break"),
            RuntimeError::Continue => write!(f, "Unexpected continue"),
        }
    }
}

pub struct Interpreter {
    global: Rc<RefCell<Environment>>,
}

impl Interpreter {
    pub fn new() -> Self {
        let global = Rc::new(RefCell::new(Environment::new()));

        // Register built-in functions
        {
            let mut env = global.borrow_mut();
            env.define("print".to_string(), Value::BuiltinFunction("print".to_string()));
            env.define("println".to_string(), Value::BuiltinFunction("println".to_string()));
            env.define("sqrt".to_string(), Value::BuiltinFunction("sqrt".to_string()));
            env.define("sin".to_string(), Value::BuiltinFunction("sin".to_string()));
            env.define("cos".to_string(), Value::BuiltinFunction("cos".to_string()));
            env.define("abs".to_string(), Value::BuiltinFunction("abs".to_string()));
            env.define("floor".to_string(), Value::BuiltinFunction("floor".to_string()));
            env.define("ceil".to_string(), Value::BuiltinFunction("ceil".to_string()));
            env.define("round".to_string(), Value::BuiltinFunction("round".to_string()));
            env.define("min".to_string(), Value::BuiltinFunction("min".to_string()));
            env.define("max".to_string(), Value::BuiltinFunction("max".to_string()));
            env.define("pow".to_string(), Value::BuiltinFunction("pow".to_string()));
            env.define("len".to_string(), Value::BuiltinFunction("len".to_string()));
            env.define("push".to_string(), Value::BuiltinFunction("push".to_string()));
            env.define("pop".to_string(), Value::BuiltinFunction("pop".to_string()));
            env.define("type_of".to_string(), Value::BuiltinFunction("type_of".to_string()));
            env.define("to_string".to_string(), Value::BuiltinFunction("to_string".to_string()));
            env.define("to_int".to_string(), Value::BuiltinFunction("to_int".to_string()));
            env.define("to_float".to_string(), Value::BuiltinFunction("to_float".to_string()));
            env.define("filter".to_string(), Value::BuiltinFunction("filter".to_string()));
            env.define("map".to_string(), Value::BuiltinFunction("map".to_string()));
            env.define("reduce".to_string(), Value::BuiltinFunction("reduce".to_string()));
            env.define("sum".to_string(), Value::BuiltinFunction("sum".to_string()));
            env.define("random".to_string(), Value::BuiltinFunction("random".to_string()));
            env.define("range".to_string(), Value::BuiltinFunction("range".to_string()));
        }

        Interpreter { global }
    }

    pub fn run(&mut self, program: &Program) -> Result<Value, RuntimeError> {
        let mut result = Value::Null;

        for stmt in &program.statements {
            result = self.execute_stmt(stmt, self.global.clone())?;
        }

        Ok(result)
    }

    fn execute_stmt(&mut self, stmt: &Stmt, env: Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
        match stmt {
            Stmt::Expr(expr) => self.evaluate(expr, env),

            Stmt::Let { name, value } => {
                let val = if let Some(expr) = value {
                    self.evaluate(expr, env.clone())?
                } else {
                    Value::Null
                };
                env.borrow_mut().define(name.clone(), val);
                Ok(Value::Null)
            }

            Stmt::State { name, value } => {
                // Check if state already exists
                if env.borrow().get_state(name).is_none() {
                    let val = self.evaluate(value, env.clone())?;
                    env.borrow_mut().define_state(name.clone(), val);
                }
                Ok(Value::Null)
            }

            Stmt::Function { name, params, body } => {
                let func = Value::Function {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    closure: env.clone(),
                };
                env.borrow_mut().define(name.clone(), func);
                Ok(Value::Null)
            }

            Stmt::Return(expr) => {
                let value = if let Some(e) = expr {
                    self.evaluate(e, env)?
                } else {
                    Value::Null
                };
                Err(RuntimeError::Return(value))
            }

            Stmt::If { condition, then_branch, else_branch } => {
                let cond = self.evaluate(condition, env.clone())?;

                if cond.is_truthy() {
                    for s in then_branch {
                        self.execute_stmt(s, env.clone())?;
                    }
                } else if let Some(else_stmts) = else_branch {
                    for s in else_stmts {
                        self.execute_stmt(s, env.clone())?;
                    }
                }

                Ok(Value::Null)
            }

            Stmt::While { condition, body } => {
                loop {
                    let cond = self.evaluate(condition, env.clone())?;
                    if !cond.is_truthy() {
                        break;
                    }

                    for s in body {
                        match self.execute_stmt(s, env.clone()) {
                            Err(RuntimeError::Break) => return Ok(Value::Null),
                            Err(RuntimeError::Continue) => break,
                            Err(e) => return Err(e),
                            Ok(_) => {}
                        }
                    }
                }
                Ok(Value::Null)
            }

            Stmt::For { var, iter, body } => {
                let iterable = self.evaluate(iter, env.clone())?;

                let items = match iterable {
                    Value::Array(arr) => arr,
                    Value::Range { start, end, step } => {
                        let mut items = Vec::new();
                        let mut i = start;
                        if step > 0 {
                            while i < end {
                                items.push(Value::Integer(i));
                                i += step;
                            }
                        } else if step < 0 {
                            while i > end {
                                items.push(Value::Integer(i));
                                i += step;
                            }
                        }
                        items
                    }
                    _ => return Err(RuntimeError::Error("Cannot iterate over non-iterable".to_string())),
                };

                for item in items {
                    let loop_env = Rc::new(RefCell::new(Environment::with_parent(env.clone())));
                    loop_env.borrow_mut().define(var.clone(), item);

                    for s in body {
                        match self.execute_stmt(s, loop_env.clone()) {
                            Err(RuntimeError::Break) => return Ok(Value::Null),
                            Err(RuntimeError::Continue) => break,
                            Err(e) => return Err(e),
                            Ok(_) => {}
                        }
                    }
                }

                Ok(Value::Null)
            }

            Stmt::Loop { body } => {
                loop {
                    for s in body {
                        match self.execute_stmt(s, env.clone()) {
                            Err(RuntimeError::Break) => return Ok(Value::Null),
                            Err(RuntimeError::Continue) => break,
                            Err(e) => return Err(e),
                            Ok(_) => {}
                        }
                    }
                }
            }

            Stmt::Break => Err(RuntimeError::Break),
            Stmt::Continue => Err(RuntimeError::Continue),

            Stmt::Struct { name, fields: _ } => {
                // For now, structs are just registered as a type marker
                env.borrow_mut().define(name.clone(), Value::Symbol(format!("struct:{}", name)));
                Ok(Value::Null)
            }

            Stmt::Enum { name, variants } => {
                // Register enum variants
                for variant in variants {
                    let key = format!("{}::{}", name, variant.name);
                    if variant.fields.is_empty() {
                        env.borrow_mut().define(key, Value::EnumVariant {
                            enum_name: name.clone(),
                            variant: variant.name.clone(),
                            values: vec![],
                        });
                    }
                }
                Ok(Value::Null)
            }
        }
    }

    fn evaluate(&mut self, expr: &Expr, env: Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Integer(n) => Ok(Value::Integer(*n)),
            Expr::Float(n) => Ok(Value::Float(*n)),
            Expr::String(s) => Ok(Value::String(self.interpolate_string(s, env)?)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::Symbol(s) => Ok(Value::Symbol(s.clone())),

            Expr::Array(elements) => {
                let mut values = Vec::new();
                for e in elements {
                    values.push(self.evaluate(e, env.clone())?);
                }
                Ok(Value::Array(values))
            }

            Expr::Object(fields) => {
                let mut obj = HashMap::new();
                for (key, expr) in fields {
                    let value = self.evaluate(expr, env.clone())?;
                    obj.insert(key.clone(), value);
                }
                Ok(Value::Object(obj))
            }

            Expr::Identifier(name) => {
                env.borrow().get(name).ok_or_else(|| {
                    RuntimeError::Error(format!("Undefined variable: {}", name))
                })
            }

            Expr::BinaryOp { op, left, right } => {
                let left_val = self.evaluate(left, env.clone())?;
                let right_val = self.evaluate(right, env)?;
                self.apply_binary_op(*op, left_val, right_val)
            }

            Expr::UnaryOp { op, expr } => {
                let val = self.evaluate(expr, env)?;
                match op {
                    UnaryOp::Neg => match val {
                        Value::Integer(n) => Ok(Value::Integer(-n)),
                        Value::Float(n) => Ok(Value::Float(-n)),
                        _ => Err(RuntimeError::Error("Cannot negate non-number".to_string())),
                    },
                    UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
                }
            }

            Expr::Call { callee, args } => {
                let func = self.evaluate(callee, env.clone())?;
                let mut arg_values = Vec::new();
                for arg in args {
                    arg_values.push(self.evaluate(arg, env.clone())?);
                }
                self.call_function(func, arg_values, env)
            }

            Expr::MethodCall { object, method, args } => {
                let obj = self.evaluate(object, env.clone())?;
                let mut arg_values = Vec::new();
                for arg in args {
                    arg_values.push(self.evaluate(arg, env.clone())?);
                }
                self.call_method(obj, method, arg_values, env)
            }

            Expr::PropertyAccess { object, property } => {
                let obj = self.evaluate(object, env)?;
                match obj {
                    Value::Object(map) => {
                        map.get(property).cloned().ok_or_else(|| {
                            RuntimeError::Error(format!("Property '{}' not found", property))
                        })
                    }
                    Value::Array(arr) if property == "length" => {
                        Ok(Value::Integer(arr.len() as i64))
                    }
                    Value::String(s) if property == "length" => {
                        Ok(Value::Integer(s.len() as i64))
                    }
                    _ => Err(RuntimeError::Error(format!("Cannot access property '{}' on {:?}", property, obj))),
                }
            }

            Expr::IndexAccess { object, index } => {
                let obj = self.evaluate(object, env.clone())?;
                let idx = self.evaluate(index, env)?;

                match (&obj, &idx) {
                    (Value::Array(arr), Value::Integer(i)) => {
                        let index = if *i < 0 {
                            (arr.len() as i64 + i) as usize
                        } else {
                            *i as usize
                        };
                        arr.get(index).cloned().ok_or_else(|| {
                            RuntimeError::Error(format!("Index {} out of bounds", i))
                        })
                    }
                    (Value::Object(map), Value::String(key)) => {
                        map.get(key).cloned().ok_or_else(|| {
                            RuntimeError::Error(format!("Key '{}' not found", key))
                        })
                    }
                    (Value::String(s), Value::Integer(i)) => {
                        let index = if *i < 0 {
                            (s.len() as i64 + i) as usize
                        } else {
                            *i as usize
                        };
                        s.chars().nth(index)
                            .map(|c| Value::String(c.to_string()))
                            .ok_or_else(|| RuntimeError::Error(format!("Index {} out of bounds", i)))
                    }
                    _ => Err(RuntimeError::Error("Invalid index operation".to_string())),
                }
            }

            Expr::Lambda { params, body } => {
                Ok(Value::Lambda {
                    params: params.clone(),
                    body: body.clone(),
                    closure: env,
                })
            }

            Expr::Block(stmts) => {
                let block_env = Rc::new(RefCell::new(Environment::with_parent(env)));
                let mut result = Value::Null;

                for stmt in stmts {
                    match self.execute_stmt(stmt, block_env.clone()) {
                        Ok(val) => result = val,
                        Err(RuntimeError::Return(val)) => return Err(RuntimeError::Return(val)),
                        Err(e) => return Err(e),
                    }
                }

                // If the last statement is an expression, return it
                if let Some(Stmt::Expr(e)) = stmts.last() {
                    result = self.evaluate(e, block_env)?;
                }

                Ok(result)
            }

            Expr::If { condition, then_branch, else_branch } => {
                let cond = self.evaluate(condition, env.clone())?;

                if cond.is_truthy() {
                    self.evaluate(then_branch, env)
                } else if let Some(else_expr) = else_branch {
                    self.evaluate(else_expr, env)
                } else {
                    Ok(Value::Null)
                }
            }

            Expr::Match { value, arms } => {
                let val = self.evaluate(value, env.clone())?;

                for arm in arms {
                    let match_env = Rc::new(RefCell::new(Environment::with_parent(env.clone())));

                    if self.match_pattern(&arm.pattern, &val, match_env.clone())? {
                        // Check guard if present
                        if let Some(guard) = &arm.guard {
                            let guard_result = self.evaluate(guard, match_env.clone())?;
                            if !guard_result.is_truthy() {
                                continue;
                            }
                        }
                        return self.evaluate(&arm.body, match_env);
                    }
                }

                Err(RuntimeError::Error("No matching pattern".to_string()))
            }

            Expr::Dataflow { left, right } => {
                let left_val = self.evaluate(left, env.clone())?;

                // Handle object merge: value @ { field: x }
                if let Expr::Object(fields) = right.as_ref() {
                    if let Value::Object(mut obj) = left_val {
                        for (key, expr) in fields {
                            let value = self.evaluate(expr, env.clone())?;
                            obj.insert(key.clone(), value);
                        }
                        return Ok(Value::Object(obj));
                    }
                }

                // Handle function call: value @ function(args)
                if let Expr::Call { callee, args } = right.as_ref() {
                    let func = self.evaluate(callee, env.clone())?;
                    let mut arg_values = vec![left_val];
                    for arg in args {
                        arg_values.push(self.evaluate(arg, env.clone())?);
                    }
                    return self.call_function(func, arg_values, env);
                }

                // Handle method-like call: value @ method()
                if let Expr::Identifier(name) = right.as_ref() {
                    // Treat as a function call with left as first argument
                    let func = env.borrow().get(name);
                    if let Some(func) = func {
                        return self.call_function(func, vec![left_val], env);
                    }
                }

                Err(RuntimeError::Error("Invalid dataflow expression".to_string()))
            }

            Expr::Assign { target, value } => {
                let val = self.evaluate(value, env.clone())?;

                match target.as_ref() {
                    Expr::Identifier(name) => {
                        if !env.borrow_mut().set(name, val.clone()) {
                            return Err(RuntimeError::Error(format!("Undefined variable: {}", name)));
                        }
                        Ok(val)
                    }
                    Expr::IndexAccess { object, index } => {
                        let obj = self.evaluate(object, env.clone())?;
                        let idx = self.evaluate(index, env.clone())?;

                        match (object.as_ref(), &obj, &idx) {
                            (Expr::Identifier(name), Value::Array(arr), Value::Integer(i)) => {
                                let mut arr = arr.clone();
                                let index = if *i < 0 {
                                    (arr.len() as i64 + i) as usize
                                } else {
                                    *i as usize
                                };
                                if index < arr.len() {
                                    arr[index] = val.clone();
                                    env.borrow_mut().set(name, Value::Array(arr));
                                }
                                Ok(val)
                            }
                            (Expr::Identifier(name), Value::Object(map), Value::String(key)) => {
                                let mut map = map.clone();
                                map.insert(key.clone(), val.clone());
                                env.borrow_mut().set(name, Value::Object(map));
                                Ok(val)
                            }
                            _ => Err(RuntimeError::Error("Invalid assignment target".to_string())),
                        }
                    }
                    Expr::PropertyAccess { object, property } => {
                        let obj = self.evaluate(object, env.clone())?;

                        match (object.as_ref(), obj) {
                            (Expr::Identifier(name), Value::Object(mut map)) => {
                                map.insert(property.clone(), val.clone());
                                env.borrow_mut().set(name, Value::Object(map));
                                Ok(val)
                            }
                            _ => Err(RuntimeError::Error("Invalid property assignment".to_string())),
                        }
                    }
                    _ => Err(RuntimeError::Error("Invalid assignment target".to_string())),
                }
            }

            Expr::CompoundAssign { op, target, value } => {
                let current = self.evaluate(target, env.clone())?;
                let operand = self.evaluate(value, env.clone())?;
                let result = self.apply_binary_op(*op, current, operand)?;

                // Reuse assignment logic
                let assign = Expr::Assign {
                    target: target.clone(),
                    value: Box::new(match &result {
                        Value::Integer(n) => Expr::Integer(*n),
                        Value::Float(n) => Expr::Float(*n),
                        Value::String(s) => Expr::String(s.clone()),
                        Value::Bool(b) => Expr::Bool(*b),
                        _ => return Err(RuntimeError::Error("Cannot assign compound result".to_string())),
                    }),
                };

                // Actually set the value directly
                match target.as_ref() {
                    Expr::Identifier(name) => {
                        if !env.borrow_mut().set(name, result.clone()) {
                            return Err(RuntimeError::Error(format!("Undefined variable: {}", name)));
                        }
                        Ok(result)
                    }
                    _ => self.evaluate(&assign, env),
                }
            }

            Expr::ForExpr { var, iter, body } => {
                let iterable = self.evaluate(iter, env.clone())?;

                let items = match iterable {
                    Value::Array(arr) => arr,
                    Value::Range { start, end, step } => {
                        let mut items = Vec::new();
                        let mut i = start;
                        if step > 0 {
                            while i < end {
                                items.push(Value::Integer(i));
                                i += step;
                            }
                        } else if step < 0 {
                            while i > end {
                                items.push(Value::Integer(i));
                                i += step;
                            }
                        }
                        items
                    }
                    _ => return Err(RuntimeError::Error("Cannot iterate over non-iterable".to_string())),
                };

                let mut results = Vec::new();
                for item in items {
                    let loop_env = Rc::new(RefCell::new(Environment::with_parent(env.clone())));
                    loop_env.borrow_mut().define(var.clone(), item);
                    let result = self.evaluate(body, loop_env)?;
                    results.push(result);
                }

                Ok(Value::Array(results))
            }

            Expr::Range { start, end, step } => {
                let start_val = match self.evaluate(start, env.clone())? {
                    Value::Integer(n) => n,
                    _ => return Err(RuntimeError::Error("Range start must be an integer".to_string())),
                };
                let end_val = match self.evaluate(end, env.clone())? {
                    Value::Integer(n) => n,
                    _ => return Err(RuntimeError::Error("Range end must be an integer".to_string())),
                };
                let step_val = if let Some(s) = step {
                    match self.evaluate(s, env)? {
                        Value::Integer(n) => n,
                        _ => return Err(RuntimeError::Error("Range step must be an integer".to_string())),
                    }
                } else {
                    1
                };

                Ok(Value::Range {
                    start: start_val,
                    end: end_val,
                    step: step_val,
                })
            }

            Expr::EnumVariant { enum_name, variant, args } => {
                let values = if let Some(args) = args {
                    let mut vals = Vec::new();
                    for arg in args {
                        vals.push(self.evaluate(arg, env.clone())?);
                    }
                    vals
                } else {
                    vec![]
                };

                Ok(Value::EnumVariant {
                    enum_name: enum_name.clone(),
                    variant: variant.clone(),
                    values,
                })
            }
        }
    }

    fn interpolate_string(&mut self, s: &str, env: Rc<RefCell<Environment>>) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' && chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut expr_str = String::new();
                let mut brace_count = 1;

                while let Some(c) = chars.next() {
                    if c == '{' {
                        brace_count += 1;
                        expr_str.push(c);
                    } else if c == '}' {
                        brace_count -= 1;
                        if brace_count == 0 {
                            break;
                        }
                        expr_str.push(c);
                    } else {
                        expr_str.push(c);
                    }
                }

                // Parse and evaluate the expression
                let mut lexer = crate::lexer::Lexer::new(&expr_str);
                let tokens = lexer.tokenize();
                let mut parser = crate::parser::Parser::new(tokens);

                // Try to parse as expression
                match parser.parse() {
                    Ok(program) => {
                        if let Some(Stmt::Expr(expr)) = program.statements.first() {
                            match self.evaluate(expr, env.clone()) {
                                Ok(val) => result.push_str(&format!("{}", val)),
                                Err(_) => result.push_str(&format!("${{{}}}", expr_str)),
                            }
                        }
                    }
                    Err(_) => result.push_str(&format!("${{{}}}", expr_str)),
                }
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }

    fn apply_binary_op(&self, op: BinaryOp, left: Value, right: Value) -> Result<Value, RuntimeError> {
        match (op, &left, &right) {
            // Arithmetic operations
            (BinaryOp::Add, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
            (BinaryOp::Add, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (BinaryOp::Add, Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (BinaryOp::Add, Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a + *b as f64)),
            (BinaryOp::Add, Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
            (BinaryOp::Add, Value::String(a), b) => Ok(Value::String(format!("{}{}", a, b))),
            (BinaryOp::Add, a, Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),

            (BinaryOp::Sub, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a - b)),
            (BinaryOp::Sub, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (BinaryOp::Sub, Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
            (BinaryOp::Sub, Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a - *b as f64)),

            (BinaryOp::Mul, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a * b)),
            (BinaryOp::Mul, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (BinaryOp::Mul, Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
            (BinaryOp::Mul, Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a * *b as f64)),

            (BinaryOp::Div, Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    Err(RuntimeError::Error("Division by zero".to_string()))
                } else {
                    Ok(Value::Integer(a / b))
                }
            }
            (BinaryOp::Div, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (BinaryOp::Div, Value::Integer(a), Value::Float(b)) => Ok(Value::Float(*a as f64 / b)),
            (BinaryOp::Div, Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a / *b as f64)),

            (BinaryOp::Mod, Value::Integer(a), Value::Integer(b)) => {
                if *b == 0 {
                    Err(RuntimeError::Error("Modulo by zero".to_string()))
                } else {
                    Ok(Value::Integer(a % b))
                }
            }
            (BinaryOp::Mod, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),

            (BinaryOp::Pow, Value::Integer(a), Value::Integer(b)) => {
                Ok(Value::Integer((*a as f64).powf(*b as f64) as i64))
            }
            (BinaryOp::Pow, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(*b))),
            (BinaryOp::Pow, Value::Integer(a), Value::Float(b)) => Ok(Value::Float((*a as f64).powf(*b))),
            (BinaryOp::Pow, Value::Float(a), Value::Integer(b)) => Ok(Value::Float(a.powf(*b as f64))),

            // Comparison operations
            (BinaryOp::Eq, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a == b)),
            (BinaryOp::Eq, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a == b)),
            (BinaryOp::Eq, Value::String(a), Value::String(b)) => Ok(Value::Bool(a == b)),
            (BinaryOp::Eq, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
            (BinaryOp::Eq, Value::Null, Value::Null) => Ok(Value::Bool(true)),
            (BinaryOp::Eq, Value::Symbol(a), Value::Symbol(b)) => Ok(Value::Bool(a == b)),
            (BinaryOp::Eq, _, _) => Ok(Value::Bool(false)),

            (BinaryOp::Ne, _, _) => {
                let eq = self.apply_binary_op(BinaryOp::Eq, left, right)?;
                match eq {
                    Value::Bool(b) => Ok(Value::Bool(!b)),
                    _ => Ok(Value::Bool(true)),
                }
            }

            (BinaryOp::Lt, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a < b)),
            (BinaryOp::Lt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
            (BinaryOp::Lt, Value::Integer(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) < *b)),
            (BinaryOp::Lt, Value::Float(a), Value::Integer(b)) => Ok(Value::Bool(*a < (*b as f64))),

            (BinaryOp::Le, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a <= b)),
            (BinaryOp::Le, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
            (BinaryOp::Le, Value::Integer(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) <= *b)),
            (BinaryOp::Le, Value::Float(a), Value::Integer(b)) => Ok(Value::Bool(*a <= (*b as f64))),

            (BinaryOp::Gt, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a > b)),
            (BinaryOp::Gt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
            (BinaryOp::Gt, Value::Integer(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) > *b)),
            (BinaryOp::Gt, Value::Float(a), Value::Integer(b)) => Ok(Value::Bool(*a > (*b as f64))),

            (BinaryOp::Ge, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a >= b)),
            (BinaryOp::Ge, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
            (BinaryOp::Ge, Value::Integer(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) >= *b)),
            (BinaryOp::Ge, Value::Float(a), Value::Integer(b)) => Ok(Value::Bool(*a >= (*b as f64))),

            // Logical operations
            (BinaryOp::And, _, _) => Ok(Value::Bool(left.is_truthy() && right.is_truthy())),
            (BinaryOp::Or, _, _) => Ok(Value::Bool(left.is_truthy() || right.is_truthy())),

            _ => Err(RuntimeError::Error(format!(
                "Invalid binary operation {:?} on {:?} and {:?}",
                op, left, right
            ))),
        }
    }

    fn call_function(&mut self, func: Value, args: Vec<Value>, env: Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
        match func {
            Value::Function { params, body, closure, .. } => {
                let func_env = Rc::new(RefCell::new(Environment::with_parent(closure)));

                for (i, param) in params.iter().enumerate() {
                    let arg = args.get(i).cloned().unwrap_or(Value::Null);
                    func_env.borrow_mut().define(param.clone(), arg);
                }

                let mut result = Value::Null;
                for stmt in &body {
                    match self.execute_stmt(stmt, func_env.clone()) {
                        Ok(val) => result = val,
                        Err(RuntimeError::Return(val)) => return Ok(val),
                        Err(e) => return Err(e),
                    }
                }

                Ok(result)
            }

            Value::Lambda { params, body, closure } => {
                let func_env = Rc::new(RefCell::new(Environment::with_parent(closure)));

                for (i, param) in params.iter().enumerate() {
                    let arg = args.get(i).cloned().unwrap_or(Value::Null);
                    func_env.borrow_mut().define(param.clone(), arg);
                }

                self.evaluate(&body, func_env)
            }

            Value::BuiltinFunction(name) => self.call_builtin(&name, args, env),

            _ => Err(RuntimeError::Error(format!("Cannot call non-function: {:?}", func))),
        }
    }

    fn call_method(&mut self, obj: Value, method: &str, args: Vec<Value>, _env: Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
        match (&obj, method) {
            (Value::Array(arr), "length") => Ok(Value::Integer(arr.len() as i64)),
            (Value::Array(arr), "push") => {
                let mut new_arr = arr.clone();
                for arg in args {
                    new_arr.push(arg);
                }
                Ok(Value::Array(new_arr))
            }
            (Value::Array(arr), "pop") => {
                let mut new_arr = arr.clone();
                new_arr.pop();
                Ok(Value::Array(new_arr))
            }
            (Value::Array(arr), "filter") => {
                if let Some(Value::Lambda { params, body, closure }) = args.first() {
                    let mut result = Vec::new();
                    for item in arr {
                        let func_env = Rc::new(RefCell::new(Environment::with_parent(closure.clone())));
                        if let Some(param) = params.first() {
                            func_env.borrow_mut().define(param.clone(), item.clone());
                        }
                        let predicate = self.evaluate(body, func_env)?;
                        if predicate.is_truthy() {
                            result.push(item.clone());
                        }
                    }
                    Ok(Value::Array(result))
                } else {
                    Err(RuntimeError::Error("filter requires a function argument".to_string()))
                }
            }
            (Value::Array(arr), "map") => {
                if let Some(Value::Lambda { params, body, closure }) = args.first() {
                    let mut result = Vec::new();
                    for item in arr {
                        let func_env = Rc::new(RefCell::new(Environment::with_parent(closure.clone())));
                        if let Some(param) = params.first() {
                            func_env.borrow_mut().define(param.clone(), item.clone());
                        }
                        let mapped = self.evaluate(body, func_env)?;
                        result.push(mapped);
                    }
                    Ok(Value::Array(result))
                } else {
                    Err(RuntimeError::Error("map requires a function argument".to_string()))
                }
            }
            (Value::String(s), "length") => Ok(Value::Integer(s.len() as i64)),
            (Value::String(s), "to_uppercase") => Ok(Value::String(s.to_uppercase())),
            (Value::String(s), "to_lowercase") => Ok(Value::String(s.to_lowercase())),
            (Value::String(s), "trim") => Ok(Value::String(s.trim().to_string())),
            (Value::String(s), "split") => {
                if let Some(Value::String(sep)) = args.first() {
                    let parts: Vec<Value> = s.split(sep).map(|p| Value::String(p.to_string())).collect();
                    Ok(Value::Array(parts))
                } else {
                    Err(RuntimeError::Error("split requires a string argument".to_string()))
                }
            }
            (Value::String(s), "contains") => {
                if let Some(Value::String(substr)) = args.first() {
                    Ok(Value::Bool(s.contains(substr.as_str())))
                } else {
                    Err(RuntimeError::Error("contains requires a string argument".to_string()))
                }
            }
            (Value::String(s), "starts_with") => {
                if let Some(Value::String(prefix)) = args.first() {
                    Ok(Value::Bool(s.starts_with(prefix.as_str())))
                } else {
                    Err(RuntimeError::Error("starts_with requires a string argument".to_string()))
                }
            }
            (Value::String(s), "ends_with") => {
                if let Some(Value::String(suffix)) = args.first() {
                    Ok(Value::Bool(s.ends_with(suffix.as_str())))
                } else {
                    Err(RuntimeError::Error("ends_with requires a string argument".to_string()))
                }
            }
            _ => Err(RuntimeError::Error(format!("Unknown method '{}' on {:?}", method, obj))),
        }
    }

    fn call_builtin(&mut self, name: &str, args: Vec<Value>, env: Rc<RefCell<Environment>>) -> Result<Value, RuntimeError> {
        match name {
            "print" => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        print!(" ");
                    }
                    print!("{}", arg);
                }
                Ok(Value::Null)
            }
            "println" => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        print!(" ");
                    }
                    print!("{}", arg);
                }
                println!();
                Ok(Value::Null)
            }
            "sqrt" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Float((*n as f64).sqrt())),
                    Some(Value::Float(n)) => Ok(Value::Float(n.sqrt())),
                    _ => Err(RuntimeError::Error("sqrt requires a number".to_string())),
                }
            }
            "sin" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Float((*n as f64).sin())),
                    Some(Value::Float(n)) => Ok(Value::Float(n.sin())),
                    _ => Err(RuntimeError::Error("sin requires a number".to_string())),
                }
            }
            "cos" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Float((*n as f64).cos())),
                    Some(Value::Float(n)) => Ok(Value::Float(n.cos())),
                    _ => Err(RuntimeError::Error("cos requires a number".to_string())),
                }
            }
            "abs" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Integer(n.abs())),
                    Some(Value::Float(n)) => Ok(Value::Float(n.abs())),
                    _ => Err(RuntimeError::Error("abs requires a number".to_string())),
                }
            }
            "floor" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Integer(*n)),
                    Some(Value::Float(n)) => Ok(Value::Integer(n.floor() as i64)),
                    _ => Err(RuntimeError::Error("floor requires a number".to_string())),
                }
            }
            "ceil" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Integer(*n)),
                    Some(Value::Float(n)) => Ok(Value::Integer(n.ceil() as i64)),
                    _ => Err(RuntimeError::Error("ceil requires a number".to_string())),
                }
            }
            "round" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Integer(*n)),
                    Some(Value::Float(n)) => Ok(Value::Integer(n.round() as i64)),
                    _ => Err(RuntimeError::Error("round requires a number".to_string())),
                }
            }
            "min" => {
                match (args.get(0), args.get(1)) {
                    (Some(Value::Integer(a)), Some(Value::Integer(b))) => Ok(Value::Integer(*a.min(b))),
                    (Some(Value::Float(a)), Some(Value::Float(b))) => Ok(Value::Float(a.min(*b))),
                    (Some(Value::Integer(a)), Some(Value::Float(b))) => Ok(Value::Float((*a as f64).min(*b))),
                    (Some(Value::Float(a)), Some(Value::Integer(b))) => Ok(Value::Float(a.min(*b as f64))),
                    _ => Err(RuntimeError::Error("min requires two numbers".to_string())),
                }
            }
            "max" => {
                match (args.get(0), args.get(1)) {
                    (Some(Value::Integer(a)), Some(Value::Integer(b))) => Ok(Value::Integer(*a.max(b))),
                    (Some(Value::Float(a)), Some(Value::Float(b))) => Ok(Value::Float(a.max(*b))),
                    (Some(Value::Integer(a)), Some(Value::Float(b))) => Ok(Value::Float((*a as f64).max(*b))),
                    (Some(Value::Float(a)), Some(Value::Integer(b))) => Ok(Value::Float(a.max(*b as f64))),
                    _ => Err(RuntimeError::Error("max requires two numbers".to_string())),
                }
            }
            "pow" => {
                match (args.get(0), args.get(1)) {
                    (Some(Value::Integer(base)), Some(Value::Integer(exp))) => {
                        Ok(Value::Integer((*base as f64).powf(*exp as f64) as i64))
                    }
                    (Some(Value::Float(base)), Some(Value::Float(exp))) => {
                        Ok(Value::Float(base.powf(*exp)))
                    }
                    (Some(Value::Integer(base)), Some(Value::Float(exp))) => {
                        Ok(Value::Float((*base as f64).powf(*exp)))
                    }
                    (Some(Value::Float(base)), Some(Value::Integer(exp))) => {
                        Ok(Value::Float(base.powf(*exp as f64)))
                    }
                    _ => Err(RuntimeError::Error("pow requires two numbers".to_string())),
                }
            }
            "len" => {
                match args.first() {
                    Some(Value::Array(arr)) => Ok(Value::Integer(arr.len() as i64)),
                    Some(Value::String(s)) => Ok(Value::Integer(s.len() as i64)),
                    Some(Value::Object(obj)) => Ok(Value::Integer(obj.len() as i64)),
                    _ => Err(RuntimeError::Error("len requires an array, string, or object".to_string())),
                }
            }
            "push" => {
                match args.first() {
                    Some(Value::Array(arr)) => {
                        let mut new_arr = arr.clone();
                        for arg in args.iter().skip(1) {
                            new_arr.push(arg.clone());
                        }
                        Ok(Value::Array(new_arr))
                    }
                    _ => Err(RuntimeError::Error("push requires an array".to_string())),
                }
            }
            "pop" => {
                match args.first() {
                    Some(Value::Array(arr)) => {
                        let mut new_arr = arr.clone();
                        new_arr.pop();
                        Ok(Value::Array(new_arr))
                    }
                    _ => Err(RuntimeError::Error("pop requires an array".to_string())),
                }
            }
            "type_of" => {
                let type_name = match args.first() {
                    Some(Value::Integer(_)) => "integer",
                    Some(Value::Float(_)) => "float",
                    Some(Value::String(_)) => "string",
                    Some(Value::Bool(_)) => "bool",
                    Some(Value::Null) => "null",
                    Some(Value::Symbol(_)) => "symbol",
                    Some(Value::Array(_)) => "array",
                    Some(Value::Object(_)) => "object",
                    Some(Value::Function { .. }) => "function",
                    Some(Value::Lambda { .. }) => "function",
                    Some(Value::BuiltinFunction(_)) => "function",
                    Some(Value::Range { .. }) => "range",
                    Some(Value::EnumVariant { .. }) => "enum",
                    None => "null",
                };
                Ok(Value::String(type_name.to_string()))
            }
            "to_string" => {
                match args.first() {
                    Some(v) => Ok(Value::String(format!("{}", v))),
                    None => Ok(Value::String("null".to_string())),
                }
            }
            "to_int" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Integer(*n)),
                    Some(Value::Float(n)) => Ok(Value::Integer(*n as i64)),
                    Some(Value::String(s)) => {
                        s.parse::<i64>()
                            .map(Value::Integer)
                            .map_err(|_| RuntimeError::Error("Cannot convert string to int".to_string()))
                    }
                    Some(Value::Bool(b)) => Ok(Value::Integer(if *b { 1 } else { 0 })),
                    _ => Err(RuntimeError::Error("Cannot convert to int".to_string())),
                }
            }
            "to_float" => {
                match args.first() {
                    Some(Value::Integer(n)) => Ok(Value::Float(*n as f64)),
                    Some(Value::Float(n)) => Ok(Value::Float(*n)),
                    Some(Value::String(s)) => {
                        s.parse::<f64>()
                            .map(Value::Float)
                            .map_err(|_| RuntimeError::Error("Cannot convert string to float".to_string()))
                    }
                    _ => Err(RuntimeError::Error("Cannot convert to float".to_string())),
                }
            }
            "filter" => {
                match (args.get(0), args.get(1)) {
                    (Some(Value::Array(arr)), Some(func)) => {
                        let mut result = Vec::new();
                        for item in arr {
                            let predicate = self.call_function(func.clone(), vec![item.clone()], env.clone())?;
                            if predicate.is_truthy() {
                                result.push(item.clone());
                            }
                        }
                        Ok(Value::Array(result))
                    }
                    _ => Err(RuntimeError::Error("filter requires an array and a function".to_string())),
                }
            }
            "map" => {
                match (args.get(0), args.get(1)) {
                    (Some(Value::Array(arr)), Some(func)) => {
                        let mut result = Vec::new();
                        for item in arr {
                            let mapped = self.call_function(func.clone(), vec![item.clone()], env.clone())?;
                            result.push(mapped);
                        }
                        Ok(Value::Array(result))
                    }
                    _ => Err(RuntimeError::Error("map requires an array and a function".to_string())),
                }
            }
            "reduce" => {
                match (args.get(0), args.get(1), args.get(2)) {
                    (Some(Value::Array(arr)), Some(func), Some(initial)) => {
                        let mut acc = initial.clone();
                        for item in arr {
                            acc = self.call_function(func.clone(), vec![acc, item.clone()], env.clone())?;
                        }
                        Ok(acc)
                    }
                    _ => Err(RuntimeError::Error("reduce requires an array, a function, and an initial value".to_string())),
                }
            }
            "sum" => {
                match args.first() {
                    Some(Value::Array(arr)) => {
                        let mut sum = 0i64;
                        let mut is_float = false;
                        let mut sum_f = 0.0f64;

                        for item in arr {
                            match item {
                                Value::Integer(n) => {
                                    if is_float {
                                        sum_f += *n as f64;
                                    } else {
                                        sum += n;
                                    }
                                }
                                Value::Float(n) => {
                                    if !is_float {
                                        is_float = true;
                                        sum_f = sum as f64;
                                    }
                                    sum_f += n;
                                }
                                _ => return Err(RuntimeError::Error("sum requires an array of numbers".to_string())),
                            }
                        }

                        if is_float {
                            Ok(Value::Float(sum_f))
                        } else {
                            Ok(Value::Integer(sum))
                        }
                    }
                    _ => Err(RuntimeError::Error("sum requires an array".to_string())),
                }
            }
            "random" => {
                use std::time::{SystemTime, UNIX_EPOCH};
                let seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;

                // Simple LCG random
                let random = ((seed.wrapping_mul(1103515245).wrapping_add(12345)) >> 16) as f64 / 32768.0;

                match (args.get(0), args.get(1)) {
                    (Some(Value::Integer(min)), Some(Value::Integer(max))) => {
                        let range = max - min;
                        Ok(Value::Integer(min + (random * range as f64) as i64))
                    }
                    (Some(Value::Float(min)), Some(Value::Float(max))) => {
                        let range = max - min;
                        Ok(Value::Float(min + random * range))
                    }
                    (None, None) => Ok(Value::Float(random)),
                    _ => Err(RuntimeError::Error("random requires two numbers or no arguments".to_string())),
                }
            }
            "range" => {
                match (args.get(0), args.get(1), args.get(2)) {
                    (Some(Value::Integer(start)), Some(Value::Integer(end)), step) => {
                        let step_val = match step {
                            Some(Value::Integer(s)) => *s,
                            None => 1,
                            _ => return Err(RuntimeError::Error("range step must be an integer".to_string())),
                        };
                        Ok(Value::Range {
                            start: *start,
                            end: *end,
                            step: step_val,
                        })
                    }
                    _ => Err(RuntimeError::Error("range requires integer arguments".to_string())),
                }
            }
            _ => Err(RuntimeError::Error(format!("Unknown builtin function: {}", name))),
        }
    }

    fn match_pattern(&self, pattern: &Pattern, value: &Value, env: Rc<RefCell<Environment>>) -> Result<bool, RuntimeError> {
        match pattern {
            Pattern::Wildcard => Ok(true),

            Pattern::Variable(name) => {
                env.borrow_mut().define(name.clone(), value.clone());
                Ok(true)
            }

            Pattern::Literal(literal) => {
                match (literal, value) {
                    (Expr::Integer(a), Value::Integer(b)) => Ok(a == b),
                    (Expr::Float(a), Value::Float(b)) => Ok(a == b),
                    (Expr::String(a), Value::String(b)) => Ok(a == b),
                    (Expr::Bool(a), Value::Bool(b)) => Ok(a == b),
                    (Expr::Null, Value::Null) => Ok(true),
                    (Expr::Symbol(a), Value::Symbol(b)) => Ok(a == b),
                    _ => Ok(false),
                }
            }

            Pattern::Array(patterns) => {
                if let Value::Array(values) = value {
                    if patterns.len() != values.len() {
                        return Ok(false);
                    }
                    for (p, v) in patterns.iter().zip(values.iter()) {
                        if !self.match_pattern(p, v, env.clone())? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }

            Pattern::Object(fields) => {
                if let Value::Object(obj) = value {
                    for (key, pattern) in fields {
                        if let Some(val) = obj.get(key) {
                            if !self.match_pattern(pattern, val, env.clone())? {
                                return Ok(false);
                            }
                        } else {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }
}

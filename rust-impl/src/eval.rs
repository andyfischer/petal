use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::ast::*;
use crate::env_scope::Environment;
use crate::value::Value;

/// Control flow signals propagated via the error channel.
/// Return and Break propagate up through ? until caught at the right boundary.
#[derive(Debug)]
pub enum Signal {
    Error(String),
    Return(Value),
    Break,
}

impl From<String> for Signal {
    fn from(s: String) -> Self {
        Signal::Error(s)
    }
}

impl From<&str> for Signal {
    fn from(s: &str) -> Self {
        Signal::Error(s.to_string())
    }
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Signal::Error(msg) => write!(f, "{}", msg),
            Signal::Return(_) => write!(f, "return outside of function"),
            Signal::Break => write!(f, "break outside of loop"),
        }
    }
}

type Res = Result<Value, Signal>;

pub struct Interpreter {
    global_env: Environment,
    enum_variants: HashMap<String, String>,
    state_store: HashMap<usize, Value>,
    state_initialized: HashMap<usize, bool>,
    active_state_names: Vec<(String, usize)>,
    pub output: Vec<String>,
    capture_output: bool,
}

impl Interpreter {
    pub fn new() -> Self {
        let env = Environment::new();
        let mut interp = Self {
            global_env: env,
            enum_variants: HashMap::new(),
            state_store: HashMap::new(),
            state_initialized: HashMap::new(),
            active_state_names: Vec::new(),
            output: Vec::new(),
            capture_output: false,
        };
        interp.register_builtins();
        interp
    }

    fn register_builtins(&mut self) {
        let builtins = [
            "print", "range", "len", "push", "str", "abs", "sqrt", "floor", "ceil", "float",
            "int", "random", "type", "append", "pop", "keys", "values", "contains",
            "min", "max", "round",
        ];
        for name in builtins {
            self.global_env
                .set(name, Value::BuiltinFunction(name.to_string()));
        }
    }

    pub fn run(&mut self, program: &[Stmt]) -> Result<Value, String> {
        let env = self.global_env.clone();
        match self.exec_stmts(program, &env) {
            Ok(v) => Ok(v),
            Err(Signal::Error(e)) => Err(e),
            Err(Signal::Return(v)) => Ok(v),
            Err(Signal::Break) => Err("break outside of loop".to_string()),
        }
    }

    fn exec_stmts(&mut self, stmts: &[Stmt], env: &Environment) -> Res {
        let mut last = Value::Nil;
        for stmt in stmts {
            last = self.exec_stmt(stmt, env)?;
        }
        Ok(last)
    }

    fn exec_stmt(&mut self, stmt: &Stmt, env: &Environment) -> Res {
        match stmt {
            Stmt::Let { name, value } => {
                let val = self.eval_expr(value, env)?;
                env.set(name, val);
                Ok(Value::Nil)
            }
            Stmt::Assign { target, value } => {
                let val = self.eval_expr(value, env)?;
                self.exec_assign(target, val, env)?;
                Ok(Value::Nil)
            }
            Stmt::Expr(expr) => self.eval_expr(expr, env),
            Stmt::FnDecl { name, params, body } => {
                let func = Value::Function {
                    name: name.clone(),
                    params: params.clone(),
                    body: body.clone(),
                    closure: env.clone(),
                };
                env.set(name, func);
                Ok(Value::Nil)
            }
            Stmt::EnumDecl { name: _, variants } => {
                for variant in variants {
                    self.enum_variants
                        .insert(variant.name.clone(), variant.name.clone());
                    if variant.fields.is_empty() {
                        env.set(
                            &variant.name,
                            Value::EnumVariant {
                                name: variant.name.clone(),
                                data: Vec::new(),
                            },
                        );
                    } else {
                        env.set(
                            &variant.name,
                            Value::BuiltinFunction(format!("__enum__{}", variant.name)),
                        );
                    }
                }
                Ok(Value::Nil)
            }
            Stmt::For { var, iter, body } => {
                let iter_val = self.eval_expr(iter, env)?;
                let items = match &iter_val {
                    Value::List(list) => list.borrow().clone(),
                    _ => return Err(Signal::Error(format!(
                        "Cannot iterate over {}",
                        iter_val.type_name()
                    ))),
                };
                for item in items {
                    let loop_env = Environment::with_parent(env);
                    loop_env.set(var, item);
                    match self.exec_stmts(body, &loop_env) {
                        Ok(_) => {}
                        Err(Signal::Break) => break,
                        Err(e) => return Err(e), // propagate Return and Error
                    }
                }
                Ok(Value::Nil)
            }
            Stmt::While { condition, body } => {
                loop {
                    let cond = self.eval_expr(condition, env)?;
                    if !cond.is_truthy() {
                        break;
                    }
                    let loop_env = Environment::with_parent(env);
                    match self.exec_stmts(body, &loop_env) {
                        Ok(_) => {}
                        Err(Signal::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Nil)
            }
            Stmt::Return(expr) => {
                let val = if let Some(e) = expr {
                    self.eval_expr(e, env)?
                } else {
                    Value::Nil
                };
                Err(Signal::Return(val))
            }
            Stmt::Break => Err(Signal::Break),
            Stmt::State { name, init, id } => {
                if !self.state_initialized.get(id).copied().unwrap_or(false) {
                    let val = self.eval_expr(init, env)?;
                    self.state_store.insert(*id, val.clone());
                    self.state_initialized.insert(*id, true);
                    env.set(name, val);
                } else if let Some(val) = self.state_store.get(id) {
                    env.set(name, val.clone());
                }
                self.active_state_names.push((name.clone(), *id));
                Ok(Value::Nil)
            }
        }
    }

    fn exec_assign(&mut self, target: &AssignTarget, value: Value, env: &Environment) -> Res {
        match target {
            AssignTarget::Name(name) => {
                self.update_state_if_needed(name, &value);
                if !env.assign(name, value.clone()) {
                    env.set(name, value);
                }
            }
            AssignTarget::Field(object, field) => {
                let obj = self.eval_expr(object, env)?;
                match obj {
                    Value::Record(map) => {
                        map.borrow_mut().insert(field.clone(), value);
                    }
                    _ => return Err(Signal::Error(format!("Cannot set field on {}", obj.type_name()))),
                }
            }
            AssignTarget::Index(object, index) => {
                let obj = self.eval_expr(object, env)?;
                let idx = self.eval_expr(index, env)?;
                match (&obj, &idx) {
                    (Value::List(list), Value::Int(i)) => {
                        let mut list = list.borrow_mut();
                        let i = *i as usize;
                        if i < list.len() {
                            list[i] = value;
                        } else {
                            return Err(Signal::Error(format!(
                                "Index {} out of bounds (len {})", i, list.len()
                            )));
                        }
                    }
                    _ => return Err(Signal::Error(format!(
                        "Cannot index-assign {} with {}",
                        obj.type_name(), idx.type_name()
                    ))),
                }
            }
        }
        Ok(Value::Nil)
    }

    fn update_state_if_needed(&mut self, name: &str, value: &Value) {
        for (sname, sid) in self.active_state_names.iter().rev() {
            if sname == name {
                self.state_store.insert(*sid, value.clone());
                return;
            }
        }
    }

    fn eval_expr(&mut self, expr: &Expr, env: &Environment) -> Res {
        match expr {
            Expr::Literal(lit) => Ok(self.literal_to_value(lit)),
            Expr::Ident(name) => env
                .get(name)
                .ok_or_else(|| Signal::Error(format!("Undefined variable: {}", name))),
            Expr::BinaryOp { op, left, right } => {
                if *op == BinOp::And {
                    let l = self.eval_expr(left, env)?;
                    if !l.is_truthy() {
                        return Ok(Value::Bool(false));
                    }
                    let r = self.eval_expr(right, env)?;
                    return Ok(Value::Bool(r.is_truthy()));
                }
                if *op == BinOp::Or {
                    let l = self.eval_expr(left, env)?;
                    if l.is_truthy() {
                        return Ok(Value::Bool(true));
                    }
                    let r = self.eval_expr(right, env)?;
                    return Ok(Value::Bool(r.is_truthy()));
                }
                let l = self.eval_expr(left, env)?;
                let r = self.eval_expr(right, env)?;
                self.eval_binop(*op, &l, &r)
            }
            Expr::UnaryOp { op, operand } => {
                let val = self.eval_expr(operand, env)?;
                self.eval_unaryop(*op, &val)
            }
            Expr::Call { function, args } => {
                let func = self.eval_expr(function, env)?;
                let mut arg_vals = Vec::new();
                for arg in args {
                    arg_vals.push(self.eval_expr(arg, env)?);
                }
                self.call_function(&func, arg_vals)
            }
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                let cond = self.eval_expr(condition, env)?;
                if cond.is_truthy() {
                    let block_env = Environment::with_parent(env);
                    self.exec_stmts(then_body, &block_env)
                } else if let Some(else_br) = else_body {
                    match else_br {
                        ElseBranch::Block(stmts) => {
                            let block_env = Environment::with_parent(env);
                            self.exec_stmts(stmts, &block_env)
                        }
                        ElseBranch::ElseIf(expr) => self.eval_expr(expr, env),
                    }
                } else {
                    Ok(Value::Nil)
                }
            }
            Expr::Match { subject, arms } => {
                let val = self.eval_expr(subject, env)?;
                for arm in arms {
                    let match_env = Environment::with_parent(env);
                    if self.match_pattern(&arm.pattern, &val, &match_env)? {
                        if let Some(guard) = &arm.guard {
                            let guard_val = self.eval_expr(guard, &match_env)?;
                            if !guard_val.is_truthy() {
                                continue;
                            }
                        }
                        return self.eval_expr(&arm.body, &match_env);
                    }
                }
                Err(Signal::Error(format!("No matching pattern for value: {}", val)))
            }
            Expr::List(elements) => {
                let mut items = Vec::new();
                for elem in elements {
                    items.push(self.eval_expr(elem, env)?);
                }
                Ok(Value::List(Rc::new(RefCell::new(items))))
            }
            Expr::Record(fields) => {
                let mut map = BTreeMap::new();
                for (key, value) in fields {
                    map.insert(key.clone(), self.eval_expr(value, env)?);
                }
                Ok(Value::Record(Rc::new(RefCell::new(map))))
            }
            Expr::FieldAccess { object, field } => {
                let obj = self.eval_expr(object, env)?;
                match &obj {
                    Value::Record(map) => {
                        let map = map.borrow();
                        map.get(field)
                            .cloned()
                            .ok_or_else(|| Signal::Error(format!("No field '{}' on record", field)))
                    }
                    _ => Err(Signal::Error(format!(
                        "Cannot access field '{}' on {}",
                        field,
                        obj.type_name()
                    ))),
                }
            }
            Expr::IndexAccess { object, index } => {
                let obj = self.eval_expr(object, env)?;
                let idx = self.eval_expr(index, env)?;
                match (&obj, &idx) {
                    (Value::List(list), Value::Int(i)) => {
                        let list = list.borrow();
                        let idx = if *i < 0 {
                            (list.len() as i64 + *i) as usize
                        } else {
                            *i as usize
                        };
                        list.get(idx)
                            .cloned()
                            .ok_or_else(|| Signal::Error(format!(
                                "Index {} out of bounds (len {})", i, list.len()
                            )))
                    }
                    (Value::Record(map), Value::String(key)) => {
                        let map = map.borrow();
                        map.get(key)
                            .cloned()
                            .ok_or_else(|| Signal::Error(format!("No key '{}' on record", key)))
                    }
                    _ => Err(Signal::Error(format!(
                        "Cannot index {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    ))),
                }
            }
            Expr::Block(stmts) => {
                let block_env = Environment::with_parent(env);
                self.exec_stmts(stmts, &block_env)
            }
            Expr::Lambda { params, body } => Ok(Value::Lambda {
                params: params.clone(),
                body: body.clone(),
                closure: env.clone(),
            }),
        }
    }

    fn literal_to_value(&self, lit: &Literal) -> Value {
        match lit {
            Literal::Nil => Value::Nil,
            Literal::Bool(b) => Value::Bool(*b),
            Literal::Int(n) => Value::Int(*n),
            Literal::Float(f) => Value::Float(*f),
            Literal::String(s) => Value::String(s.clone()),
        }
    }

    fn eval_binop(&self, op: BinOp, left: &Value, right: &Value) -> Res {
        match op {
            BinOp::Add => self.numeric_op(left, right, |a, b| a + b, |a, b| a + b),
            BinOp::Sub => self.numeric_op(left, right, |a, b| a - b, |a, b| a - b),
            BinOp::Mul => self.numeric_op(left, right, |a, b| a * b, |a, b| a * b),
            BinOp::Div => match (left, right) {
                (_, Value::Int(0)) => Err("Division by zero".into()),
                (_, Value::Float(f)) if *f == 0.0 => Err("Division by zero".into()),
                _ => self.numeric_op(left, right, |a, b| a / b, |a, b| a / b),
            },
            BinOp::Mod => self.numeric_op(left, right, |a, b| a % b, |a, b| a % b),
            BinOp::Eq => Ok(Value::Bool(left == right)),
            BinOp::Ne => Ok(Value::Bool(left != right)),
            BinOp::Lt => self.compare_op(left, right, |ord| ord == std::cmp::Ordering::Less),
            BinOp::Le => self.compare_op(left, right, |ord| ord != std::cmp::Ordering::Greater),
            BinOp::Gt => self.compare_op(left, right, |ord| ord == std::cmp::Ordering::Greater),
            BinOp::Ge => self.compare_op(left, right, |ord| ord != std::cmp::Ordering::Less),
            BinOp::Concat => {
                let l = left.to_display_string();
                let r = right.to_display_string();
                Ok(Value::String(format!("{}{}", l, r)))
            }
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn numeric_op(
        &self,
        left: &Value,
        right: &Value,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> Res {
        match (left, right) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(*a, *b))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float_op(*a, *b))),
            (Value::Int(a), Value::Float(b)) => Ok(Value::Float(float_op(*a as f64, *b))),
            (Value::Float(a), Value::Int(b)) => Ok(Value::Float(float_op(*a, *b as f64))),
            _ => Err(Signal::Error(format!(
                "Cannot perform arithmetic on {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        }
    }

    fn compare_op(
        &self,
        left: &Value,
        right: &Value,
        pred: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Res {
        let ord = match (left, right) {
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(std::cmp::Ordering::Equal),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            _ => return Err(Signal::Error(format!(
                "Cannot compare {} and {}",
                left.type_name(),
                right.type_name()
            ))),
        };
        Ok(Value::Bool(pred(ord)))
    }

    fn eval_unaryop(&self, op: UnaryOp, val: &Value) -> Res {
        match op {
            UnaryOp::Neg => match val {
                Value::Int(n) => Ok(Value::Int(-n)),
                Value::Float(f) => Ok(Value::Float(-f)),
                _ => Err(Signal::Error(format!("Cannot negate {}", val.type_name()))),
            },
            UnaryOp::Not => Ok(Value::Bool(!val.is_truthy())),
        }
    }

    fn call_function(&mut self, func: &Value, args: Vec<Value>) -> Res {
        match func {
            Value::Function {
                name: _,
                params,
                body,
                closure,
            } => {
                if args.len() != params.len() {
                    return Err(Signal::Error(format!(
                        "Expected {} arguments, got {}",
                        params.len(),
                        args.len()
                    )));
                }
                let func_env = Environment::with_parent(closure);
                for (param, arg) in params.iter().zip(args.into_iter()) {
                    func_env.set(param, arg);
                }
                match self.exec_stmts(body, &func_env) {
                    Ok(v) => Ok(v),
                    Err(Signal::Return(v)) => Ok(v), // catch return
                    Err(e) => Err(e),
                }
            }
            Value::Lambda {
                params,
                body,
                closure,
            } => {
                if args.len() != params.len() {
                    return Err(Signal::Error(format!(
                        "Expected {} arguments, got {}",
                        params.len(),
                        args.len()
                    )));
                }
                let func_env = Environment::with_parent(closure);
                for (param, arg) in params.iter().zip(args.into_iter()) {
                    func_env.set(param, arg);
                }
                match self.exec_stmts(body, &func_env) {
                    Ok(v) => Ok(v),
                    Err(Signal::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            Value::BuiltinFunction(name) => self.call_builtin(name, args),
            Value::EnumVariant { name, data } if data.is_empty() && args.is_empty() => {
                Ok(Value::EnumVariant {
                    name: name.clone(),
                    data: Vec::new(),
                })
            }
            _ => Err(Signal::Error(format!("Cannot call {}", func.type_name()))),
        }
    }

    fn call_builtin(&mut self, name: &str, args: Vec<Value>) -> Res {
        if let Some(enum_name) = name.strip_prefix("__enum__") {
            return Ok(Value::EnumVariant {
                name: enum_name.to_string(),
                data: args,
            });
        }

        match name {
            "print" => {
                let parts: Vec<String> = args.iter().map(|v| v.to_display_string()).collect();
                let line = parts.join(" ");
                if self.capture_output {
                    self.output.push(line);
                } else {
                    println!("{}", line);
                }
                Ok(Value::Nil)
            }
            "range" => {
                if args.len() != 2 {
                    return Err("range() expects 2 arguments".into());
                }
                let start = match &args[0] {
                    Value::Int(n) => *n,
                    _ => return Err("range() expects integer arguments".into()),
                };
                let end = match &args[1] {
                    Value::Int(n) => *n,
                    _ => return Err("range() expects integer arguments".into()),
                };
                let items: Vec<Value> = (start..end).map(Value::Int).collect();
                Ok(Value::List(Rc::new(RefCell::new(items))))
            }
            "len" => {
                if args.len() != 1 { return Err("len() expects 1 argument".into()); }
                match &args[0] {
                    Value::List(list) => Ok(Value::Int(list.borrow().len() as i64)),
                    Value::String(s) => Ok(Value::Int(s.len() as i64)),
                    _ => Err(Signal::Error(format!("Cannot get length of {}", args[0].type_name()))),
                }
            }
            "push" => {
                if args.len() != 2 { return Err("push() expects 2 arguments".into()); }
                match &args[0] {
                    Value::List(list) => {
                        list.borrow_mut().push(args[1].clone());
                        Ok(Value::Nil)
                    }
                    _ => Err("push() expects a list as first argument".into()),
                }
            }
            "str" => {
                if args.len() != 1 { return Err("str() expects 1 argument".into()); }
                Ok(Value::String(args[0].to_display_string()))
            }
            "abs" => {
                if args.len() != 1 { return Err("abs() expects 1 argument".into()); }
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(n.abs())),
                    Value::Float(f) => Ok(Value::Float(f.abs())),
                    _ => Err("abs() expects a number".into()),
                }
            }
            "sqrt" => {
                if args.len() != 1 { return Err("sqrt() expects 1 argument".into()); }
                let n = to_float(&args[0])?;
                Ok(Value::Float(n.sqrt()))
            }
            "floor" => {
                if args.len() != 1 { return Err("floor() expects 1 argument".into()); }
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => Ok(Value::Float(f.floor())),
                    _ => Err("floor() expects a number".into()),
                }
            }
            "ceil" => {
                if args.len() != 1 { return Err("ceil() expects 1 argument".into()); }
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => Ok(Value::Float(f.ceil())),
                    _ => Err("ceil() expects a number".into()),
                }
            }
            "float" => {
                if args.len() != 1 { return Err("float() expects 1 argument".into()); }
                let f = to_float(&args[0])?;
                Ok(Value::Float(f))
            }
            "int" => {
                if args.len() != 1 { return Err("int() expects 1 argument".into()); }
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => Ok(Value::Int(*f as i64)),
                    Value::String(s) => s.parse::<i64>().map(Value::Int)
                        .map_err(|_| Signal::Error(format!("Cannot convert '{}' to int", s))),
                    _ => Err(Signal::Error(format!("Cannot convert {} to int", args[0].type_name()))),
                }
            }
            "random" => {
                if args.len() != 2 { return Err("random() expects 2 arguments".into()); }
                let min = to_float(&args[0])?;
                let max = to_float(&args[1])?;
                let pseudo = ((std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos() as f64) / 4_294_967_295.0)
                    * (max - min) + min;
                Ok(Value::Float(pseudo))
            }
            "type" => {
                if args.len() != 1 { return Err("type() expects 1 argument".into()); }
                Ok(Value::String(args[0].type_name().to_string()))
            }
            "pop" => {
                if args.len() != 1 { return Err("pop() expects 1 argument".into()); }
                match &args[0] {
                    Value::List(list) => Ok(list.borrow_mut().pop().unwrap_or(Value::Nil)),
                    _ => Err("pop() expects a list".into()),
                }
            }
            "append" => {
                if args.len() != 2 { return Err("append() expects 2 arguments".into()); }
                match &args[0] {
                    Value::List(list) => {
                        list.borrow_mut().push(args[1].clone());
                        Ok(Value::Nil)
                    }
                    _ => Err("append() expects a list".into()),
                }
            }
            "keys" => {
                if args.len() != 1 { return Err("keys() expects 1 argument".into()); }
                match &args[0] {
                    Value::Record(map) => {
                        let keys: Vec<Value> = map.borrow().keys().map(|k| Value::String(k.clone())).collect();
                        Ok(Value::List(Rc::new(RefCell::new(keys))))
                    }
                    _ => Err("keys() expects a record".into()),
                }
            }
            "values" => {
                if args.len() != 1 { return Err("values() expects 1 argument".into()); }
                match &args[0] {
                    Value::Record(map) => {
                        let vals: Vec<Value> = map.borrow().values().cloned().collect();
                        Ok(Value::List(Rc::new(RefCell::new(vals))))
                    }
                    _ => Err("values() expects a record".into()),
                }
            }
            "contains" => {
                if args.len() != 2 { return Err("contains() expects 2 arguments".into()); }
                match &args[0] {
                    Value::List(list) => Ok(Value::Bool(list.borrow().iter().any(|v| v == &args[1]))),
                    Value::String(s) => match &args[1] {
                        Value::String(sub) => Ok(Value::Bool(s.contains(sub.as_str()))),
                        _ => Err("contains() on string expects a string".into()),
                    },
                    _ => Err("contains() expects a list or string".into()),
                }
            }
            "min" => {
                if args.len() != 2 { return Err("min() expects 2 arguments".into()); }
                let less = self.compare_op(&args[0], &args[1], |ord| ord == std::cmp::Ordering::Less)?;
                Ok(if less == Value::Bool(true) { args[0].clone() } else { args[1].clone() })
            }
            "max" => {
                if args.len() != 2 { return Err("max() expects 2 arguments".into()); }
                let greater = self.compare_op(&args[0], &args[1], |ord| ord == std::cmp::Ordering::Greater)?;
                Ok(if greater == Value::Bool(true) { args[0].clone() } else { args[1].clone() })
            }
            "round" => {
                if args.len() != 1 { return Err("round() expects 1 argument".into()); }
                match &args[0] {
                    Value::Int(n) => Ok(Value::Int(*n)),
                    Value::Float(f) => Ok(Value::Float(f.round())),
                    _ => Err("round() expects a number".into()),
                }
            }
            _ => Err(Signal::Error(format!("Unknown builtin function: {}", name))),
        }
    }

    fn match_pattern(&self, pattern: &Pattern, value: &Value, env: &Environment) -> Result<bool, Signal> {
        match pattern {
            Pattern::Wildcard => Ok(true),
            Pattern::Literal(lit) => {
                let pat_val = self.literal_to_value(lit);
                Ok(pat_val == *value)
            }
            Pattern::Variable(name) => {
                if self.enum_variants.contains_key(name) {
                    match value {
                        Value::EnumVariant { name: vname, data } => {
                            Ok(vname == name && data.is_empty())
                        }
                        _ => Ok(false),
                    }
                } else {
                    env.set(name, value.clone());
                    Ok(true)
                }
            }
            Pattern::Variant { name, fields } => match value {
                Value::EnumVariant { name: vname, data } => {
                    if vname != name || data.len() != fields.len() {
                        return Ok(false);
                    }
                    for (pat, val) in fields.iter().zip(data.iter()) {
                        if !self.match_pattern(pat, val, env)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                _ => Ok(false),
            },
            Pattern::List { elements, rest } => match value {
                Value::List(list) => {
                    let list = list.borrow();
                    if let Some(rest_name) = rest {
                        if list.len() < elements.len() {
                            return Ok(false);
                        }
                        for (pat, val) in elements.iter().zip(list.iter()) {
                            if !self.match_pattern(pat, val, env)? {
                                return Ok(false);
                            }
                        }
                        let rest_vals: Vec<Value> = list[elements.len()..].to_vec();
                        env.set(rest_name, Value::List(Rc::new(RefCell::new(rest_vals))));
                        Ok(true)
                    } else {
                        if list.len() != elements.len() {
                            return Ok(false);
                        }
                        for (pat, val) in elements.iter().zip(list.iter()) {
                            if !self.match_pattern(pat, val, env)? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                }
                _ => Ok(false),
            },
            Pattern::Record(fields) => match value {
                Value::Record(map) => {
                    let map = map.borrow();
                    for (key, pat) in fields {
                        if let Some(val) = map.get(key) {
                            if !self.match_pattern(pat, val, env)? {
                                return Ok(false);
                            }
                        } else {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                _ => Ok(false),
            },
        }
    }
}

fn to_float(val: &Value) -> Result<f64, Signal> {
    match val {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(Signal::Error(format!("Cannot convert {} to float", val.type_name()))),
    }
}

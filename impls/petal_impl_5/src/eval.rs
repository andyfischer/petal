use crate::{Env, ProgramKey, StackKey, Value, TermOp, Program};
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;

pub fn eval(env: &mut Env, stack_key: StackKey) -> Result<Value, String> {
    let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
    let program_key = stack.program_key;

    // Evaluate the program
    let program = env
        .get_program(program_key)
        .ok_or("Program not found")?
        .clone();

    eval_term(env, stack_key, program.entry_term, &program)
}

pub fn eval_term(
    env: &mut Env,
    stack_key: StackKey,
    term_id: usize,
    program: &Program,
) -> Result<Value, String> {
    if term_id >= program.terms.len() {
        return Err("Invalid term ID".to_string());
    }

    let term = &program.terms[term_id];

    match &term.op {
        TermOp::Constant(val) => Ok(val.clone()),

        TermOp::Add => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            add_values(&left, &right)
        }

        TermOp::Sub => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            sub_values(&left, &right)
        }

        TermOp::Mul => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            mul_values(&left, &right)
        }

        TermOp::Div => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            div_values(&left, &right)
        }

        TermOp::Mod => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            mod_values(&left, &right)
        }

        TermOp::Eq => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(values_equal(&left, &right)))
        }

        TermOp::Lt => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(compare_values(&left, &right)? < 0))
        }

        TermOp::Gt => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(compare_values(&left, &right)? > 0))
        }

        TermOp::Lte => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(compare_values(&left, &right)? <= 0))
        }

        TermOp::Gte => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(compare_values(&left, &right)? >= 0))
        }

        TermOp::And => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            if !left.is_truthy() {
                return Ok(Value::Bool(false));
            }
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(right.is_truthy()))
        }

        TermOp::Or => {
            let left = eval_term(env, stack_key, term.inputs[0], program)?;
            if left.is_truthy() {
                return Ok(Value::Bool(true));
            }
            let right = eval_term(env, stack_key, term.inputs[1], program)?;
            Ok(Value::Bool(right.is_truthy()))
        }

        TermOp::Not => {
            let val = eval_term(env, stack_key, term.inputs[0], program)?;
            Ok(Value::Bool(!val.is_truthy()))
        }

        TermOp::Branch { then_id, else_id } => {
            let cond = eval_term(env, stack_key, term.inputs[0], program)?;
            if cond.is_truthy() {
                eval_term(env, stack_key, *then_id, program)
            } else {
                eval_term(env, stack_key, *else_id, program)
            }
        }

        TermOp::Call(name) => {
            let args: Result<Vec<_>, _> = term
                .inputs
                .iter()
                .map(|&input_id| eval_term(env, stack_key, input_id, program))
                .collect();
            let args = args?;

            // Check for user-defined function first
            if let Some(func) = env.get_function(name).cloned() {
                if !func.is_builtin {
                    return eval_user_function(env, stack_key, &func, args, program);
                }
            }

            // Fall back to built-in
            eval_builtin_call(name, args)
        }

        TermOp::ListIndex => {
            let list = eval_term(env, stack_key, term.inputs[0], program)?;
            let index = eval_term(env, stack_key, term.inputs[1], program)?;

            match (list, index) {
                (Value::List(list), Value::Int(idx)) => {
                    let list_ref = list.borrow();
                    if idx >= 0 && (idx as usize) < list_ref.len() {
                        Ok(list_ref[idx as usize].clone())
                    } else {
                        Err("Index out of bounds".to_string())
                    }
                }
                _ => Err("Invalid list indexing".to_string()),
            }
        }

        TermOp::ListConcat => {
            let mut list = Vec::new();
            for &input_id in &term.inputs {
                let val = eval_term(env, stack_key, input_id, program)?;
                list.push(val);
            }
            Ok(Value::List(Rc::new(RefCell::new(list))))
        }

        TermOp::GetField(field) => {
            let val = eval_term(env, stack_key, term.inputs[0], program)?;

            match val {
                Value::Map(map) => {
                    let map_ref = map.borrow();
                    Ok(map_ref
                        .get(field)
                        .cloned()
                        .unwrap_or(Value::Nil))
                }
                _ => Err(format!("Cannot get field {} from {:?}", field, val)),
            }
        }

        TermOp::SetField(field) => {
            let map = eval_term(env, stack_key, term.inputs[0], program)?;
            let val = eval_term(env, stack_key, term.inputs[1], program)?;

            match map {
                Value::Map(map) => {
                    map.borrow_mut().insert(field.clone(), val);
                    Ok(Value::Map(map))
                }
                _ => Err("Cannot set field on non-map".to_string()),
            }
        }

        TermOp::Return => {
            if term.inputs.is_empty() {
                Ok(Value::Nil)
            } else {
                eval_term(env, stack_key, term.inputs[0], program)
            }
        }

        TermOp::Sequence { terms } => {
            let mut last_value = Value::Nil;
            for &term_id in terms {
                last_value = eval_term(env, stack_key, term_id, program)?;
            }
            Ok(last_value)
        }

        TermOp::Let { var, init, body } => {
            // Evaluate the init expression
            let init_value = eval_term(env, stack_key, *init, program)?;

            // Save current bindings
            let old_binding = {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                stack.bindings.insert(var.clone(), init_value)
            };

            // Evaluate body with binding in place
            let result = eval_term(env, stack_key, *body, program);

            // Restore old binding
            {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                if let Some(old_val) = old_binding {
                    stack.bindings.insert(var.clone(), old_val);
                } else {
                    stack.bindings.remove(var);
                }
            }

            result
        }

        TermOp::Var(name) => {
            // Look up variable in bindings or state
            let value = {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                // Check bindings first (let variables)
                if let Some(v) = stack.bindings.get(name).cloned() {
                    Some(v)
                } else {
                    // Then check state (state variables)
                    stack.state.get(name).cloned()
                }
            };
            match value {
                Some(v) => Ok(v),
                None => Err(format!("Undefined variable: {}", name)),
            }
        }

        TermOp::StateDef { var, init, body, state_id: _ } => {
            // Check if state already exists (persistence across invocations)
            let needs_init = {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                !stack.state.contains_key(var)
            };

            if needs_init {
                // Initialize state
                let init_value = eval_term(env, stack_key, *init, program)?;
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                stack.state.insert(var.clone(), init_value);
            }

            // Evaluate body with state variable available
            eval_term(env, stack_key, *body, program)
        }

        TermOp::StateRead(name) => {
            let value = {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                stack.state.get(name).cloned()
            };
            Ok(value.unwrap_or(Value::Nil))
        }

        TermOp::StateWrite { var, value } => {
            let val = eval_term(env, stack_key, *value, program)?;
            {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                stack.state.insert(var.clone(), val.clone());
            }
            Ok(val)
        }

        TermOp::FunctionDef { name, params, body, next } => {
            // Register the function
            let func = crate::Function {
                name: name.clone(),
                params: params.clone(),
                body: Rc::new(program.clone()),
                body_term_id: *body,
                is_builtin: false,
            };
            env.add_function(name.clone(), func);

            // Continue evaluating the next term
            eval_term(env, stack_key, *next, program)
        }

        TermOp::For { var, iter, body } => {
            // Evaluate the iterable
            let iterable = eval_term(env, stack_key, *iter, program)?;

            // Extract items from iterable
            let items = match iterable {
                Value::List(list) => list.borrow().clone(),
                _ => return Err(format!("Cannot iterate over {:?}", iterable)),
            };

            // Iterate with loop variable binding
            let mut last_value = Value::Nil;
            for item in items {
                // Save current binding
                let old_binding = {
                    let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                    stack.bindings.insert(var.clone(), item)
                };

                // Execute body
                last_value = eval_term(env, stack_key, *body, program)?;

                // Restore old binding
                {
                    let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                    if let Some(old_val) = old_binding {
                        stack.bindings.insert(var.clone(), old_val);
                    } else {
                        stack.bindings.remove(var);
                    }
                }
            }

            Ok(last_value)
        }

        TermOp::While { cond, body } => {
            let mut last_value = Value::Nil;

            loop {
                // Evaluate condition
                let cond_value = eval_term(env, stack_key, *cond, program)?;

                // Check if we should continue
                if !cond_value.is_truthy() {
                    break;
                }

                // Execute body
                last_value = eval_term(env, stack_key, *body, program)?;
            }

            Ok(last_value)
        }

        TermOp::Mutate { var, op, value } => {
            // Get current value
            let current = {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                // Check bindings first, then state
                if let Some(v) = stack.bindings.get(var).cloned() {
                    Some(v)
                } else {
                    stack.state.get(var).cloned()
                }
            };

            let current_val = current.ok_or(format!("Undefined variable: {}", var))?;

            // Evaluate the right-hand side
            let rhs = eval_term(env, stack_key, *value, program)?;

            // Apply the operation
            let result = match op.as_str() {
                "+" => add_values(&current_val, &rhs)?,
                "-" => sub_values(&current_val, &rhs)?,
                "*" => mul_values(&current_val, &rhs)?,
                "/" => div_values(&current_val, &rhs)?,
                _ => return Err(format!("Unknown mutation operator: {}", op)),
            };

            // Update in bindings or state
            {
                let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
                if stack.bindings.contains_key(var) {
                    stack.bindings.insert(var.clone(), result.clone());
                } else {
                    stack.state.insert(var.clone(), result.clone());
                }
            }

            Ok(result)
        }
    }
}

fn add_values(left: &Value, right: &Value) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
        (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{}{}", a, b))),
        _ => Err(format!("Cannot add {:?} and {:?}", left, right)),
    }
}

fn sub_values(left: &Value, right: &Value) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a - *b as f64)),
        _ => Err(format!("Cannot subtract {:?} and {:?}", left, right)),
    }
}

fn mul_values(left: &Value, right: &Value) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a * *b as f64)),
        _ => Err(format!("Cannot multiply {:?} and {:?}", left, right)),
    }
}

fn div_values(left: &Value, right: &Value) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => {
            if *b == 0 {
                Err("Division by zero".to_string())
            } else {
                Ok(Value::Int(a / b))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Err("Division by zero".to_string())
            } else {
                Ok(Value::Float(a / b))
            }
        }
        (Value::Int(a), Value::Float(b)) => {
            if *b == 0.0 {
                Err("Division by zero".to_string())
            } else {
                Ok(Value::Float(*a as f64 / b))
            }
        }
        (Value::Float(a), Value::Int(b)) => {
            if *b == 0 {
                Err("Division by zero".to_string())
            } else {
                Ok(Value::Float(a / *b as f64))
            }
        }
        _ => Err(format!("Cannot divide {:?} and {:?}", left, right)),
    }
}

fn mod_values(left: &Value, right: &Value) -> Result<Value, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => {
            if *b == 0 {
                Err("Division by zero".to_string())
            } else {
                Ok(Value::Int(a % b))
            }
        }
        _ => Err(format!("Cannot modulo {:?} and {:?}", left, right)),
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => (a - b).abs() < 1e-10,
        (Value::Int(a), Value::Float(b)) => (*a as f64 - b).abs() < 1e-10,
        (Value::Float(a), Value::Int(b)) => (a - *b as f64).abs() < 1e-10,
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

fn compare_values(left: &Value, right: &Value) -> Result<i32, String> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(if a < b {
            -1
        } else if a > b {
            1
        } else {
            0
        }),
        (Value::Float(a), Value::Float(b)) => Ok(if a < b {
            -1
        } else if a > b {
            1
        } else {
            0
        }),
        (Value::Int(a), Value::Float(b)) => {
            let a = *a as f64;
            Ok(if a < *b { -1 } else if a > *b { 1 } else { 0 })
        }
        (Value::Float(a), Value::Int(b)) => {
            let b = *b as f64;
            Ok(if a < &b { -1 } else if a > &b { 1 } else { 0 })
        }
        _ => Err(format!("Cannot compare {:?} and {:?}", left, right)),
    }
}

fn eval_user_function(
    env: &mut Env,
    stack_key: StackKey,
    func: &crate::Function,
    args: Vec<Value>,
    program: &Program,
) -> Result<Value, String> {
    if func.params.len() != args.len() {
        return Err(format!(
            "Function {} expects {} arguments, got {}",
            func.name,
            func.params.len(),
            args.len()
        ));
    }

    // Save current bindings
    let saved_bindings = {
        let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
        stack.bindings.clone()
    };

    // Bind parameters to arguments
    {
        let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
        for (param, arg) in func.params.iter().zip(args.iter()) {
            stack.bindings.insert(param.clone(), arg.clone());
        }
    }

    // Evaluate the function body using the stored program
    let result = eval_term(env, stack_key, func.body_term_id, &func.body);

    // Restore bindings
    {
        let stack = env.get_stack(stack_key).ok_or("Stack not found")?;
        stack.bindings = saved_bindings;
    }

    result
}

fn eval_builtin_call(name: &str, args: Vec<Value>) -> Result<Value, String> {
    match name {
        "print" => {
            for arg in &args {
                println!("{}", arg.to_string());
            }
            Ok(Value::Nil)
        }

        "len" => {
            if args.len() != 1 {
                return Err("len expects 1 argument".to_string());
            }

            match &args[0] {
                Value::String(s) => Ok(Value::Int(s.len() as i64)),
                Value::List(list) => Ok(Value::Int(list.borrow().len() as i64)),
                Value::Map(map) => Ok(Value::Int(map.borrow().len() as i64)),
                _ => Err("len requires string, list, or map".to_string()),
            }
        }

        "range" => {
            if args.len() != 2 {
                return Err("range expects 2 arguments".to_string());
            }

            let start = args[0].as_int()?;
            let end = args[1].as_int()?;

            let mut list = Vec::new();
            for i in start..end {
                list.push(Value::Int(i));
            }

            Ok(Value::List(Rc::new(RefCell::new(list))))
        }

        "push" => {
            if args.len() < 2 {
                return Err("push expects 2 arguments".to_string());
            }

            match &args[0] {
                Value::List(list) => {
                    list.borrow_mut().push(args[1].clone());
                    Ok(args[0].clone())
                }
                _ => Err("push requires a list".to_string()),
            }
        }

        "pop" => {
            if args.len() != 1 {
                return Err("pop expects 1 argument".to_string());
            }

            match &args[0] {
                Value::List(list) => Ok(list.borrow_mut().pop().unwrap_or(Value::Nil)),
                _ => Err("pop requires a list".to_string()),
            }
        }

        "to_string" => {
            if args.len() != 1 {
                return Err("to_string expects 1 argument".to_string());
            }
            Ok(Value::String(args[0].to_string()))
        }

        "to_int" => {
            if args.len() != 1 {
                return Err("to_int expects 1 argument".to_string());
            }

            match &args[0] {
                Value::Int(n) => Ok(Value::Int(*n)),
                Value::Float(f) => Ok(Value::Int(*f as i64)),
                Value::String(s) => {
                    match s.parse::<i64>() {
                        Ok(n) => Ok(Value::Int(n)),
                        Err(_) => Err("Cannot parse as int".to_string()),
                    }
                }
                _ => Err("to_int requires int, float, or string".to_string()),
            }
        }

        "to_float" => {
            if args.len() != 1 {
                return Err("to_float expects 1 argument".to_string());
            }

            match &args[0] {
                Value::Int(n) => Ok(Value::Float(*n as f64)),
                Value::Float(f) => Ok(Value::Float(*f)),
                Value::String(s) => {
                    match s.parse::<f64>() {
                        Ok(f) => Ok(Value::Float(f)),
                        Err(_) => Err("Cannot parse as float".to_string()),
                    }
                }
                _ => Err("to_float requires int, float, or string".to_string()),
            }
        }

        _ => Err(format!("Unknown function: {}", name)),
    }
}

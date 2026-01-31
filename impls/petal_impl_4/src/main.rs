use std::env;
use std::fs;
use std::path::Path;

use petal::{Env, Value, Heap, StringId, ListId, MapId};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("Usage: petal <file.ptl>");
        println!("       petal run <file.ptl>");
        println!("       petal eval <expression>");
        std::process::exit(1);
    }

    let command = &args[1];

    match command.as_str() {
        "run" => {
            if args.len() < 3 {
                println!("Usage: petal run <file.ptl>");
                std::process::exit(1);
            }
            let file_path = &args[2];
            run_file(file_path);
        }
        "eval" => {
            if args.len() < 3 {
                println!("Usage: petal eval <expression>");
                std::process::exit(1);
            }
            let expr = &args[2];
            eval_expression(expr);
        }
        _ => {
            // Assume it's a file path
            run_file(command);
        }
    }
}

fn run_file(file_path: &str) {
    if !Path::new(file_path).exists() {
        eprintln!("Error: File '{}' not found", file_path);
        std::process::exit(1);
    }

    let source = fs::read_to_string(file_path)
        .unwrap_or_else(|e| {
            eprintln!("Error reading file: {}", e);
            std::process::exit(1);
        });

    let mut env = Env::new();

    // Register built-in functions
    register_builtins(&mut env);

    // Load and run the program
    match env.load_program(&source) {
        Ok(program) => {
            match env.create_stack(program) {
                Ok(stack) => {
                    match env.run(stack) {
                        Ok(result) => {
                            // Only print the result if it's not nil
                            if !matches!(result, Value::Nil) {
                                println!("{}", value_to_string(&env, &result));
                            }
                        }
                        Err(e) => {
                            eprintln!("Runtime error: {:?}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error creating stack: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {:?}", e);
            std::process::exit(1);
        }
    }
}

fn eval_expression(expr: &str) {
    let mut env = Env::new();
    register_builtins(&mut env);

    // Wrap expression in a return statement so it produces a value
    let source = format!("return {}", expr);

    match env.load_program(&source) {
        Ok(program) => {
            match env.create_stack(program) {
                Ok(stack) => {
                    match env.run(stack) {
                        Ok(result) => {
                            println!("{}", value_to_string(&env, &result));
                        }
                        Err(e) => {
                            eprintln!("Runtime error: {:?}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error creating stack: {:?}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {:?}", e);
            std::process::exit(1);
        }
    }
}

fn register_builtins(env: &mut Env) {
    // print - prints values
    env.register_builtin("print", |heap, args| {
        let strings: Vec<String> = args.iter()
            .map(|v| value_to_string_heap(heap, v))
            .collect();
        println!("{}", strings.join(" "));
        Value::Nil
    });

    // len - returns length of string or list
    env.register_builtin("len", |heap, args| {
        if args.is_empty() {
            return Value::Nil;
        }
        match &args[0] {
            Value::String(s) => {
                if let Some(s) = heap.get_string(*s) {
                    Value::Int(s.len() as i64)
                } else {
                    Value::Nil
                }
            }
            Value::List(l) => {
                if let Some(list) = heap.get_list(*l) {
                    Value::Int(list.len() as i64)
                } else {
                    Value::Nil
                }
            }
            _ => Value::Nil,
        }
    });

    // push - adds element to list
    env.register_builtin("push", |heap, args| {
        if args.len() < 2 {
            return Value::Nil;
        }
        match &args[0] {
            Value::List(l) => {
                let value = args[1].clone();
                heap.push_to_list(*l, value);
                Value::List(*l)
            }
            _ => Value::Nil,
        }
    });

    // pop - removes and returns last element
    env.register_builtin("pop", |heap, args| {
        if args.is_empty() {
            return Value::Nil;
        }
        match &args[0] {
            Value::List(l) => {
                heap.pop_from_list(*l).unwrap_or(Value::Nil)
            }
            _ => Value::Nil,
        }
    });

    // range - creates a range of numbers
    env.register_builtin("range", |heap, args| {
        let start = match args.get(0) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let end = match args.get(1) {
            Some(Value::Int(n)) => *n,
            _ => start,
        };

        let list_id = heap.alloc_list();
        for i in start..end {
            heap.push_to_list(list_id, Value::Int(i));
        }
        Value::List(list_id)
    });

    // random - random number (simplified)
    env.register_builtin("random", |heap, args| {
        let min = match args.get(0) {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(n)) => *n as f64,
            _ => 0.0,
        };
        let max = match args.get(1) {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(n)) => *n as f64,
            _ => 1.0,
        };
        // Simple pseudo-random for demonstration
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as f64;
        let rand = (now.sin() + 1.0) / 2.0; // Normalize to 0-1
        Value::Float(min + rand * (max - min))
    });

    // sqrt - square root
    env.register_builtin("sqrt", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.sqrt()),
            Some(Value::Int(n)) => Value::Float((*n as f64).sqrt()),
            _ => Value::Nil,
        }
    });

    // sin, cos, tan - trigonometric functions
    env.register_builtin("sin", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.sin()),
            Some(Value::Int(n)) => Value::Float((*n as f64).sin()),
            _ => Value::Nil,
        }
    });

    env.register_builtin("cos", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.cos()),
            Some(Value::Int(n)) => Value::Float((*n as f64).cos()),
            _ => Value::Nil,
        }
    });

    env.register_builtin("tan", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.tan()),
            Some(Value::Int(n)) => Value::Float((*n as f64).tan()),
            _ => Value::Nil,
        }
    });

    // floor, ceil, round
    env.register_builtin("floor", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.floor()),
            Some(Value::Int(n)) => Value::Int(*n),
            _ => Value::Nil,
        }
    });

    env.register_builtin("ceil", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.ceil()),
            Some(Value::Int(n)) => Value::Int(*n),
            _ => Value::Nil,
        }
    });

    env.register_builtin("round", |_heap, args| {
        match args.get(0) {
            Some(Value::Float(f)) => Value::Float(f.round()),
            Some(Value::Int(n)) => Value::Int(*n),
            _ => Value::Nil,
        }
    });

    // type - returns type name as string
    env.register_builtin("type", |heap, args| {
        if args.is_empty() {
            return Value::Nil;
        }
        let type_name = match &args[0] {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Function(_) => "function",
        };
        Value::String(heap.alloc_string(type_name))
    });
}

fn value_to_string(env: &Env, value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            // Format without unnecessary decimal places
            if f.fract() == 0.0 {
                format!("{:.1}", f)
            } else {
                format!("{}", f)
            }
        }
        Value::String(s) => {
            if let Some(s) = env.get_string(*s) {
                s.to_string()
            } else {
                "<invalid string>".to_string()
            }
        }
        Value::List(l) => {
            if let Some(list) = env.get_list(*l) {
                let items: Vec<String> = list.iter()
                    .map(|v| value_to_string(env, v))
                    .collect();
                format!("[{}]", items.join(", "))
            } else {
                "<invalid list>".to_string()
            }
        }
        Value::Map(m) => {
            if let Some(map) = env.get_map(*m) {
                let items: Vec<String> = map.iter()
                    .map(|(k, v)| {
                        format!("{}: {}", value_to_string(env, k), value_to_string(env, v))
                    })
                    .collect();
                format!("{{ {} }}", items.join(", "))
            } else {
                "<invalid map>".to_string()
            }
        }
        Value::Function(f) => format!("<function {:?}>", f),
    }
}

fn value_to_string_heap(heap: &Heap, value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            if f.fract() == 0.0 {
                format!("{:.1}", f)
            } else {
                format!("{}", f)
            }
        }
        Value::String(s) => {
            if let Some(s) = heap.get_string(*s) {
                s.to_string()
            } else {
                "<invalid string>".to_string()
            }
        }
        Value::List(l) => {
            if let Some(list) = heap.get_list(*l) {
                let items: Vec<String> = list.iter()
                    .map(|v| value_to_string_heap(heap, v))
                    .collect();
                format!("[{}]", items.join(", "))
            } else {
                "<invalid list>".to_string()
            }
        }
        Value::Map(m) => {
            if let Some(map) = heap.get_map(*m) {
                let items: Vec<String> = map.iter()
                    .map(|(k, v)| {
                        format!("{}: {}", value_to_string_heap(heap, k), value_to_string_heap(heap, v))
                    })
                    .collect();
                format!("{{ {} }}", items.join(", "))
            } else {
                "<invalid map>".to_string()
            }
        }
        Value::Function(f) => format!("<function {:?}>", f),
    }
}

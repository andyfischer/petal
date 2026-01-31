//! Petal language CLI

use petal::{Env, Error};
use std::env;
use std::fs;
use std::process;

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: petal <file.ptl>");
        eprintln!("   or: petal --repl");
        process::exit(1);
    }

    if args[1] == "--repl" {
        return run_repl();
    }

    let filename = &args[1];
    let source = fs::read_to_string(filename)
        .map_err(|e| Error::Runtime(format!("Failed to read file: {}", e)))?;

    let mut env = Env::new();

    // Register built-in functions
    register_builtins(&mut env);

    let program = env.load_program(&source)?;
    let stack = env.create_stack(program)?;
    let result = env.run(stack)?;

    println!("{:?}", result);

    Ok(())
}

fn run_repl() -> Result<(), Error> {
    use std::io::{self, Write};

    println!("Petal REPL v0.1.0");
    println!("Type ':help' for help, ':exit' to exit\n");

    let mut env = Env::new();
    register_builtins(&mut env);

    let mut line = String::new();
    let mut stack_key = None;

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        line.clear();
        if io::stdin().read_line(&mut line).is_err() {
            break;
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            ":help" => {
                println!("Commands:");
                println!("  :help     Show this help");
                println!("  :exit     Exit the REPL");
                println!("  :clear    Clear all definitions");
            }
            ":exit" => break,
            ":clear" => {
                env = Env::new();
                register_builtins(&mut env);
                stack_key = None;
                println!("Cleared all definitions");
            }
            _ => {
                match env.load_program(input) {
                    Ok(program) => {
                        match env.create_stack(program) {
                            Ok(stack) => {
                                match env.run(stack) {
                                    Ok(value) => println!("{:?}", value),
                                    Err(e) => println!("Error: {}", e),
                                }
                            }
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                    Err(e) => println!("Parse error: {}", e),
                }
            }
        }
    }

    println!("\nGoodbye!");
    Ok(())
}

fn register_builtins(env: &mut Env) {
    env.register_builtin("print", |args| {
        for arg in args {
            print!("{:?} ", arg);
        }
        println!();
        Ok(petal::Value::Nil)
    });

    env.register_builtin("len", |args| {
        match &args[0] {
            petal::Value::List(list_id) => {
                // TODO: Implement list operations
                Ok(petal::Value::Int(0))
            }
            petal::Value::Map(map_id) => {
                // TODO: Implement map operations
                Ok(petal::Value::Int(0))
            }
            petal::Value::String(string_id) => {
                // TODO: Implement string operations
                Ok(petal::Value::Int(0))
            }
            _ => Err(petal::Error::InvalidOperation("len requires a collection".to_string())),
        }
    });

    env.register_builtin("pow", |args| {
        match (&args[0], &args[1]) {
            (petal::Value::Int(a), petal::Value::Int(b)) => {
                Ok(petal::Value::Int(a.pow(*b as u32)))
            }
            (petal::Value::Float(a), petal::Value::Float(b)) => {
                Ok(petal::Value::Float(a.powf(*b)))
            }
            (petal::Value::Int(a), petal::Value::Float(b)) => {
                Ok(petal::Value::Float((*a as f64).powf(*b)))
            }
            (petal::Value::Float(a), petal::Value::Int(b)) => {
                Ok(petal::Value::Float(a.powi(*b as i32)))
            }
            _ => Err(petal::Error::InvalidOperation("pow requires numeric arguments".to_string())),
        }
    });
}

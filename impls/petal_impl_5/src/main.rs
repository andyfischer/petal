use petal::{Env, Value};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "run" => run_file(&args)?,
        "repl" => run_repl()?,
        "--version" => println!("Petal 0.1.0"),
        "--help" | "-h" => print_usage(),
        _ => {
            if args[1].ends_with(".ptl") {
                run_single_file(&args[1])?;
            } else {
                eprintln!("Unknown command: {}", args[1]);
                print_usage();
            }
        }
    }

    Ok(())
}

fn print_usage() {
    println!("Petal Programming Language - v0.1.0");
    println!();
    println!("Usage:");
    println!("  petal <file.ptl>     Execute a Petal script");
    println!("  petal run <file.ptl> Execute a Petal script");
    println!("  petal repl           Start interactive REPL");
    println!("  petal --help         Show this help");
    println!("  petal --version      Show version");
}

fn run_file(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 {
        eprintln!("Usage: petal run <file.ptl>");
        return Ok(());
    }

    run_single_file(&args[2])
}

fn run_single_file(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(path).exists() {
        eprintln!("Error: File not found: {}", path);
        return Ok(());
    }

    let source = fs::read_to_string(path)?;

    let mut env = Env::new();
    let program_key = env.load_program(&source)?;
    let stack_key = env.create_stack(program_key)?;
    let result = env.run(stack_key)?;

    match result {
        Value::Nil => {}
        _ => println!("{}", result.to_string()),
    }

    Ok(())
}

fn run_repl() -> Result<(), Box<dyn std::error::Error>> {
    println!("Petal REPL - v0.1.0");
    println!("Type 'exit' or 'quit' to exit");
    println!();

    let mut env = Env::new();
    let mut buffer = String::new();

    loop {
        print!("> ");
        io::stdout().flush()?;

        buffer.clear();
        io::stdin().read_line(&mut buffer)?;

        let input = buffer.trim();

        if input.is_empty() {
            continue;
        }

        if input == "exit" || input == "quit" {
            break;
        }

        match execute_repl_line(&mut env, input) {
            Ok(output) => {
                if !output.is_empty() {
                    println!("{}", output);
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
            }
        }
    }

    Ok(())
}

fn execute_repl_line(env: &mut Env, source: &str) -> Result<String, String> {
    let program_key = env.load_program(source)?;
    let stack_key = env.create_stack(program_key)?;
    let result = env.run(stack_key)?;

    match result {
        Value::Nil => Ok(String::new()),
        _ => Ok(result.to_string()),
    }
}

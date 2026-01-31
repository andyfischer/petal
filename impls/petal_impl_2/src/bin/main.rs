use petal::{Env, StepResult, Value};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: petal <file.ptl> [--step]");
        eprintln!("       petal repl");
        process::exit(1);
    }

    match args[1].as_str() {
        "repl" => run_repl(),
        filename => {
            let step_mode = args.get(2).map(|s| s == "--step").unwrap_or(false);
            run_file(filename, step_mode);
        }
    }
}

fn run_file(filename: &str, step_mode: bool) {
    let source = match fs::read_to_string(filename) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("Error reading file '{}': {}", filename, err);
            process::exit(1);
        }
    };

    let mut env = Env::new();

    let program_key = match env.load_program(&source) {
        Ok(key) => key,
        Err(err) => {
            eprintln!("Parse error: {}", err);
            process::exit(1);
        }
    };

    let stack_key = match env.create_stack(program_key) {
        Ok(key) => key,
        Err(err) => {
            eprintln!("Stack creation error: {}", err);
            process::exit(1);
        }
    };

    if step_mode {
        println!("=== Step-by-step execution mode ===");
        let mut step_count = 0;

        loop {
            println!("\n--- Step {} ---", step_count);

            match env.step(stack_key) {
                Ok(StepResult::Continue) => {
                    step_count += 1;
                }
                Ok(StepResult::Complete) => {
                    let stack = env.get_stack(stack_key).unwrap();
                    println!("\nProgram completed!");
                    println!("Result: {:?}", stack.result);
                    break;
                }
                Ok(StepResult::Error) | Err(_) => {
                    eprintln!("Runtime error during execution");
                    process::exit(1);
                }
            }

            // Wait for user input to continue
            print!("Press Enter to continue...");
            io::stdout().flush().unwrap();
            let mut input = String::new();
            io::stdin().read_line(&mut input).unwrap();
        }
    } else {
        match env.run(stack_key) {
            Ok(value) => {
                if !matches!(value, Value::Nil) {
                    println!("{:?}", value);
                }
            }
            Err(err) => {
                eprintln!("Runtime error: {}", err);
                process::exit(1);
            }
        }
    }
}

fn run_repl() {
    println!("Petal REPL - type 'exit' to quit");

    let mut env = Env::new();
    let mut line_num = 0;
    let mut prev_stack_key = None;

    loop {
        print!("petal[{}]> ", line_num);
        io::stdout().flush().unwrap();

        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(err) => {
                eprintln!("Input error: {}", err);
                continue;
            }
        }

        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "exit" || input == "quit" {
            break;
        }

        let program_key = match env.load_program(input) {
            Ok(key) => key,
            Err(err) => {
                eprintln!("Parse error: {}", err);
                continue;
            }
        };

        let stack_key = match env.create_stack(program_key) {
            Ok(key) => key,
            Err(err) => {
                eprintln!("Stack creation error: {}", err);
                continue;
            }
        };

        // Copy globals from previous stack to maintain REPL state
        if let Some(prev_key) = prev_stack_key {
            let globals_copy = env.get_stack(prev_key).map(|s| s.globals.clone());
            if let Some(globals) = globals_copy {
                if let Some(new_stack) = env.get_stack_mut(stack_key) {
                    new_stack.globals = globals;
                }
            }
        }

        match env.run(stack_key) {
            Ok(value) => {
                println!("{:?}", value);
            }
            Err(err) => {
                eprintln!("Runtime error: {}", err);
            }
        }

        prev_stack_key = Some(stack_key);
        line_num += 1;
    }

    println!("Goodbye!");
}

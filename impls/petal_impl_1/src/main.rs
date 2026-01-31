//! Petal CLI - Run Petal programs

use std::env;
use std::fs;
use std::io::{self, BufRead, Write};

use petal::{Env, Value};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        // REPL mode
        run_repl();
    } else if args[1] == "--help" || args[1] == "-h" {
        print_help();
    } else if args[1] == "--version" || args[1] == "-v" {
        println!("Petal 0.1.0");
    } else if args[1] == "--trace" {
        if args.len() < 3 {
            eprintln!("Usage: petal --trace <file.ptl>");
            std::process::exit(1);
        }
        run_file_with_trace(&args[2]);
    } else if args[1] == "--slice" {
        if args.len() < 3 {
            eprintln!("Usage: petal --slice <file.ptl>");
            std::process::exit(1);
        }
        run_file_with_slice(&args[2]);
    } else {
        // File mode
        run_file(&args[1]);
    }
}

fn print_help() {
    println!("Petal Programming Language");
    println!();
    println!("Usage:");
    println!("  petal              Start REPL");
    println!("  petal <file.ptl>   Run a Petal script");
    println!("  petal --trace <file.ptl>  Run with execution trace");
    println!("  petal --slice <file.ptl>  Show program slicing/projection");
    println!("  petal --help       Show this help");
    println!("  petal --version    Show version");
    println!();
    println!("Examples:");
    println!("  petal examples/hello.ptl");
    println!("  petal --trace examples/provenance.ptl");
    println!("  petal --slice examples/provenance.ptl");
}

fn run_file(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", path, e);
            std::process::exit(1);
        }
    };

    let mut env = Env::new();

    match env.load_program(&source) {
        Ok(program_key) => {
            match env.create_stack(program_key) {
                Ok(stack_key) => {
                    match env.run(stack_key) {
                        Ok(result) => {
                            if !matches!(result, Value::Nil) {
                                let display = format_value(&env, &result);
                                println!("{}", display);
                            }
                        }
                        Err(e) => {
                            eprintln!("Runtime error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error creating stack: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_file_with_trace(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", path, e);
            std::process::exit(1);
        }
    };

    let mut env = Env::new();

    match env.load_program(&source) {
        Ok(program_key) => {
            match env.create_stack(program_key) {
                Ok(stack_key) => {
                    match env.run_with_tracing(stack_key) {
                        Ok((result, trace)) => {
                            // Print the result
                            if !matches!(result, Value::Nil) {
                                let display = format_value(&env, &result);
                                println!("Result: {}", display);
                            }

                            // Print trace summary
                            let summary = trace.summary();
                            println!();
                            println!("=== Execution Trace ===");
                            println!("Total steps: {}", summary.total_steps);
                            println!("Unique terms: {}", summary.unique_terms);
                            println!();

                            // Print each step
                            println!("Steps:");
                            for (i, step) in trace.all_steps().iter().enumerate() {
                                let inputs_str = if step.inputs.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        " <- [{}]",
                                        step.inputs
                                            .iter()
                                            .map(|id| format!("t{}", id.0))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )
                                };
                                println!(
                                    "  {:3}. t{}{} = {}",
                                    i + 1,
                                    step.term_id.0,
                                    inputs_str,
                                    format_value(&env, &step.output)
                                );
                            }

                            // Show provenance for the final term
                            if let Some(last_step) = trace.all_steps().last() {
                                println!();
                                println!("=== Provenance for t{} ===", last_step.term_id.0);
                                let influences = trace.get_influences(last_step.term_id);
                                if influences.is_empty() {
                                    println!("  (no dependencies)");
                                } else {
                                    println!(
                                        "  Influenced by: {}",
                                        influences
                                            .iter()
                                            .map(|id| format!("t{}", id.0))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Runtime error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error creating stack: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_file_with_slice(path: &str) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file '{}': {}", path, e);
            std::process::exit(1);
        }
    };

    let mut env = Env::new();

    match env.load_program(&source) {
        Ok(program_key) => {
            // First run with tracing to get dynamic information
            match env.create_stack(program_key) {
                Ok(stack_key) => {
                    match env.run_with_tracing(stack_key) {
                        Ok((result, trace)) => {
                            // Get the program entry point
                            let program = env.get_program(program_key).unwrap();
                            let entry = program.entry();

                            println!("=== Program Analysis ===");
                            println!("Entry point: t{}", entry.0);
                            println!();

                            // Backward slice from entry
                            let backward = env.backward_slice(program_key, entry).unwrap();
                            println!("Backward slice (what influences the result):");
                            println!("  {} terms included", backward.size());
                            let mut terms: Vec<_> = backward.included_terms.iter().collect();
                            terms.sort();
                            println!(
                                "  Terms: {}",
                                terms
                                    .iter()
                                    .map(|t| format!("t{}", t.0))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            println!();

                            // Dynamic slice using trace
                            if let Some(last_step) = trace.all_steps().last() {
                                let dynamic = env.dynamic_slice(&trace, last_step.term_id);
                                println!(
                                    "Dynamic slice (what was actually used for t{}):",
                                    last_step.term_id.0
                                );
                                println!("  {} terms included", dynamic.size());
                                let mut dyn_terms: Vec<_> = dynamic.included_terms.iter().collect();
                                dyn_terms.sort();
                                println!(
                                    "  Terms: {}",
                                    dyn_terms
                                        .iter()
                                        .map(|t| format!("t{}", t.0))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                );
                            }

                            println!();
                            println!("=== DOT Graph (backward slice) ===");
                            println!("{}", backward.to_dot(program));

                            if !matches!(result, Value::Nil) {
                                println!("Result: {}", format_value(&env, &result));
                            }
                        }
                        Err(e) => {
                            eprintln!("Runtime error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error creating stack: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_repl() {
    println!("Petal 0.1.0 - Type 'exit' or Ctrl+D to quit");
    println!();

    let mut env = Env::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("petal> ");
        stdout.flush().unwrap();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => {
                // EOF
                println!();
                break;
            }
            Ok(_) => {
                let line = line.trim();

                if line.is_empty() {
                    continue;
                }

                if line == "exit" || line == "quit" {
                    break;
                }

                match env.load_program(line) {
                    Ok(program_key) => {
                        match env.create_stack(program_key) {
                            Ok(stack_key) => {
                                match env.run(stack_key) {
                                    Ok(result) => {
                                        let display = format_value(&env, &result);
                                        println!("=> {}", display);
                                    }
                                    Err(e) => {
                                        eprintln!("Error: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Error: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }
}

fn format_value(env: &Env, value: &Value) -> String {
    petal::env::value_to_string(env, value)
}

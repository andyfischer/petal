mod ast;
mod interpreter;
mod lexer;
mod parser;
mod token;

use interpreter::Interpreter;
use lexer::Lexer;
use parser::Parser;
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};

fn run_file(path: &str) -> Result<(), String> {
    let source = fs::read_to_string(path)
        .map_err(|e| format!("Error reading file: {}", e))?;

    run(&source)
}

fn run(source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize();

    let mut parser = Parser::new(tokens);
    let program = parser.parse()
        .map_err(|e| format!("{}", e))?;

    let mut interpreter = Interpreter::new();
    interpreter.run(&program)
        .map_err(|e| format!("{}", e))?;

    Ok(())
}

fn run_repl() {
    println!("Petal Programming Language v0.1.0");
    println!("Type 'exit' or 'quit' to exit, 'help' for help.");
    println!();

    let mut interpreter = Interpreter::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("petal> ");
        stdout.flush().unwrap();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();

                if trimmed.is_empty() {
                    continue;
                }

                match trimmed {
                    "exit" | "quit" => break,
                    "help" => {
                        println!("Petal REPL Help:");
                        println!("  exit, quit  - Exit the REPL");
                        println!("  help        - Show this help message");
                        println!();
                        println!("Examples:");
                        println!("  let x = 42");
                        println!("  println(x * 2)");
                        println!("  fn square(n) {{ return n * n }}");
                        println!("  square(5)");
                        continue;
                    }
                    _ => {}
                }

                let mut lexer = Lexer::new(&line);
                let tokens = lexer.tokenize();

                let mut parser = Parser::new(tokens);
                match parser.parse() {
                    Ok(program) => {
                        match interpreter.run(&program) {
                            Ok(value) => {
                                // Only print non-null results
                                match &value {
                                    interpreter::Value::Null => {}
                                    _ => println!("{}", value),
                                }
                            }
                            Err(e) => eprintln!("Error: {}", e),
                        }
                    }
                    Err(e) => eprintln!("Parse error: {}", e),
                }
            }
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        }
    }

    println!("Goodbye!");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() > 1 {
        // Run file
        if let Err(e) = run_file(&args[1]) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    } else {
        // Run REPL
        run_repl();
    }
}

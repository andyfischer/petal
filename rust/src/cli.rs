//! CLI argument parsing and subcommand dispatch.

use std::fs;
use std::process;

use crate::compiler::Compiler;
use crate::env::Env;
use crate::ir_display::display_program;
use crate::lexer::Lexer;
use crate::native_fn::NativeFnTable;
use crate::parse::Parser;
use crate::program::ProgramId;

pub enum Command {
    Run,
    ShowIr { json: bool },
    ShowAst { json: bool },
    ShowTokens { json: bool },
}

pub enum SourceInput {
    File(String),
    Inline(String),
}

pub struct CliArgs {
    pub command: Command,
    pub source: SourceInput,
}

pub fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        print_usage();
        process::exit(1);
    }

    let first = &args[0];

    match first.as_str() {
        "run" => parse_run_args(&args[1..]),
        "show-ir" => parse_show_args(&args[1..], |json| Command::ShowIr { json }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        // Backward compat: petal -e <code> or petal <file>
        "-e" => {
            if args.len() < 2 {
                eprintln!("Usage: petal -e <code>");
                process::exit(1);
            }
            CliArgs {
                command: Command::Run,
                source: SourceInput::Inline(args[1].clone()),
            }
        }
        _ => {
            // Treat as file path
            CliArgs {
                command: Command::Run,
                source: SourceInput::File(first.clone()),
            }
        }
    }
}

fn parse_run_args(args: &[String]) -> CliArgs {
    if args.is_empty() {
        eprintln!("Usage: petal run <file>");
        process::exit(1);
    }
    if args[0] == "-e" {
        if args.len() < 2 {
            eprintln!("Usage: petal run -e <code>");
            process::exit(1);
        }
        CliArgs {
            command: Command::Run,
            source: SourceInput::Inline(args[1].clone()),
        }
    } else {
        CliArgs {
            command: Command::Run,
            source: SourceInput::File(args[0].clone()),
        }
    }
}

fn parse_show_args(args: &[String], make_cmd: impl Fn(bool) -> Command) -> CliArgs {
    let mut json = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "-e" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Expected code after -e");
                    process::exit(1);
                }
                source = Some(SourceInput::Inline(args[i].clone()));
            }
            _ => {
                source = Some(SourceInput::File(args[i].clone()));
            }
        }
        i += 1;
    }

    let source = source.unwrap_or_else(|| {
        eprintln!("Expected a file path or -e <code>");
        process::exit(1);
    });

    CliArgs {
        command: make_cmd(json),
        source,
    }
}

fn print_usage() {
    eprintln!("Usage: petal <command> [options] <file>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  run <file>                     Execute a program");
    eprintln!("  run -e <code>                  Execute inline code");
    eprintln!("  show-ir [--json] <file>        Display compiled IR");
    eprintln!("  show-ast [--json] <file>       Display parsed AST");
    eprintln!("  show-tokens [--json] <file>    Display lexer tokens");
    eprintln!();
    eprintln!("  petal <file>                   Shorthand for 'run'");
    eprintln!("  petal -e <code>                Shorthand for 'run -e'");
}

fn read_source(input: &SourceInput) -> String {
    match input {
        SourceInput::File(path) => match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading file '{}': {}", path, e);
                process::exit(1);
            }
        },
        SourceInput::Inline(code) => code.clone(),
    }
}

pub fn execute(cli: CliArgs) {
    let source = read_source(&cli.source);

    match cli.command {
        Command::Run => {
            let mut env = Env::new();
            if let Err(e) = env.run_source(&source) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        Command::ShowTokens { json } => {
            let mut lexer = Lexer::new(&source);
            match lexer.tokenize() {
                Ok(_) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&lexer.tokens).unwrap()
                        );
                    } else {
                        for (i, token) in lexer.tokens.iter().enumerate() {
                            println!("{}: {:?}", i, token);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Lexer error: {}", e);
                    process::exit(1);
                }
            }
        }
        Command::ShowAst { json } => {
            let mut lexer = Lexer::new(&source);
            if let Err(e) = lexer.tokenize() {
                eprintln!("Lexer error: {}", e);
                process::exit(1);
            }
            let mut parser = Parser::new(lexer.tokens);
            match parser.parse_program() {
                Ok(stmts) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&stmts).unwrap());
                    } else {
                        for stmt in &stmts {
                            println!("{:#?}", stmt);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Parse error: {}", e);
                    process::exit(1);
                }
            }
        }
        Command::ShowIr { json } => {
            let mut lexer = Lexer::new(&source);
            if let Err(e) = lexer.tokenize() {
                eprintln!("Lexer error: {}", e);
                process::exit(1);
            }
            let mut parser = Parser::new(lexer.tokens);
            let stmts = match parser.parse_program() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Parse error: {}", e);
                    process::exit(1);
                }
            };
            let compiler = Compiler::new();
            let mut natives = NativeFnTable::new();
            crate::builtins::register_builtins(&mut natives);
            let program = compiler.compile(&stmts, source.clone(), ProgramId(0), &natives);
            if json {
                println!("{}", serde_json::to_string_pretty(&program).unwrap());
            } else {
                print!("{}", display_program(&program));
            }
        }
    }
}

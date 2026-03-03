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
    ShowProvenance { json: bool, term: String },
    ShowDependents { json: bool, term: String },
    ShowSlice { json: bool, terms: Vec<String> },
    ShowGraph,
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
        "help" | "--help" | "-h" => {
            print_usage();
            process::exit(0);
        }
        "run" => parse_run_args(&args[1..]),
        "show-ir" => parse_show_args(&args[1..], |json| Command::ShowIr { json }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "show-provenance" => parse_provenance_args(&args[1..]),
        "show-dependents" => parse_term_query_args(&args[1..], |json, term| Command::ShowDependents { json, term }),
        "show-slice" => parse_slice_args(&args[1..]),
        "show-graph" => parse_show_args(&args[1..], |_json| Command::ShowGraph),
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

fn parse_provenance_args(args: &[String]) -> CliArgs {
    parse_term_query_args(args, |json, term| Command::ShowProvenance { json, term })
}

/// Parse args for commands that take --term, --json, and a source (provenance, dependents).
fn parse_term_query_args(args: &[String], make_cmd: impl Fn(bool, String) -> Command) -> CliArgs {
    let mut json = false;
    let mut source: Option<SourceInput> = None;
    let mut term: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--term" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Expected term name or id after --term");
                    process::exit(1);
                }
                term = Some(args[i].clone());
            }
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

    let term = term.unwrap_or_else(|| {
        eprintln!("Expected --term <name_or_id>");
        process::exit(1);
    });

    CliArgs {
        command: make_cmd(json, term),
        source,
    }
}

fn parse_slice_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut source: Option<SourceInput> = None;
    let mut terms: Vec<String> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--term" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Expected term name or id after --term");
                    process::exit(1);
                }
                terms.push(args[i].clone());
            }
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

    if terms.is_empty() {
        eprintln!("Expected at least one --term <name_or_id>");
        process::exit(1);
    }

    CliArgs {
        command: Command::ShowSlice { json, terms },
        source,
    }
}

fn print_usage() {
    let out = "\
Usage: petal <command> [options] <file>

Commands:
  run <file>                     Execute a program
  run -e <code>                  Execute inline code
  show-ir [--json] <file>        Display compiled IR
  show-ast [--json] <file>       Display parsed AST
  show-tokens [--json] <file>    Display lexer tokens
  show-provenance [--json] --term <name> <file>
                                 Trace provenance (backward slice) of a term
  show-dependents [--json] --term <name> <file>
                                 Trace dependents (forward slice) of a term
  show-slice [--json] --term <name> [--term <name2>] <file>
                                 Compute minimal dataflow slice for targets
  show-graph <file>              Output DOT-format dataflow graph

  petal <file>                   Shorthand for 'run'
  petal -e <code>                Shorthand for 'run -e'";
    eprintln!("{}", out);
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

/// Run the lexer, parser, and compiler pipeline. Returns the compiled Program.
fn compile_source(source: &str) -> crate::program::Program {
    let mut lexer = Lexer::new(source);
    if let Err(e) = lexer.tokenize() {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
    let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
    let stmts = match parser.parse_program() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };
    let compiler = Compiler::new();
    let mut natives = NativeFnTable::new();
    crate::builtins::register_builtins(&mut natives);
    compiler.compile(&stmts, source.to_string(), ProgramId(0), &natives)
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
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            }
        }
        Command::ShowAst { json } => {
            let mut lexer = Lexer::new(&source);
            if let Err(e) = lexer.tokenize() {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
            let mut parser = Parser::new(lexer.tokens, lexer.token_spans);
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
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            }
        }
        Command::ShowIr { json } => {
            let program = compile_source(&source);
            if json {
                println!("{}", serde_json::to_string_pretty(&program).unwrap());
            } else {
                print!("{}", display_program(&program));
            }
        }
        Command::ShowProvenance { json, term: term_query } => {
            let program = compile_source(&source);

            let root_id = match program.find_term(&term_query) {
                Some(id) => id,
                None => {
                    eprintln!("Term '{}' not found", term_query);
                    process::exit(1);
                }
            };

            let root_term = program.get_term(root_id);
            let (ancestor_ids, edges) = program.trace_provenance(root_id);

            if json {
                let root_json = term_to_json(root_term);
                let ancestors_json: Vec<_> = ancestor_ids
                    .iter()
                    .map(|&id| term_to_json(program.get_term(id)))
                    .collect();
                let edges_json: Vec<_> = edges
                    .iter()
                    .map(|(from, to)| {
                        serde_json::json!({ "from": from.0, "to": to.0 })
                    })
                    .collect();
                let output = serde_json::json!({
                    "root": root_json,
                    "ancestors": ancestors_json,
                    "edges": edges_json,
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                println!("Provenance of t{} ({}):", root_id.0,
                    root_term.name.as_deref().unwrap_or("unnamed"));
                println!("  op: {:?}", root_term.op);
                println!("  inputs: {:?}", root_term.inputs.iter().map(|i| i.0).collect::<Vec<_>>());
                println!();
                println!("Ancestors ({}):", ancestor_ids.len());
                for &aid in &ancestor_ids {
                    let t = program.get_term(aid);
                    println!("  t{}: {:?} {}", t.id.0, t.op,
                        t.name.as_deref().unwrap_or(""));
                }
                println!();
                println!("Edges ({}):", edges.len());
                for (from, to) in &edges {
                    println!("  t{} -> t{}", from.0, to.0);
                }
            }
        }
        Command::ShowDependents { json, term: term_query } => {
            let program = compile_source(&source);

            let root_id = match program.find_term(&term_query) {
                Some(id) => id,
                None => {
                    eprintln!("Term '{}' not found", term_query);
                    process::exit(1);
                }
            };

            let root_term = program.get_term(root_id);
            let (dependent_ids, edges) = program.trace_dependents(root_id);

            if json {
                let root_json = term_to_json(root_term);
                let dependents_json: Vec<_> = dependent_ids
                    .iter()
                    .map(|&id| term_to_json(program.get_term(id)))
                    .collect();
                let edges_json: Vec<_> = edges
                    .iter()
                    .map(|(from, to)| {
                        serde_json::json!({ "from": from.0, "to": to.0 })
                    })
                    .collect();
                let output = serde_json::json!({
                    "root": root_json,
                    "dependents": dependents_json,
                    "edges": edges_json,
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                println!("Dependents of t{} ({}):", root_id.0,
                    root_term.name.as_deref().unwrap_or("unnamed"));
                println!("  op: {:?}", root_term.op);
                println!();
                println!("Downstream ({}):", dependent_ids.len());
                for &did in &dependent_ids {
                    let t = program.get_term(did);
                    println!("  t{}: {:?} {}", t.id.0, t.op,
                        t.name.as_deref().unwrap_or(""));
                }
                println!();
                println!("Edges ({}):", edges.len());
                for (from, to) in &edges {
                    println!("  t{} -> t{}", from.0, to.0);
                }
            }
        }
        Command::ShowSlice { json, terms: term_queries } => {
            let program = compile_source(&source);

            let mut target_ids = Vec::new();
            for query in &term_queries {
                match program.find_term(query) {
                    Some(id) => target_ids.push(id),
                    None => {
                        eprintln!("Term '{}' not found", query);
                        process::exit(1);
                    }
                }
            }

            let slice_ids = program.slice(&target_ids);

            if json {
                let terms_json: Vec<_> = slice_ids
                    .iter()
                    .map(|&id| term_to_json(program.get_term(id)))
                    .collect();
                let output = serde_json::json!({
                    "targets": target_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                    "slice": terms_json,
                });
                println!("{}", serde_json::to_string_pretty(&output).unwrap());
            } else {
                println!("Slice for targets: {}", target_ids.iter()
                    .map(|id| format!("t{}", id.0))
                    .collect::<Vec<_>>()
                    .join(", "));
                println!();
                println!("Terms ({}):", slice_ids.len());
                for &sid in &slice_ids {
                    let t = program.get_term(sid);
                    println!("  t{}: {:?} {}", t.id.0, t.op,
                        t.name.as_deref().unwrap_or(""));
                }
            }
        }
        Command::ShowGraph => {
            let program = compile_source(&source);
            println!("{}", program_to_dot(&program));
        }
    }
}

/// Generate a DOT-format graph representation of the program's dataflow.
fn program_to_dot(program: &crate::program::Program) -> String {
    use std::fmt::Write;
    let mut dot = String::new();
    writeln!(dot, "digraph dataflow {{").unwrap();
    writeln!(dot, "  rankdir=TB;").unwrap();
    writeln!(dot, "  node [shape=box, fontname=\"monospace\", fontsize=10];").unwrap();
    writeln!(dot, "  edge [fontname=\"monospace\", fontsize=8];").unwrap();

    for term in &program.terms {
        let label = if let Some(ref name) = term.name {
            format!("t{}: {} ({:?})", term.id.0, name, term.op)
        } else {
            format!("t{}: {:?}", term.id.0, term.op)
        };
        // Escape quotes in label
        let label = label.replace('"', "\\\"");

        // Color by operation type
        let color = match &term.op {
            crate::program::TermOp::Constant(_) => "lightblue",
            crate::program::TermOp::StateInit | crate::program::TermOp::StateRead | crate::program::TermOp::StateWrite => "lightyellow",
            crate::program::TermOp::Call | crate::program::TermOp::MethodCall(_) => "lightgreen",
            crate::program::TermOp::Branch | crate::program::TermOp::Match => "lightsalmon",
            crate::program::TermOp::ForLoop | crate::program::TermOp::WhileLoop => "plum",
            crate::program::TermOp::MakeClosure(_) => "lightcoral",
            _ => "white",
        };

        writeln!(dot, "  t{} [label=\"{}\", style=filled, fillcolor={}];",
            term.id.0, label, color).unwrap();

        // Dataflow edges (input -> term)
        for input_id in &term.inputs {
            writeln!(dot, "  t{} -> t{};", input_id.0, term.id.0).unwrap();
        }

        // Control flow edges (term -> child blocks, dashed)
        for child_block in &term.child_blocks {
            let block = program.get_block(*child_block);
            if let Some(entry) = block.entry {
                writeln!(dot, "  t{} -> t{} [style=dashed, color=gray];",
                    term.id.0, entry.0).unwrap();
            }
        }
    }

    writeln!(dot, "}}").unwrap();
    dot
}

fn term_to_json(term: &crate::program::Term) -> serde_json::Value {
    // Simplified term representation for provenance output
    let op = serde_json::to_value(&term.op).unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "id": term.id.0,
        "op": op,
        "name": term.name,
        "inputs": term.inputs.iter().map(|i| i.0).collect::<Vec<_>>(),
    })
}

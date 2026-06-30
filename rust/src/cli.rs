//! CLI argument parsing and subcommand dispatch.

use std::fs;
use std::process;

use crate::compiler::Compiler;
use crate::dot_graph::program_to_dot;
use crate::env::Env;
use crate::ir_display::display_program_with;
use crate::lexer::Lexer;
use crate::native_fn::NativeFnTable;
use crate::parse::Parser;
use crate::program::ProgramId;

pub enum Command {
    Run { json: bool, trace: bool, record_trace: Option<String>, ir: bool, dup_stats: bool },
    Check { json: bool },
    Explain { json: bool, term: String },
    ShowIr { json: bool, all: bool },
    ShowAst { json: bool },
    ShowTokens { json: bool },
    ShowProvenance { json: bool, term: String },
    ShowDependents { json: bool, term: String },
    ShowSlice { json: bool, terms: Vec<String> },
    ShowGraph { all: bool },
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
        "check" => parse_show_args(&args[1..], |json| Command::Check { json }),
        "explain" => parse_term_query_args(&args[1..], |json, term| Command::Explain { json, term }),
        "show-ir" => parse_show_with_all(&args[1..], |json, all| Command::ShowIr { json, all }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "show-provenance" => parse_provenance_args(&args[1..]),
        "show-dependents" => parse_term_query_args(&args[1..], |json, term| Command::ShowDependents { json, term }),
        "show-slice" => parse_slice_args(&args[1..]),
        "show-graph" => parse_show_with_all(&args[1..], |_json, all| Command::ShowGraph { all }),
        _ => {
            // Shorthand: `petal <file>` runs the file (same as `petal run <file>`).
            CliArgs {
                command: Command::Run { json: false, trace: false, record_trace: None, ir: false, dup_stats: false },
                source: SourceInput::File(first.clone()),
            }
        }
    }
}

fn parse_run_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut trace = false;
    let mut record_trace: Option<String> = None;
    let mut ir = false;
    let mut dup_stats = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--trace" => trace = true,
            "--ir" => ir = true,
            "--dup-stats" => dup_stats = true,
            "--record-trace" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Expected path after --record-trace");
                    process::exit(1);
                }
                record_trace = Some(args[i].clone());
            }
            "-e" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Usage: petal run -e <code>");
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
        eprintln!("Usage: petal run [--json] [--trace] [--record-trace <path>] [--ir] [--dup-stats] <file>");
        process::exit(1);
    });

    CliArgs {
        command: Command::Run { json, trace, record_trace, ir, dup_stats },
        source,
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

/// Like `parse_show_args` but also accepts `--all` to include phantom builtin
/// terms in the output. Used by `show-ir` / `show-graph`.
fn parse_show_with_all(args: &[String], make_cmd: impl Fn(bool, bool) -> Command) -> CliArgs {
    let mut json = false;
    let mut all = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--all" => all = true,
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
        command: make_cmd(json, all),
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
  check [--json] <file>          Lex+parse+compile without executing (exit 0/1)
  run [--json] [--trace] [--record-trace <path>] [--ir] [--dup-stats] <file>
                                 Execute a program
                                 --ir: load <file> as JSON IR (show-ir --json
                                 output) instead of source; use '-' for stdin
                                 --dup-stats: print value-duplication stats to
                                 stderr after the run (debug builds / dup-stats
                                 feature)
  explain [--json] --term <name> <file>
                                 Run with trace, show value chain for a term
                                 --json: emit errors as structured JSON
                                 --trace: emit per-term events to stderr
                                 (PETAL_DEBUG=1 also enables trace)
  run -e <code>                  Execute inline code
  show-ir [--json] [--all] <file> Display compiled IR (--all to include builtin phantoms)
  show-ast [--json] <file>       Display parsed AST
  show-tokens [--json] <file>    Display lexer tokens
  show-provenance [--json] --term <name> <file>
                                 Trace provenance (backward slice) of a term
  show-dependents [--json] --term <name> <file>
                                 Trace dependents (forward slice) of a term
  show-slice [--json] --term <name> [--term <name2>] <file>
                                 Compute minimal dataflow slice for targets
  show-graph [--all] <file>      Output DOT-format dataflow graph (--all to include builtins)

  petal <file>                   Shorthand for 'run'";
    eprintln!("{}", out);
}

fn read_source(input: &SourceInput) -> String {
    match input {
        // "-" reads from stdin (e.g. `show-ir --json -e ... | petal run --ir -`).
        SourceInput::File(path) if path == "-" => {
            use std::io::Read;
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("Error reading stdin: {}", e);
                process::exit(1);
            }
            buf
        }
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
        Command::Run { json, trace, record_trace, ir, dup_stats } => {
            if trace || std::env::var("PETAL_DEBUG").is_ok() {
                unsafe { std::env::set_var("PETAL_TRACE", "1"); }
            }
            let mut env = Env::new();
            if record_trace.is_some() {
                env.trace_mut().enable();
            }
            let load_result = if ir {
                env.load_program_ir(&source)
            } else {
                env.load_program(&source)
            };
            let pid = match load_result {
                Ok(pid) => pid,
                Err(e) => {
                    let phase = classify_load_error(&e);
                    if json {
                        println!("{}", error_to_json(&e, phase));
                    } else {
                        eprintln!("Error: {}", e);
                    }
                    process::exit(1);
                }
            };
            let sid = match env.create_stack(pid) {
                Ok(sid) => sid,
                Err(e) => {
                    if json {
                        println!("{}", error_to_json(&e, "compile"));
                    } else {
                        eprintln!("Error: {}", e);
                    }
                    process::exit(1);
                }
            };
            let run_result = env.run(sid);

            if let Some(path) = &record_trace {
                write_trace_to_file(&env, pid, path);
            }

            if dup_stats {
                eprintln!("{}", env.dup_stats());
            }

            if let Err(e) = run_result {
                if json {
                    println!("{}", error_to_json(&e, "runtime"));
                } else {
                    eprintln!("Error: {}", e);
                }
                process::exit(1);
            }
        }
        Command::Explain { json, term: term_query } => {
            let mut env = Env::new();
            env.trace_mut().enable();
            let pid = match env.load_program(&source) {
                Ok(pid) => pid,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    process::exit(1);
                }
            };
            let sid = env.create_stack(pid).unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                process::exit(1);
            });
            // Run to completion (ignore errors — we still want the partial trace)
            let _ = env.run(sid);

            let program = env.get_program(pid).expect("program");
            let target_id = match program.find_term(&term_query) {
                Some(id) => id,
                None => term_not_found(program, &term_query),
            };

            let entries = env.trace().explain(program, env.heap(), target_id, 16);

            // Pretty header — use the resolved term name if available so an
            // `--term 72` query still shows `(total)` instead of `(72)`.
            let header_name = program
                .get_term(target_id)
                .name
                .clone()
                .unwrap_or_else(|| {
                    if term_query.parse::<u32>().is_ok() || term_query.starts_with('t') {
                        "unnamed".to_string()
                    } else {
                        term_query.clone()
                    }
                });

            if json {
                let entries_json: Vec<_> = entries.iter().map(|e| e.to_json()).collect();
                let out = serde_json::json!({
                    "term_id": target_id.0,
                    "name": header_name,
                    "chain": entries_json,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            } else {
                println!("Explain t{} ({}):", target_id.0, header_name);
                println!("  Provenance chain:");
                for (i, e) in entries.iter().enumerate() {
                    let loc = match (e.line, e.column) {
                        (Some(l), Some(c)) => format!("[line {}, column {}]", l, c),
                        _ => "[no location]".to_string(),
                    };
                    let name = e.name.as_deref().unwrap_or("-");
                    let value = e.value.as_deref().unwrap_or("<not executed>");
                    let arrow = if i == 0 { "=>" } else { " ." };
                    println!("    {} t{} {} {} = {}", arrow, e.term_id.0, name, loc, value);
                }
            }
        }
        Command::Check { json } => {
            let mut env = Env::new();
            let is_empty = source.trim().is_empty();
            match env.load_program(&source) {
                Ok(_) => {
                    if json {
                        if is_empty {
                            println!("{{\"ok\": true, \"warning\": \"empty program\"}}");
                        } else {
                            println!("{{\"ok\": true}}");
                        }
                    } else if is_empty {
                        eprintln!("warning: empty program");
                    }
                    // Otherwise silent on success, like most linters
                }
                Err(e) => {
                    let phase = classify_load_error(&e);
                    if json {
                        println!("{}", error_to_json(&e, phase));
                    } else {
                        eprintln!("Error: {}", e);
                    }
                    process::exit(1);
                }
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
        Command::ShowIr { json, all } => {
            let program = compile_source(&source);
            if json {
                println!("{}", serde_json::to_string_pretty(&program).unwrap());
            } else {
                print!("{}", display_program_with(&program, !all));
            }
        }
        Command::ShowProvenance { json, term: term_query } => {
            let program = compile_source(&source);

            let root_id = match program.find_term(&term_query) {
                Some(id) => id,
                None => term_not_found(&program, &term_query),
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
                None => term_not_found(&program, &term_query),
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
                    None => term_not_found(&program, query),
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
        Command::ShowGraph { all } => {
            let program = compile_source(&source);
            println!("{}", program_to_dot(&program, !all));
        }
    }
}

/// Classify a load_program error into "lex" or "parse" based on the message
/// shape. The lexer's error messages mention specific characters; parser
/// errors mention tokens and grammar expectations.
fn classify_load_error(e: &str) -> &'static str {
    if e.contains("Unexpected character")
        || e.contains("Unterminated")
        || e.contains("braced expression")
    {
        "lex"
    } else {
        "parse"
    }
}

/// Print a "not found" error for a `--term` lookup with a did-you-mean hint
/// listing up to 10 available named terms, then exit.
fn term_not_found(program: &crate::program::Program, query: &str) -> ! {
    eprintln!("Term '{}' not found", query);
    let names = program.named_terms();
    if !names.is_empty() {
        let shown: Vec<_> = names.iter().take(10).cloned().collect();
        let suffix = if names.len() > 10 {
            format!(", ... ({} more)", names.len() - 10)
        } else {
            String::new()
        };
        eprintln!("Available named terms: {}{}", shown.join(", "), suffix);
    }
    process::exit(1);
}

/// Write the Env's trace buffer to `path` as pretty-printed JSON.
fn write_trace_to_file(env: &Env, pid: crate::program::ProgramId, path: &str) {
    let Some(program) = env.get_program(pid) else {
        eprintln!("write_trace: program {} not found", pid.0);
        return;
    };
    let json = env.trace().to_json(program, env.heap());
    match serde_json::to_string_pretty(&json) {
        Ok(s) => {
            if let Err(e) = fs::write(path, s) {
                eprintln!("Failed to write trace to {}: {}", path, e);
            }
        }
        Err(e) => eprintln!("Failed to serialize trace: {}", e),
    }
}

/// Parse an error string into a structured JSON object.
/// Extracts `[line N, column M]`, `Caused by:` (provenance), and
/// `Stack trace:` suffixes produced by the evaluator, lexer, and parser.
fn error_to_json(err: &str, phase: &str) -> String {
    // Split off stack trace first (always last)
    let (head, stack) = match err.split_once("\nStack trace:") {
        Some((h, rest)) => (h.to_string(), split_indented_lines(rest)),
        None => (err.to_string(), Vec::new()),
    };

    // Split off provenance ("Caused by:") next
    let (head, caused_by) = match head.split_once("\nCaused by:") {
        Some((h, rest)) => (h.to_string(), split_indented_lines(rest)),
        None => (head, Vec::new()),
    };

    // Extract [line N, column M] from the primary message line
    let (message, line, column) = parse_line_column(&head);

    let obj = serde_json::json!({
        "error": true,
        "phase": phase,
        "message": message,
        "line": line,
        "column": column,
        "caused_by": caused_by,
        "stack": stack,
    });
    serde_json::to_string_pretty(&obj).unwrap()
}

fn split_indented_lines(s: &str) -> Vec<String> {
    s.lines()
        .map(|l| l.trim_start().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Extract `[line N, column M]` suffix from an error message.
fn parse_line_column(s: &str) -> (String, Option<u32>, Option<u32>) {
    if let Some(open) = s.rfind(" [line ") {
        let rest = &s[open + 7..];
        if let Some(close) = rest.find(']') {
            let inner = &rest[..close];
            // inner = "N, column M"
            if let Some((l, c)) = inner.split_once(", column ")
                && let (Ok(line), Ok(col)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>())
            {
                return (s[..open].to_string(), Some(line), Some(col));
            }
        }
    }
    (s.to_string(), None, None)
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

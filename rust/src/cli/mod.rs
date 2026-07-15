//! CLI argument parsing and subcommand dispatch.
//!
//! Argument parsing lives in [`args`]; the per-subcommand handlers live in
//! [`handlers`]. This module owns the shared vocabulary ([`Command`],
//! [`SourceInput`], [`CliArgs`]), the two public entry points [`parse_args`]
//! and [`execute`], and the error-reporting helpers ([`die`] / [`die_plain`]).

use std::fs;
use std::path::PathBuf;
use std::process;

mod args;
mod handlers;

pub enum Command {
    Run {
        json: bool,
        trace: bool,
        record_trace: Option<String>,
        ir: bool,
        dup_stats: bool,
        no_opt: bool,
        /// Turn on the pending absorption trace and print the frame pending
        /// report to stderr after the run (also enabled by `PETAL_TRACE_PENDING`).
        trace_pending: bool,
    },
    Check {
        json: bool,
        /// Exit non-zero when type-checker warnings exist (for CI). Plain
        /// `check` always exits 0.
        strict: bool,
    },
    Lint {
        fix: bool,
        check: bool,
    },
    Explain {
        json: bool,
        term: String,
    },
    ShowIr {
        json: bool,
        all: bool,
    },
    ShowBytecode {
        json: bool,
    },
    ShowAst {
        json: bool,
    },
    ShowTokens {
        json: bool,
    },
    ShowProvenance {
        json: bool,
        term: String,
    },
    ShowDependents {
        json: bool,
        term: String,
    },
    ShowSlice {
        json: bool,
        terms: Vec<String>,
    },
    ShowGraph {
        all: bool,
    },
    /// Run the program and emit the frame pending report (a JSON array of every
    /// live pending resource). The observability counterpart to `run`.
    PendingReport {
        json: bool,
    },
}

pub enum SourceInput {
    File(String),
    Inline(String),
}

pub struct CliArgs {
    pub command: Command,
    pub source: SourceInput,
    /// Module search directories from `-I <dir>` (see docs/module-system.md).
    pub include_dirs: Vec<PathBuf>,
}

pub fn parse_args() -> CliArgs {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    // Extract `-I <dir>` / `-I<dir>` module search paths uniformly, wherever
    // they appear; every subcommand that compiles accepts them.
    let mut args: Vec<String> = Vec::new();
    let mut include_dirs: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == "-I" {
            i += 1;
            if i >= raw.len() {
                eprintln!("Expected directory after -I");
                process::exit(1);
            }
            include_dirs.push(PathBuf::from(&raw[i]));
        } else if let Some(dir) = raw[i].strip_prefix("-I").filter(|d| !d.is_empty()) {
            include_dirs.push(PathBuf::from(dir));
        } else {
            args.push(raw[i].clone());
        }
        i += 1;
    }

    if args.is_empty() {
        print_usage();
        process::exit(1);
    }

    let mut cli = args::dispatch_args(&args);
    cli.include_dirs = include_dirs;
    cli
}

fn print_usage() {
    let out = "\
Usage: petal <command> [options] <file>

Commands:
  check [--json] <file>          Lex+parse+compile without executing (exit 0/1)
  run [--json] [--trace] [--record-trace <path>] [--ir] [--dup-stats] [--trace-pending] <file>
                                 Execute a program
                                 --ir: load <file> as JSON IR (show-ir --json
                                 output) instead of source; use '-' for stdin
                                 --dup-stats: print value-duplication and heap
                                 allocation stats to stderr after the run (debug
                                 builds / dup-stats feature)
                                 --trace-pending: record pending absorptions and
                                 print the frame pending report to stderr after
                                 the run (PETAL_TRACE_PENDING=1 also enables it)
  explain [--json] --term <name> <file>
                                 Run with trace, show value chain for a term
                                 --json: emit errors as structured JSON
                                 --trace: emit per-term events to stderr
                                 (PETAL_DEBUG=1 also enables trace)
  run -e <code>                  Execute inline code
  lint [--fix | --check] <file>  Normalize source (2-space indent, rebind `x = f(x)` -> `f(@x)`)
                                 default: report and exit 1 if changes needed
                                 --fix: rewrite the file in place
                                 --check: CI mode, exit 0/1 with no output on success
  lint -e <code>                 Lint inline code, print result to stdout
  show-ir [--json] [--all] <file> Display compiled IR (--all to include builtin phantoms)
  show-bytecode [--json] <file>  Display the bytecode lowering of the compiled IR
  show-ast [--json] <file>       Display parsed AST
  show-tokens [--json] <file>    Display lexer tokens
  show-provenance [--json] --term <name> <file>
                                 Trace provenance (backward slice) of a term
  show-dependents [--json] --term <name> <file>
                                 Trace dependents (forward slice) of a term
  show-slice [--json] --term <name> [--term <name2>] <file>
                                 Compute minimal dataflow slice for targets
  show-graph [--all] <file>      Output DOT-format dataflow graph (--all to include builtins)
  pending-report [--json] <file> Run the program and report every live pending
                                 resource (state, age, origin, absorbed count).
                                 --json emits the raw report array for tooling.

  petal <file>                   Shorthand for 'run'

Options accepted by every compiling command:
  -I <dir>                       Add a module search directory (repeatable).
                                 Imports also resolve from the importing
                                 file's directory and PETAL_PATH.";
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

/// Print an error and exit(1). In `--json` mode the error is emitted as a JSON
/// object tagged with `phase`; otherwise as a plain `Error: …` line on stderr.
fn die(json: bool, err: &str, phase: &str) -> ! {
    if json {
        println!("{}", error_to_json(err, phase));
    } else {
        eprintln!("Error: {}", err);
    }
    process::exit(1);
}

/// Print a plain `Error: …` line and exit(1), for commands with no JSON mode.
fn die_plain(err: &str) -> ! {
    eprintln!("Error: {}", err);
    process::exit(1);
}

pub fn execute(cli: CliArgs) {
    let CliArgs {
        command,
        source: source_input,
        include_dirs,
    } = cli;
    let source = read_source(&source_input);

    match command {
        Command::Run {
            json,
            trace,
            record_trace,
            ir,
            dup_stats,
            no_opt,
            trace_pending,
        } => {
            handlers::handle_run(
                json,
                trace,
                record_trace,
                ir,
                dup_stats,
                no_opt,
                trace_pending,
                &source,
                &source_input,
                &include_dirs,
            );
        }
        Command::PendingReport { json } => {
            handlers::handle_pending_report(json, &source, &source_input, &include_dirs);
        }
        Command::Explain { json, term } => {
            handlers::handle_explain(json, term, &source, &source_input, &include_dirs);
        }
        Command::Check { json, strict } => {
            handlers::handle_check(json, strict, &source, &source_input, &include_dirs);
        }
        Command::Lint { fix, check } => {
            handlers::handle_lint(fix, check, &source, &source_input, &include_dirs);
        }
        Command::ShowTokens { json } => {
            handlers::handle_show_tokens(json, &source);
        }
        Command::ShowAst { json } => {
            handlers::handle_show_ast(json, &source);
        }
        Command::ShowIr { json, all } => {
            handlers::handle_show_ir(json, all, &source, &source_input, &include_dirs);
        }
        Command::ShowBytecode { json } => {
            handlers::handle_show_bytecode(json, &source, &source_input, &include_dirs);
        }
        Command::ShowProvenance { json, term } => {
            handlers::handle_show_provenance(json, term, &source, &source_input, &include_dirs);
        }
        Command::ShowDependents { json, term } => {
            handlers::handle_show_dependents(json, term, &source, &source_input, &include_dirs);
        }
        Command::ShowSlice { json, terms } => {
            handlers::handle_show_slice(json, terms, &source, &source_input, &include_dirs);
        }
        Command::ShowGraph { all } => {
            handlers::handle_show_graph(all, &source, &source_input, &include_dirs);
        }
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

/// Extract a `[line N, column M]` (entry file) or `[file.ptl line N, column M]`
/// (imported module) suffix from an error message. Returns
/// (message, line, column) — the file name, when present, is left in the
/// message (a structured `file` field can follow in a later diagnostics pass).
fn parse_line_column(s: &str) -> (String, Option<u32>, Option<u32>) {
    if let Some(open) = s.rfind(" [line ") {
        let rest = &s[open + 7..];
        if let Some((line, col)) = parse_position_body(rest) {
            return (s[..open].to_string(), Some(line), Some(col));
        }
    }
    // Module-file variant: find the last "[...]" group whose body ends with
    // "line N, column M" after a file name.
    if let Some(open) = s.rfind(" [")
        && let Some(rel_line) = s[open..].find(" line ")
        && let Some((line, col)) = parse_position_body(&s[open + rel_line + 6..])
    {
        return (s[..open].to_string(), Some(line), Some(col));
    }
    (s.to_string(), None, None)
}

/// Parse `N, column M]...` into (N, M).
fn parse_position_body(rest: &str) -> Option<(u32, u32)> {
    let close = rest.find(']')?;
    let (l, c) = rest[..close].split_once(", column ")?;
    Some((l.trim().parse().ok()?, c.trim().parse().ok()?))
}

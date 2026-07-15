//! Command-line argument parsing: subcommand dispatch and the per-command
//! `parse_*` functions that build a `CliArgs`.

use std::process;

use super::{CliArgs, Command, SourceInput, print_usage};

pub(super) fn dispatch_args(args: &[String]) -> CliArgs {
    let first = &args[0];

    match first.as_str() {
        "help" | "--help" | "-h" => {
            print_usage();
            process::exit(0);
        }
        "run" => parse_run_args(&args[1..]),
        "check" => parse_check_args(&args[1..]),
        "lint" => parse_lint_args(&args[1..]),
        "explain" => {
            parse_term_query_args(&args[1..], |json, term| Command::Explain { json, term })
        }
        "show-ir" => parse_show_with_all(&args[1..], |json, all| Command::ShowIr { json, all }),
        "show-bytecode" => parse_show_args(&args[1..], |json| Command::ShowBytecode { json }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "show-provenance" => parse_provenance_args(&args[1..]),
        "show-dependents" => parse_term_query_args(&args[1..], |json, term| {
            Command::ShowDependents { json, term }
        }),
        "show-slice" => parse_slice_args(&args[1..]),
        "show-graph" => parse_show_with_all(&args[1..], |_json, all| Command::ShowGraph { all }),
        "pending-report" => parse_show_args(&args[1..], |json| Command::PendingReport { json }),
        _ => {
            // Shorthand: `petal <file> [flags]` runs the file (same as
            // `petal run <file> [flags]`). Parse the full arg list so flags
            // like `--no-opt` are honored, not silently dropped.
            parse_run_args(args)
        }
    }
}

fn parse_run_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut trace = false;
    let mut record_trace: Option<String> = None;
    let mut ir = false;
    let mut dup_stats = false;
    let mut no_opt = false;
    let mut trace_pending = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--trace" => trace = true,
            "--ir" => ir = true,
            "--dup-stats" => dup_stats = true,
            "--no-opt" => no_opt = true,
            "--trace-pending" => trace_pending = true,
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
        command: Command::Run {
            json,
            trace,
            record_trace,
            ir,
            dup_stats,
            no_opt,
            trace_pending,
        },
        source,
        include_dirs: Vec::new(),
    }
}

fn parse_lint_args(args: &[String]) -> CliArgs {
    let mut fix = false;
    let mut check = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--fix" => fix = true,
            "--check" => check = true,
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
        eprintln!("Usage: petal lint [--fix | --check] <file>  |  petal lint -e <code>");
        process::exit(1);
    });

    CliArgs {
        command: Command::Lint { fix, check },
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `check`: `--json` plus `--strict` (exit non-zero when
/// warnings exist).
fn parse_check_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut strict = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--strict" => strict = true,
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
        eprintln!("Usage: petal check [--json] [--strict] <file>  |  petal check -e <code>");
        process::exit(1);
    });

    CliArgs {
        command: Command::Check { json, strict },
        source,
        include_dirs: Vec::new(),
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
        include_dirs: Vec::new(),
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
        include_dirs: Vec::new(),
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
        include_dirs: Vec::new(),
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
        include_dirs: Vec::new(),
    }
}

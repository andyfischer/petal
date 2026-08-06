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
        "version" | "--version" | "-V" => {
            println!("petal {}", env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }
        "run" => parse_run_args(&args[1..]),
        "lsp" => parse_lsp_args(&args[1..]),
        "check" => parse_check_args(&args[1..]),
        "lint" => parse_lint_args(&args[1..]),
        "lint-fix" => parse_lint_fix_args(&args[1..]),
        "explain" => {
            parse_term_query_args(&args[1..], |json, term| Command::Explain { json, term })
        }
        "show-ir" => parse_show_with_all(&args[1..], |json, all| Command::ShowIr { json, all }),
        "show-bytecode" => parse_show_args(&args[1..], |json| Command::ShowBytecode { json }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "show-provenance" => parse_provenance_args(&args[1..]),
        "propose-edit" => parse_propose_edit_args(&args[1..]),
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
    let mut observe = false;
    let mut trace_emits = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--trace" => trace = true,
            "--observe" => observe = true,
            "--trace-emits" => trace_emits = true,
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
        eprintln!("Usage: petal run [--json] [--trace] [--record-trace <path>] [--observe] [--trace-emits] [--ir] [--dup-stats] <file>");
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
            observe,
            trace_emits,
        },
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `propose-edit`, the goal-based direct-manipulation query:
/// `--channel <name> --emit <n> --arg <k> --to <value>` are required;
/// `--configurable <var>` / `--static <var>` repeat; `--apply` rewrites the
/// file when the goal resolves to a single proposal.
fn parse_propose_edit_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal propose-edit --channel <name> --emit <n> --arg <k> --to <value> \
                 [--configurable <var>]* [--static <var>]* [--apply] [--json] <file>";
    let mut json = false;
    let mut apply = false;
    let mut channel: Option<String> = None;
    let mut emit: Option<usize> = None;
    let mut arg_index: Option<usize> = None;
    let mut to: Option<String> = None;
    let mut configurable: Vec<String> = Vec::new();
    let mut pinned: Vec<String> = Vec::new();
    let mut source: Option<SourceInput> = None;

    fn take<'a>(args: &'a [String], i: &mut usize, flag: &str, usage: &str) -> &'a str {
        *i += 1;
        if *i >= args.len() {
            eprintln!("Expected a value after {flag}. {usage}");
            process::exit(1);
        }
        &args[*i]
    }

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--apply" => apply = true,
            "--channel" => channel = Some(take(args, &mut i, "--channel", usage).to_string()),
            "--emit" => {
                emit = take(args, &mut i, "--emit", usage).parse().ok();
                if emit.is_none() {
                    eprintln!("--emit takes a 0-based index. {usage}");
                    process::exit(1);
                }
            }
            "--arg" => {
                arg_index = take(args, &mut i, "--arg", usage).parse().ok();
                if arg_index.is_none() {
                    eprintln!("--arg takes a 0-based index. {usage}");
                    process::exit(1);
                }
            }
            "--to" => to = Some(take(args, &mut i, "--to", usage).to_string()),
            "--configurable" => {
                configurable.push(take(args, &mut i, "--configurable", usage).to_string())
            }
            "--static" => pinned.push(take(args, &mut i, "--static", usage).to_string()),
            "-e" => {
                let code = take(args, &mut i, "-e", usage).to_string();
                source = Some(SourceInput::Inline(code));
            }
            _ => {
                source = Some(SourceInput::File(args[i].clone()));
            }
        }
        i += 1;
    }

    let (Some(channel), Some(emit), Some(arg), Some(to), Some(source)) =
        (channel, emit, arg_index, to, source)
    else {
        eprintln!("{usage}");
        process::exit(1);
    };

    CliArgs {
        command: Command::ProposeEdit {
            json,
            channel,
            emit,
            arg,
            to,
            configurable,
            pinned,
            apply,
        },
        source,
        include_dirs: Vec::new(),
    }
}

/// `lsp` takes no source: the server is fed documents over the protocol, so
/// the `source` slot is a placeholder that `execute` never reads.
fn parse_lsp_args(args: &[String]) -> CliArgs {
    if let Some(unexpected) = args.first() {
        eprintln!("Unexpected argument '{}'. Usage: petal lsp", unexpected);
        process::exit(1);
    }

    CliArgs {
        command: Command::Lsp,
        source: SourceInput::Inline(String::new()),
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

/// `lint-fix <file>` — `lint --fix` under its own name, because rewriting a
/// file in place is the thing most callers want and a flag is easy to forget.
/// It takes a path only: there is no file to rewrite for inline `-e` code.
fn parse_lint_fix_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal lint-fix <file>";
    let mut source: Option<SourceInput> = None;
    for arg in args {
        if arg.starts_with('-') {
            eprintln!("Unexpected option '{}'. {}", arg, usage);
            process::exit(1);
        }
        if source.is_some() {
            eprintln!("lint-fix takes a single file. {}", usage);
            process::exit(1);
        }
        source = Some(SourceInput::File(arg.clone()));
    }

    let source = source.unwrap_or_else(|| {
        eprintln!("{}", usage);
        process::exit(1);
    });

    CliArgs {
        command: Command::Lint {
            fix: true,
            check: false,
        },
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `check`: `--json`, `--strict` (exit non-zero when warnings
/// exist), and `--ir` (read JSON IR instead of source, as `run --ir` does).
fn parse_check_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut strict = false;
    let mut ir = false;
    let mut source: Option<SourceInput> = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--strict" => strict = true,
            "--ir" => ir = true,
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
        eprintln!("Usage: petal check [--json] [--strict] [--ir] <file>  |  petal check -e <code>");
        process::exit(1);
    });

    CliArgs {
        command: Command::Check { json, strict, ir },
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

//! Command-line argument parsing: subcommand dispatch and the per-command
//! `parse_*` functions that build a `CliArgs`.

use std::process;

use super::{CliArgs, Command, ErrorFormat, ProposeEditOpts, RunOpts, SourceInput, print_usage};

/// The "no source given" message shared by the show/query commands.
const MISSING_SOURCE: &str = "Expected a file path or -e <code>";

/// Parse a `--seed` value: decimal, or hex with a `0x` prefix (the same two
/// spellings `PETAL_SEED` accepts). Unlike the env var, a bad value here is a
/// hard error — the user typed it on this command line and meant it.
fn parse_seed(text: &str) -> u64 {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse::<u64>().ok(),
    };
    match parsed {
        Some(v) => v,
        None => {
            eprintln!(
                "Invalid --seed value '{}': expected a u64 (decimal or 0x-hex)",
                text
            );
            process::exit(1);
        }
    }
}

/// Parse a `--error-format` value.
fn parse_error_format(text: &str) -> ErrorFormat {
    match text {
        "full" => ErrorFormat::Full,
        "bare" => ErrorFormat::Bare,
        other => {
            eprintln!(
                "Invalid --error-format '{}': expected 'full' or 'bare'",
                other
            );
            process::exit(1);
        }
    }
}

/// Consume and return the value following a flag, exiting with `expected`
/// when the command line ends first.
fn take<'a>(args: &'a [String], i: &mut usize, expected: &str) -> &'a str {
    *i += 1;
    if *i >= args.len() {
        eprintln!("{expected}");
        process::exit(1);
    }
    &args[*i]
}

/// The argument loop shared by every source-taking command: `-e <code>`
/// becomes an inline source, and every other token is offered to `on_flag`,
/// which returns whether it recognized `args[*i]` (advancing `i` past a
/// flag's value via [`take`]). An unrecognized token falls through as a file
/// path — the historical contract of every per-command loop this replaced.
/// Exits with `usage` when no source was given.
fn parse_source_args(
    args: &[String],
    usage: &str,
    mut on_flag: impl FnMut(&[String], &mut usize) -> bool,
) -> SourceInput {
    let mut source: Option<SourceInput> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-e" {
            let code = take(args, &mut i, "Expected code after -e");
            source = Some(SourceInput::Inline(code.to_string()));
        } else if !on_flag(args, &mut i) {
            source = Some(SourceInput::File(args[i].clone()));
        }
        i += 1;
    }
    source.unwrap_or_else(|| {
        eprintln!("{usage}");
        process::exit(1);
    })
}

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
        "show-ir" => parse_show_ir_args(&args[1..]),
        "ir-equal" => parse_ir_equal_args(&args[1..]),
        "show-bytecode" => parse_show_args(&args[1..], |json| Command::ShowBytecode { json }),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "show-provenance" => parse_term_query_args(&args[1..], |json, term| {
            Command::ShowProvenance { json, term }
        }),
        "propose-edit" => parse_propose_edit_args(&args[1..]),
        "show-dependents" => parse_term_query_args(&args[1..], |json, term| {
            Command::ShowDependents { json, term }
        }),
        "show-slice" => parse_slice_args(&args[1..]),
        "show-graph" => parse_show_graph_args(&args[1..]),
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
    let usage = "Usage: petal run [--json] [--trace] [--record-trace <path>] [--observe] [--trace-emits] [--ir] [--dup-stats] [--profile] [--seed <n>] [--error-format full|bare] <file>";
    let mut o = RunOpts::default();
    let source = parse_source_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--json" => o.json = true,
            "--trace" => o.trace = true,
            "--observe" => o.observe = true,
            "--trace-emits" => o.trace_emits = true,
            "--ir" => o.ir = true,
            "--dup-stats" => o.dup_stats = true,
            "--profile" => o.profile = true,
            "--no-opt" => o.no_opt = true,
            "--trace-pending" => o.trace_pending = true,
            "--record-trace" => {
                o.record_trace =
                    Some(take(args, i, "Expected path after --record-trace").to_string())
            }
            "--seed" => o.seed = Some(parse_seed(take(args, i, "Expected a number after --seed"))),
            "--error-format" => {
                o.error_format = parse_error_format(take(
                    args,
                    i,
                    "Expected 'full' or 'bare' after --error-format",
                ))
            }
            _ => return false,
        }
        true
    });

    CliArgs {
        command: Command::Run(o),
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `propose-edit`, the goal-based direct-manipulation query:
/// `--channel <name> --emit <n>` and at least one `--arg <k> --to <value>`
/// pair are required. The pair repeats to state a multi-goal batch (one
/// gesture changing several arguments), each `--to` binding to the `--arg`
/// before it. `--configurable <var>` / `--static <var>` repeat; `--apply`
/// rewrites the file when every goal resolves to a single proposal.
fn parse_propose_edit_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal propose-edit --channel <name> --emit <n> (--arg <k> --to <value>)+ \
                 [--configurable <var>]* [--static <var>]* [--apply] [--json] <file>";
    let value_after = |flag: &str| format!("Expected a value after {flag}. {usage}");
    let mut json = false;
    let mut apply = false;
    let mut channel: Option<String> = None;
    let mut emit: Option<usize> = None;
    let mut goals: Vec<(usize, String)> = Vec::new();
    let mut pending_arg: Option<usize> = None;
    let mut configurable: Vec<String> = Vec::new();
    let mut pinned: Vec<String> = Vec::new();

    let source = parse_source_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--apply" => apply = true,
            "--channel" => {
                channel = Some(take(args, i, &value_after("--channel")).to_string());
            }
            "--emit" => {
                emit = take(args, i, &value_after("--emit")).parse().ok();
                if emit.is_none() {
                    eprintln!("--emit takes a 0-based index. {usage}");
                    process::exit(1);
                }
            }
            "--arg" => {
                if pending_arg.is_some() {
                    eprintln!("--arg given twice without a --to between them. {usage}");
                    process::exit(1);
                }
                pending_arg = take(args, i, &value_after("--arg")).parse().ok();
                if pending_arg.is_none() {
                    eprintln!("--arg takes a 0-based index. {usage}");
                    process::exit(1);
                }
            }
            "--to" => {
                let Some(arg_index) = pending_arg.take() else {
                    eprintln!("--to needs an --arg before it. {usage}");
                    process::exit(1);
                };
                goals.push((arg_index, take(args, i, &value_after("--to")).to_string()));
            }
            "--configurable" => {
                configurable.push(take(args, i, &value_after("--configurable")).to_string())
            }
            "--static" => pinned.push(take(args, i, &value_after("--static")).to_string()),
            _ => return false,
        }
        true
    });

    if pending_arg.is_some() {
        eprintln!("--arg without a matching --to. {usage}");
        process::exit(1);
    }
    let (Some(channel), Some(emit)) = (channel, emit) else {
        eprintln!("{usage}");
        process::exit(1);
    };
    if goals.is_empty() {
        eprintln!("At least one --arg <k> --to <value> pair is required. {usage}");
        process::exit(1);
    }

    CliArgs {
        command: Command::ProposeEdit(ProposeEditOpts {
            json,
            channel,
            emit,
            goals,
            configurable,
            pinned,
            apply,
        }),
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
    let mut verify: Option<crate::lint::VerifyMode> = None;
    let source = parse_source_args(
        args,
        "Usage: petal lint [--fix | --check] <file>  |  petal lint -e <code>",
        |args, i| {
            match args[*i].as_str() {
                "--fix" => fix = true,
                "--check" => check = true,
                "--verify" | "--verify=ir" => verify = Some(crate::lint::VerifyMode::Ir),
                "--verify=strict" => verify = Some(crate::lint::VerifyMode::Strict),
                other if other.starts_with("--verify=") => {
                    eprintln!(
                        "Unknown --verify mode '{}' (expected 'ir' or 'strict')",
                        &other["--verify=".len()..]
                    );
                    process::exit(1);
                }
                _ => return false,
            }
            true
        },
    );

    CliArgs {
        command: Command::Lint { fix, check, verify },
        source,
        include_dirs: Vec::new(),
    }
}

/// `ir-equal [--json] <a.ptl> <b.ptl>` — the two-file IR comparison. The
/// first path is the original (its spans are what diffs point at), the second
/// the rewritten side.
fn parse_ir_equal_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal ir-equal [--json] <a.ptl> <b.ptl>";
    let mut json = false;
    let mut paths: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other if other.starts_with("--") => {
                eprintln!("Unexpected option '{}'. {}", other, usage);
                process::exit(1);
            }
            other => paths.push(other.to_string()),
        }
    }
    if paths.len() != 2 {
        eprintln!("ir-equal takes exactly two files. {}", usage);
        process::exit(1);
    }
    CliArgs {
        command: Command::IrEqual {
            json,
            other: paths[1].clone(),
        },
        source: SourceInput::File(paths[0].clone()),
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
            verify: None,
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
    let mut error_format = ErrorFormat::Full;
    let source = parse_source_args(
        args,
        "Usage: petal check [--json] [--strict] [--ir] [--error-format full|bare] <file>  |  petal check -e <code>",
        |args, i| {
            match args[*i].as_str() {
                "--json" => json = true,
                "--strict" => strict = true,
                "--ir" => ir = true,
                "--error-format" => {
                    error_format = parse_error_format(take(
                        args,
                        i,
                        "Expected 'full' or 'bare' after --error-format",
                    ))
                }
                _ => return false,
            }
            true
        },
    );

    CliArgs {
        command: Command::Check {
            json,
            strict,
            ir,
            error_format,
        },
        source,
        include_dirs: Vec::new(),
    }
}

fn parse_show_args(args: &[String], make_cmd: impl Fn(bool) -> Command) -> CliArgs {
    let mut json = false;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| match args[*i].as_str() {
        "--json" => {
            json = true;
            true
        }
        _ => false,
    });

    CliArgs {
        command: make_cmd(json),
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `show-ir`: `--json`, `--all` (include phantom builtin terms
/// and prelude/module content), and `--user-only` (with `--json`: emit the
/// filtered user-only view instead of the complete interchange Program).
fn parse_show_ir_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut all = false;
    let mut user_only = false;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--all" => all = true,
            "--user-only" => user_only = true,
            _ => return false,
        }
        true
    });

    if user_only && !json {
        eprintln!(
            "--user-only requires --json (text output is already filtered; use --all to see everything)"
        );
        process::exit(1);
    }
    if user_only && all {
        eprintln!("--user-only and --all are mutually exclusive");
        process::exit(1);
    }

    CliArgs {
        command: Command::ShowIr {
            json,
            all,
            user_only,
        },
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `show-graph`: `--all` includes phantom builtin terms.
fn parse_show_graph_args(args: &[String]) -> CliArgs {
    let mut all = false;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--all" => all = true,
            // Previously parsed and silently discarded; saying so beats
            // emitting DOT to a caller that asked for JSON.
            "--json" => {
                eprintln!("show-graph has no --json output (it emits DOT)");
                process::exit(1);
            }
            _ => return false,
        }
        true
    });

    CliArgs {
        command: Command::ShowGraph { all },
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for commands that take --term, --json, and a source
/// (explain, provenance, dependents).
fn parse_term_query_args(args: &[String], make_cmd: impl Fn(bool, String) -> Command) -> CliArgs {
    let mut json = false;
    let mut term: Option<String> = None;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--term" => {
                term = Some(take(args, i, "Expected term name or id after --term").to_string());
            }
            _ => return false,
        }
        true
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
    let mut terms: Vec<String> = Vec::new();
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--term" => {
                terms.push(take(args, i, "Expected term name or id after --term").to_string());
            }
            _ => return false,
        }
        true
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

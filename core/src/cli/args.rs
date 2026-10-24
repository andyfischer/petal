//! Command-line argument parsing: subcommand dispatch and the per-command
//! `parse_*` functions that build a `CliArgs`.

use std::process;

use super::help;
use super::{CliArgs, Command, ErrorFormat, GraphQuery, ProposeEditOpts, RunOpts, SourceInput};

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

/// The command being parsed, for the "see 'petal help <command>'" pointer in
/// [`parse_source_args`]'s errors. Set once by [`dispatch_args`]; `run` when
/// the command line used the `petal <file>` shorthand.
static COMMAND: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// Exit over an argument no parser recognized, with the command's usage line
/// (when it has one) and a pointer to its help page.
fn reject_argument(problem: String, usage: &str) -> ! {
    let command = COMMAND.get().copied().unwrap_or("run");
    eprintln!("{problem}");
    if usage.starts_with("Usage:") {
        eprintln!("{usage}");
    }
    eprintln!("See 'petal help {command}'.");
    process::exit(1);
}

/// The argument loop shared by every source-taking command: `-e <code>`
/// becomes an inline source, and every other token is offered to `on_flag`,
/// which returns whether it recognized `args[*i]` (advancing `i` past a
/// flag's value via [`take`]). An unrecognized token is the file path —
/// unless it looks like an option, or a source was already given. Both used
/// to be taken as the path, so `petal run x.ptl --iter 3` answered
/// `Error reading file '3'`; now the first stray token is named. A lone `-`
/// is still a path (stdin).
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
            if source.is_some() {
                reject_argument(
                    "Unexpected '-e': a source was already given (one file or one -e <code>)"
                        .to_string(),
                    usage,
                );
            }
            source = Some(SourceInput::Inline(code.to_string()));
        } else if !on_flag(args, &mut i) {
            let arg = &args[i];
            if arg.starts_with('-') && arg != "-" {
                reject_argument(format!("Unknown option '{arg}'"), usage);
            }
            if source.is_some() {
                reject_argument(
                    format!(
                        "Unexpected argument '{arg}': a source was already given (one file or \
                         one -e <code>)"
                    ),
                    usage,
                );
            }
            source = Some(SourceInput::File(arg.clone()));
        }
        i += 1;
    }
    source.unwrap_or_else(|| {
        eprintln!("{usage}");
        process::exit(1);
    })
}

/// What to say about a first word that is neither a command nor a file:
/// `None` when it should be run as a file after all. A word that names an
/// existing path, ends in `.ptl`, or is an option (`-e`, `--json`, `-`) keeps
/// the `petal <file>` shorthand, so a missing `script.ptl` is still reported
/// as a missing file.
fn unknown_command_message(word: &str) -> Option<String> {
    if word.starts_with('-') || word.ends_with(".ptl") || std::path::Path::new(word).exists() {
        return None;
    }
    let mut msg = format!("petal: '{word}' is not a petal command or a file.");
    // Commands people reach for from other toolchains, which have no namesake
    // here but do have an answer.
    let pointer = match word {
        "repl" | "eval" | "exec" => {
            Some("petal has no REPL; 'petal run -e <code>' runs a snippet.".to_string())
        }
        "tests" | "spec" => Some("Did you mean 'petal test'?".to_string()),
        "build" | "compile" => {
            Some("Did you mean 'petal check' (compile without running)?".to_string())
        }
        "format" => Some("Did you mean 'petal fmt'?".to_string()),
        _ => help::closest_command(word).map(|c| format!("Did you mean 'petal {c}'?")),
    };
    if let Some(pointer) = pointer {
        msg.push(' ');
        msg.push_str(&pointer);
    }
    msg.push_str(" See 'petal help'.");
    Some(msg)
}

pub(super) fn dispatch_args(args: &[String]) -> CliArgs {
    let first = &args[0];
    let _ = COMMAND.set(help::command_name(first).unwrap_or("run"));

    // `petal <command> --help` reads as a request for that command's page,
    // wherever the flag sits — the parsers below would otherwise take it for
    // a file path.
    if args.len() > 1
        && help::is_command(first)
        && args[1..].iter().any(|a| a == "--help" || a == "-h")
    {
        help::print_command_help(first);
    }

    match first.as_str() {
        "help" | "--help" | "-h" => match args.get(1) {
            Some(topic) => help::print_command_help(topic),
            None => {
                help::print_usage(true);
                process::exit(0);
            }
        },
        "version" | "--version" | "-V" => {
            // The commit and the build date too: a stale binary on the PATH
            // looks exactly like a current one without them.
            println!("{}", crate::version::describe());
            process::exit(0);
        }
        "run" => parse_run_args(&args[1..]),
        "bench" => parse_bench_args(&args[1..]),
        "lsp" => parse_lsp_args(&args[1..]),
        "packages" => parse_packages_args(&args[1..]),
        "init" | "add" | "remove" | "install" | "update" | "vendor" | "bundle" => {
            super::pm::parse(first, &args[1..])
        }
        "check" => parse_check_args(&args[1..]),
        "fmt" => parse_fmt_args(&args[1..]),
        "lint" => parse_lint_args(&args[1..]),
        "test" => parse_test_args(&args[1..]),
        "fix-imports" => parse_fix_imports_args(&args[1..]),
        "suggest" => parse_suggest_args(&args[1..]),
        "apply-change" => parse_apply_change_args(&args[1..]),
        "explain" => {
            parse_term_query_args(&args[1..], |json, term| Command::Explain { json, term })
        }
        "show-ir" => parse_show_ir_args(&args[1..]),
        "ir-equal" => parse_ir_equal_args(&args[1..]),
        "show-bytecode" => parse_show_args(&args[1..], |json| Command::ShowBytecode { json }),
        "show-static" => parse_show_static_args(&args[1..]),
        "show-ast" => parse_show_args(&args[1..], |json| Command::ShowAst { json }),
        "show-tokens" => parse_show_args(&args[1..], |json| Command::ShowTokens { json }),
        "graph" => parse_graph_args(&args[1..]),
        "propose-edit" => parse_propose_edit_args(&args[1..]),
        "show-graph" => parse_show_graph_args(&args[1..]),
        "pending-report" => parse_show_args(&args[1..], |json| Command::PendingReport { json }),
        _ => {
            // Shorthand: `petal <file> [flags]` runs the file (same as
            // `petal run <file> [flags]`). Parse the full arg list so flags
            // like `--no-opt` are honored, not silently dropped. A word that
            // is no file either is a mistyped command, and says so rather
            // than failing later with "Error reading file 'repl'".
            if let Some(msg) = unknown_command_message(first) {
                eprintln!("{msg}");
                process::exit(1);
            }
            parse_run_args(args)
        }
    }
}

fn parse_run_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal run [--json] [--trace] [--record-trace <path>] [--observe] [--trace-emits] [--ir] [--dup-stats] [--profile] [--profile-slow <us>] [--effect-audit] [--static-audit] [--static-only] [--seed <n>] [--runs <n>] [--policy <name>] [--state-storage <file>] [--error-format full|bare] <file>";
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
            "--profile-slow" => {
                let text = take(args, i, "Expected a number of microseconds after --profile-slow");
                let us = text.parse::<f64>().ok().filter(|us| us.is_finite() && *us >= 0.0);
                o.profile_slow_us = Some(us.unwrap_or_else(|| {
                    eprintln!(
                        "Invalid --profile-slow value '{text}': expected a number of \
                         microseconds (0.5, 1, 250)"
                    );
                    process::exit(1)
                }));
                o.profile = true;
            }
            "--effect-audit" => o.effect_audit = true,
            "--static-audit" => {
                if o.static_audit == crate::static_audit::StaticAuditMode::Off {
                    o.static_audit = crate::static_audit::StaticAuditMode::Watch;
                }
            }
            "--static-only" => {
                o.static_audit = crate::static_audit::StaticAuditMode::StaticOnly;
            }
            "--no-opt" => o.policy = Some(crate::policy::RunPolicy::BASELINE),
            "--policy" => {
                let spec = take(args, i, "Expected a run policy after --policy");
                o.policy = Some(crate::policy::RunPolicy::parse(spec).unwrap_or_else(|e| {
                    eprintln!("{e}");
                    process::exit(1)
                }))
            }
            "--trace-pending" => o.trace_pending = true,
            "--record-trace" => {
                o.record_trace =
                    Some(take(args, i, "Expected path after --record-trace").to_string())
            }
            "--seed" => o.seed = Some(parse_seed(take(args, i, "Expected a number after --seed"))),
            "--runs" => {
                let text = take(args, i, "Expected a run count after --runs");
                o.runs = Some(text.parse::<u32>().ok().filter(|n| *n > 0).unwrap_or_else(|| {
                    eprintln!("Invalid --runs value '{text}': expected a count of at least 1");
                    process::exit(1)
                }))
            }
            "--state-storage" => {
                o.state_storage =
                    Some(take(args, i, "Expected a file path after --state-storage").to_string())
            }
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

/// `bench --fn <name> [--fn <name>]... [--iters N] [--seed <n>]
/// [--json] <file>` — measure what a call of each named function costs.
fn parse_bench_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal bench --fn <name> [--fn <name>]... [--iters <n>] \
                 [--seed <n>] [--json] <file>";
    let mut o = super::BenchOpts::default();
    let source = parse_source_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--json" => o.json = true,
            "--fn" => o
                .fns
                .push(take(args, i, "Expected a function name after --fn").to_string()),
            "--iters" => {
                let text = take(args, i, "Expected a run count after --iters");
                match text.parse::<u32>() {
                    Ok(n) if n > 0 => o.iters = Some(n),
                    _ => {
                        eprintln!("Invalid --iters value '{text}': expected a positive integer");
                        process::exit(1);
                    }
                }
            }
            "--seed" => o.seed = Some(parse_seed(take(args, i, "Expected a number after --seed"))),
            _ => return false,
        }
        true
    });
    if o.fns.is_empty() {
        eprintln!("bench needs at least one --fn <name>. {usage}");
        process::exit(1);
    }

    CliArgs {
        command: Command::Bench(o),
        source,
        include_dirs: Vec::new(),
    }
}

/// Parse args for `propose-edit`, the goal-based direct-manipulation query.
///
/// Every goal is `--channel <name> --emit <n>`, an address (`--arg <k>`, and
/// for v2 also `--native-arg <k>` or `--value`, optionally `--field <path>`),
/// then `--to <value>` (or, v2 only, `--by <delta>`). Goals repeat; each binds
/// to the channel and emit given last before it.
///
/// Two engines answer it. v2 (the default; `--v2` is accepted and changes
/// nothing) re-runs the program speculatively and solves through function
/// calls (docs/dev/direct-manipulation-v2-plan.md §4.2); its flags are parsed
/// by [`super::dm_v2::V2Parser`]. v1 (legacy, `--v1` only) inverts the
/// arithmetic of the emitting call's arguments. Under v1, `--configurable` /
/// `--static` name preferred / pinned variables; under v2 they read as
/// `--allow` / `--deny`, which take any selector.
fn parse_propose_edit_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal propose-edit [--v1|--v2] --channel <name> --emit <n> \
                 ((--arg|--native-arg <k> | --value) [--field <path>] (--to <value> | --by <delta>))+ \
                 [--allow <sel>]* [--deny <sel>]* [--configurable <var>]* [--static <var>]* \
                 [--alternatives N] [--explain] [--apply [--pick N]] [--json] <file>";
    let mut engine: Option<&'static str> = None;
    let mut parser = super::dm_v2::V2Parser::new();
    let mut configurable: Vec<String> = Vec::new();
    let mut pinned: Vec<String> = Vec::new();

    let source = parse_source_args(args, usage, |args, i| {
        match args[*i].as_str() {
            flag @ ("--v1" | "--v2") => {
                let e = if flag == "--v1" { "v1" } else { "v2" };
                if engine.is_some_and(|prev| prev != e) {
                    eprintln!("--v1 and --v2 are exclusive. {usage}");
                    process::exit(1);
                }
                engine = Some(e);
                return true;
            }
            // v1's spellings; v2 reads them as --allow / --deny.
            "--configurable" => {
                let v = take(args, i, "Expected a value after --configurable").to_string();
                parser.opts.allow.push(v.clone());
                configurable.push(v);
                return true;
            }
            "--static" => {
                let v = take(args, i, "Expected a value after --static").to_string();
                parser.opts.deny.push(v.clone());
                pinned.push(v);
                return true;
            }
            _ => {}
        }
        match parser.flag(args, i) {
            Ok(consumed) => consumed,
            Err(e) => {
                eprintln!("{e} {usage}");
                process::exit(1);
            }
        }
    });

    let v2_only = parser.v2_only.clone();
    let multi_emit = parser.multi_emit;
    let v2 = match parser.finish() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}. {usage}");
            process::exit(1);
        }
    };
    let use_v2 = match engine {
        Some("v2") => true,
        Some(_) => {
            if let Some(flag) = v2_only.first() {
                eprintln!("{flag} needs the v2 engine (drop --v1). {usage}");
                process::exit(1);
            }
            if multi_emit {
                eprintln!("v1 answers goals on one emit only (drop --v1 for several). {usage}");
                process::exit(1);
            }
            false
        }
        // v2 is the default engine (docs/dev/direct-manipulation-v2-plan.md §9).
        None => true,
    };

    let first = &v2.goals[0];
    let mut goals: Vec<(usize, String)> = Vec::new();
    if !use_v2 {
        for g in &v2.goals {
            match (&g.addr, &g.value) {
                (super::dm_v2::GoalAddr::Arg(k), super::dm_v2::GoalValue::To(t)) => goals.push((*k, t.clone())),
                _ => unreachable!("v2-only goal forms select v2"),
            }
        }
    }

    CliArgs {
        command: Command::ProposeEdit(ProposeEditOpts {
            json: v2.json,
            channel: first.channel.clone(),
            emit: first.emit,
            goals,
            configurable,
            pinned,
            apply: v2.apply,
            v2: use_v2.then_some(v2),
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

fn parse_packages_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut path: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other if !other.starts_with('-') && path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!(
                    "Unexpected argument '{}'. Usage: petal packages [--json] [-I <dir>]... [path]",
                    other
                );
                process::exit(1);
            }
        }
    }

    CliArgs {
        command: Command::Packages { json, path },
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// `suggest [--only <kinds>] [--apply | --verify] [--json]
/// [--from <file>]... <file>` — suggest safe refactors. `--from` is
/// repeatable: each names another entry point to compile for its call sites,
/// which is how a library module (whose own compile has no callers) learns
/// what its parameters are passed.
fn parse_suggest_args(args: &[String]) -> CliArgs {
    let mut opts = super::SuggestArgs::default();
    let source = parse_source_args(
        args,
        "Usage: petal suggest [--only types|named-args|return-types|advice] [--apply | --verify] \
         [--json] [--from <file>]... <file>  |  \
         petal suggest -e <code>",
        |args, i| {
            match args[*i].as_str() {
                "--json" => opts.json = true,
                "--apply" => opts.apply = true,
                "--verify" => opts.verify = true,
                "--from" => {
                    opts.from.push(std::path::PathBuf::from(take(
                        args,
                        i,
                        "--from needs a file path",
                    )));
                }
                "--only" => {
                    let list = take(
                        args,
                        i,
                        "--only needs a kind: 'types', 'named-args', 'return-types' or 'advice'",
                    );
                    opts.kinds = crate::suggest::Kinds::parse(list).unwrap_or_else(|e| {
                        eprintln!("{e}");
                        process::exit(1);
                    });
                }
                _ => return false,
            }
            true
        },
    );

    CliArgs {
        command: Command::Suggest(opts),
        source,
        include_dirs: Vec::new(),
    }
}

/// `apply-change <operation> <file> [--target <path>] [--from <file>]...
/// [--dry-run]` — a refactor that changes behaviour. The first
/// bare word is the operation and the second the file; which flags an
/// operation needs is its own business (`handle_apply_change`).
fn parse_apply_change_args(args: &[String]) -> CliArgs {
    let mut opts = super::ApplyChangeArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => opts.dry_run = true,
            "--target" => {
                opts.target = Some(take(args, &mut i, "--target needs a path, e.g. step/vx").into());
            }
            "--from" => {
                opts.from.push(std::path::PathBuf::from(take(
                    args,
                    &mut i,
                    "--from needs a file path",
                )));
            }
            flag if flag.starts_with("--") => {
                eprintln!("Unknown option '{flag}'. See 'petal help apply-change'.");
                process::exit(1);
            }
            word if opts.operation.is_none() => opts.operation = Some(word.to_string()),
            word if opts.file.is_none() => opts.file = Some(word.to_string()),
            word => {
                eprintln!("Unexpected argument '{word}'. See 'petal help apply-change'.");
                process::exit(1);
            }
        }
        i += 1;
    }

    CliArgs {
        command: Command::ApplyChange(opts),
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// Split a `--flag=a,b` / `--flag a,b` list value.
fn rule_list(value: &str) -> Vec<String> {
    let rules: Vec<String> = value
        .split(',')
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect();
    for r in &rules {
        if !crate::lint::is_rule(r) {
            eprintln!("Unknown lint rule '{r}'. See 'petal lint --rules'.");
            process::exit(1);
        }
    }
    rules
}

/// The paths and `-e` code shared by `fmt` and `lint`. Any argument that is
/// not a flag is a path (a file, or a directory searched for `.ptl` files);
/// `-` reads stdin.
struct PathArgs {
    paths: Vec<String>,
    inline: Option<String>,
}

fn parse_path_args(
    args: &[String],
    usage: &str,
    mut on_flag: impl FnMut(&[String], &mut usize) -> bool,
) -> PathArgs {
    let mut out = PathArgs {
        paths: Vec::new(),
        inline: None,
    };
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-e" {
            out.inline = Some(take(args, &mut i, "Expected code after -e").to_string());
        } else if !on_flag(args, &mut i) {
            if args[i].starts_with('-') && args[i] != "-" {
                eprintln!("Unexpected option '{}'. {}", args[i], usage);
                process::exit(1);
            }
            out.paths.push(args[i].clone());
        }
        i += 1;
    }
    if out.inline.is_some() && !out.paths.is_empty() {
        eprintln!("-e takes no paths. {usage}");
        process::exit(1);
    }
    out
}

/// `fmt [--check] [--diff] [<path>…]` — gofmt/deno-fmt shaped: rewrite in
/// place by default, `.` when no path is given.
fn parse_fmt_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal fmt [--check] [--diff] [<path>...]  |  petal fmt -e <code>";
    let mut check = false;
    let mut diff = false;
    let p = parse_path_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--check" => check = true,
            "--diff" | "-d" => diff = true,
            _ => return false,
        }
        true
    });
    CliArgs {
        command: Command::Fmt {
            paths: p.paths,
            inline: p.inline,
            check,
            diff,
        },
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// `fix-imports [--check] [--diff] [<path>…]` — shaped like `fmt`.
fn parse_fix_imports_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal fix-imports [--check] [--diff] [-I <dir>]... [<path>...]  |  \
                 petal fix-imports -e <code>";
    let mut check = false;
    let mut diff = false;
    let p = parse_path_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--check" => check = true,
            "--diff" | "-d" => diff = true,
            _ => return false,
        }
        true
    });
    CliArgs {
        command: Command::FixImports {
            paths: p.paths,
            inline: p.inline,
            check,
            diff,
        },
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

fn parse_lint_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal lint [--fix] [--json] [--rules-include=<r,..>] \
                 [--rules-exclude=<r,..>] [<path>...]  |  petal lint --rules  |  petal lint -e <code>";
    let mut opts = super::LintArgs::default();
    let p = parse_path_args(args, usage, |args, i| {
        let arg = args[*i].as_str();
        let value = |i: &mut usize, flag: &str| -> String {
            match arg.split_once('=') {
                Some((_, v)) => v.to_string(),
                None => take(args, i, &format!("Expected a rule list after {flag}")).to_string(),
            }
        };
        match arg {
            "--fix" => opts.fix = true,
            "--json" => opts.json = true,
            "--rules" => opts.list_rules = true,
            "--verify" | "--verify=ir" => opts.verify = Some(crate::lint::VerifyMode::Ir),
            "--verify=strict" => opts.verify = Some(crate::lint::VerifyMode::Strict),
            other if other.starts_with("--verify=") => {
                eprintln!(
                    "Unknown --verify mode '{}' (expected 'ir' or 'strict')",
                    &other["--verify=".len()..]
                );
                process::exit(1);
            }
            other if other == "--rules-include" || other.starts_with("--rules-include=") => {
                opts.rules.include = Some(rule_list(&value(i, "--rules-include")));
            }
            other if other == "--rules-exclude" || other.starts_with("--rules-exclude=") => {
                opts.rules.exclude = rule_list(&value(i, "--rules-exclude"));
            }
            _ => return false,
        }
        true
    });
    opts.paths = p.paths;
    opts.inline = p.inline;
    CliArgs {
        command: Command::Lint(opts),
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// `test [--filter <text>] [--json] [--list] [--show-output] [--timeout <s>]
/// [<path>…]` — `.` when no path is given.
fn parse_test_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal test [--filter <text>] [--json] [--list] [--show-output] \
                 [--timeout <seconds>] [<path>...]  |  petal test -e <code>";
    let mut opts = super::TestArgs::default();
    let p = parse_path_args(args, usage, |args, i| {
        let arg = args[*i].as_str();
        let value = |i: &mut usize, flag: &str, what: &str| -> String {
            match arg.split_once('=') {
                Some((_, v)) => v.to_string(),
                None => take(args, i, &format!("Expected {what} after {flag}")).to_string(),
            }
        };
        match arg {
            "--json" => opts.json = true,
            "--list" => opts.list = true,
            "--show-output" => opts.show_output = true,
            other if other == "--filter" || other.starts_with("--filter=") => {
                opts.filter = Some(value(i, "--filter", "text"));
            }
            other if other == "--timeout" || other.starts_with("--timeout=") => {
                let text = value(i, "--timeout", "a number of seconds");
                match text.parse::<f64>() {
                    Ok(s) if s >= 0.0 && s.is_finite() => opts.timeout = Some(s),
                    _ => {
                        eprintln!("--timeout takes a number of seconds (0 for none), got '{text}'");
                        process::exit(1);
                    }
                }
            }
            _ => return false,
        }
        true
    });
    opts.paths = p.paths;
    opts.inline = p.inline;
    CliArgs {
        command: Command::Test(opts),
        source: SourceInput::Inline(String::new()),
        include_dirs: Vec::new(),
    }
}

/// `ir-equal [--json] [--named-args] <a.ptl> <b.ptl>` — the
/// two-file IR comparison. The first path is the original (its spans are what
/// diffs point at), the second the rewritten side.
fn parse_ir_equal_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal ir-equal [--json] [--named-args] <a.ptl> <b.ptl>";
    let mut json = false;
    let mut named_args = false;
    let mut paths: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--named-args" => named_args = true,
            other if other.starts_with("--") => {
                eprintln!("Unexpected option '{}'. {}", other, usage);
                process::exit(1);
            }
            other => paths.push(other.to_string()),
        }
        i += 1;
    }
    if paths.len() != 2 {
        eprintln!("ir-equal takes exactly two files. {}", usage);
        process::exit(1);
    }
    CliArgs {
        command: Command::IrEqual {
            json,
            other: paths[1].clone(),
            named_args,
        },
        source: SourceInput::File(paths[0].clone()),
        include_dirs: Vec::new(),
    }
}

/// Parse args for `check`: `--json`, `--strict` (exit non-zero when warnings
/// exist), `--lenient` (exit 0 despite checker errors), and `--ir` (read JSON
/// IR instead of source, as `run --ir` does). Any number of files: the first
/// is the command's source, the rest go in `more` and are checked the same
/// way, each as its own program.
fn parse_check_args(args: &[String]) -> CliArgs {
    let usage = "Usage: petal check [--json] [--strict | --lenient] [--ir] [--error-format full|bare] <file>...  |  petal check -e <code>";
    let mut json = false;
    let mut strict = false;
    let mut lenient = false;
    let mut ir = false;
    let mut error_format = ErrorFormat::Full;
    let mut first_file = false;
    let mut more: Vec<String> = Vec::new();
    let source = parse_source_args(args, usage, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--strict" => strict = true,
            "--lenient" => lenient = true,
            "--ir" => ir = true,
            "--error-format" => {
                error_format = parse_error_format(take(
                    args,
                    i,
                    "Expected 'full' or 'bare' after --error-format",
                ))
            }
            arg if arg.starts_with('-') && arg != "-" => return false,
            // The first file is the source, which `parse_source_args` takes.
            _ if !first_file => {
                first_file = true;
                return false;
            }
            arg => more.push(arg.to_string()),
        }
        true
    });
    if !more.is_empty() {
        if matches!(&source, SourceInput::Inline(_)) {
            reject_argument(
                format!("Unexpected argument '{}': -e checks one snippet", more[0]),
                usage,
            );
        }
        if matches!(&source, SourceInput::File(f) if f == "-") || more.iter().any(|f| f == "-") {
            reject_argument("'-' (stdin) can only be checked on its own".to_string(), usage);
        }
    }

    CliArgs {
        command: Command::Check {
            json,
            strict,
            lenient,
            ir,
            error_format,
            more,
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

/// Parse args for `show-static`: `--json`, and `--all` to include the
/// declarations of imported modules.
fn parse_show_static_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut all = false;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--all" => all = true,
            _ => return false,
        }
        true
    });
    CliArgs {
        command: Command::ShowStatic { json, all },
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

/// `petal graph` and its three aliases. `alias` is the query an old command
/// name fixes; for `graph` itself the query follows from `--direction` and
/// how many `--term`s were given.
fn parse_graph_args(args: &[String]) -> CliArgs {
    let mut json = false;
    let mut terms: Vec<String> = Vec::new();
    let mut forward: Option<bool> = None;
    let mut slice = false;
    let source = parse_source_args(args, MISSING_SOURCE, |args, i| {
        match args[*i].as_str() {
            "--json" => json = true,
            "--term" => {
                terms.push(take(args, i, "Expected term name or id after --term").to_string());
            }
            "--slice" => slice = true,
            "--direction" => {
                let dir = take(args, i, "Expected back or forward after --direction");
                forward = Some(match dir {
                    "back" | "backward" => false,
                    "forward" => true,
                    other => {
                        eprintln!("Unknown --direction '{other}' (expected back or forward)");
                        process::exit(1);
                    }
                });
            }
            _ => return false,
        }
        true
    });

    if terms.is_empty() {
        eprintln!("Expected at least one --term <name_or_id>");
        process::exit(1);
    }
    if slice && forward == Some(true) {
        eprintln!("--slice is a backward query; it cannot be combined with --direction forward");
        process::exit(1);
    }

    let query = match (forward.unwrap_or(false), terms.len()) {
        (true, _) => GraphQuery::Dependents,
        (false, 1) if !slice => GraphQuery::Provenance,
        (false, _) => GraphQuery::Slice,
    };

    CliArgs {
        command: Command::Graph { json, terms, query },
        source,
        include_dirs: Vec::new(),
    }
}

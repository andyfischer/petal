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

/// How human-readable (non-`--json`) errors are printed.
///
/// `Bare` exists for differential testing: two sources that differ only in
/// indentation or blank lines must produce byte-identical error output, which
/// the position suffix and the echoed source line break. See
/// docs/dev/refactor-verification.md.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ErrorFormat {
    /// `Error: msg [line N, column M]` plus the caret snippet (the default).
    #[default]
    Full,
    /// Just the message text — no `Error:` prefix, no position, no snippet.
    Bare,
}

/// Set by [`execute`] from `--error-format`, read by the `die*` helpers.
/// A global rather than a parameter because every handler already funnels its
/// failures through `die`/`die_with`/`die_error`, and threading a mode through
/// all of them would touch far more code than it explains.
static BARE_ERRORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn set_error_format(format: ErrorFormat) {
    BARE_ERRORS.store(
        format == ErrorFormat::Bare,
        std::sync::atomic::Ordering::Relaxed,
    );
}

pub(super) fn bare_errors() -> bool {
    BARE_ERRORS.load(std::sync::atomic::Ordering::Relaxed)
}

pub enum Command {
    Run {
        json: bool,
        trace: bool,
        record_trace: Option<String>,
        ir: bool,
        dup_stats: bool,
        /// Count instructions, builtin calls and collections during the run and
        /// print the histogram to stderr afterwards (see `crate::profile`).
        profile: bool,
        no_opt: bool,
        /// Turn on the pending absorption trace and print the frame pending
        /// report to stderr after the run (also enabled by `PETAL_TRACE_PENDING`).
        trace_pending: bool,
        /// Record the last value bound to every named term and dump the lot
        /// after the run — "what is everything right now", including after a
        /// runtime error.
        observe: bool,
        /// Trace every buffered emit back to its call site and dump the values
        /// with their resolved source attribution after the run — the
        /// observation half of direct manipulation (docs/direct-manipulation.md).
        trace_emits: bool,
        /// Seed the run's PRNG (`--seed N`), so `random()` replays. Overrides
        /// `PETAL_SEED`; without either, the seed comes from the clock.
        seed: Option<u64>,
        /// How errors are printed (`--error-format full|bare`).
        error_format: ErrorFormat,
    },
    Check {
        json: bool,
        /// Exit non-zero when type-checker warnings exist (for CI). Plain
        /// `check` always exits 0.
        strict: bool,
        /// Load the input as JSON IR (`show-ir --json` output) instead of
        /// source, then check it lowers — same flag as `run --ir`.
        ir: bool,
        /// How errors are printed (`--error-format full|bare`).
        error_format: ErrorFormat,
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
        /// With `--json`: emit the filtered user-only view (phantom builtin
        /// terms and prelude/module content removed). A debugging view, not
        /// loadable by `run --ir`.
        user_only: bool,
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
    /// Run the program with emit tracing, then answer one or more
    /// manipulation goals — "this argument of the call that produced emit N
    /// should be VALUE" — with candidate source edits (see
    /// `crate::direct_manipulation`). Repeated `--arg`/`--to` pairs form a
    /// batch that must resolve consistently (a drag changes x and y at once).
    ProposeEdit {
        json: bool,
        /// Output channel the emit was pushed into (e.g. "draw_commands").
        channel: String,
        /// 0-based index of the emit within the channel's buffer.
        emit: usize,
        /// The goals, one per `--arg <k> --to <value>` pair: 0-based argument
        /// position and the value it should evaluate to, as source-ish text
        /// (`55`, `2.5`, `true`, `hello`).
        goals: Vec<(usize, String)>,
        /// Variables the host prefers to edit (`--configurable name`).
        configurable: Vec<String>,
        /// Variables that must not be edited (`--static name`).
        pinned: Vec<String>,
        /// Apply the edit to the file in place — only when exactly one
        /// proposal remains after policy filtering.
        apply: bool,
    },
    /// Serve the language server over stdio. Takes no source file — documents
    /// arrive over the protocol.
    Lsp,
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
  check [--json] [--strict] [--ir] [--error-format full|bare] <file>
                                 Lex+parse+compile+lower without executing
                                 (exit 0/1)
                                 --ir: check <file> as JSON IR (show-ir --json
                                 output) instead of source; use '-' for stdin
  run [--json] [--trace] [--record-trace <path>] [--observe] [--trace-emits] [--ir] [--dup-stats] [--trace-pending] [--seed <n>] [--error-format full|bare] <file>
                                 Execute a program
                                 --ir: load <file> as JSON IR (show-ir --json
                                 output) instead of source; use '-' for stdin
                                 --observe: after the run, dump the last value
                                 bound to every named variable, keyed by
                                 function-qualified name (fn-local 'x' inside
                                 'fn f' reads as 'f.x'). Dumped even when the
                                 run errors; --json emits it as an object
                                 --dup-stats: print value-duplication and heap
                                 allocation stats to stderr after the run (debug
                                 builds / dup-stats feature)
                                 --trace-pending: record pending absorptions and
                                 print the frame pending report to stderr after
                                 the run (PETAL_TRACE_PENDING=1 also enables it)
                                 --trace-emits: attribute every buffered emit
                                 (push_output / draw commands) to the call that
                                 produced it and dump values + call sites +
                                 per-argument edit info after the run; --json
                                 emits the structured report
                                 --seed <n>: seed the PRNG so random() replays
                                 (decimal or 0x-hex; PETAL_SEED=<n> does the
                                 same for every command, flag wins)
                                 --error-format bare: print only the error
                                 message on stderr, with no [line N, column M]
                                 suffix and no echoed source line / caret, so
                                 two sources differing only in layout fail
                                 identically. Also on 'check'.
  propose-edit --channel <name> --emit <n> (--arg <k> --to <value>)+
               [--configurable <var>]* [--static <var>]* [--apply] [--json] <file>
                                 Run with emit tracing, then propose source
                                 edits that make argument <k> of the call that
                                 produced emit <n> evaluate to <value>. Repeat
                                 --arg/--to pairs to state a multi-goal batch
                                 (one gesture changing several arguments),
                                 resolved consistently. Several proposals may
                                 come back when several variables feed a value;
                                 narrow with --configurable / --static, or
                                 declare knobs in-source with `config let`.
                                 --apply rewrites the file when every goal
                                 resolves to exactly one proposal.
  explain [--json] --term <name> <file>
                                 Run with trace, show value chain for a term
                                 --json: emit errors as structured JSON
                                 --trace: emit per-term events to stderr
                                 (PETAL_DEBUG=1 also enables trace)
  run -e <code>                  Execute inline code
  lint [--fix | --check] <file>  Normalize source (2-space indent, drop identity
                                 casts like int(n) where n is already an int)
                                 default: report and exit 1 if changes needed
                                 --fix: rewrite the file in place
                                 --check: CI mode, exit 0/1 with no output on success
  lint -e <code>                 Lint inline code, print result to stdout
  lint-fix <file>                Same as 'lint --fix <file>': rewrite in place.
                                 Makes no change if the file fails to parse.
  show-ir [--json] [--all] [--user-only] <file>
                                 Display compiled IR. Text output hides builtin
                                 phantom terms and the auto-loaded prelude /
                                 imported modules; --all restores them.
                                 --json emits the complete Program object (the
                                 `run --ir` interchange format); add --user-only
                                 for a filtered debugging view with phantoms,
                                 prelude content, and prelude-only constants
                                 removed (not loadable by `run --ir`)
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

  lsp                            Serve the language server over stdio
                                 (Content-Length-framed JSON-RPC). Editors
                                 spawn this; it takes no file.

  petal <file>                   Shorthand for 'run'
  version | --version | -V       Print the Petal version and exit

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
    die_with(json, err, phase, serde_json::Value::Null)
}

/// `die`, plus a `warnings` array on the JSON error object. Diagnostics
/// describe the source and do not depend on the phase that failed, so a
/// command that dies late (lowering, say) still reports them — otherwise a
/// broken file measures as warning-free.
fn die_with(json: bool, err: &str, phase: &str, warnings: serde_json::Value) -> ! {
    if json {
        let mut obj = error_json_value(err, phase);
        if !warnings.is_null() {
            obj["warnings"] = warnings;
        }
        println!("{}", serde_json::to_string_pretty(&obj).unwrap());
    } else if bare_errors() {
        eprintln!("{}", bare_error_text(err));
    } else {
        eprintln!("Error: {}", err);
    }
    process::exit(1);
}

/// Reduce a rendered error to just its message text: drop the `Caused by:` /
/// `Stack trace:` sections, drop the echoed source line and caret, and strip
/// the `[line N, column M]` suffix from every line that has one.
///
/// Position stripping reuses [`parse_line_column`], the same function the
/// `--json` error object uses for its `message` field, so the two views never
/// drift apart.
fn bare_error_text(err: &str) -> String {
    let head = err
        .split("\nCaused by:")
        .next()
        .unwrap_or(err)
        .split("\nStack trace:")
        .next()
        .unwrap_or(err);
    head.lines()
        .filter(|line| !is_snippet_line(line))
        .map(|line| parse_line_column(line).0)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Is this one of the three lines `format_source_snippet` emits (`  |`,
/// `2 | source`, `  |   ^^^`)? Everything before the gutter bar is a line
/// number or blanks, which no message line looks like.
fn is_snippet_line(line: &str) -> bool {
    match line.split_once('|') {
        Some((gutter, _)) => gutter.chars().all(|c| c.is_ascii_digit() || c == ' '),
        None => false,
    }
}

/// [`die_with`] for a typed front-end failure ([`crate::error::LoadError`]).
///
/// The phase is read off the error rather than guessed from its text (the old
/// `classify_load_error`, which tagged every compiler diagnostic `"parse"`).
/// Everything else about the output is unchanged: `message`, `line` and
/// `column` still come from rendering the error and stripping the trailing
/// position suffix, so `line`/`column` name the *last* diagnostic exactly as
/// the old `rfind` did. The one addition is `errors`, an array with one entry
/// per diagnostic — the structure a caller needs when the compiler reports
/// several at once.
fn die_error(
    json: bool,
    err: &crate::error::LoadError,
    warnings: serde_json::Value,
    source: &str,
) -> ! {
    let text = err.to_string();
    if !json {
        // Human output gets the same caret block a runtime error and a type
        // warning get. A parse error used to be the odd one out: a bare
        // `[line N, column M]` with nothing to look at.
        //
        // Only entry-file items can be underlined — a module's own source is
        // not what was handed to this command, and the span indexes it.
        let mut rendered = String::new();
        for (i, item) in err.items.iter().enumerate() {
            if i > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&item.to_string());
            if item.file.is_none()
                && let Some(span) = &item.span
                && let Some(snippet) = crate::backend::errors::format_source_snippet(source, span)
            {
                rendered.push('\n');
                rendered.push_str(&snippet);
            }
        }
        die_with(json, &rendered, err.phase.as_str(), warnings);
    }
    let mut obj = error_json_value(&text, err.phase.as_str());
    if !warnings.is_null() {
        obj["warnings"] = warnings;
    }
    obj["errors"] = serde_json::Value::Array(
        err.items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "message": item.message,
                    "line": item.span.map(|s| s.start.line),
                    "column": item.span.map(|s| s.start.column),
                    "file": item.file,
                })
            })
            .collect(),
    );
    println!("{}", serde_json::to_string_pretty(&obj).unwrap());
    process::exit(1);
}

/// Print a plain `Error: …` line and exit(1), for commands with no JSON mode.
fn die_plain(err: &str) -> ! {
    if bare_errors() {
        eprintln!("{}", bare_error_text(err));
        process::exit(1);
    }
    eprintln!("Error: {}", err);
    process::exit(1);
}

pub fn execute(cli: CliArgs) {
    let CliArgs {
        command,
        source: source_input,
        include_dirs,
    } = cli;

    // `lsp` has no source file to read — and reading stdin here would eat the
    // very stream the server needs.
    if let Command::Lsp = command {
        handlers::handle_lsp();
        return;
    }

    let source = read_source(&source_input);

    match command {
        Command::Run {
            json,
            trace,
            record_trace,
            ir,
            dup_stats,
            profile,
            no_opt,
            trace_pending,
            observe,
            trace_emits,
            seed,
            error_format,
        } => {
            set_error_format(error_format);
            handlers::handle_run(
                json,
                trace,
                record_trace,
                ir,
                dup_stats,
                profile,
                no_opt,
                trace_pending,
                observe,
                trace_emits,
                seed,
                &source,
                &source_input,
                &include_dirs,
            );
        }
        Command::ProposeEdit {
            json,
            channel,
            emit,
            goals,
            configurable,
            pinned,
            apply,
        } => {
            handlers::handle_propose_edit(
                json,
                &channel,
                emit,
                &goals,
                &configurable,
                &pinned,
                apply,
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
        Command::Check {
            json,
            strict,
            ir,
            error_format,
        } => {
            set_error_format(error_format);
            handlers::handle_check(json, strict, ir, &source, &source_input, &include_dirs);
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
        Command::ShowIr {
            json,
            all,
            user_only,
        } => {
            handlers::handle_show_ir(json, all, user_only, &source, &source_input, &include_dirs);
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
        // Handled above, before the source read.
        Command::Lsp => unreachable!(),
    }
}

/// Parse an error string into a structured JSON object.
/// Extracts `[line N, column M]`, `Caused by:` (provenance), and
/// `Stack trace:` suffixes produced by the evaluator, lexer, and parser.
pub(super) fn error_json_value(err: &str, phase: &str) -> serde_json::Value {
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

    serde_json::json!({
        "error": true,
        "phase": phase,
        "message": message,
        "line": line,
        "column": column,
        "caused_by": caused_by,
        "stack": stack,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_error_drops_position_snippet_and_sections() {
        let full = "Cannot access field 'foo' on int [line 2, column 7]\n  |\n2 | print(x.foo.bar)\n  |       ^^^^^\nCaused by:\n  x [line 1, column 9]";
        assert_eq!(bare_error_text(full), "Cannot access field 'foo' on int");
    }

    #[test]
    fn bare_error_keeps_every_diagnostic_of_a_multi_error_render() {
        let full = "Unexpected token: '=' [line 1, column 9]\n  |\n1 | let x = =\n  |         ^\nUnexpected token: ')' [line 3, column 2]";
        assert_eq!(
            bare_error_text(full),
            "Unexpected token: '='\nUnexpected token: ')'"
        );
    }

    #[test]
    fn bare_error_leaves_a_positionless_message_alone() {
        assert_eq!(bare_error_text("Stack not found"), "Stack not found");
    }
}

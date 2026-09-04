//! `petal --help`, `petal help` and `petal help <command>`.
//!
//! The top-level page is a one-line-per-command index, grouped by what the
//! user is trying to do; everything a command actually accepts lives on its
//! own page, reached with `petal help <command>` or `petal <command> --help`.
//! The layout follows `git help`: a short usage line, grouped summaries, and
//! man-page-shaped NAME / SYNOPSIS / DESCRIPTION / OPTIONS sections per
//! command.

use std::process;

/// The top-level command index. Each entry is a command name and the
/// one-line summary shown next to it; the groups are the sections of the
/// index page.
const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "run and check programs",
        &[
            ("run", "Execute a program"),
            ("check", "Compile a program without executing it"),
            ("lsp", "Serve the language server over stdio"),
            (
                "packages",
                "List the libraries the search path makes available",
            ),
        ],
    ),
    (
        "tidy and compare source",
        &[
            ("lint", "Report the source normalization a file needs"),
            ("lint-fix", "Apply the lint rewrite to a file in place"),
            ("ir-equal", "Compare two files' compiled IR for equivalence"),
        ],
    ),
    (
        "inspect the compiler's work",
        &[
            ("show-tokens", "Display lexer tokens"),
            ("show-ast", "Display the parsed AST"),
            ("show-ir", "Display the compiled IR"),
            ("show-bytecode", "Display the bytecode lowering of the IR"),
            ("show-graph", "Emit the dataflow graph in DOT format"),
        ],
    ),
    (
        "trace values and dataflow",
        &[
            ("explain", "Show the value chain that produced a term"),
            ("show-provenance", "Trace the backward slice of a term"),
            ("show-dependents", "Trace the forward slice of a term"),
            ("show-slice", "Compute the minimal slice for some targets"),
            (
                "pending-report",
                "Report every live pending resource after a run",
            ),
            (
                "propose-edit",
                "Propose source edits that change an emitted value",
            ),
        ],
    ),
];

/// Print the top-level index. `to_stdout` is true when help was asked for and
/// false when it is the reaction to a bad command line.
pub(super) fn print_usage(to_stdout: bool) {
    let mut out = String::from(
        "usage: petal [--version] [--help] <command> [<options>] <file>\n\n\
         These are the Petal commands used in various situations:\n",
    );
    for (group, commands) in GROUPS {
        out.push_str(&format!("\n{group}\n"));
        for (name, summary) in *commands {
            out.push_str(&format!("   {name:<16} {summary}\n"));
        }
    }
    out.push_str(
        "\n\
         'petal <file>' is shorthand for 'petal run <file>', and every command\n\
         that compiles also takes '-e <code>' in place of a file and '-I <dir>'\n\
         to add a module search directory.\n\n\
         See 'petal help <command>' to read about a specific command.",
    );
    if to_stdout {
        println!("{out}");
    } else {
        eprintln!("{out}");
    }
}

/// Print `petal help <command>`, or explain that there is no such command.
/// Exits the process either way — help is always the whole invocation.
pub(super) fn print_command_help(name: &str) -> ! {
    match page(name) {
        Some(text) => {
            println!("{}", text.replace("{COMMON}", COMMON).trim_end());
            process::exit(0);
        }
        None => {
            eprintln!("petal: '{name}' is not a petal command. See 'petal help'.");
            process::exit(1);
        }
    }
}

/// Is `name` a command with a help page? Used to decide whether a bare
/// `petal help <word>` is a topic or a typo.
pub(super) fn is_command(name: &str) -> bool {
    page(name).is_some()
}

fn page(name: &str) -> Option<&'static str> {
    Some(match name {
        "run" => RUN,
        "check" => CHECK,
        "lint" => LINT,
        "lint-fix" => LINT_FIX,
        "ir-equal" => IR_EQUAL,
        "explain" => EXPLAIN,
        "show-ir" => SHOW_IR,
        "show-bytecode" => SHOW_BYTECODE,
        "show-ast" => SHOW_AST,
        "show-tokens" => SHOW_TOKENS,
        "show-provenance" => SHOW_PROVENANCE,
        "show-dependents" => SHOW_DEPENDENTS,
        "show-slice" => SHOW_SLICE,
        "show-graph" => SHOW_GRAPH,
        "pending-report" => PENDING_REPORT,
        "propose-edit" => PROPOSE_EDIT,
        "lsp" => LSP,
        "packages" => PACKAGES,
        _ => return None,
    })
}

/// The `-I` / `-e` paragraph every compiling command repeats.
const COMMON: &str = "\
COMMON OPTIONS
       -e <code>
              Read the program from the command line instead of a file.

       -I <dir>
              Add a module search directory. Repeatable. Imports also
              resolve from the importing file's directory and PETAL_PATH.
";

const RUN: &str = "\
NAME
       petal-run - Execute a program

SYNOPSIS
       petal run [<options>] <file>
       petal run [<options>] -e <code>
       petal <file>

DESCRIPTION
       Compiles <file> and runs it. 'petal <file>' with no subcommand is
       shorthand for this command, and accepts the same options.

OPTIONS
       --json
              Emit errors as structured JSON. With --observe or
              --trace-emits, emit those reports as JSON too.

       --trace
              Emit per-term events to stderr. PETAL_DEBUG=1 does the same.

       --record-trace <path>
              Write the execution trace to <path>.

       --ir   Load <file> as JSON IR ('show-ir --json' output) rather than
              source. Use '-' to read the IR from stdin.

       --observe
              After the run, dump the last value bound to every named
              variable, keyed by function-qualified name: an fn-local 'x'
              inside 'fn f' reads as 'f.x'. Dumped even when the run errors.

       --trace-emits
              Attribute every buffered emit (push_output, draw commands) to
              the call that produced it, and dump the values with their call
              sites and per-argument edit info after the run.

       --trace-pending
              Record pending absorptions and print the frame pending report
              to stderr after the run. PETAL_TRACE_PENDING=1 does the same.

       --dup-stats
              Print value-duplication and heap allocation stats to stderr
              after the run. Debug builds / the dup-stats feature only.

       --profile
              Count instructions, builtin calls and collections during the
              run and print the histogram to stderr.

       --no-opt
              Skip the optimizer. Output must be identical either way, so a
              difference is a bug in an optimization pass.

       --seed <n>
              Seed the PRNG so random() replays. Decimal or 0x-hex.
              PETAL_SEED=<n> does the same for every command; the flag wins.

       --error-format full|bare
              'bare' prints only the error message on stderr, with no
              [line N, column M] suffix and no echoed source line or caret,
              so two sources differing only in layout fail identically.

{COMMON}
SEE ALSO
       petal help check, petal help show-ir, petal help pending-report
";

const CHECK: &str = "\
NAME
       petal-check - Compile a program without executing it

SYNOPSIS
       petal check [<options>] <file>
       petal check [<options>] -e <code>

DESCRIPTION
       Lexes, parses, compiles and lowers the program, then stops. Exits 0
       when it compiles and 1 when it does not, so it is the cheap gate for
       editors and CI.

OPTIONS
       --json
              Emit errors and warnings as structured JSON.

       --strict
              Exit non-zero when type-checker warnings exist. Plain 'check'
              exits 0 for a program that only has warnings.

       --ir   Check <file> as JSON IR ('show-ir --json' output) rather than
              source. Use '-' to read the IR from stdin.

       --error-format full|bare
              'bare' prints only the error message, with no position suffix
              and no source snippet. Same flag as 'run'.

{COMMON}
SEE ALSO
       petal help run, petal help lint
";

const LINT: &str = "\
NAME
       petal-lint - Report the source normalization a file needs

SYNOPSIS
       petal lint [--fix | --check] [--verify[=ir|strict]] <file>
       petal lint [<options>] -e <code>

DESCRIPTION
       Normalizes source: 2-space indentation, and semantic tidying such as
       dropping identity casts like int(n) where n is already an int. With
       no option it reports the change and exits 1 if one is needed. With
       -e it prints the linted code to stdout.

OPTIONS
       --fix  Rewrite the file in place. 'petal lint-fix' is the same thing
              under its own name.

       --check
              CI mode: exit 0 or 1 with no output on success.

       --verify[=ir|strict]
              Prove the rewrite before writing it, by compiling both sides
              and comparing their IR. Not provably equal means no write and
              exit 3.

              --verify=ir (the default) accepts the semantic passes
              (identity casts, if-chain to match) as expected-to-differ and
              proves only the formatting pass.

              --verify=strict demands IR equality of the whole rewrite, so a
              file with a semantic rewrite pending exits 3 and needs a
              run-diff instead.

{COMMON}
SEE ALSO
       petal help lint-fix, petal help ir-equal
";

const LINT_FIX: &str = "\
NAME
       petal-lint-fix - Apply the lint rewrite to a file in place

SYNOPSIS
       petal lint-fix <file>

DESCRIPTION
       The same as 'petal lint --fix <file>', under its own name because
       rewriting the file is what most callers want and a flag is easy to
       forget. Makes no change if the file fails to parse.

       It takes a single path and no options: there is no file to rewrite
       for inline -e code.

SEE ALSO
       petal help lint
";

const IR_EQUAL: &str = "\
NAME
       petal-ir-equal - Compare two files' compiled IR for equivalence

SYNOPSIS
       petal ir-equal [--json] <a.ptl> <b.ptl>

DESCRIPTION
       Compiles both files and compares their IR, ignoring everything
       positional: spans, comments and whitespace. <a.ptl> is the original,
       and its spans are what reported differences point at; <b.ptl> is the
       rewritten side.

       Exits 0 when the two are equivalent, 1 with the first difference, and
       2 when a side fails to compile.

OPTIONS
       --json
              Emit the comparison result as structured JSON.

       -I <dir>
              Add a module search directory. Repeatable.

SEE ALSO
       petal help lint, petal help show-ir
";

const EXPLAIN: &str = "\
NAME
       petal-explain - Show the value chain that produced a term

SYNOPSIS
       petal explain --term <name_or_id> [--json] <file>

DESCRIPTION
       Runs the program with tracing on, then prints the chain of values
       that produced <term> — what it was computed from, and what those were
       computed from, back to the source.

OPTIONS
       --term <name_or_id>
              The term to explain, by name or by IR id. Required.

       --json
              Emit the chain, and any errors, as structured JSON.

{COMMON}
SEE ALSO
       petal help show-provenance, petal help show-dependents,
       petal help show-slice
";

const SHOW_IR: &str = "\
NAME
       petal-show-ir - Display the compiled IR

SYNOPSIS
       petal show-ir [--json] [--all | --user-only] <file>

DESCRIPTION
       Compiles the program and prints its IR. Text output hides builtin
       phantom terms and the auto-loaded prelude and imported modules.

       'show-ir --json' is also Petal's interchange format: its output loads
       back into 'petal run --ir' and 'petal check --ir'.

OPTIONS
       --json
              Emit the complete Program object rather than the text view.

       --all  Include phantom builtin terms and prelude / module content.

       --user-only
              With --json: emit a filtered debugging view, with phantoms,
              prelude content and prelude-only constants removed. Not
              loadable by 'run --ir'. Requires --json, and is mutually
              exclusive with --all.

{COMMON}
SEE ALSO
       petal help run, petal help show-bytecode, petal help ir-equal
";

const SHOW_BYTECODE: &str = "\
NAME
       petal-show-bytecode - Display the bytecode lowering of the IR

SYNOPSIS
       petal show-bytecode [--json] <file>

DESCRIPTION
       Compiles the program to IR, lowers the IR to bytecode, and prints the
       result — what the VM actually executes.

OPTIONS
       --json
              Emit the bytecode as structured JSON.

{COMMON}
SEE ALSO
       petal help show-ir
";

const SHOW_AST: &str = "\
NAME
       petal-show-ast - Display the parsed AST

SYNOPSIS
       petal show-ast [--json] <file>

DESCRIPTION
       Parses the program and prints its abstract syntax tree, before any
       compilation or lowering.

OPTIONS
       --json
              Emit the AST as structured JSON.

{COMMON}
SEE ALSO
       petal help show-tokens, petal help show-ir
";

const SHOW_TOKENS: &str = "\
NAME
       petal-show-tokens - Display lexer tokens

SYNOPSIS
       petal show-tokens [--json] <file>

DESCRIPTION
       Lexes the program and prints the token stream — the first stage of
       the pipeline, useful when a syntax error does not say what you
       expected.

OPTIONS
       --json
              Emit the tokens as structured JSON.

{COMMON}
SEE ALSO
       petal help show-ast
";

const SHOW_PROVENANCE: &str = "\
NAME
       petal-show-provenance - Trace the backward slice of a term

SYNOPSIS
       petal show-provenance --term <name_or_id> [--json] <file>

DESCRIPTION
       Prints everything <term> was computed from: its backward slice
       through the dataflow graph.

OPTIONS
       --term <name_or_id>
              The term to trace, by name or by IR id. Required.

       --json
              Emit the slice as structured JSON.

{COMMON}
SEE ALSO
       petal help show-dependents, petal help show-slice, petal help explain
";

const SHOW_DEPENDENTS: &str = "\
NAME
       petal-show-dependents - Trace the forward slice of a term

SYNOPSIS
       petal show-dependents --term <name_or_id> [--json] <file>

DESCRIPTION
       Prints everything that depends on <term>: its forward slice through
       the dataflow graph, and so what a change to it would reach.

OPTIONS
       --term <name_or_id>
              The term to trace, by name or by IR id. Required.

       --json
              Emit the slice as structured JSON.

{COMMON}
SEE ALSO
       petal help show-provenance, petal help show-slice
";

const SHOW_SLICE: &str = "\
NAME
       petal-show-slice - Compute the minimal slice for some targets

SYNOPSIS
       petal show-slice --term <name_or_id> [--term <name2>]... [--json] <file>

DESCRIPTION
       Computes the minimal dataflow slice that the given targets need: the
       smallest part of the program that still produces them.

OPTIONS
       --term <name_or_id>
              A target term, by name or by IR id. Repeat for several
              targets. At least one is required.

       --json
              Emit the slice as structured JSON.

{COMMON}
SEE ALSO
       petal help show-provenance, petal help show-dependents
";

const SHOW_GRAPH: &str = "\
NAME
       petal-show-graph - Emit the dataflow graph in DOT format

SYNOPSIS
       petal show-graph [--all] <file>

DESCRIPTION
       Prints the program's dataflow graph as DOT, ready to pipe into
       Graphviz. There is no --json output — the format is DOT.

OPTIONS
       --all  Include phantom builtin terms.

{COMMON}
SEE ALSO
       petal help show-slice, petal help show-ir
";

const PENDING_REPORT: &str = "\
NAME
       petal-pending-report - Report every live pending resource after a run

SYNOPSIS
       petal pending-report [--json] <file>

DESCRIPTION
       Runs the program, then reports every pending resource still live at
       the end: its state, age, origin, and how many absorptions it took.
       The observability counterpart to 'run'.

OPTIONS
       --json
              Emit the raw report array, for tooling.

{COMMON}
SEE ALSO
       petal help run
";

const PROPOSE_EDIT: &str = "\
NAME
       petal-propose-edit - Propose source edits that change an emitted value

SYNOPSIS
       petal propose-edit --channel <name> --emit <n>
                          (--arg <k> --to <value>)...
                          [--configurable <var>]... [--static <var>]...
                          [--apply] [--json] <file>

DESCRIPTION
       Runs the program with emit tracing, then works backwards: given an
       emitted value, propose the source edits that would make argument <k>
       of the call that produced it evaluate to <value>. This is the writing
       half of direct manipulation — the host says what the user dragged
       something to, and gets back the edits that mean it.

       Several proposals may come back when several variables feed a value.
       Narrow them with --configurable and --static, or declare the knobs
       in-source with 'config let'.

OPTIONS
       --channel <name>
              The output channel the emit was pushed into, e.g.
              draw_commands. Required.

       --emit <n>
              0-based index of the emit within that channel's buffer.
              Required.

       --arg <k> --to <value>
              Argument <k> (0-based) of the producing call should evaluate
              to <value>, written as source-ish text: 55, 2.5, true, hello.
              At least one pair is required, and the pair repeats to state a
              multi-goal batch — one gesture changing several arguments,
              resolved consistently. Each --to binds to the --arg before it.

       --configurable <var>
              A variable the host prefers to edit. Repeatable.

       --static <var>
              A variable that must not be edited. Repeatable.

       --apply
              Rewrite the file in place, but only when every goal resolves
              to exactly one proposal.

       --json
              Emit the proposals as structured JSON.

{COMMON}
SEE ALSO
       petal help run
";

const LSP: &str = "\
NAME
       petal-lsp - Serve the language server over stdio

SYNOPSIS
       petal lsp

DESCRIPTION
       Speaks the Language Server Protocol over stdio, as Content-Length-
       framed JSON-RPC. Editors spawn this; it takes no file and no options,
       because documents arrive over the protocol.

SEE ALSO
       petal help check
";

const PACKAGES: &str = "\
NAME
       petal-packages - List the libraries the search path makes available

SYNOPSIS
       petal packages [--json] [-I <dir>]...

DESCRIPTION
       A Petal library is a directory holding a petal.toml manifest:

           [package]
           name = \"bloom\"
           version = \"0.1.0\"
           modules = \"src\"      # optional; defaults to src/

       Every -I directory is searched for such libraries: the directory
       itself, and each directory directly under it. A library named N makes
       its modules importable as `import N/<module>`, and this command prints
       what was found, one library per line with its modules under it.

       A petal.toml that will not parse is an error here, and in every
       command that takes -I — a library the user pointed at and that failed
       to load should say so, not go quietly missing.

OPTIONS
       --json
              Emit the list as structured JSON.

       -I <dir>
              A directory to search for libraries. Repeatable.

SEE ALSO
       petal help run
";

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command in the index has a page, and every page is reachable
    /// from the index — the two lists drift apart silently otherwise.
    #[test]
    fn the_index_and_the_pages_agree() {
        let mut listed: Vec<&str> = Vec::new();
        for (_, commands) in GROUPS {
            for (name, _) in *commands {
                assert!(page(name).is_some(), "'{name}' is listed with no page");
                listed.push(name);
            }
        }
        for name in ALL_PAGES {
            assert!(listed.contains(name), "'{name}' has a page but is unlisted");
        }
    }

    /// Every page substitutes its {COMMON} placeholder, if it has one, and
    /// none is left with a stray unsubstituted brace.
    #[test]
    fn pages_render_without_placeholders() {
        for name in ALL_PAGES {
            let text = page(name).unwrap().replace("{COMMON}", COMMON);
            assert!(!text.contains('{'), "'{name}' has an unsubstituted brace");
            assert!(
                text.starts_with("NAME\n       petal-"),
                "'{name}' does not open with a NAME section"
            );
        }
    }

    const ALL_PAGES: &[&str] = &[
        "run",
        "check",
        "lint",
        "lint-fix",
        "ir-equal",
        "explain",
        "show-ir",
        "show-bytecode",
        "show-ast",
        "show-tokens",
        "show-provenance",
        "show-dependents",
        "show-slice",
        "show-graph",
        "pending-report",
        "propose-edit",
        "lsp",
        "packages",
    ];
}

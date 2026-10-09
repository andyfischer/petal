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
            ("bench", "Measure what a call of a named function costs"),
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
            ("fmt", "Rewrite files in the canonical layout"),
            ("lint", "Report code that has a better spelling, and fix it"),
            ("suggest", "Suggest safe refactors for a file"),
            ("lint-fix", "The same as 'lint --fix'"),
            (
                "apply-change",
                "Carry out a refactor that changes behaviour",
            ),
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
            ("graph", "Walk the dataflow graph back or forward from terms"),
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
            match closest_command(name) {
                Some(c) => eprintln!(
                    "petal: '{name}' is not a petal command. Did you mean '{c}'? See 'petal help'."
                ),
                None => eprintln!("petal: '{name}' is not a petal command. See 'petal help'."),
            }
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
        "bench" => BENCH,
        "fmt" => FMT,
        "lint" => LINT,
        "suggest" => SUGGEST,
        "lint-fix" => LINT_FIX,
        "apply-change" => APPLY_CHANGE,
        "ir-equal" => IR_EQUAL,
        "explain" => EXPLAIN,
        "show-ir" => SHOW_IR,
        "show-bytecode" => SHOW_BYTECODE,
        "show-ast" => SHOW_AST,
        "show-tokens" => SHOW_TOKENS,
        "graph" => GRAPH,
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

const BENCH: &str = "\
NAME
       petal-bench - Measure what a call of a named function costs

SYNOPSIS
       petal bench --fn <name> [--fn <name>]... [<options>] <file>
       petal bench --fn <name> [--fn <name>]... [<options>] -e <code>

DESCRIPTION
       Runs <file> the way 'petal run' does, repeatedly, and reports for each
       named function what one call cost, over the calls the script itself
       makes: how many there were, instructions per call, and milliseconds
       per call (mean, minimum, median, 95th percentile), then heap
       allocations, list/record copies, and garbage collections per call.

       Everything is measured twice, with the optimizer on and with it off,
       and the last column is the optimized figure relative to the
       unoptimized one. The two runs differ only in the optimizer: memoization
       and every other part of the run policy are the same on both sides, and
       the first is optimized even when PETAL_OPT=off or PETAL_POLICY=baseline
       is set.

       Each figure comes in two forms. The headline is inclusive: the
       function and everything it calls. 'self' leaves out the user functions
       it calls (builtins it calls stay in). For a recursive function the
       inclusive figures are per outermost call, so nothing is counted twice,
       and the self figures are per call.

       The file is run once as a warm-up, then again until about one second
       has passed, for each of the two measurements. Every run starts from
       scratch, as a separate 'petal run' would. The script's own output is
       not printed. A run that fails ends the command with the error and no
       report.

       <name> is a function's name as written in the source, wherever it is
       declared: at the top level, inside another function, or in an imported
       module. A bare method name selects that method on every class
       ('area'); 'Class.method' selects one. Several functions with the same
       name are reported separately, each with the file and line it is
       declared at. A name that matches nothing is an error; a function that
       exists and is never called is reported as such.

       Timing a call is not free: about 50 ns for each call of a benched
       function and for each user function it calls directly. The report
       states the figure measured on this machine. It is inside the times
       shown, so a function that does only a few instructions' work reads
       as mostly overhead; its instruction count is exact regardless.

OPTIONS
       --fn <name>
              A function to measure. Repeatable; at least one is required.

       --iters <n>
              Run the file exactly <n> times per measurement (after the
              warm-up) rather than for about a second.

       --json
              Print the report as one JSON object: 'runs' (per measurement:
              policy, run count, ms per run, ms to lower, instructions per
              run), 'timer_overhead_ns', and 'functions', each with 'opt',
              'no_opt' and 'delta_pct'. Per-call values are null for a
              function that was never called. Errors are JSON too.

       --seed <n>
              Seed every run's PRNG, so a script that calls random() makes
              the same calls on each run. Decimal or 0x-hex.

       --host core
              Accepted for symmetry with 'check'. bench runs what 'run'
              runs, which is the core host; a script written for another
              host needs that host's own runner.

{COMMON}";

/// The canonical `&'static` spelling of a command name, for messages that
/// outlive the argument vector. `None` when `name` is not a command.
pub(super) fn command_name(name: &str) -> Option<&'static str> {
    command_names().find(|c| *c == name)
}

/// Every command in the index, in index order.
fn command_names() -> impl Iterator<Item = &'static str> {
    GROUPS
        .iter()
        .flat_map(|(_, commands)| commands.iter().map(|(name, _)| *name))
}

/// The command `word` is most plausibly a typo of: the nearest by edit
/// distance, when it is near enough (a third of the word, at least one edit)
/// to be a slip rather than a different word. A unique command that `word` is
/// a prefix of counts too (`show-prov`).
pub(super) fn closest_command(word: &str) -> Option<&'static str> {
    let mut prefixed = command_names().filter(|c| word.len() >= 3 && c.starts_with(word));
    if let (Some(only), None) = (prefixed.next(), prefixed.next()) {
        return Some(only);
    }
    let budget = (word.chars().count() / 3).max(1);
    command_names()
        .map(|c| (edit_distance(word, c), c))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

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
              Count copy-on-write duplications and heap allocations during
              the run and print them to stderr afterwards.

       --profile
              Count instructions, builtin calls and collections during the
              run and print the histogram to stderr.

       --effect-audit
              Watch what every native does during the run and report, on
              stderr, each one whose behavior differs from the effect row it
              declared at registration: under-declared (a bug) or
              over-declared. See docs/tasks/declarative-effect-refactoring.md.

       --policy <name>
              Run under a named run policy: fast (the default), baseline
              (no optimizer, no memoization), explain, or replay, optionally
              followed by modifiers such as -memo or +gate. Output must be
              identical under every policy, so a difference is a bug in the
              layer the two policies differ by. PETAL_POLICY=<name> does the
              same for every command and embedder; the flag wins.

       --no-opt
              Same as --policy baseline.

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
       Lexes, parses, compiles and lowers the program, then stops. Exits 1
       when it does not compile, or when the checker finds an error: a line
       that fails whenever it runs, such as a call to or read of a name
       nothing defines, or a call whose argument count no overload accepts.
       Other findings (type mismatches against annotations, discarded
       results) are warnings and do not change the exit code unless
       --strict is given. Exits 0 otherwise, so it is the cheap gate for
       editors and CI.

OPTIONS
       --json
              Emit errors and warnings as structured JSON.

       --strict
              Exit non-zero when type-checker warnings exist. Plain 'check'
              exits 0 for a program that only has warnings. The 'export'
              deprecation is printed but not counted: the old spelling
              still works, and 'petal lint' is the gate for it.

       --lenient
              Report checker errors but exit 0 unless the program fails to
              compile or lower. For tools that only ask whether a file
              compiles, such as a corpus sweep over scripts for several
              hosts.

       --ir   Check <file> as JSON IR ('show-ir --json' output) rather than
              source. Use '-' to read the IR from stdin.

       --host core|ui|garden|garden-config|sdl
              The host the script runs in, which decides the natives a call
              may name. A call to, or read of, a name that neither the
              program nor the host defines is an error (it fails with
              'Unknown builtin' or 'Undefined variable' when the line runs).
              'ui' (the default) is the core builtins, the petal-ui natives
              and the 'ui' prelude as an implicit import; 'garden' adds
              Garden's panel and config natives; 'sdl' adds
              petal-desktop-sdl's; 'garden-config' is Garden's config host
              (init.ptl, layout scripts): core plus the layout builders,
              without petal-ui or its prelude; 'core' is the core builtins
              alone. 'petal run' is the core host, so a script that passes
              here under the default can still fail there with 'Undefined
              variable: ui'; '--host core' checks what 'petal run' can run.

       --native <name>[,<name>...]
              Further natives the host registers, on top of --host's.
              Repeatable.

       --error-format full|bare
              'bare' prints only the error message, with no position suffix
              and no source snippet. Same flag as 'run'.

{COMMON}
SEE ALSO
       petal help run, petal help lint
";

const FMT: &str = "\
NAME
       petal-fmt - Rewrite files in the canonical layout

SYNOPSIS
       petal fmt [--check] [--diff] [<path>...]
       petal fmt -e <code>
       petal fmt -

DESCRIPTION
       Formats every .ptl file under the given paths (directories are
       searched recursively, skipping dot-directories, node_modules and
       target; the default is the current directory) and rewrites them in
       place. There are no style options: one layout, like gofmt.

       What it changes is whitespace only: indentation (2 spaces per open
       construct), spacing within a line (one space around binary operators
       and after commas and colons, none inside brackets or before commas),
       trailing whitespace, runs of blank lines (at most one), and the final
       newline. It never wraps lines, reorders code, or touches the inside of
       strings or JSX text. Choices about which code to write belong to
       'petal lint'.

       The one word it rewrites is the deprecated modifier 'export', which
       becomes 'pub'. The two declare the same thing.

       Alignment is kept. A run of spaces that lines a token up with one on
       the line above or below (a table of records, a column of =), a
       repeated double space that groups arguments, a trailing comment's
       column, and a continuation line lined up under its open bracket all
       survive, moving only as far as the code they hang from moves.

       Every result is checked before it is written: the formatted text must
       lex to exactly the original tokens (an 'export' modifier counting as
       its 'pub'). A file that does not parse is
       reported and left alone.

       To keep lines exactly as written, put '// petal-fmt-ignore' on the
       line before one line, or wrap a region in '// petal-fmt-off' and
       '// petal-fmt-on'. '// petal-fmt-ignore-file' anywhere in a file skips
       the whole file.

OPTIONS
       --check
              Write nothing; list the files that would change and exit 1 if
              there are any. For CI.

       --diff, -d
              Write nothing; print a unified diff of what would change. Exits
              1 if anything would.

       -e <code>
              Format inline code and print the result.

       -      Read source from stdin and print the result.

SEE ALSO
       petal help lint
";

const LINT: &str = "\
NAME
       petal-lint - Report code that has a better spelling, and fix it

SYNOPSIS
       petal lint [--fix [--verify[=ir|strict]]] [--json]
                  [--rules-include=<rules>] [--rules-exclude=<rules>] [<path>...]
       petal lint --rules
       petal lint [<options>] -e <code>

DESCRIPTION
       Runs named rules over every .ptl file under the given paths (the
       default is the current directory) and prints one line per finding:

              file:line:column: rule: message

       and exits 1 if there are any. Every rule carries a fix, and --fix
       applies them in place; a fixed file is then formatted with
       'petal fmt', since a rewrite can move code between lines. Layout is
       not lint's business: an unformatted file with no findings is clean.

       Each fix is gated: if the file compiled before, it must compile after,
       or nothing is written.

       Rules (see 'petal lint --rules'):

       prefer-let
              a var that no nested function shares becomes a let
       no-redundant-cast
              int(n), float(x) or str(s) on a value already of that type
       prefer-match
              an if/elsif chain testing one value against literals becomes
              a match
       prefer-compound-assign
              x = x + e becomes x += e
       prefer-pub
              the deprecated export modifier becomes pub

       To silence a rule, put '// petal-lint-ignore <rule> [<rule>...]' on
       the line before the finding or at the end of its line; with no rule
       names it silences them all. '// petal-lint-ignore-file [<rule>...]'
       does the same for a whole file. Text after '--' is a reason. A
       silenced finding's fix is not applied.

OPTIONS
       --fix  Apply the fixes in place. With -e, print the fixed code.

       --verify[=ir|strict]
              Prove the rewrite before writing it, by compiling both sides
              and comparing their IR. Not provably acceptable means no write
              and exit 3. --verify=ir (the default) accepts the rules that
              change the IR by design and proves the rest; --verify=strict
              demands IR equality of the whole rewrite.

       --json Print the findings as a JSON array of objects with file, line,
              column, rule, message and fixed.

       --rules
              List the rules and exit.

       --rules-include=<rule>,...
              Run only these rules.

       --rules-exclude=<rule>,...
              Run every rule but these.

{COMMON}
SEE ALSO
       petal help fmt, petal help ir-equal
";

const SUGGEST: &str = "\
NAME
       petal-suggest - Suggest safe refactors for a file

SYNOPSIS
       petal suggest [--only <kind>[,<kind>]] [--apply | --verify]
                     [--host <host>] [--json] [--from <file>]... <file>
       petal suggest [<options>] -e <code>

DESCRIPTION
       Proposes changes to a file that make it say more, each with the reason
       behind it. Four kinds:

       types
              Type annotations the program already implies, read from its
              own call sites and function bodies.

       named-args
              Named arguments for calls that pass three or more arguments by
              position: 'draw_rect(0, 0, 320, 48, panel)' becomes
              'draw_rect(x: 0, y: 0, w: 320, h: 48, c: panel)'.

       return-types
              A return type for a function that ends in a loop and declares
              none: '-> list' where a caller uses the list the loop collects,
              '-> nil' where none does.

       advice
              Comments on code that looks like it could be something simpler,
              where what to write instead is the author's call: a loop that
              is a hand-written sort, say. Advice is a heuristic and carries
              no rewrite, so --apply never acts on it.

       This is a suggestion channel, not a check. Nothing here runs during an
       ordinary compile, nothing here can fail a build, and nothing is
       written unless --apply is given. 'petal check' remains the tool that
       warns; this is the tool that proposes.

       Most suggestions leave the compiled program exactly as it was, but that
       is not a property of the command: a '-> nil' return type changes what
       its function compiles to, on purpose. See RETURN TYPES.

TYPE ANNOTATIONS
       What counts as evidence, for a parameter: the types callers actually
       pass; the declared type of a slot the parameter is forwarded into; and
       a field read, which proves the value is record-shaped (a plain record
       has the same fields a class does, so the class itself is reported as a
       hint rather than written). For a return type: the body's tail
       expression and every explicit return.

       Numbers are treated differently in the two slots. A parameter is a
       precondition, so numeric evidence always proposes 'num' — the callers
       this compile can see are not the callers there are. A return type is a
       promise the body actually keeps, so it stays precise.

       Suggestions compound: an applied annotation is evidence for the next
       pass. Re-running until it reports nothing is the intended workflow.

NAMED ARGUMENTS
       A call is rewritten only where its callee is known for certain and
       accepts names: a fn (each overload variant by itself), a lambda held
       in a let, a class constructor, a function of an imported module or of
       the host's prelude, a method call the checker pinned to one class, and
       a builtin that declares its parameter names. A parameter or any other
       value that merely holds a function, a method dispatched at runtime,
       and a variadic builtin (print) are left alone.

       The rewritten call must be the same call: it selects the same
       overload variant under the name-aware rule, every argument fills the
       slot it filled, and the arguments stay in the order written, so they
       are evaluated in the same order. A call is also left alone when two
       variants both take that many arguments and call them different things
       — the ui prelude's draw_line takes seven as (x1, y1, x2, y2, r, g, b)
       and as (x1, y1, x2, y2, c, a, width) — since only one of the two
       readings could be written down.

       Positional arguments must come first, so the choice is where the names
       start. They start as early as they can, after:

       o  the receiver of a method call or the piped value of 'x |> f(...)';
       o  placeholder parameters, whose names say nothing: a name starting
          with '_', neighbours that only count from 'a' ('a, b', 'a, b, c'),
          and neighbours numbered on one stem ('p1, p2', 'c0, c1');
       o  the subject, when the first parameter is the thing operated on
          (self, this, value, list, collection, string, record, array, text,
          rect, or a short form: s, str, txt, v, val, xs, lst, arr, items,
          and r unless a g follows it): 'clamp(v, lo: 0, hi: 1)';
       o  arguments already spelled like their parameter: 'box(x, y, w: 3,
          h: 4)', not 'box(x: x, y: y, ...)'.

       Arguments already named are kept as written, and an applied file
       yields no further suggestion.

       Three calls are left alone because the names would only repeat what
       the call already says: a bare colour, whose arguments are the channels
       (r, g, b) or (r, g, b, a) and nothing else — 'clear(18, 20, 28)'; a
       function literal that would get a one-letter name — 'reduce(xs, 0,
       fn(a, b) -> a + b)', not 'f: fn(a, b) -> ...'; and a call where more
       of the names would echo their own argument than add to it —
       'hash(ix: ix + 1, iy: iy, seed: seed)'.

RETURN TYPES
       A 'for' in tail position collects a list, and that list is the
       function's implicit return. A function that ends in a loop and
       declares no return type therefore builds a list on every call whether
       or not anyone wants it. Declaring '-> nil' turns the implicit return
       off: the tail becomes an ordinary statement and the loop allocates
       nothing. Declaring '-> list' says the list is the point.

       Which one applies is read from the calls in view:

       o  some call uses the result: '-> list' is suggested;
       o  called, and no call uses the result: '-> nil' is suggested;
       o  never called, or 'pub': both options are reported under 'choose:'
          and --apply skips it, since the callers that would settle it (a
          host calling by name, an importing module) cannot be seen.

       A result is used when the call is bound, passed, returned or
       collected, and unused when the call is a statement. A call that is
       another un-annotated function's tail is used exactly when that
       function's result is. A function read as a value rather than called
       ('map(xs, f)') is treated as 'never called', and so is one whose
       name a local binding shadows somewhere. --from <file> adds that
       file's calls.

       Only loop tails are covered. A function that also has a 'return
       <value>' of its own is left alone, and one where only some branches
       end in a loop is offered '-> nil' or nothing.

       '-> nil' is not IR-preserving and is not held to the IR; it rests on
       the calls that were read. '-> list' is, and --apply proves it.

ADVICE
       Advice is for what can be noticed but not rewritten. Each piece names
       a place, says what it looks like, and stops there: detection is a
       heuristic ('looks like'), the right replacement depends on what the
       code was meant to do, and nothing could prove the two alike. So there
       is no 'suggest:' line, --apply writes nothing for it, and --verify
       does not count it. The rules:

       hand-written-sort
              A loop that inserts each element into place by rebuilding the
              list — an insertion sort written out by hand. 'sort(list,
              compare)' and 'sort_by(list, key)' do it in one call.

OPTIONS
       --only <kind>[,<kind>]
              Look for these kinds only: 'types', 'named-args',
              'return-types', 'advice'. All four by default.

       --apply
              Write the suggestions into the file, each kind behind its own
              proof. An annotation is kept when the annotated source still
              compiles and gains no type-checker warning the original did not
              already have. A named-argument rewrite is kept when the
              rewritten source compiles to the same IR — the comparison of
              'petal ir-equal --named-args', in which a call may differ only
              in how its arguments are written and only where both provably
              bind alike — and gains no warning either. A '-> list' return
              type is kept when the rewritten source compiles to the same IR
              and gains no warning; a '-> nil' one when it compiles and gains
              no warning, the IR being what it is there to change. A
              suggestion that fails is dropped and named on stderr; exit 3,
              with no write, when none passes.

       --verify
              Run the --apply proofs and report, writing nothing. Exit 3 if
              any suggestion fails its proof.

       --host <host>
              The host the script runs in, as for 'petal check': 'core',
              'ui' (the default), 'garden', 'garden-config' or 'sdl'. It
              decides which prelude is imported implicitly, and so what a
              bare 'draw_rect' is. A script for an embedding with natives of
              its own should say 'core'.

       --from <file>
              Also compile <file> for its call sites. A library module
              compiled on its own has no callers, so its parameters have no
              call-site evidence; point this at an app that uses the library.
              Repeatable. Type annotations and return types.

       --json Emit the suggestions as JSON, in source order. Each has a
              'kind' ('type-annotation', 'named-args', 'return-type' or
              'advice'), its reason, and its 'edits': the insertion offsets
              and the exact text to insert. A return type has a 'usage'
              ('used', 'unused', 'unknown'), a 'type' that is null when the
              choice is the author's (its 'edits' is then empty), and
              'preserves_ir'. Advice has a 'rule' and a 'message' and its
              'edits' is always empty.

{COMMON}
SEE ALSO
       petal help check, petal help lint, petal help ir-equal
";

const LINT_FIX: &str = "\
NAME
       petal-lint-fix - The same as 'lint --fix'

SYNOPSIS
       petal lint-fix [<options>] [<path>...]

DESCRIPTION
       The same as 'petal lint --fix', under its own name because fixing the
       files is what most callers want and a flag is easy to forget. Takes
       every option 'petal lint' does.

SEE ALSO
       petal help lint
";

const APPLY_CHANGE: &str = "\
NAME
       petal-apply-change - Carry out a refactor that changes behaviour

SYNOPSIS
       petal apply-change <operation> <file> [<options>]
       petal apply-change convert-to-var <file> --target <path>
                          [--from <file>]... [--dry-run] [--host <host>]
       petal apply-change

DESCRIPTION
       Carries out a change the author has decided on, everywhere it has to
       be made, across every file it reaches. Unlike 'petal lint --fix' and
       'petal suggest --apply', which are held to leaving the program the
       same, an operation here changes what the program does. That is its
       purpose.

       What it promises instead: it edits only the mentions that resolve to
       the target named on the command line; it refuses, with the lines
       responsible, rather than guess at a rewrite it does not have; and it
       writes nothing unless the result compiles. Everything it does not
       touch, comments and layout included, is left as it was.

       With no operation, lists the operations.

OPERATIONS
       convert-to-var
              Turn a 'let' into a 'var', or a 'state' into a 'state var': the
              change from a dataflow binding to a mutable cell that a
              function can write. It is the fix for \"`x` is bound outside
              this function\".

              The declaration's keyword changes. Every '=' write on the
              binding gains 'set' ('x = e', 'x += e', 'x.f = e', 'x[i] = e'),
              in the declaring function and in nested ones. Every read inside
              a nested function or lambda gains 'get'; a read in the
              declaring function stays bare. A function that mentions the
              binding stops reading the value captured where it was written
              and reads the live cell, so a program that relied on the
              snapshot behaves differently afterwards.

              Shadowing is respected: where the name is bound again, by
              another declaration, a parameter, a 'for' variable or a match
              pattern, those mentions belong to that binding and are left
              alone.

              Refused when the binding is rebound with '@x' ('@' is let-only),
              when it is a 'config let', and when a file that imports it
              writes it.

TARGET PATHS
       --target names one binding by the declarations it is written inside,
       outermost first, joined by '/':

              step/vx                'vx', declared in the function 'step'
              build_view/row/out[2]  the second 'out' in 'row', in 'build_view'
              /score                 the module-level 'score'

       A segment names a declaration: a named 'fn' ('Class.method' for a
       method) or a 'let', 'var', 'state' or 'state var' binding. A
       declaration encloses whatever is written inside its text, so a
       function encloses its body and 'let row = fn(i) ... end' encloses the
       lambda's body. Control flow and anonymous functions are transparent: a
       'let' inside an 'if', a loop or a callback belongs to the nearest
       declaration around it. Parameters, 'for' variables and pattern
       bindings are not declarations and cannot be named.

       A leading '/' starts the path at module level. Without one, the path
       matches any binding whose full path ends with it, so 'vx' alone is
       enough when there is only one.

       'name[n]' is the n-th declaration called 'name' directly inside its
       parent, counting from 1 in source order over every kind of
       declaration. A segment without an index matches all of them.

       The path has to match exactly one binding. When it matches none, or
       several, the error lists the full paths of the candidates, each with
       its line.

IMPORTERS
       A module-level binding declared 'pub' is visible to other files, so
       the change follows it there. An importer may read an exported 'var'
       but never write it: a bare read inside one of the importer's functions
       gains 'get', a qualified read ('m.x') needs nothing, and an importer
       that assigns the name ('x = e', '@x', 'm.x = e') stops the whole
       change.

       Importers are looked for in one of two places, and the report says
       which, and names every importer it found:

       --from <file>
              The entry file of a program that uses the module. Its imports
              are followed, and every file reached is considered. Repeatable.
              The same flag, with the same meaning, as 'petal suggest
              --from'.

       With no --from, every '.ptl' file under the target's project root is
       considered: the nearest directory at or above the target that holds a
       'petal.toml', or the target's own directory when there is none. An
       importer outside that directory is not found; name its program with
       --from.

OPTIONS
       --target <path>
              The binding to change. Required by convert-to-var.

       --from <file>
              See IMPORTERS. Each --from file is also compiled by the gate.

       --dry-run
              Print the change as a unified diff on stdout and write nothing.
              Everything else, the gate included, runs as usual.

       --host <host>
              The host the scripts are written for, as for 'petal check'
              ('core', 'ui' — the default — 'garden', 'garden-config',
              'sdl'). It decides which prelude the gate imports implicitly.

       -I <dir>
              Add a module search directory. Repeatable.

THE GATE
       Before anything is written, every file that would change and every
       --from file is compiled with all the edits in place. Each must
       compile. A file that already failed to compile may keep the compile
       errors it had, and gain none: that is what lets a file with several
       bindings to convert be fixed one at a time. It is named in a 'note:'.

EXIT STATUS
       0 when the change was written (or, with --dry-run, would have been),
       1 when it was refused or failed. On 1 no file has been touched.

SEE ALSO
       petal help lint, petal help suggest, petal help check
";

const IR_EQUAL: &str = "\
NAME
       petal-ir-equal - Compare two files' compiled IR for equivalence

SYNOPSIS
       petal ir-equal [--json] [--named-args [--host <host>]] <a.ptl> <b.ptl>

DESCRIPTION
       Compiles both files and compares their IR, ignoring everything
       positional: spans, comments and whitespace. <a.ptl> is the original,
       and its spans are what reported differences point at; <b.ptl> is the
       rewritten side.

       Exits 0 when the two are equivalent, 1 with the first difference, and
       2 when a side fails to compile.

       The names a call writes its arguments with are part of the IR, so
       'f(1, 2)' and 'f(x: 1, y: 2)' differ: whether they mean the same
       depends on what 'f' is. --named-args answers that instead of refusing.

OPTIONS
       --json
              Emit the comparison result as structured JSON.

       --named-args
              Accept two calls that differ only in writing an argument by
              name instead of by position, where the callee is known for
              certain on both sides and both calls select the same overload
              variant and put every argument in the same parameter. A callee
              that cannot be pinned down (a parameter, a method dispatched at
              runtime) is still a difference. This is the proof 'petal
              suggest --apply' holds its named-argument rewrites to.

       --host <host>
              With --named-args: the host both files are written for, as for
              'petal check' ('core', 'ui' — the default — 'garden',
              'garden-config', 'sdl'). It decides which prelude is imported
              implicitly.

       -I <dir>
              Add a module search directory. Repeatable.

SEE ALSO
       petal help lint, petal help suggest, petal help show-ir
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

const GRAPH: &str = "\
NAME
       petal-graph - Walk the dataflow graph back or forward from terms

SYNOPSIS
       petal graph --term <name_or_id> [--term <name2>]...
                   [--direction back|forward] [--json] <file>

DESCRIPTION
       One dataflow query with one result shape. The program is compiled,
       never run.

       --direction back with one --term: everything the term was computed
       from (its ancestors). Same as show-provenance.

       --direction back with several --terms: the slice of the program the
       targets need, closed over every write to any var they read. Same as
       show-slice.

       --direction forward: everything that depends on the terms, including
       may-edges through var cells and method dispatch. Same as
       show-dependents.

       The JSON result always has the fields direction, targets, terms,
       edges, frontier, complete and minimal.
       complete and minimal are false when the walk met a var cell: backward
       it stopped there, forward it crossed a may-edge. frontier lists each
       such cell with every write that could supply it.

OPTIONS
       --term <name_or_id>
              A target term, by name or by IR id. Repeatable; at least one
              is required.

       --direction back|forward
              Which way to walk. Defaults to back.

       --json
              Emit the result as structured JSON.

{COMMON}
SEE ALSO
       petal help explain, petal help show-graph
";

const SHOW_PROVENANCE: &str = "\
NAME
       petal-show-provenance - Trace the backward slice of a term

SYNOPSIS
       petal show-provenance --term <name_or_id> [--json] <file>

DESCRIPTION
       Prints everything <term> was computed from: its backward slice
       through the dataflow graph. An alias for
       `petal graph --direction back --term <term>`.

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
       the dataflow graph, and so what a change to it would reach. An alias
       for `petal graph --direction forward --term <term>`.

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
       smallest part of the program that still produces them. An alias for
       `petal graph` with several --terms (always the slice, even for one).

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
        "fmt",
        "lint",
        "lint-fix",
        "ir-equal",
        "explain",
        "show-ir",
        "show-bytecode",
        "show-ast",
        "show-tokens",
        "graph",
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

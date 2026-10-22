# Petal CLI Reference

The `petal` binary runs programs, checks them without running, tidies source,
dumps each compiler stage (tokens, AST, IR, bytecode), and answers questions
about a program's dataflow.

## Usage

```
petal <command> [options] <file>
petal <command> [options] -e '<code>'
petal <file>                      # same as: petal run <file>
petal help <command>              # the manual page for one command
petal --version
```

`petal --version` (also `petal version`, `-V`) says what the binary is: the
version, the git commit it was built from with that commit's date, and the
date it was built.

```
$ petal --version
petal 0.1.0 (4d1b86b0c1 2026-10-13, built 2026-10-14)
```

The commit has `-dirty` after it when the build's tree had uncommitted changes
to tracked files, and `-vendored` when the sources were a copy inside another
repository (the commit is then the one its `VENDORED.md` records). If a
`petal` on your `PATH` rejects syntax the documentation shows, check this line
first: an old binary gives no other sign. An embedder reads the same from
`petal::version` (`describe()`, `GIT_COMMIT`, `GIT_DATE`, `BUILD_DATE`).

A first word that is neither a command nor a file is reported as an unknown
command, with the nearest real one when there is one (`petal chek x.ptl` →
`Did you mean 'petal check'?`). A word ending in `.ptl`, or naming a file
that exists, is always the `petal <file>` shorthand.

A source-taking command takes exactly one source — one file or one `-e` —
and refuses an option it does not know, rather than reading it as the file:

```
$ petal run app.ptl --iter 3
Unknown option '--iter'
Usage: petal run [--json] [--trace] ... <file>
See 'petal help run'.
```

### Common options

Every command that compiles a program accepts these:

- `-e '<code>'` — read the program from the command line instead of a file.
- `-I <dir>` — add a directory of *packages*. Repeatable. A directory
  holding a `petal.toml`, and each directory directly under one, becomes
  importable by its manifest name (`import bloom/menu`); the directories in
  the `PETAL_PATH` environment variable are searched the same way. It is not
  a place loose files are found: a file is imported relative to the importer
  (`import ./util`), and a bare name is always a package. A file inside a
  package reaches that package by name with no `-I`, and so does a file in
  a project whose `petal.toml` lists the package under `[dependencies]`: the
  nearest `petal.toml` is found by walking up from the file. See
  [packages.md](packages.md) and
  [`packages`](#packages--show-the-project-and-the-libraries-available).
- With `-e` or stdin there is no importing file, so `import ./x` means
  `x.ptl` in the current directory.

### Environment variables

| Variable | Effect |
|---|---|
| `PETAL_PATH` | Directories to search for packages, as with `-I` |
| `PETAL_HOME` | Replaces `~/.petal`, which holds the package cache (`cache/`) |
| `PETAL_OFFLINE` | `1` forbids fetching git dependencies and on-demand installs |
| `PETAL_SEED` | Seeds the random-number generator, as `run --seed <n>` does. The flag wins |
| `PETAL_POLICY` | The run policy, as `run --policy <name>` does, for every command and every embedder. The flag wins. A value that does not parse is reported on stderr and ignored |
| `PETAL_OPT` | `off`, `none` or `0` is `PETAL_POLICY=baseline` (what `run --no-opt` does). Read only when `PETAL_POLICY` is not set |
| `PETAL_DEBUG` | Set to anything: per-term execution events on stderr, as `run --trace` |
| `PETAL_TRACE_PENDING` | `1` is `run --trace-pending` |
| `PETAL_STATIC_AUDIT` | `watch` (or `1`) starts every `Env` of the process under the [static audit](#checking-the-answer---static-audit) and prints each finding on stderr |
| `PETAL_STATIC_CHECK` | `1` makes every `Env` check the static values it keeps for function bodies instead of trusting them: each is rebuilt and compared, and a mismatch is printed on stderr as a `static check` line. See [dev/performance.md](dev/performance.md#static-values) |
| `PETAL_OPT_STATS` | Set to anything: when a program is lowered, print what the copy-propagation pass changed and the static-value cache's layout on stderr |

The options below name the variable that does the same thing; this table is
the whole list.

### Commands at a glance

| Command | Purpose |
|---------|---------|
| `run` | Execute a program |
| `check` | Compile without executing |
| `test` | Run the `test` blocks of files and directories |
| `bench` | Measure what a call of a named function costs |
| `lsp` | Serve the language server over stdio |
| `packages` | Show the project's dependencies and required host interfaces, and the libraries available |
| `init` | Start a project: write a `petal.toml` |
| `add` / `remove` | Add or remove a dependency, then install |
| `install` | Fetch the project's dependencies and write `petal.lock` |
| `update` | Re-resolve locked dependencies from their sources |
| `vendor` | Copy the project's git dependencies into `vendor/` |
| `bundle` | Emit a program's packages as one JSON document |
| `fmt` | Rewrite files in the canonical layout |
| `lint` | Report code with a better spelling; `--fix` applies it |
| `fix-imports` | Migrate older code: rewrite its import lines and add the imports it is missing |
| `suggest` | Suggest safe refactors for a file |
| `apply-change` | Carry out a refactor that changes behaviour |
| `ir-equal` | Compare two files' compiled IR |
| `show-tokens` | Lexer output |
| `show-ast` | Parser output |
| `show-ir` | Compiled IR (term graph) |
| `show-bytecode` | Bytecode lowering of the IR |
| `show-static` | Which top-level declarations are static, and why the others are not |
| `show-graph` | Dataflow graph as Graphviz DOT |
| `explain` | Run, then show the value chain that produced a term |
| `graph` | Dataflow walk back or forward from one or more terms |
| `pending-report` | Run, then report every live pending resource |
| `propose-edit` | Propose source edits that change an emitted value |

Every command except `lsp`, `fmt`, `fix-imports`, `apply-change`, `show-graph` and the
package-manager commands (`init` … `bundle`) accepts `--json` for machine-readable output.

## Commands

### `run` — Execute a program

```
petal run [options] <file.ptl>
petal run [options] -e '<code>'
petal <file.ptl>
```

Checks the program, then runs it. Output goes to stdout. Exits 1 on error.

`run` starts with what [`check`](#check--compile-without-running) does, and
prints the same findings to stderr. A program with a check **error** is not
run: no line of it executes, whether or not the line at fault would have been
reached, and the command exits 1. Warnings are printed and the program runs.

```
$ petal run -e 'print(1 + 2)'
3
```

`run` provides the core builtins and the builtin packages (`creative`, `vec2`,
`noise`, `color`, `json`, `io`, `autodiff`) and nothing else. A script that
imports a host's interface — `import ui`, or a package that requires it such
as `bloom` — is refused when it loads, before any line runs, with a message
naming what this host lacks; run it with that host's runner (petal-sdl,
`petal-ui-run`, Garden). [`check`](#check--compile-without-running) checks
such a script without running it.

The nearest `petal.toml` above the file is the script's project: its
`[dependencies]` are importable by their keys with no flag, and a git
dependency the script imports is [installed on
demand](packages.md#install-on-demand).

Options:

- `--json` — emit errors as structured JSON:
  `{message, line, column, caused_by[], stack[], phase}`, plus an `errors[]`
  array for front-end failures. See [Error phases](#error-phases). With
  `--observe` or `--trace-emits`, those reports are JSON too.
- `--error-format full|bare` — how errors print on stderr. `full` (the
  default) is `Error: <message> [line N, column M]` followed by the source line
  and a caret. `bare` prints only the message, so two sources that differ only
  in layout fail identically (see
  [refactor verification](dev/refactor-verification.md)). Type-checker
  warnings lose their position line and snippet too. A line number quoted
  inside a message's own prose (`written on line 775`) is part of the
  message and stays. `--json` output is unaffected.
- `--ir` — load `<file>` as JSON IR (the output of `show-ir --json`) instead of
  source. Use `-` to read from stdin:
  `petal show-ir --json -e 'print(42)' | petal run --ir -`.
- `--seed <n>` — seed the random-number generator, so `random`, `random_int`
  and `choose` replay identically. Decimal or `0x`-hex. Without it the seed
  comes from the clock, so even two runs of the same file differ.
  `PETAL_SEED=<n>` does the same for every command; the flag wins when both
  are set. An embedder calls `Env::set_seed(n)` once before the first run; the
  stream then advances across runs, and a fork copies it.
- `--policy <name>` — run under a named run policy: `fast`, `baseline`,
  `explain` or `replay`, optionally followed by modifiers: `+` or `-` and one
  of `memo`, `gate`, `opt`, `static`, `fnstatic`, `intrinsics` (`fast-memo`,
  `replay-static`). Without the flag (and without `PETAL_POLICY`), `run` uses
  `fast-memo-static`, because a script that runs once has no later run to
  replay a memoized call or reuse a kept value in. Output must be identical
  under every policy. `PETAL_POLICY=<name>` does the same for every command
  and embedder; the flag wins. What each name and modifier switches is in
  [Run policies](internals/run-policies.md).
- `--no-opt` — same as `--policy baseline`. `PETAL_OPT=off` does the same.
- `--runs <n>` — run the script `n` times on one stack, each from the top, the
  way a host runs it every frame: `state`, memo records and static values
  carry from one run to the next, and the default policy is then the host's
  `fast`. The first run that fails ends the session. `petal run --runs 2
  --policy replay f.ptl` against `--policy replay-static` is the static-value
  cache on against off, and against `--policy replay-fnstatic` it is the
  cache with and without the values it keeps inside function bodies. A static value the cache could not keep (one that
  holds a function, a `var` cell, a host handle or a pending result) is
  rebuilt on every run, and `petal run` says which on stderr, once:
  `warning: const ICONS (app.ptl:12) holds a pending result, which cannot be
  kept between runs, so it is rebuilt on every run`.
- `--state-storage <file>` — keep `state` between runs in `<file>`. See
  [State between runs](#state-between-runs).

#### State between runs

A `state` value lasts as long as the host that runs the script keeps it
([features/inline-state.md](features/inline-state.md#how-long-a-slot-lives)). A
frame-loop host (petal-sdl, Garden) runs the script again every frame and keeps
its state from frame to frame. `petal run` is one run in one process, so by
default every invocation starts from the initial values:

```
$ cat counter.ptl
state hits = 0
hits += 1
print(hits)
$ petal run counter.ptl
warning: this script declares `state` (`hits`), and 'petal run' does not keep state between runs: every run starts from the initial values. Re-run with --state-storage <file> to save state to a file and load it on the next run.
1
```

The warning goes to stderr, whenever the script declares any `state` and
`--state-storage` was not given. With the flag, every state value is loaded
from the file before the run and saved to it after:

```
$ petal run --state-storage counter.json counter.ptl
1
$ petal run --state-storage counter.json counter.ptl
2
```

The file is created if it does not exist, and a missing or empty file is a
first run. It holds every slot: top-level state, and the slots a declaration
inside a function, a loop or a `state(key)` group owns (one per call path). It
is JSON:

```json
{
  "format": "petal-state",
  "slots": [
    {
      "key": "28008301391528919",
      "name": "hits",
      "value": 2
    }
  ],
  "version": 1
}
```

`key` is the declaration's id and identifies the slot; `name` is for reading. A
slot reached through calls or loops also has a `path`. A top-level slot may be
written by hand with only `name` and `value`. Values that JSON has no spelling
for are objects tagged `"$petal"`: enum variants, class instances, records
whose fields are not in sorted order, `vec2`/`vec3`, f64 arrays, and
non-finite floats.

- **A value that cannot be stored** — a function, a host handle, a pending
  value, a UI element, or a list or record holding one — is left out with a
  warning naming the `state`. That declaration starts from its initial value on
  the next run.
- **A file that cannot be used** stops the command before the script runs
  (exit 1, phase `state-storage` under `--json`) and is left as it was: it is
  not valid JSON, not a state storage file, a version this `petal` does not
  read, or none of the state in it is declared by this script (it was written
  by a different script).
- **State the script no longer declares** — a renamed or deleted `state` —
  is not loaded and is dropped from the file on save, with a warning, as long
  as some other slot in the file still belongs to the script.
- **A run that fails** does not write the file, so it keeps what the last
  successful run left.

A slot is matched by the same ids a hot reload uses, so the edits that keep
state across a reload keep it across runs, and the ones that drop it there
(renaming the variable, its enclosing function or its module) drop it here.

Tracing and inspection options:

- `--trace` — write per-term execution events to stderr as they happen.
  `PETAL_DEBUG=1` does the same.
- `--record-trace <path>` — write the full execution trace to `<path>` as JSON
  after the run.
- `--observe` — after the run, dump the last value bound to every named
  variable. Names are function-qualified: a `b` inside `fn f` is `f.b`, so it
  does not collide with a top-level `b`. One slot per binding, last write wins,
  so a loop variable reports its final value, and a function's local reports
  its last call. Nesting composes (`outer.inner.x`), and an anonymous function
  contributes `fn<id>`. Only function bodies qualify a name: a `let` inside a
  top-level `if` reads as its bare name. A binding that never executed is
  absent, not `nil`, so "the `else` arm did not run" and "the `else` arm bound
  nil" stay different.

  ```
  $ petal run --observe -e 'let a = 1
  fn f(y)
    let b = y * 2
    b
  end
  print(f(a))'
  2

  Observed values (3):
    a   = 1
    f   = "<function>"
    f.b = 2
  ```

  The dump is printed even when the run fails, since the values bound before
  the error are the point. With `--json` it is a single object; on a failing
  run it rides on the error object as an `observations` field. A failing run
  still exits 1. Where observation sits among the other provenance tools is
  in [features/provenance.md](features/provenance.md#reading-values-without-a-query-observation).
- `--trace-emits` — attribute every buffered emit (`push_output`, draw
  commands) to the call that produced it, and dump the values with their call
  sites and per-argument edit info after the run. With `--json` the report is
  `{channels: {<name>: [emit, ...]}}`, each emit
  `{index, value, term, callee, span, args}` and each argument
  `{index, kind, value, span, editable_span}`, where `kind` is `literal` (the
  argument is a literal in the call), `binding` (a name that resolves to a
  literal elsewhere, which other calls may read) or `computed`, and `value` is
  the literal's value or `null`. `index` within a channel is the emit address
  `propose-edit --emit` takes. This is the observing half of
  [`propose-edit`](#propose-edit--propose-source-edits-that-change-an-emitted-value);
  see [features/direct-manipulation.md](features/direct-manipulation.md).
- `--trace-pending` — record pending absorptions and print the frame pending
  report to stderr after the run. `PETAL_TRACE_PENDING=1` does the same. For
  the report as the main output, see
  [`pending-report`](#pending-report--report-live-pending-resources); for what
  is being absorbed, [features/pending-values.md](features/pending-values.md).
- `--profile` — count instructions, builtin calls and collections during the
  run, from its first instruction, and print the report to stderr: the opcode
  and builtin histograms, instructions per function, native time by native
  and **by calling line**, the **native calls of a microsecond or longer** by
  calling line, and **heap allocations by line**. See
  [performance.md](dev/performance.md#the-measurement-loop).
- `--profile-slow <us>` — with `--profile` (which it implies): the threshold
  of the slow-call list, in microseconds, in place of 1.
- `--dup-stats` — count copy-on-write duplications and heap allocations
  during the run and print them to stderr afterwards. Works in any build.
- `--static-audit` — watch the dynamic inputs while each top-level term that
  [`show-static`](#show-static--static-declarations) calls static is
  evaluated, report every one touched on stderr, and exit 1 if there was one.
  See [Checking the answer](#checking-the-answer---static-audit).
- `--static-only` — evaluate only the top-level statements that are static,
  under the same audit. Described in the same section.
- `--effect-audit` — watch what every native does during the run and report
  on stderr each one whose behaviour differs from the effect row it declared
  when it was registered: under-declared (a bug) or over-declared. See
  [Catching it with the effect audit](usage/ffi.md#catching-it-with-the-effect-audit),
  and [dev/native-effects.md](dev/native-effects.md) for the rows and
  the other tools that check them.

### `bench` — What a call of a function costs

```
petal bench --fn <name> [--fn <name>]... [--iters <n>] [--seed <n>] [--json] <file.ptl>
```

Runs the file the way `run` does, repeatedly, and reports what one call of
each named function cost, over the calls the script itself makes. Where
`run --profile` describes a whole run, `bench` answers "how expensive is
`step`?".

```
$ petal bench test/benchmarks/life.ptl --fn step
bench test/benchmarks/life.ptl
  opt       44 runs      20.146 ms/run       3,913,944 instructions/run   0.861042 ms to lower  (policy fast)
  no-opt    36 runs      25.991 ms/run       6,393,899 instructions/run   0.055416 ms to lower  (policy baseline+memo+gate)
  timing costs about 48 ns per timed call (each call of a benched function, and each
  user function it calls directly); that is inside the times below, not subtracted

fn step  line 26
                                    opt         no-opt   opt vs no-opt
  calls/run                          40             40
  instructions/call            97,663.6      159,557.6   -38.8%
    self                          5,403         10,345   -47.8%
  ms/call                      0.501205       0.645980   -22.4%
    min                        0.428584       0.558084   -23.2%
    median                     0.471666       0.603958   -21.9%
    p95                        0.645833       0.888792   -27.3%
    self                       0.143209       0.192124   -25.5%
  allocations/call                   17            289   -94.1%
  copies/call                         0            272   -100.0%
    bytes copied/call                 0         48,960   -100.0%
  collections/call                    0         0.0500
    gc ms/call                 0.000000       0.009922
```

How to read it:

- **Two measurements.** Everything is measured with the optimizer on (`opt`,
  the `fast` policy a host that runs a script every frame uses; a plain `run`
  uses `fast-memo-static`, so its calls are never memoized where these may be) and
  again with it off (`no-opt`); the last
  column is the optimized figure relative to the unoptimized one. Only the
  optimizer differs between the two: memoization and the rest of the
  [run policy](internals/run-policies.md) are the same on both sides, which is why
  the second policy reads `baseline+memo+gate` rather than `baseline`. The
  rest of the policy follows `PETAL_POLICY` as it does for `run`, except that
  an environment that turns the optimizer off (`PETAL_OPT=off`,
  `PETAL_POLICY=baseline`) still gets an optimized `opt` side.
- **Inclusive and self.** The headline figure of each pair is *inclusive*: the
  function and everything it calls. `self` leaves out the user functions it
  calls; builtins it calls stay in. Allocations, copies and collections are
  inclusive.
- **Recursion.** For a recursive function the inclusive figures are per
  *outermost* call (a call made while no other call of the same function is on
  the stack), so the calls underneath are counted once, and an extra
  `outermost/run` row appears. The self figures are per call, recursive ones
  included.
- **ms/call** is the mean; `min`, `median` and `p95` are over individual calls
  (over a random sample of 65,536 of them once there are more, which the report
  says).
- **copies/call** counts copy-on-write duplications of a list, record or f64
  array: the cost of a value being immutable, and what the optimizer's in-place
  updates remove. **allocations/call** counts new heap objects of every kind.
  **collections/call** and **gc ms/call** are the garbage collections that ran
  during a call; their time is inside `ms/call`.
- **ms to lower** is the time spent lowering the program to bytecode, which is
  where the optimizer's passes run. It is a one-time cost per program, kept out
  of `ms/run`.

The file is run once as a warm-up and then until about one second has passed,
for each of the two measurements; `--iters <n>` runs it exactly `n` times
instead. Every run starts from scratch, with empty `state`, as a separate
`petal run` would. The script's own output is not printed. A run that fails
ends the command with the error (exit 1) and no report. `--seed <n>` seeds
every run's PRNG, for a script whose calls depend on `random()`.

`--fn` takes a function's name as written in the source, wherever it is
declared: at the top level, nested in another function, or in an imported
module (reported with its `file:line`). A bare method name selects that method
on every class (`--fn area`); `--fn Rect.area` selects one. Functions that
share a name are reported separately. A name that matches nothing is an error
that lists the functions there are; a function that exists and is never called
gets a `never called` row.

Timing is not free: each call of a benched function, and each user function it
calls directly, costs two clock reads (about 50 ns together on an Apple
M-series machine; the report prints the figure it measured). That time is
inside the reported milliseconds, so a function a few instructions long reads
as mostly overhead, and the clock's own resolution (about 42 ns on macOS)
shows as a `min` of zero. The instruction, allocation and copy counts are
exact regardless, and the instructions do not change: a benched run executes
on the same dispatch loop as an ordinary one.

`bench` runs what `run` runs, which is the core host. A script written for
another host (a petal-ui panel, a Garden pane) needs that host's own runner:
see `bench_panel` in [dev/performance.md](dev/performance.md). To bench a
library such a script uses, write a small console driver that imports it and
calls the function.

With `--json` the report is one object (this one is from a separate run of
the same command, so its timings differ a little from the text above):

```json
{
  "file": "test/benchmarks/life.ptl",
  "host": "core",
  "runs": {
    "opt":    { "policy": "fast", "runs": 40,
                "ms_per_run": { "min": 19.459542, "median": 19.89675 },
                "lower_ms": 0.857792, "instructions_per_run": 3913944 },
    "no_opt": { "policy": "baseline+memo+gate", "runs": 37,
                "ms_per_run": { "min": 24.884959, "median": 25.342167 },
                "lower_ms": 0.052, "instructions_per_run": 6393899 }
  },
  "timer_overhead_ns": 48.2,
  "functions": [
    {
      "name": "step", "query": "step", "file": null, "line": 26,
      "opt": {
        "calls": 1600, "calls_per_run": 40.0, "outermost_calls": 1600, "replayed_calls": 0,
        "instructions_per_call": { "inclusive": 97663.6, "self": 5403.0,
                                   "min": 97343, "max": 100855 },
        "ms_per_call": { "inclusive": 0.548808, "self": 0.14145, "min": 0.45125,
                         "median": 0.470334, "p95": 0.818083, "max": 11.928625,
                         "percentiles_sampled": false },
        "allocations_per_call": 17.0, "copies_per_call": 0.0, "copied_bytes_per_call": 0.0,
        "gc_collections_per_call": 0.0, "gc_ms_per_call": 0.0
      },
      "no_opt": { "...": "the same fields" },
      "delta_pct": { "instructions_per_call": -38.791, "ms_per_call": -13.785,
                     "allocations_per_call": -94.118, "copies_per_call": -100.0 }
    }
  ]
}
```

The 11.9 ms `max` against a 0.47 ms median is one call the OS interrupted: it
is why the report leads with min, median and p95 rather than the mean alone.

`line` is the line the function is declared on, and `file` is null for a
function in the entry file. `calls` is the total over all
measured runs. Every per-call value is null for a function that was never
called, and a `delta_pct` entry is null when either side is missing or the
unoptimized figure is zero. `replayed_calls` counts calls the memo replayed
from a record instead of running; they are in no other figure. Errors are the
usual JSON error object, with `"phase": "bench"` for a bad `--fn`.

### `check` — Compile without running

```
petal check [--json] [--strict | --lenient] [--ir] [--error-format full|bare] <file.ptl>...
petal check [--json] [--strict | --lenient] [--error-format full|bare] -e '<code>'
```

Lexes, parses, compiles and lowers the program to bytecode, then stops. Exits 1
when any of that fails, or when the checker finds an **error**: a line that
fails whenever it runs (see [Errors and warnings](#errors-and-warnings)).
Exits 0 otherwise. This is the cheap gate for editors and CI.

Given several files (`petal check a.ptl b.ptl`, or a shell glob), `check`
checks each as its own program with the same flags, prints each file's
findings under a `==> <file> <==` header (a file with nothing to say prints
nothing), and exits non-zero when any file fails, ending with
`check failed for 1 of 3 files: b.ptl`. With `--json` it prints one object,
`{"ok": <bool>, "files": [...]}`, each entry that file's `--json` object plus
its `"file"`. `-` (stdin) and `-e` check one source only.

Options:

- `--json` — emit errors and warnings as JSON. When the program compiles:
  `{"ok": <bool>, "warnings": [...]}`, each entry
  `{message, severity, line, column, file}` — `severity` is `"warning"` or
  `"error"`, `ok` is `false` when any entry is an error, and `file` is `null`
  for the entry file. When it does not compile:
  `{message, line, column, phase, errors, ...}` — see
  [Error phases](#error-phases).
- `--strict` — exit 1 when there are warnings too. Plain `check` exits 0 for a
  program that only has warnings.
- `--lenient` — report checker errors but exit 0 unless the program fails to
  compile or lower. For a sweep that only asks "does this compile?" over
  scripts written for several hosts.
- `--ir` — check `<file>` as JSON IR instead of source; `-` reads stdin, as
  with `run --ir`. The IR is validated, then lowered, so a third-party IR
  emitter can be checked without running its output:
  `emit-my-ir | petal check --json --ir -`. A load failure is reported with
  `"phase": "parse"`; IR that loads but cannot be lowered gets `"phase": "lower"`.
- `--error-format full|bare` — as on [`run`](#run--execute-a-program).

#### Which names exist

No host is consulted. A name exists
because the file declares it, an import binds it, or it is a `core`/`std`
builtin. The domain builtins (`lerp`, `noise`, `vec2`, `hsv`, `json_parse`, …)
are behind imports of their own (`import creative`, `import noise`, …; see
[Builtins](Builtins.md#which-builtins-need-an-import)); an unimported one is
an error that names the import to add. A script written for a host imports the host's **interface package**,
whose `native fn` declarations ([usage/ffi.md](usage/ffi.md#native-interfaces-native-fn))
make each native known, with its parameters, whether or not this process
registers it:

| The script is for | It writes | `check` finds the package |
| --- | --- | --- |
| any petal-ui host | `import ui` | bundled in the binary |
| a host with only petal-ui's input half | `import ui/input` | bundled |
| the bloom / text_layout / glsl libraries | `import bloom`, … | bundled |
| a Garden panel | `import ui`, `import garden` | a path dependency in the project's `petal.toml`, or `-I garden/garden-script/packages` |
| Garden's config host (`init.ptl`) | `import garden_config` | the same |
| petal-desktop-sdl's extras | `import sdl` | a path dependency, or `-I integrations/petal-desktop-sdl/packages` |
| your own host | `import <its package>` | `<name> = { path = "…" }` under `[dependencies]`, or `-I <the directory holding it>` |

`check` finds the nearest `petal.toml` by walking up from the script, as every
command that compiles does, so inside a project no flag is needed. The
example trees in this repository carry such a manifest (`examples/petal.toml`
and others), which is why `petal check examples/games/flappy/app.ptl` resolves
`import garden` by itself.

A script written for a host that puts names in scope without an import (it
binds its interface as an implicit prelude, or joins library files ahead of
each script into one program) is checked as that host compiles it when its
project's `petal.toml` has a [`[host]`](packages.md#host) section, the same
way `petal lsp` analyzes it: `imports` are bound like the core prelude `std`,
and the `prelude` files are joined ahead of the script (a script that is one
of them sees only the files listed before it). Findings keep the script's own
lines and snippets; one in a prelude file is left to the check of that file,
and a prelude file that does not compile is named in the error
(`lib/theme.ptl: … [line 4, column 3]`). A prelude file that cannot be read
is a `warning:` line. `-e`, stdin and `--ir` read no `[host]`.

When the packages a script loads state `[requires]`, a passing `check` ends
with one line naming them — `requires host interfaces: ui >=0.1 (bloom)` —
and `--json` carries a `requires` array of `{interface, version, package}`.
`check` never fails on them; a run in a host that does not provide one does
([packages.md](packages.md#requires)).

`check` and `run` agree on every script: `run` begins with the same check and
does not start a program that fails it (`run` reads no `[host]`: a host
script runs in its host). The one thing `run` adds is linking.
`check` never links, so a script that imports `ui` checks clean; `petal run`
of it stops after the check, before any line runs, because `run` registers the
core builtins only:

```
$ petal run -e 'import ui
clear(0, 0, 0)'
Error: module 'ui/input' needs native fns this host does not register: mouse_x, mouse_y, hovered, mouse_down, +14 more; module 'ui' needs native fns this host does not register: clear, text_width, text_metrics, text_wrap, +32 more — the program cannot run here; run it with a host that provides them (`ui` is the petal-ui interface: use a petal-ui runner such as petal-sdl; `petal run` provides the core builtins only)
```

A UI script runs in a petal-ui runner such as `petal-sdl`
([usage/building-apps.md](usage/building-apps.md)). A script that uses petal-ui names with
no `import ui` gets the line from
[`petal fix-imports`](#fix-imports--migrate-imports).

#### Errors and warnings

Compiling runs the optional type checker (see
[Type Annotations](language-guide.md#type-annotations)) and a few lints. Their
findings never stop the program from compiling or running; they print to
stderr with a source caret, or appear in the `warnings` array with `--json`.
Each has a severity.

An **error** is a line that fails whenever it runs, so `check` exits 1 on it:

- a call to, or read of, a name that nothing defines
  (`unknown function \`frob\``, `undefined variable \`nope\``) — it would
  fail with `Unknown function` or `Undefined variable`;
- a qualifier read as a value, or a member a module does not export
  (`'hud' is a module, not a value`, `module 'hud' has no export 'nope'`);
- a call whose argument count no overload accepts (`f(1)` where every `f`
  takes two arguments) — for a function a plain import bound, counted
  against the module the name resolved to;
- a call to a builtin or a host native with a count its `native fn`
  declaration does not take (`len(1, 2)`, `mouse_x(3)`); a native declared
  with a rest parameter takes any count above its fixed ones;
- method syntax on a plain value (a number, a string, a list, a vector)
  that needs a packaged builtin the file has not imported (`v.mag()` with no
  `import creative`);
- a named argument no parameter has, or one that fills a slot twice — on a
  `fn`, or on a builtin, which is checked against the parameter names it
  declares (`clamp(v, low: 0, hi: 1)` is `clamp() has no parameter named
  'low'`);
- a required parameter left without a value, and a call that more than one
  overload's default parameter values could accept;
- a named argument to a builtin that declares none (`print(1, sep: 2)`).

A host native is held to the parameter names and counts its interface
package declares, like a builtin.

`petal run` refuses every one of these before the first line runs, whether or
not the line would have been reached: it prints what `check` prints, then
`N errors found by check; the program was not run`, and exits 1. With `--json`
the error object has `"phase": "check"` and one `errors` entry per finding.

```
$ petal check -e 'print(nope)'
error: undefined variable `nope`
 --> [line 1, column 7]
  |
1 | print(nope)
  |       ^^^^
1 error found by check
$ echo $?
1
```

An unknown-name error on a host's name means the script does not import the
host's interface package (or `check` was not given the `-I` that finds it).
`run` reports these the same way and does not start the program.

Everything else is a **warning**, and does not change the exit code unless
`--strict` is set:

```
$ petal check -e 'let x: int = "s"'
warning: type mismatch: `x` declared `int` but assigned `string`
 --> [line 1, column 14]
  |
1 | let x: int = "s"
  |              ^^^
$ echo $?
0
```

The same channel carries a few warnings that are not about types: a discarded
pure call (`push(xs, x)` whose result is thrown away), a function that captures
a module `state` rebound below it, a call to a declaration further down the
file that could not be hoisted (the rules are in
[syntax/declaration-order.md](syntax/declaration-order.md#when-a-fn-is-not-hoisted)), and a key name written as a literal that is
not one (`key_pressed("enter")`, which is always false: `did you mean
"return"?`; the names are in [ui-reference.md](ui-reference.md#input)).

#### Error phases

Every `--json` error object carries a `phase` naming the stage that rejected
the program:

| `phase` | Meaning |
| --- | --- |
| `lex` | Tokenizing failed — an unterminated string, an unexpected character, a bad color literal. |
| `parse` | The tokens are not a valid program — a missing `=`, an unclosed construct, an unexpected token. |
| `module` | An `import` could not be resolved, or the imports form a cycle. |
| `compile` | The program parses but is not well-formed — writing a `var` with `=`, assigning to a binding from an outer function, importing a name a module does not export. |
| `check` | The program compiled, and the check found an error: a line that fails whenever it runs (`run` only; `check` reports these in `warnings` with `"ok": false`). |
| `lower` | The term graph could not be lowered to bytecode. |
| `link` | The program needs a host interface or a native this host does not provide (`run` only). |
| `runtime` | The program compiled and ran, and failed during execution (`run` only). |
| `state-storage` | The `--state-storage` file could not be loaded or written (`run` only). |

`message` is the whole human-readable error and `line`/`column` locate its last
diagnostic. Front-end failures also carry `errors`, one entry per diagnostic;
the compiler reports every error it finds, not only the last:

```
$ petal check --json -e 'var x = 1
x = 2'
{
  "error": true,
  "phase": "compile",
  "message": "`x` is a `var`; use `set x = ...` to write it",
  "line": 2,
  "column": 1,
  "errors": [
    {
      "message": "`x` is a `var`; use `set x = ...` to write it",
      "line": 2,
      "column": 1,
      "file": null
    }
  ]
}
```

Each entry's `message` has no position suffix; `file` is the module's display
name, or `null` for the entry file.

### `test` — Run test blocks

```bash
petal test                          # every .ptl file under the current directory
petal test src/sim.ptl tests/       # files and directories
petal test --filter spawn           # only the tests whose name contains "spawn"
petal test --list                   # name the tests, run nothing
petal test --json                   # one JSON document instead of the report
petal test -e 'test "sum" do expect_eq(1 + 1, 2) end'
```

Finds the `test "name" do … end` blocks of every `.ptl` file under the given
paths and runs each one in isolation: the file's top level on a fresh stack,
then the test's body. A file with no test block is passed over without being
loaded. Each file is the entry of its own program, with its imports and its
project's `petal.toml` resolved as `run` resolves them, and only the tests
written in that file are run.

```console
$ petal test examples/console
examples/console/fibonacci.ptl
  ok    the two definitions agree
  ok    known values
examples/console/text_utils.ptl
  ok    truncate leaves a short string alone
  ok    truncate cuts a long string and marks the cut
  ok    shout

5 tests: 5 passed (2 files)
```

A test is `ok`, `FAIL` (an `expect`, `expect_eq` or `expect_near` did not
hold; each failure is listed with its file and line and both values) or
`ERROR` (it raised, or ran past the time limit). The exit status is 1 when a
test fails or errors, when a file with tests cannot be loaded, checked or
linked here, or when an expectation at a file's top level fails; otherwise 0.

| Flag | |
|------|---|
| `--filter <text>` | Run only the tests whose name contains `<text>` |
| `--list` | Print the tests that would run and run nothing |
| `--show-output` | Show what every test printed, not only the failing ones |
| `--timeout <seconds>` | Stop a test that runs longer, the file's top level included (default 60; 0 is no limit) |
| `--json` | Print a JSON document: `ok`, `summary`, and per file its `tests` with `status` (`passed`, `failed`, `error`), `failures`, `error`, `checks`, `output` and `duration_ms` |

The language, the JSON shape and running tests from a host are described in
[testing.md](testing.md).

### `fmt` — Canonical layout

```
petal fmt [<path>...]          # rewrite in place; directories recurse; default "."
petal fmt --check [<path>...]  # write nothing; list files that would change; exit 1 if any
petal fmt --diff [<path>...]   # write nothing; print a unified diff (also -d)
petal fmt -e '<code>'          # print the formatted code
petal fmt -                    # stdin to stdout
```

Shaped like `gofmt`/`deno fmt`: in-place by default, no style options, `.ptl`
files found by walking directories (dot-directories, `node_modules` and
`target` are skipped). A file that does not parse is reported and left alone;
the rest still run.

fmt touches whitespace only (see [dev/linter-plan.md](dev/linter-plan.md)),
with one exception listed last:

- **Indentation** — 2 spaces per open construct, from the token stream.
- **Spacing within a line** — one space around binary and assignment operators,
  `->` and `|>`, and after `,` and `:`; none inside `()` `[]` `{}` or a string's
  `{…}` hole, none before `,` `:` `;`, none around `.` `?.` `..`, none after a
  unary `-` `!` or `@` or a spread `...`, and none between a callee and its `(`.
  Any other run of blanks becomes one space.
- **Blank lines** — at most one in a row, none at the start or end of the file,
  exactly one final newline. Trailing whitespace goes.
- **`export` → `pub`** — the deprecated spelling of the modifier is rewritten.
  The two words declare the same thing, so the program does not change.

It keeps what the author aligned. A run of spaces that lines a token up with a
token on the line above or below (a table of records, a column of `=`), a
double space repeated after commas to group arguments, a trailing comment's
column, and a continuation line lined up under its open bracket or its `if` all
survive, moving only as far as the line they hang from moves.

It never wraps lines, reorders anything, or edits strings, raw strings or JSX
text, and it checks every result: the output must lex to exactly the input's
tokens (an `export` modifier counting as its `pub`), or the file is refused with a `fmt bug` error.

Opt out with `// petal-fmt-ignore` (the next line stays as written),
`// petal-fmt-off` … `// petal-fmt-on` (a region), or `// petal-fmt-ignore-file`.

### `fix-imports` — Migrate imports

```
petal fix-imports [-I <dir>]... [<path>...]   # rewrite in place; directories recurse; default "."
petal fix-imports --check [<path>...]         # write nothing; list files that would change; exit 1 if any
petal fix-imports --diff [<path>...]          # write nothing; print a unified diff (also -d)
petal fix-imports -e '<code>'                 # print the fixed code
petal fix-imports -                           # stdin to stdout
```

Migrates code written before the current module system, where a bare name
could be a sibling file, `import m: *` existed, and a host's names and every
builtin were in scope with no import. It edits import lines and nothing
else:

| Before | After | When |
|---|---|---|
| `import level` | `import ./level` | `level.ptl` sits beside the file and no package is called `level` |
| `import color` (with `color.ptl` beside the file) | `import ./color` | a package is *also* called that (the bundled `color`), and the file takes a name from it that the package does not export, or writes `: *` — it was written for the sibling. When every name it takes is the package's the line is left alone; when it takes nothing by name, a note says both exist |
| `import rng` (found through `-I shared`) | `import ../shared/rng` | the same `-I` is passed here |
| `import m: *`, `pub import m: *` | `import m`, `pub import m` | always |
| `import m: a` … `m.b` | adds `import m` | the file reaches through `m.`, which a selective import no longer binds |
| *(nothing)* | adds `import kit` | a bare name nothing in the file provides is exported by exactly one available package |
| *(nothing)* | adds `import creative`, `import vec2`, `import noise`, … | the same, for a builtin that now lives in a bundled package (`lerp`, `vec2`, `noise`, `hsv`, `json_parse`, `symbol`, …; [the list](Builtins.md#which-builtins-need-an-import)) |
| *(nothing)* | adds `import vec2` for `v.normalize()` | a moved builtin called with method syntax (a `check` error on a plain value, otherwise a failure when the line runs): added when nothing in the program defines a function, class method or record field of that name |
| *(nothing)* | adds `import ui` | the same, for a host's names: a script that used petal-ui names with no import gets its `import ui` (and, with the host's package a dependency of the project or on `-I`: `import garden`, `import sdl`, …). When several packages export the name and all but one are built on that one (bloom's `button` over ui's), the one underneath is imported |

"Available" is the packages bundled in the binary, the `[dependencies]` of
the project the file is in (the nearest `petal.toml` above it), and the
packages under `-I` and `PETAL_PATH`. With `-e` or stdin there is no file and
so no project. A path that already resolves as a package is never
rewritten, so the command is safe on migrated code and running it twice
changes nothing more. It prints each file it rewrote with one line per edit:

```
$ petal fix-imports old.ptl
old.ptl
  line 1: `import level` -> `import ./level`
  line 2: `import ease: *` -> `import ease`
  added `import creative` (for `lerp`)
  added `import tw` (for `twice`)
fixed imports in 1 of 1 file(s)
```

The import lines of every file are rewritten first, and only then is each
file compiled to look for missing imports, so the order files are given in
does not matter and `--check` sees what a real run would. A file that does
not parse is reported and skipped. A file that does not compile still has
its import lines fixed, and gets no imports added; a name that several
packages export is reported and left for you to choose. A package that does
not load yet (it is itself unmigrated) is named when a file is still
missing a name it might have provided.

It does not resolve an [ambiguity](module-system.md#which-name-wins) that
the new bare binding of plain imports introduces; the compiler reports those
where the name is used. It is a command of its own rather than a `lint`
rule: a lint fix starts from a file that compiles, and an old-style file
does not.

A file with the comment `// petal-fix-imports-ignore` on a line of its own
gets no import *added* (its import lines are still rewritten). It is for a
file whose unknown names are not meant to come from a package — a shader
written as Petal lambdas, whose `step` is GLSL's and not bloom's.

### `lint` — Rules with fixes

```
petal lint [<path>...]                       # report findings; exit 1 if any
petal lint --fix [<path>...]                 # apply every fix in place
petal lint --json [<path>...]                # findings as JSON
petal lint --rules                           # list the rules
petal lint --rules-include=prefer-let ...    # run only these (comma-separated)
petal lint --rules-exclude=prefer-match ...  # run all but these
petal lint -e '<code>'                       # report; with --fix, print the fixed code

petal lint --fix --verify <path>             # prove the rewrite before writing it
petal lint --fix --verify=strict <path>      # demand full IR equality
```

Shaped like `deno lint`: each finding names its rule, and most rules have a
fix. A rule that cannot show its rewrite to be the same program reports the
site and leaves it (the "Fix" column says when), and three rules only report.
`--fix` applies what has a fix, prints what is left, and exits 1 while
anything is; `--json` gives each finding a `fixable` field.

```
$ petal lint app.ptl
app.ptl:12:3: prefer-let: `var n` is never shared with a nested function; declare it with `let`
app.ptl:14:5: prefer-compound-assign: `n = n + …` can be written `n += …`
found 2 problem(s) in 1 file(s); run `petal lint --fix` to fix them
```

| Rule | Finds | Fix |
|---|---|---|
| `prefer-let` | a `var` whose every read and write stays in its function | `var` → `let`, `set x = …` → `x = …`, `get x` → `x` |
| `no-redundant-cast` | `int(n)` where `n` is provably an `int` (likewise `float`, `str`); and `float(n)` on a proven `int` or `num` that is an operand of `+ - * / %` whose other operand is a proven `float`, since that arithmetic converts `n` the same way (`float(i) * 0.5`) | delete the call; parentheses follow the slot (`2 * int(a + 1)` → `2 * (a + 1)`). Of `float(a) / float(b)` only the left cast goes, and a cast beside an `int` operand always stays: `/` on two ints truncates |
| `prefer-match` | an `if`/`elsif` chain testing one subject against string, bool or nil literals | rewrite as a `match` |
| `prefer-repeat` | a `for` over `range(n)` / `range(0, n)` whose whole body appends one string literal (or `string` parameter) to an accumulator that starts as a string | `x ++= repeat(s, n)` — the loop copies `x` on every pass |
| `prefer-field-set` | `set X = {...get X, f: v}` (or `X = {...X, f: v}`): the whole record is copied to change a field | `set X.f = v`, one write per field. Applied where the file shows `X` to hold a plain record (declared once, with a record literal, and only ever assigned record literals): spreading `nil` gives a record where a field write to `nil` fails. Several fields are split only when no value calls anything or reads `X`. Otherwise reported |
| `prefer-compound-assign` | `x = x op e` for an operator with a compound form; also a field or element written twice, `set xs[i] = get xs[i] + d`, `r.n = r.n * 2`, where the place is names, literals and operators only | `x op= e`, `set xs[i] += d` |
| `neg-zero-sub` | `0 - x` and `0.0 - x` | `-x` where the zero is `0` and `x` is a proven `int`. Otherwise reported: `0.0 - 0.0` is `0.0` and `-0.0` is not (`atan2` and a division tell them apart), and `0.0 - n` makes a float of an int |
| `prefer-any` | `len(filter(xs, p)) > 0` (also `!= 0`, `>= 1`; and `== 0`, `< 1`, `<= 0` for `!any`) | `any(xs, p)`, which stops at the first match. Applied where `p` is a lambda written in place whose body is one expression with no call but to pure builtins; otherwise reported, since `any` does not call `p` on the rest. (A pure body can still raise on a later element, such as `x.v` of an int: the fixed program then finishes where the old one raised, never the reverse.) |
| `dead-alias` | `let a = v` in a function, where `v` is a parameter or a local of that function that is not written from there on (nor in a loop around the alias, nor by any nested function) and `a` is only read | delete the `let` and read `v` (`get v` for a `var` read from a nested function). Reported only, when the `let` shares its line |
| `shadows-builtin` | a `fn` with the name of a core builtin, a standard-prelude function, or a function of a bundled package the file imports whole, taking an argument count the existing one takes. Every call in the file reaches the new one | none (callers may be in other files) |
| `reimplements-builtin` | a `fn` whose whole body calls one builtin with its own parameters in order, or is `if r == nil then d else field(r, k, d) end` (`field` already answers the fallback for `nil`) | none |
| `int-division` (opt-in) | `a / b` on two proven ints, whose truncated quotient is then an operand of float arithmetic or the argument of `float(...)` | none: either reading can be meant |
| `prefer-pub` | the deprecated `export` modifier, on any declaration or `import` | `export` → `pub` |
| `state-key-deprecated` | every `state(key)` declaration, which is deprecated (the compiler warns about it too, under the same rule id) | `state(k)` → `memo(k)` where the slot is never written: not `state(k) var`, not `pub state(k)`, and only when the rewrite compiles, since a `memo` rejects every write. Otherwise reported |
| `state-as-constant` (opt-in) | a plain top-level `state` (no key, not `state var`; `pub state` included, since importers cannot write an export) that nothing in the program assigns and whose initializer is static | `state` → `const` |

Layout is not a finding: that is `fmt`. A file `--fix` changes is formatted
afterwards, because a rewritten chain moves code between lines. Every fix is
compile-gated: if the original compiled, the fixed file must too, or nothing is
written.

`int-division` is **opt-in** because the shape it finds is usually meant:
over the fifteen Cheesecake games all seven of its findings were a row or
column index scaled by a float (`float(k / 2) * 3.0`). What authors cast
defensively against is a literal fraction, and that is a `petal check`
warning with no rule to enable: `` `1 / 2` is 0: `/` on two ints drops the
remainder ``.

`state-as-constant` is **opt-in**: it runs only when `--rules-include` names
it (`petal lint --rules-include=state-as-constant`, alone or with other
rules), and `petal lint --rules` marks it so. Every other rule's fix is
another spelling of the same program; this one is a judgement. A
never-assigned `state` is a constant as far as the program can tell, but a
host can write a `state` by name, saved state can be restored into it, its
value survives a hot reload where a `const` is rebuilt, and its initializer
runs once in every host where a `const` is built once only if the host's run
keeps static values. Where any of that matters, leave the `state` and put
`// petal-lint-ignore state-as-constant` on its line.

`state-key-deprecated` reports every keyed `state`: its slot is one per key
value, whichever caller reaches it, so it is a global hidden in a function
(see [Keyed state](features/inline-state.md#keyed-state-statekey)). A slot
nothing writes only ever holds what its initializer computed for its key,
which is a [`memo`](features/memo.md), and `--fix` respells it so. It finds
"nothing writes" by compiling the rewrite. One difference stays the
author's to judge: a memo is a cache, recomputed after a reload that changes
the program or a run that does not reach it, so an initializer that is not a
pure function of its key (`state(id) tint = random(0.0, 1.0)`) would draw a
new value. Keep such a slot (in a top-level `state var` Map, or as plain
`state`) and silence the site with `// petal-lint-ignore state-key-deprecated`.
The compile warning for the same site (`petal check`, `petal run`, the
language server) honours that comment as well.

`state-as-constant` also asks the compiler rather than the text: it
compiles the file, classifies it as [`show-static`](#show-static--static-declarations)
does, and compiles its own rewrite, keeping only the declarations that are
accepted as `const`. A `state` built from a host's natives is therefore left
alone (lint has no host). Converting one `state` can make the next one's
initializer static (`state SORTED = sort(RAW)`), and the rule repeats until
nothing changes, so one `--fix` is enough.

Silence a rule with `// petal-lint-ignore <rule> [<rule>…]` on the line before
the finding or at the end of its line (no names silences every rule), or for a
whole file with `// petal-lint-ignore-file [<rule>…]`. Text after `--` is a
reason. A silenced finding's fix is not applied.

#### `--verify` — prove the rewrite

`--verify` compiles the original and the rewritten text and compares their IR
(the same comparison as [`ir-equal`](#ir-equal--are-two-files-the-same-program)).
Nothing is written unless the comparison is acceptable; a rewrite that cannot
be accepted exits **3**, distinct from the plain "problems found" exit 1. It
works with and without `--fix`.

| Mode | Demands | When a semantic pass fired |
|---|---|---|
| `--verify` (= `--verify=ir`, the default) | Formatting and the IR-invisible rules must not change the IR | Allowed: reported as an expected IR change, file still written |
| `--verify=strict` | The whole rewrite must be IR-equal | Refused: exit 3, file untouched |

Formatting, `prefer-compound-assign` and `prefer-pub` are meant to leave the IR unchanged.
The cast, `match`, `var`, `repeat`, `prefer-field-set`, `neg-zero-sub`, `prefer-any`, `dead-alias`,
`state-as-constant` and `state-key-deprecated` rules change it by design. On such a file the default mode proves
the part it can, prints the first difference, and says that a run diff is what
would prove the rest:

```
$ petal lint --fix --verify app.ptl
verify: app.ptl: rewrite changed IR (1 prefer-match); formatting was proven IR-equal.
        First difference:
function `label` body: statement count differs (at 2:6)
  original: 4
  rewritten: 2
verify: run-diff verification needed for the semantic rules
```

If formatting ever moves the IR, that is a bug: `--verify` reports it as one
and refuses to write, whatever the mode.

### `suggest` — Suggest safe refactors for a file

```
petal suggest [--only <kind>[,<kind>]] [--apply | --verify]
              [--json] [--from <file>]... <file>
petal suggest [<options>] -e <code>
```

Proposes changes to a file that make it say more, each with the reason behind
it. A **suggestion channel**, not a check: nothing here runs during an ordinary
compile, nothing can fail a build, and nothing is written without `--apply`.
See [suggestions-plan.md](dev/suggestions-plan.md).

There are four kinds, and `--only` picks between them:

| Kind | What it proposes | The proof `--apply` holds it to |
|---|---|---|
| `types` | A type annotation the program already implies | Still compiles, and gains no type-checker warning |
| `named-args` | Named arguments for a call that passes three or more by position | Compiles to the same IR ([`ir-equal --named-args`](#ir-equal--are-two-files-the-same-program)), and gains no warning |
| `return-types` | `-> list` or `-> nil` for an un-annotated function that ends in a loop | `-> list`: compiles to the same IR, and gains no warning. `-> nil`: compiles and gains no warning — **the IR changes, by design** |
| `advice` | Nothing — a comment on code that looks like it could be simpler | None: there is no rewrite, so `--apply` never acts on it |

**Not every suggestion preserves the compiled program.** Three of the four
kinds leave the IR alone or have no edit at all. `return-types` is the
exception: `-> nil` turns off a function's implicit return, so the function
stops building a list — which is the point of suggesting it. It is offered only
where no call in view reads that list; see [Return types](#return-types).

```
$ petal suggest app.ptl

app.ptl:22  fn step
  suggest: -> float
  because: the body's tail expression is `float`

app.ptl:41  fn scale_rect
  suggest: r: record
  because: read with .h, .w, .x, .y (all fields of `Rect` — narrow it by hand
           if a plain record is never passed)

app.ptl:58:3  call draw_rect
  suggest: draw_rect(x: 0, y: 0, w: 320, h: 48, c: PANEL)
  because: `draw_rect` is `draw_rect(x, y, w, h, c, a = …)`

app.ptl:61:11  call clamp
  suggest: clamp(t, lo: 0, hi: 1)
  because: `clamp` is the builtin `clamp(value, lo, hi)`; `value` stays positional

app.ptl:70  fn draw_all
  suggest: -> nil
  because: ends in a `for` loop, which collects a list as its implicit return,
           but none of its 3 calls uses it — `-> nil` turns the implicit
           return off, so the loop builds no list

2 type annotations across 24 functions; 2 calls that could name their arguments;
1 return type for a loop's implicit return.
Re-run with --apply to write them.
```

#### Type annotations

Reads what a program's own call sites and function bodies already say about
types nobody wrote down.

Evidence, for a parameter: the types callers actually pass, the declared type
of a slot the parameter is forwarded into, and a field read (which proves
record-shaped — a plain record has the same fields a class does, so the
matching class is named as a hint rather than written). For a return type: the
body's tail expression and every explicit `return`.

Numbers are treated differently in the two slots. A parameter is a
precondition, so numeric evidence always proposes `num`: the callers this
compile can see are not the callers there are. A return type is a promise the
body keeps, so it stays precise.

Suggestions **compound** — an applied annotation is evidence for the next pass
— so re-running until it reports nothing is the intended workflow.

#### Return types

A `for` in tail position collects a list, and that list is the function's
[implicit return](syntax/implicit-return-values.md). An un-annotated function that
ends in a loop therefore does not say which of two things it is: a mapping
whose callers want the list, or a side-effect loop that builds one on every
call for nobody. Declaring it settles that — `-> list` documents the first,
and `-> nil` turns the implicit return off so the second allocates nothing.

Which one a function is shows in how it is called:

| The calls in view | Reported as | `--apply` |
|---|---|---|
| some call uses the result | `suggest: -> list` | writes it, proven IR-equal |
| called, and no call uses the result | `suggest: -> nil` | writes it; the IR changes |
| never called, or `pub` | `choose: -> list or -> nil` | skips it |

A call's result is *used* when the call is bound, passed, returned, collected
by a loop, or written with an [in-out argument](syntax/rebind-operator.md); it
is unused when the call is a statement. A call that is the tail of another
un-annotated function is used exactly when *that* function's result is, and the
analysis follows such chains. Doubt resolves toward leaving the choice to the
author: a function that is read as a value (`map(xs, f)`) rather than called
is reported with both options, as is one whose only caller is itself
undetermined, and one whose name a local binding (a `let`, a parameter, a
nested `fn`) shadows somewhere — calls are matched by name, so a statement call
through that name may not be a call to it. A use anywhere wins over `pub`.

"Never called" usually means "called by the host" — a UI app's `draw`, an
embedding's callback — and those callers cannot be seen, which is why that
case is never written. For a `pub` function, `--from <app>` supplies the
callers a library module lacks.

The kind covers **loop tails only**. A function with a branch whose tail is a
loop and another whose tail is not (`if n > 0 then for … end else 0 end`) is
offered `-> nil` when its result is unused and nothing otherwise — there is no
one type to name. A function with a `return <value>` of its own is left alone.

```
app.ptl:9  fn orphan
  choose: -> list or -> nil
  because: ends in a `for` loop, which collects a list as its implicit return;
           nothing in view calls it. Declare `-> list` if callers read the
           list, or `-> nil` if the loop is run for its side effects (no list
           is built)
```

In `--json` this is an item with `"kind": "return-type"`: `function`, `arity`,
`line`, `usage` (`used`, `unused` or `unknown`), `type` (`"list"`, `"nil"`, or
`null` when the choice is the author's), `options` (always `["list", "nil"]`),
`preserves_ir` (false for `-> nil`), and `edits` — empty when `type` is null.

#### Advice

Some things can be noticed but not rewritten: the detection is a heuristic, the
right replacement depends on what the code was meant to do, and nothing could
prove the two alike. Advice says so and stops there — a place, a comment, and
the rule that produced it. It has no `suggest:` line, `--apply` writes nothing
for it, and `--verify` does not count it.

```
app.ptl:266:3  fn sort_tasks
  advice: this looks like a hand-written insertion sort, which rebuilds the
          list once per element. `sort(list, compare)` takes a comparator …
          The test here is `task_less(t, x)`, so `sort(list, task_less)` is
          likely the whole loop.
  rule: hand-written-sort
```

| Rule | What it notices |
|---|---|
| `hand-written-sort` | A loop that inserts each element into place by rebuilding the list, where [`sort(list, compare)` or `sort_by(list, key)`](Builtins.md#sortlist-compare) is one call |

In `--json`, advice is an item with `"kind": "advice"`, a `rule`, a `message`,
and an `edits` list that is always empty.

#### Named arguments

`draw_rect(0, 0, 320, 48, panel)` says nothing about which number is which;
`draw_rect(x: 0, y: 0, w: 320, h: 48, c: panel)` does. Every call that writes
**three or more** arguments, at least one of them by position, is a candidate.

**The callee must be known for certain, and accept names.** It is read from the
compiled program, not guessed from the text: the call is followed back to the
function it must run, so a name that was rebound or shadowed on the way is
followed too.

| Rewritten | Left alone |
|---|---|
| a `fn`, each overload variant by itself | a parameter, or any value that merely holds a function |
| a lambda held in a `let` | a function read out of a record or a list |
| a class constructor | a name rebound under a branch or in a loop |
| a function of an imported module (`shapes.area(…)`) or of a host's interface package (`ui`) | a method dispatched on its receiver at runtime (`xs.slice(…)`) |
| a method call the checker pinned to one class | a variadic builtin (`print`) |
| a builtin that declares its parameter names ([Builtins.md](Builtins.md)) | a native declared with a rest parameter (`...args`), which has no parameter names |

**The rewritten call must be the same call.** It selects the same overload
variant under the [name-aware rule](syntax/function-overloading.md), every argument
fills the parameter it filled before, and the arguments stay in the order they
were written — so they are evaluated in the same order.

**Two shapes on one count are left alone.** Proving the rewrite changes nothing
is not the same as proving the names are true. When two variants of an overload
set both take a call of that many arguments and call them different things,
only one of the two readings could be written down, so neither is. The `ui`
package's `draw_line` takes seven arguments as the flat `(x1, y1, x2, y2, r, g,
b)` and as `(x1, y1, x2, y2, c, a, width)`; a seven-argument `draw_line` gets
no suggestion, an eight-argument one (flat only) does. Variants that agree on
the names — one shape declared at several lengths — are named. A *single*
function that serves two shapes from one parameter list cannot be told apart
this way: its parameter names are all it declares, and they are what is
suggested.

**Which arguments get a name.** Positional arguments must come before named
ones, so the only choice is where the names start. They start as early as they
can, after:

- the **receiver** of a method call or the **piped value** of `x |> f(…)`,
  which is not written between the parentheses;
- **placeholder parameters**, whose names say nothing about the role: a name
  starting with `_`; neighbouring parameters that only count through the
  alphabet from `a` (`a, b`, `a, b, c`); and neighbouring parameters numbered
  in sequence on one stem (`p1, p2`, `c0, c1`). Everything up to the last
  placeholder stays positional — `lerp(0, 10, t: 0.5)`. Coordinates such as
  `x1, y1, x2, y2` are not a run (the stems alternate), and a lone `a` is
  alpha, not a placeholder;
- the **subject**, when the first parameter is the thing being operated on —
  `self`, `this`, `value`, `list`, `collection`, `string`, `record`, `array`,
  `text` or `rect`, or one of the short forms scripts write for those roles
  (`s`, `str`, `txt`, `v`, `val`, `xs`, `lst`, `arr`, `items`, and a leading `r`
  — a rect — unless a `g` follows it): `slice(xs, start: 1, end: 3)`,
  `tx(title, x: 12, y: 8, style: S_TITLE)`, `pill(r, label: "Fit", active: on)`;
- arguments **already spelled like their parameter**, for as long as the run
  lasts: `box(x, y, w: 3, h: 4)`, not `box(x: x, y: y, …)`. Once a name has
  been written every later argument needs one, so `box(x: 1, y: y, w: 3, h: h)`.

Arguments already named are kept exactly as written, and a call with nothing
left to name gets no suggestion — so applying is idempotent: a second run
reports nothing.

**Calls not worth naming.** A rewrite can be safe and still read worse than
what it replaces. Three shapes are left alone, each because the names would
only repeat what the call already says:

- a **bare colour** — a call whose arguments are colour channels and nothing
  else, `(r, g, b)` or `(r, g, b, a)`: `clear(18, 20, 28)` stays as it is. The
  channels at the end of a longer call are named with the rest of it:
  `draw_rect(x: 0, y: 0, w: 320, h: 48, r: 20, g: 25, b: 45)`;
- a **function literal under a one-letter name** — `reduce(xs, 0, fn(a, b) ->
  a + b)` is not rewritten to `reduce(xs, initial: 0, f: fn(a, b) -> a + b)`:
  the literal is visibly the function. A parameter with a real name
  (`on_click: fn() -> …`) is still written;
- **mostly echoes** — a call where more of the names would repeat their own
  argument than add to it: `hash(ix: ix + 1, iy: iy, seed: seed)`.

A host's functions are named like any other imported module's: with
`import ui` in the file, a bare `draw_rect` is the `ui` package's function.

#### Options

- `--only <kind>[,<kind>]` — look for these kinds only: `types`, `named-args`,
  `return-types`, `advice`. All four by default.
- `--apply` — write the suggestions into the file, each kind behind its proof
  (the table above). Annotations are proven first, return types on top of them,
  and named arguments on top of both, so no kind vouches for another. A
  `-> nil` return type is the one edit that changes what the file compiles to;
  use `--only types,named-args` for a run that must leave the IR alone. A
  suggestion
  that fails is dropped and named on stderr, and the rest are written; exit 3,
  with no write, when none passes. Edits are insertions only — comments,
  layout and everything outside the touched arguments are left as they were.

  ```
  $ petal suggest --only named-args --apply app.ptl
  verify: app.ptl: 2 named-argument rewrites proven IR-equal — the same program
  Applied 2 of 2 to app.ptl.
  ```
- `--verify` — run the `--apply` proofs and report, writing nothing. Exit 3 if
  any suggestion fails its proof.
- `--from <file>` — also compile `<file>` for its call sites. A library module
  compiled on its own has no callers, so its parameters have no call-site
  evidence; point this at an app that uses the library. Repeatable. Type
  annotations and return types (which read it for calls to the target's
  functions, by name).
- `--json` — the suggestions as JSON, in source order. Each carries its `kind`
  (`type-annotation`, `named-args`, `return-type` or `advice`), its `because`,
  and its `edits` — a list
  of `{insert_at, insert_text}`, offsets in characters into the original text.
  A type annotation also has `function`, `arity`, `slot`, `param`, `type`,
  `evidence` (and `insert_at` / `insert_text`, its one edit); a named-argument
  suggestion has `callee`, `line`, `column`, `names`, `call` and `rewrite`.

### `apply-change` — Refactors that change behaviour

```
petal apply-change <operation> <file> [<options>]
petal apply-change convert-to-var <file> --target <path> [--from <file>]...
                   [--dry-run]
petal apply-change                      # list the operations
```

Carries out a change the author has decided on, everywhere it has to be made,
across every file it reaches. [`lint --fix`](#lint--rules-with-fixes) and
[`suggest --apply`](#suggest--suggest-safe-refactors-for-a-file) are held to
leaving the program the same; an operation here changes what the program does,
and that is its purpose. What it promises instead:

- **It edits only what resolves to the target.** The target is named on the
  command line, and a mention of the same name that belongs to another binding
  is left alone.
- **It refuses rather than guesses.** A use it has no faithful rewrite for
  stops the whole change, with the lines responsible.
- **Nothing is written unless the result compiles** ([the gate](#the-gate)).

Edits are splices over the original text, so comments and layout survive. A
file that was `petal fmt`-clean before is formatted again afterwards; a file
that was not is left exactly as edited.

Exit status is 0 when the change was written (or, with `--dry-run`, would have
been) and 1 when it was refused or failed. On 1 no file has been touched.

#### `convert-to-var`

Turns a `let` into a `var`, or a `state` into a `state var`: the change from a
dataflow binding to a [mutable cell](syntax/var.md) that a
function can write. It is the fix for "`score` is bound outside this function".

```
$ petal apply-change convert-to-var game.ptl --target /score --dry-run
convert-to-var: /score: `state` -> `state var` (game.ptl line 1)
  game.ptl: 1 write -> set, 2 reads -> get, 1 read left bare
  importers: none considered, the binding is not exported
--- game.ptl
+++ game.ptl (convert-to-var)
@@ -1,6 +1,6 @@
-state score = 0
+state var score = 0
 fn award(n)
-  score = score + n
+  set score = get score + n
 end
-let label = fn() "score: {score}" end
+let label = fn() "score: {get score}" end
 print(score)
dry run: 1 file(s) would change; nothing was written
```

The rewrite, over the mentions that resolve to the target:

| Where | Before | After |
|---|---|---|
| The declaration | `let x = e`, `state x = e` | `var x = e`, `state var x = e` |
| Every `=` write, in any function | `x = e`, `x += e`, `x.f = e`, `x[i] = e` | `set x = e`, `set x += e`, `set x.f = e`, `set x[i] = e` |
| A read inside a nested `fn` or lambda (a default parameter value included) | `x` | `get x` |
| A read in the declaring function | `x` | `x` |

`pub`, a `state(key)` group and a type annotation stay where they are. The root
of a `set` target and the read a compound `set x += 1` implies are not given a
`get`; neither has a place to write one.

**This changes behaviour.** A function that mentions the binding stops reading
the value captured where the function was written and reads the live cell, and
a write from inside a function now lands in the binding everyone else sees.
A program that relied on the snapshot behaves differently afterwards. When no
function mentions the binding, the program does the same thing as before.

**Scoping.** The binding is visible from the statement after its declaration to
the end of the block that declares it, less every stretch where the name is
bound again: by another `let`/`var`/`state`, a parameter, a `for` variable, a
match-pattern binding, or a `fn` of that name. Mentions in those stretches
belong to the other binding and are not touched.

**Refused:**

- an `@x` rebind of the target. `@` desugars to `x = f(x)` and is `let`-only;
  write those calls out as `x = f(x)` first and they are converted to `set`;
- a file that imports the target and writes it (see [importers](#importers));
- a `config let` (a `var` cannot be config), a binding that is already a
  `var`, and a path that names a function.

#### Target paths

`--target` names one binding by the declarations it is written inside,
outermost first, joined by `/`:

```
step/vx                 `vx`, declared in the function `step`
build_view/row/out[2]   the second `out` declared in `row`, inside `build_view`
/score                  the module-level `score`
```

```
path    := ["/"] segment ("/" segment)*
segment := name ["[" n "]"]        n counts from 1
name    := an identifier, or Class.method for a method declaration
```

- **A segment names a declaration:** a named `fn`, or a `let`, `var`, `state`
  or `state var` binding. A parameter, a `for` variable, a match-pattern
  binding and a name first bound by a bare `x = …` are not declarations and
  cannot be named.
- **A declaration encloses what is written inside its text.** A function
  encloses its body and its default parameter values; a binding encloses its
  initializer, so `let row = fn(i) … end` encloses the lambda's body.
- **Control flow and anonymous functions are transparent.** A `let` inside an
  `if`, a loop, or a callback passed straight to a call belongs to the nearest
  declaration around it.
- **A leading `/` starts at module level.** Without one, the path matches any
  binding whose full path *ends* with it: `vx` alone is enough when the file
  has one `vx`, and `step/vx` finds `/physics/step/vx`.
- **`name[n]` is the n-th declaration called `name` directly inside its
  parent,** in source order, counting every kind of declaration, so an index
  does not move when one of them is converted. A segment without an index
  matches all of them.

The path has to match exactly one binding. Otherwise the error lists the full
path of each candidate, with its line and keyword:

```
$ petal apply-change convert-to-var game.ptl --target out
convert-to-var: the target `out` is ambiguous: it matches 2 bindings
name one of them by its full path:
  /build_view/row/out[1]  (line 3, `let`)
  /build_view/row/out[2]  (line 6, `let`)
```

#### Importers

A module-level binding declared `pub` is visible to other files, so the change
follows it into them. An importer may read an exported `var` and
[never write it](module-system.md#an-exported-var-is-read-only-to-importers):

| In the importing file | What happens |
|---|---|
| A bare read inside a function (`import m: x`, or a plain `import m`) | gains `get` |
| A bare read at the file's top level | left alone |
| A qualified read, `m.x` | left alone: it reads the current value wherever it is written |
| A write: `x = …`, `@x`, or `m.x = …` through the module's name | **the whole change is refused** |

Importers are looked for in one of two places. The report says which, and
names every importer it found:

- **`--from <file>`** (repeatable) — the entry file of a program that uses the
  module. Its imports are followed and every file reached is considered. The
  same flag, with the same meaning, as [`suggest --from`](#options).
- **Otherwise, a directory scan** — every `.ptl` file under the target's
  project root: the nearest directory at or above the target that holds a
  [`petal.toml`](#packages--show-the-project-and-the-libraries-available), or the target's own
  directory when there is none. An importer that lives outside that directory
  is not found, and is not rewritten; name its program with `--from`.

Either way each candidate's imports are resolved the way a compile resolves
them, so a file is an importer only if its `import` really lands on the target
file. A file that re-exports the name (`pub import m: x`) passes it on, and
the files that import *it* are importers too.

#### The gate

Before anything is written, every file that would change, and every `--from`
file, is compiled with all the edits in place. Each must compile, or nothing
is written.

One allowance, because the commonest reason to run an operation is a file
that does not compile yet: a file that already failed may keep the compile
errors it had, and gain none. A file with three bindings to convert gets there
one conversion at a time, and each run names it in a `note:`. A file that
failed for another reason (an import that does not resolve here, say) is a
refusal; pass `-I <dir>` so it can be compiled.

#### Options

- `--target <path>` — the binding to change ([target paths](#target-paths)).
  Required by `convert-to-var`.
- `--from <file>` — see [importers](#importers). Repeatable.
- `--dry-run` — print the change as a unified diff on stdout and write
  nothing. Everything else, the gate included, runs as usual.

### `ir-equal` — Are two files the same program?

```
petal ir-equal <a.ptl> <b.ptl>          # exit 0 equal, 1 different, 2 can't tell
petal ir-equal --json <a.ptl> <b.ptl>   # {"equal": bool, "diff"?: {...}}
petal ir-equal --named-args <a.ptl> <b.ptl>
```

Compiles both files and compares their IR, ignoring everything positional:
spans, comments, whitespace, and the numeric ids of terms, blocks and constants
(constants are compared by value). `<a.ptl>` is the original; reported
differences point at its line and column.

The exit code is a three-way answer: **0** equivalent, **1** different (the
first difference is printed), **2** a side failed to compile. Exit 2 means
"can't tell" and must not be read as "not equal". `--json` mirrors this:
`{"equal": true}`, `{"equal": false, "diff": {location, what, left, right,
line, column, position}}` (`position` is `"line:column"` as one string), or
`{"equal": false, "error": "..."}`.

Two things are deliberately treated as semantic, so a difference in them is
reported:

- **Variable names.** Petal keeps binding names in the IR and hashes them into
  `state` keys, so a rename is a difference.
- **Call structure.** A `state` slot is keyed by the call path that reaches
  it, so extracting a helper, inlining one, moving a call to another function,
  or adding an earlier call to the same callee in the same function all change
  which state slots the callees reach. `ir-equal` reports these as different
  even in a program with no `state` — it compares the IR, not reachability.
  Reformatting, renaming a local that is not a callee, and reordering unrelated
  statements stay IR-equal.

- **Argument names.** A call records the name each argument was written with,
  so `f(1, 2)` and `f(x: 1, y: 2)` differ: whether they mean the same thing
  depends on what `f` is. `--named-args` answers that instead of refusing. Two
  calls that differ only in how their arguments are written are accepted when
  the callee is known for certain on both sides — followed back through
  bindings, captures and imports to the `fn`, overload set, lambda or
  constructor it must be — and both calls select the same overload variant and
  put every argument in the same parameter. Arguments are still paired in the
  order written, so a rewrite that reorders them is a difference however they
  are named; and a callee that cannot be pinned down (a parameter, a method
  dispatched at runtime) is a difference as before. This is
  the proof [`suggest --apply`](#suggest--suggest-safe-refactors-for-a-file)
  holds its named-argument rewrites to.

When a refactor is meant to move calls, prove it with a run diff
(`tools/verify.ts`, see [dev/refactor-verification.md](dev/refactor-verification.md))
rather than `ir-equal`.

### `lsp` — Serve the language server

```
petal lsp
```

Speaks the Language Server Protocol over stdio (Content-Length-framed
JSON-RPC). It takes no file and no options: documents arrive over the
protocol. Editors spawn this as a child process.

It provides full-text document sync, diagnostics on open and change, hover,
go-to-definition, completion, and document symbols (the file's outline: its
functions, bindings, classes with their fields, and enums with their
variants). The loop exits on an `exit` notification or at EOF.

Completion, hover and go-to-definition follow imports into other files and
know the members of an enum (`Kind.` offers its variants, a variant value
offers its fields and data), and completion works while the line being typed
does not parse yet. It does not format, rename or offer code actions; use
`petal fmt`, `petal fix-imports` and `petal apply-change` for those.
Columns are UTF-16 code units unless the client offers UTF-8 or UTF-32 alone
(`positionEncoding`). A script written for a host that puts names in scope
without an import is analyzed as that host compiles it when its project's
`petal.toml` says so ([`[host]`](packages.md#host)). The
exact requests, how a document is analyzed and how positions are counted are
in [The language server](internals/lsp.md).

### `packages` — Show the project and the libraries available

```
petal packages [--json] [-I <dir>]... [path]
```

A Petal package is a directory with a `petal.toml` manifest, which names the
package, its `[dependencies]` and the host interfaces it `[requires]`;
[packages.md](packages.md#the-manifest) describes it. This
command prints what a script here can reach:

- **the project** — the nearest `petal.toml` at or above `path` (a file or a
  directory; the working directory when absent) — with its dependency graph
  as it resolves and the host interfaces the project and its dependencies
  require;
- **the search path**: the libraries under each `-I` directory (the
  directory itself, and each directory directly under it) and under the
  `PETAL_PATH` directories;
- **the packages bundled in the binary**, with the builtin ones marked.

```
$ petal packages app
project marble 0.1.0  /Users/me/marble/app
    tween 1.2.0  (path ../libs/tween)
        requires: audio ^1.0
        base  (path ../base)
    shapes 1.2.0  (git https://example.org/shapes tag v1.2.0 @ 9f3c2ab41d)
requires host interfaces:
    audio ^1.0  (package tween)
    ui >=0.1  (package marble)

No packages found on the search path. Point -I at a directory holding a
petal.toml, or at a directory of such libraries.

bundled in this binary:
    core 0.1.0  (builtins)
    …
    bloom 0.1.0  (requires ui >=0.1)
```

A dependency whose key differs from the package's own name is shown as
`key = name`. A package reached twice is expanded once.

`--json` emits `{ "project": …, "packages": [ … ], "bundled": [ … ],
"errors": [ … ] }`. `project` is `null` outside a project; otherwise it has
`name`, `version`, `root`, `module_dir`, `modules`, `locked`, `requires`,
`dependencies` (each with `name`, `source`, and either `package`, `version`,
`root`, `requires`, `repeated`, `dependencies` — plus `commit`, `tree` and
`vendored` for a git package — or an `error`) and
`required_interfaces` (`{interface, version, package}` across the graph).
Each `packages` entry has `name`, `version`, `root`, `module_dir`, `modules`
and `requires`; each `bundled` entry `name`, `version`, `modules`, `builtin`
and `requires`.

A `petal.toml` that will not parse is an error naming the file and the line —
here and in every command that takes `-I`, because a library the user pointed
at should say why it failed rather than go quietly missing:

```
Error: core-runtime/bloom/petal.toml: line 2: the value of 'name' must be a quoted string or an inline table of them ({ path = "…" }), found 'bloom'
```

This command takes no source file.

### `init`, `add`, `remove`, `install`, `update`, `vendor`, `bundle` — The package manager

```
petal init [<dir>] [--name <name>]
petal add <name> (--path <dir> | --git <url> (--tag <tag> | --rev <rev>))
petal remove <name>...
petal install [--locked]
petal update [<name>...]
petal vendor
petal bundle <entry> [-o <file> | --output <file>]
```

These work on the project of the working directory (the nearest `petal.toml`
at or above it). [packages.md](packages.md#the-package-manager) is the
reference for each command, the lockfile, the cache and `vendor/`. In short:

| Command | What it does |
|---|---|
| `init [<dir>] [--name <name>]` | Writes a `petal.toml`. Never overwrites one |
| `add <name> --path <dir>` \| `--git <url> (--tag <tag> \| --rev <rev>)` | Sets a `[dependencies]` entry (`<name>` is the import name) and installs |
| `remove <name>...` | Deletes entries and installs again |
| `install [--locked]` | Resolves the graph, fetches git dependencies, verifies them and writes `petal.lock`. `--locked` fails instead of changing the lockfile |
| `update [<name>...]` | As `install`, with the named packages (or all) resolved from their sources again |
| `vendor` | Installs, and copies each git package into `vendor/<name>` |
| `bundle <entry> [-o <file>]` | Prints the graph as one JSON document for a host with no filesystem ([Bundles](packages.md#bundles)) |

None of them takes `--json`. `check`, `run` and the other compiling commands
[install on demand](packages.md#install-on-demand); `PETAL_OFFLINE=1`
forbids fetching. `petal packages` never fetches.

### `explain` — Show the value chain that produced a term

```
petal explain [--json] --term <name|id> <file.ptl>
petal explain [--json] --term <name|id> -e '<code>'
```

Runs the program with tracing on, then walks backward from the target term
and prints every recorded value along the chain of ancestors. It answers "why
does `x` have this value?". [features/provenance.md](features/provenance.md)
shows how `explain`, [`graph`](#graph--one-query-one-result-shape) and emit
tracing answer different questions about one value.

```
$ petal explain --term total -e 'let a = 2
let b = 3
let total = a * b'
Explain t142 (total):
  Provenance chain:
    => t142 total [line 3, column 13] = 6
     . t140 - [line 3, column 13] = 2
     . t141 - [line 3, column 17] = 3
     . t138 a [line 1, column 9] = 2
     . t139 b [line 2, column 9] = 3
```

`--term` accepts a variable name (`--term total`), a bare term id
(`--term 142`), or the `t`-prefixed form (`--term t142`). A name resolves to the
last term bound to it.

With `--json`, returns
`{name, term_id, chain: [{term_id, op, name, value, line, column, boundary}, ...], complete, truncated}`.

### `pending-report` — Report live pending resources

```
petal pending-report [--json] <file.ptl>
petal pending-report [--json] -e '<code>'
```

Runs the program, then reports every resource value that is still pending or
loading at the end: its state, age in frames, absorption count, and origin
call site. It answers "why is this region blank?"; see
[features/pending-values.md](features/pending-values.md#why-is-this-region-blank)
for reading the answer.

Text output is one line per live resource, or `No pending resources.` when
there are none. With `--json`, it emits the raw report array:
`[{id, key, state, age_frames, origin, absorbed_count}, ...]`.

### `propose-edit` — Propose source edits that change an emitted value

```
petal propose-edit --channel <name> --emit <n> (--arg <k> --to <value>)...
                   [--configurable <var>]... [--static <var>]...
                   [--apply] [--json] <file.ptl>
```

Runs the program with emit tracing, then works backwards: given an emitted
value, it proposes the source edits that would make argument `<k>` of the call
that produced it evaluate to `<value>`. This is the writing half of direct
manipulation: the host says where the user dragged something, and gets back
the edits that mean it. See [features/direct-manipulation.md](features/direct-manipulation.md).

Use `run --trace-emits` first to find the emit's address. For this file:

```petal
import io
let x = 10
let offset = 5
push_output(symbol("shapes"), x + offset)
```

```
$ petal run --trace-emits sketch.ptl

Channel 'shapes' (1 emits):
  [0] push_output [line 3] <- 15
      arg 0: computed
      arg 1: computed

$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 sketch.ptl
2 proposals:
  1. set `x` to 37.5 (line 1)
  2. set `offset` to 32.5 (line 2)
Narrow with --configurable <var> / --static <var>, or apply one by hand.

$ petal propose-edit --channel shapes --emit 0 --arg 1 --to 42.5 --static x --apply sketch.ptl
1 proposal:
  1. set `offset` to 32.5 (line 2)
Applied.
```

Options:

- `--channel <name>` — the output channel the emit was pushed into. Required.
- `--emit <n>` — 0-based index of the emit within that channel. Required.
- `--arg <k> --to <value>` — argument `<k>` (0-based) of the producing call
  should evaluate to `<value>`, written as source text: `55`, `2.5`, `true`,
  `hello`. At least one pair is required. Repeat the pair for a multi-goal
  batch (one gesture changing several arguments); each `--to` binds to the
  `--arg` before it.
- `--configurable <var>` — a variable the host prefers to edit. Repeatable.
- `--static <var>` — a variable that must not be edited. Repeatable.
- `--apply` — rewrite the file in place, but only when every goal resolves to
  exactly one proposal.
- `--json` — emit the proposals as JSON, with exact spans and replacement
  text: one entry per goal under `goals`, plus the flat `proposals` key when
  there is exactly one goal.

Declaring knobs in the source with `config let` narrows proposals without
flags: config bindings are treated as configurable and every other binding as
static.

### Dump format conventions

The four stage dumps — [`show-tokens`](#show-tokens--lexer-token-stream),
[`show-ast`](#show-ast--parsed-ast),
[`show-ir`](#show-ir--compiled-ir-term-graph), and
[`show-bytecode`](#show-bytecode--bytecode-lowering) — share these
conventions, documented once here and referenced from each section.

**Spans.** Lines and columns are 1-based; span ends are exclusive (one past
the last character); byte offsets, where present, are 0-based.

- *Text dumps* print spans as `@line:col-line:col`, collapsed to `@line:col`
  when the end adds nothing (a single-character AST span; IR term lines,
  which print only the start). A location in a non-entry file is prefixed
  with the file's display name: `@std:21:1`.
- *JSON dumps* use compact arrays. Token rows carry a 4-element
  `[startLine, startCol, endLine, endCol]`. The AST and IR dumps share the
  lossless **SourceSpan** encoding
  `[startLine, startCol, startOffset, endLine, endCol, endOffset]`, with a
  seventh element — the `source_map.files` index — appended only when
  nonzero (non-entry file).

**Id prefixes.** In text dumps every id kind gets a prefix; in JSON the same
ids are bare integers (see the schema sections below for which field is
which):

| Prefix | Names | Appears as |
|--------|-------|------------|
| `tN` | TermId — a node in the IR term graph | `t117` |
| `rN` | register index in a frame / function register file | `r0` |
| `cN` | ConstantId — index into the constant table | `c3` |
| `fnN` | FunctionId (`fn0`); the bytecode text abbreviates it to `fN` (`closure f0`) | `fn0`, `f0` |
| `blockN` | BlockId | `block1` |
| `kN` | state key (a `u64`) — bytecode text, decimal; the IR text prints the same key as `key=0x…` hex | `k677…`, `key=0x5dff…` |
| `slotN` | loop-cursor slot (bytecode only) | `slot0` |

A state key names the *declaration*; the slot it selects at runtime is that key
plus the call path that reached it (see
[state-call-paths.md](dev/state-call-paths.md) §2). The path
is a runtime value, so no static dump prints one — what the dumps carry is its
static half: a call term's `call_site` id in `show-ir --json`, and `popN` on a
`state_read`/`state_write` in the bytecode text (the number of enclosing loop
steps the access drops to land on its declaration's slot, omitted when zero). A
live path appears only in host state dumps (as `counter#1/[3]/count`).

**Defaults are omitted in JSON.** Every JSON dump skips a field whose value
is its default — a `null` option, a `false` boolean, an empty
array/string/map — and readers must treat absence as the default. (Token
rows omit `value` on unit tokens; the AST omits `exported`/`is_var`/`ty`/…;
the IR omits `name`/`inputs`/`child_blocks`/… .) The one exception is the
structured `inst` object in the bytecode JSON, whose operands keep explicit
`null`s so each opcode has a fixed field set.

**Contract vs. debug view.** Exactly one dump is a stable interchange
format: the unfiltered IR JSON (`show-ir --json`), which `run --ir` /
`check --ir` load back and foreign front-ends emit (see
[ir-as-target.md](dev/ir-as-target.md)). Everything else — all four text
forms, the tokens/AST/bytecode JSON, and the filtered `--user-only` IR view
— is a debug view: kept accurate here, but versionless and subject to
change, and not loadable by anything.

### `show-tokens` — Lexer token stream

```
petal show-tokens <file.ptl>
petal show-tokens -e '<code>'
petal show-tokens --json <file.ptl>
petal show-tokens --json -e '<code>'
```

Outputs the flat token stream produced by the lexer, with source spans. Useful for debugging tokenization and verifying operator/keyword recognition.

**Text output** (default) — one token per line: index, kind, quoted value
(when the token carries one), and compact span:

```
0: Let @1:1-1:4
1: Ident "x" @1:5-1:6
2: Assign @1:7-1:8
3: Int 1 @1:9-1:10
4: Plus @1:11-1:12
5: Int 2 @1:13-1:14
6: Eof @1:14-1:14
```

**JSON output** (`--json`) — array of uniform rows:

```json
[
  {"kind": "Let", "span": [1, 1, 1, 4]},
  {"kind": "Ident", "value": "x", "span": [1, 5, 1, 6]},
  {"kind": "Assign", "span": [1, 7, 1, 8]},
  {"kind": "Int", "value": 1, "span": [1, 9, 1, 10]},
  {"kind": "Plus", "span": [1, 11, 1, 12]},
  {"kind": "Int", "value": 2, "span": [1, 13, 1, 14]},
  {"kind": "Eof", "span": [1, 14, 1, 14]}
]
```

#### Token JSON Encoding

Every row has the same shape — `{"kind", "value"?, "span"}`:

- `kind` — the token's variant name (see the table below).
- `value` — present only on value-carrying tokens. `Int`/`Float` values are
  JSON numbers; `String`/`Ident` (and `JsxTagName`/`JsxText`/`Color`) values
  are JSON strings. Unit tokens omit the field entirely.
- `span` — `[startLine, startCol, endLine, endCol]` per the shared span
  rules ([Dump format conventions](#dump-format-conventions)); token rows
  are the one 4-element span form — no byte offsets. The text form prints
  the same span as `@startLine:startCol-endLine:endCol`.

Kind names:

| Category | Examples |
|----------|---------|
| Keywords | `"Let"`, `"Var"`, `"Set"`, `"Fn"`, `"If"`, `"Else"`, `"For"`, `"In"`, `"While"`, `"Match"`, `"Return"`, `"Break"`, `"Continue"`, `"State"`, `"Memo"`, `"Enum"`, `"Const"`, `"Get"`, `"Import"`, `"Pub"`, `"Export"`, `"End"`, `"Then"`, `"Do"`, `"Elsif"`, `"When"`, `"True"`, `"False"`, `"Nil"` |
| Operators | `"Plus"`, `"Minus"`, `"Star"`, `"Slash"`, `"Percent"`, `"PlusPlus"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Bang"`, `"Assign"`, `"PlusAssign"` (and `MinusAssign`, `StarAssign`, `SlashAssign`, `PercentAssign`, `ConcatAssign`, `CoalesceAssign`), `"DoubleQuestion"`, `"QuestionDot"`, `"Pipe"`, `"Bar"`, `"At"` |
| Delimiters | `"LParen"`, `"RParen"`, `"LBrace"`, `"RBrace"`, `"LBracket"`, `"RBracket"`, `"Comma"`, `"Dot"`, `"Colon"`, `"Arrow"`, `"DotDot"`, `"DotDotDot"` |
| Special | `"Newline"`, `"Eof"` |
| Value-carrying | `{"kind": "Int", "value": 42, …}`, `{"kind": "Float", "value": 3.14, …}`, `{"kind": "String", "value": "hello", …}`, `{"kind": "Ident", "value": "myVar", …}` |

### `show-ast` — Parsed AST

```
petal show-ast <file.ptl>
petal show-ast -e '<code>'
petal show-ast --json <file.ptl>
petal show-ast --json -e '<code>'
```

Outputs the parsed abstract syntax tree — an array of `Stmt` nodes. Useful for verifying parser behavior and understanding the tree structure before compilation.

**Text output** (default) — a compact tree, one node per line: the node kind
plus its key facts inline (names, operators, literal values), children
indented two spaces, spans as `@line:col-line:col` (end-exclusive, collapsed
to `@line:col` for single-character spans). Patterns and type annotations are
rendered source-like; default facts (`exported: false`, `is_var: false`, an
absent type annotation) are elided, and modifiers show as words after the
kind (`Let var c`, `FnDecl pub f`). For this source:

```petal ignore
fn square(x: number) -> number
  x * x
end

let m = match n
  when 0 -> "zero"
  when k if k > 1 -> "big"
end
```

the dump is:

```
FnDecl square (x: number) -> number @1:1-3:4
  Expr @2:3-2:8
    BinaryOp Mul @2:3-2:8
      Ident x @2:3
      Ident x @2:7
Let m @5:1-8:4
  Match @5:9-8:4
    Ident n @5:15
    Arm 0
      Expr @6:13-6:19
        Literal "zero" @6:13-6:19
    Arm k
      Guard
        BinaryOp Gt @7:13-7:18
          Ident k @7:13
          Literal 1 @7:17
      Expr @7:22-7:27
        Literal "big" @7:22-7:27
```

Structural sub-parts that are not themselves nodes get label lines without
spans: `Then`/`Else` under an `If`, `Arm <pattern>` and `Guard` under a
`Match`, `Key` under an explicit-key `State` and under every `Memo`, `Part`/`Prop`/`Text` inside
string interpolations and elements, `Field`/`Spread` inside records. Both
forms are debug views (see
[Dump format conventions](#dump-format-conventions)); `--json` is the
machine-readable one.

**JSON output** (`--json`) — array of `Stmt` nodes. For `let x = 1 + 2`:

```json
[
  {
    "kind": {
      "Let": {
        "name": "x",
        "value": {
          "kind": {
            "BinaryOp": {
              "op": "Add",
              "left": { "kind": { "Literal": { "Int": 1 } }, "span": [1, 9, 8, 1, 10, 9] },
              "right": { "kind": { "Literal": { "Int": 2 } }, "span": [1, 13, 12, 1, 14, 13] }
            }
          },
          "span": [1, 9, 8, 1, 14, 13]
        }
      }
    },
    "span": [1, 1, 0, 1, 14, 13]
  }
]
```

Note the absent `ty`/`is_var`/`is_config`/`exported` fields on the `Let`:
default-valued fields are omitted (see below).

#### AST JSON Schema

All AST enum types use serde's externally-tagged representation. `Stmt` and
`Expr` are serialized as `{kind: <variant>, span: SourceSpan}` (a `Stmt` also
carries `"exported": true` when declared with the `pub` modifier) —
`SourceSpan`
is the same compact array encoding the IR uses
(`[startLine, startCol, startOffset, endLine, endCol, endOffset, file?]`, see
[Dump format conventions](#dump-format-conventions)), and the `<variant>`
shapes are listed in the `StmtKind` and `ExprKind` tables below.
The canonical definitions live in `core/src/ast.rs`; the tables below cover
the common variants but are not exhaustive.

**Defaults are omitted** (the shared rule — see
[Dump format conventions](#dump-format-conventions)). A field holding its
default value is left out of
the JSON rather than written explicitly: `false` flags (`exported`, `is_var`,
`is_config`), absent options (`ty`, `ret`, `resolved`, `class`, `key`,
`guard`, `else_body`, `alias`, `names`, a list pattern's `rest`), and empty
variant field lists (`EnumVariant.fields`, a `Variant` pattern's `fields`).
Read a missing field as its default. The tables below write these as
`Type (omitted when …)`.

Consumers should treat the single key of the `kind` object as the node type
(serde's externally-tagged enum encoding).

**StmtKind** (top-level statements):

| Variant | Shape |
|---------|-------|
| `Let` | `{"Let": {"name": string, "ty": TypeAnn (omitted when un-annotated), "value": Expr, "is_var": bool (omitted when false), "is_config": bool (omitted when false), "is_const": bool (omitted when false)}}` — `ty` is the optional declared type annotation; `is_var` is true for `var x = …`; `is_config` for `config let`; `is_const` for `const X = …` |
| `Assign` | `{"Assign": {"target": AssignTarget, "value": Expr}}` |
| `Set` | `{"Set": {"target": AssignTarget, "value": Expr}}` — a write to a `var` cell, `set x = …` |
| `Expr` | `{"Expr": Expr}` |
| `FnDecl` | `{"FnDecl": {"name": string, "class": string (omitted for ordinary functions), "params": Param[], "ret": TypeAnn (omitted when un-annotated), "body": Stmt[]}}` — `ret` is the optional declared return-type annotation; `class` is set for a method declaration (`fn Rect.center_x(…)` → `class: "Rect"`, `name: "Rect.center_x"`, receiver as `params[0]`) |
| `EnumDecl` | `{"EnumDecl": {"name": string, "variants": EnumVariant[]}}` |
| `ClassDecl` | `{"ClassDecl": {"name": string, "fields": ClassField[]}}` — a `class Name … end` declaration; fields are in declaration (and constructor-argument) order |
| `For` | `{"For": {"var": string, "iter": Expr, "body": Stmt[]}}` |
| `While` | `{"While": {"condition": Expr, "body": Stmt[]}}` |
| `Return` | `{"Return": Expr \| null}` |
| `Break` | `"Break"` |
| `Continue` | `"Continue"` |
| `State` | `{"State": {"name": string, "ty": TypeAnn (omitted when un-annotated), "init": Expr, "key": Expr (omitted when unkeyed), "is_var": bool (omitted when false)}}` — `ty` is the optional declared type annotation; `key` set when the source uses the `state(expr) name = init` per-iteration form; `is_var` for `state var` |
| `Memo` | `{"Memo": {"name": string, "ty": TypeAnn (omitted when un-annotated), "key": Expr, "init": Expr}}` — `memo(key) name = init`, a content-keyed cache ([memo.md](features/memo.md)). `key` is required, and there is no `var` form |
| `NativeFn` | `{"NativeFn": {"name": string, "class": string (omitted for a plain function), "params": Param[], "rest": string (omitted when there is no `...rest`), "ret": TypeAnn (omitted when un-annotated)}}` — a `native fn` declaration in an interface package |
| `Import` | `{"Import": {"module": string, "alias": string (omitted when there is no `as`), "names": string[] (omitted for a plain import)}}` — `import json: json_parse` is `{"module": "json", "names": ["json_parse"]}` |

**ExprKind** (expressions):

| Variant | Shape |
|---------|-------|
| `Literal` | `{"Literal": Literal}` |
| `Ident` | `{"Ident": string}` |
| `AtVar` | `{"AtVar": string}` — the `@x` of a [rebind](syntax/rebind-operator.md) |
| `CellGet` | `{"CellGet": string}` — `get x`, an explicit read of a `var` |
| `BinaryOp` | `{"BinaryOp": {"op": BinOp, "left": Expr, "right": Expr}}` |
| `UnaryOp` | `{"UnaryOp": {"op": UnaryOp, "operand": Expr}}` |
| `Call` | `{"Call": {"function": Expr, "args": Expr[], "arg_names": (string \| null)[] (omitted when no argument is named)}}` — `arg_names` is parallel to `args`: `f(1, y: 2)` has `[null, "y"]` |
| `If` | `{"If": {"condition": Expr, "then_body": Stmt[], "else_body": ElseBranch (omitted when there is no else)}}` |
| `Match` | `{"Match": {"subject": Expr, "arms": MatchArm[]}}` |
| `List` | `{"List": Expr[]}` |
| `Record` | `{"Record": RecordField[]}` |
| `FieldAccess` | `{"FieldAccess": {"object": Expr, "field": string}}` |
| `IndexAccess` | `{"IndexAccess": {"object": Expr, "index": Expr}}` |
| `For` | `{"For": {"var": string, "iter": Expr, "body": Stmt[]}}` — a `for` in value position, which collects a list |
| `OptionalAccess` | `{"OptionalAccess": Expr}` — wraps the `FieldAccess` or `IndexAccess` written with `?.` |
| `Lambda` | `{"Lambda": {"params": Param[], "body": Stmt[]}}` |
| `StringInterp` | `{"StringInterp": {"parts": string[], "exprs": Expr[]}}` — `parts` has one more element than `exprs` |
| `Element` | `{"Element": {"tag": string, "props": [string, Expr][], "children": JsxChild[]}}` |

**Param**: `{"name": string, "ty": TypeAnn (omitted when un-annotated), "default": Expr (omitted when the parameter has none)}` — a function/lambda parameter with its optional declared type annotation and its optional [default value](syntax/function-overloading.md#default-parameter-values). `default` is the expression as written; it is evaluated in the callee on every call that omits the argument, so it is an `Expr`, never a value. In the text output a defaulted parameter shows as `name = …` in the parameter list, with the expression printed as a `Default name` child node ahead of the body.

**ClassField**: one field of a `ClassDecl`, as `{"name": string, "ty": TypeAnn (omitted when un-annotated)}` — the same optional-annotation shape as a **Param**.

**TypeAnn**: a written type annotation as an object `{"name": string, "resolved": Type (omitted when unrecognized)}` — `name` is the type name exactly as written in the source (`"int"`, `"str"`, `"banana"`), and `resolved` is the recognized static type (omitted for an unrecognized name, e.g. `{"name": "banana"}`). An absent annotation omits the `ty`/`ret` field entirely. Annotations appear on `Let`, `State`, `Memo`, `Param`, `ClassField`, and `FnDecl.ret`; a class name resolves to a static type too, but only against the compilation's class table, so a `TypeAnn` naming one carries no `resolved` in the AST dump; they are type-checked (warnings only — see [`check`](#check--compile-without-running)) and dropped before codegen, so they never appear in the IR.

**Type**: a string naming the recognized static type — one of `"Any"`, `"Nil"`, `"Bool"`, `"Int"`, `"Float"`, `"Num"`, `"String"`, `"List"`, `"Record"`, `"Function"`, `"Enum"`, `"Vec2"`, `"Vec3"`, `"F64Array"`, `"Element"`, `"Symbol"`, `"Dual"`, `"Handle"`, `"Pending"`. Appears as the `resolved` field of a **TypeAnn**.

**RecordField**: `{"Named": [string, Expr]}` or `{"Spread": Expr}`.

**JsxChild**: `{"Text": string}` or `{"Expr": Expr}`.

**Literal**: `"Nil"`, `{"Bool": bool}`, `{"Int": number}`, `{"Float": number}`, `{"String": string}`

**BinOp**: `"Add"`, `"Sub"`, `"Mul"`, `"Div"`, `"Mod"`, `"Eq"`, `"Ne"`, `"Lt"`, `"Le"`, `"Gt"`, `"Ge"`, `"And"`, `"Or"`, `"Coalesce"`, `"Concat"`

**UnaryOp**: `"Neg"`, `"Not"`

**AssignTarget**: `{"Name": string}`, `{"Field": [Expr, string]}`, `{"Index": [Expr, Expr]}`

**ElseBranch**: `{"Block": Stmt[]}`, `{"ElseIf": Expr}`

**MatchArm**: `{"pattern": Pattern, "guard": Expr (omitted when unguarded), "body": Stmt[]}` — `body` is a statement list, like an `if` branch; its last expression is the arm's value

**Pattern**: `"Wildcard"`, `{"Literal": Literal}`, `{"Variable": string}`, `{"Variant": {"name": string, "fields": Pattern[] (omitted when empty)}}`, `{"List": {"elements": Pattern[], "rest": string (omitted when there is no `...rest`)}}`, `{"Record": [string, Pattern][]}`, `{"Or": Pattern[]}` — the same encoding the IR JSON's `match_arms` uses (see [ir-as-target.md](dev/ir-as-target.md))

**EnumVariant**: `{"name": string, "fields": string[] (omitted when empty), "data": {"key": string, "value": Expr}[] (omitted when empty)}` — `fields` are the payload field names, `data` the `key: expr` constant-data entries

### `show-ir` — Compiled IR (term graph)

```
petal show-ir [--json] [--all] [--user-only] <file.ptl>
petal show-ir [--json] [--all] [--user-only] -e '<code>'
```

Outputs the compiled intermediate representation: the term graph the compiler produces, which is then lowered to the bytecode the VM runs ([`show-bytecode`](#show-bytecode--bytecode-lowering)). This is the primary command for GUI and tooling integration.

By default the **text output hides compiler noise**: builtin "phantom" terms
(one per registered native function, see below) and everything that belongs
to a non-entry file — the auto-loaded `std` prelude and imported modules.
Ids are never renumbered by hiding; the lines are simply omitted. Pass
`--all` to include everything.

**Text output** (default). For this program:

```petal
fn double(x)
  x * 2
end
let msg = match double(4)
  when 0 -> "zero"
  when n if n > 5 -> "big"
  when _ -> "small"
end
print(msg)
```

`show-ir` prints:

```
=== Constants ===
  c0: 2
  c1: 4
  c2: "zero"
  c3: 5
  c4: "big"
  c5: "small"
  c6: "print"

=== Functions ===
  fn0: double params=["x"] body=block1 captures=[]

=== Blocks ===
block0 [root] regs=146
  t144 r139 = MakeClosure(fn0) [] ; double @1:1
  t145 r140 = Copy [t144] @4:17
  t146 r141 = Constant(4) [] @4:24
  t147 r142 = Call [t145, t146] @4:17
  t156 r143 = Match [t147] -> block2, block3, block5 ; msg @4:11
    arm0: when 0 -> block2
    arm1: when n if block4 -> block3
    arm2: when _ -> block5
  t157 r144 = Copy [t156] @9:7
  t158 r145 = BuiltinCall("print") [t157] @9:1

block2 (parent: t156) regs=1
  t148 r0 = Constant("zero") [] @5:13

block4 (guard for t156 arm1) regs=4
  binds: n=t149:r0
  t150 r1 = Copy [t149] @6:13
  t151 r2 = Constant(5) [] @6:17
  t152 r3 = Gt [t150, t151] @6:13

...

block1 (body of fn0 double) regs=5
  params: x=t139:r0  self: double=t140:r1
  t141 r2 = Copy [t139] @2:3
  t142 r3 = Constant(2) [] @2:7
  t143 r4 = Mul [t141, t142] @2:3
```

Each term line: `t{id} r{register} = {op} [{inputs}] -> {child_blocks} ; {name} @{line}:{col}`
(id prefixes and span form per
[Dump format conventions](#dump-format-conventions)).

The text form is designed to be read without cross-referencing:

- **Constants are resolved inline** in ops, following the bytecode
  disassembler's convention: `Constant(1)`, `Constant("zero")`,
  `BuiltinCall("print")`, `Error("message")`, `GetField(.x)` /
  `GetFieldOpt(.x)` / `SetField(.x)`, `MethodCall(.dist2)` (plus
  `, hint=Rect` when a class hint is present), `MakeEnumVariant(Shape.circle)`,
  `AllocMap{x, y}` (plus ` class=Rect` for class constructors), and
  `AllocElement(div, props=[width])`.
- **Match arms** print under their `Match` term, one line per arm, with the
  pattern rendered source-like (`0`, `n`, `_`, `[a, ...rest]`, `{x: xx}`,
  `Shape.circle(r)`, `"a" | "b"`), the guard block if any (`if block4`), and
  the body block. Guard blocks are labeled in their own header:
  `(guard for t156 arm1)`.
- **Enums** get a section of their own between Constants and Functions when
  the program declares any, one line per enum: each variant with its payload
  fields in parentheses and its data keys in braces
  (`Op: fight(place, won) {tone} | rest {tone}`).
- **Blocks are listed as a tree**: the root block first, then its descendant
  child blocks depth-first in term order (a match arm's guard block just
  before its body); each function's body-block tree follows as its own
  top-level section, labeled `(body of fn0 double)`.
- **Block headers name their bindings** — params, captures, the self
  reference, and match-pattern variables — with term id and register
  (`params: x=t139:r0  self: double=t140:r1`, `binds: n=t149:r0`). A function
  with [default parameter values](syntax/function-overloading.md#default-parameter-values)
  adds a `given:` segment — one hidden *was-it-passed* flag per optional
  parameter (`given: b#given=t128:r2`), which the body's first terms branch on
  to decide whether to evaluate the default — and its line in the Functions
  section gains `optional=N`. These
  binding terms are hidden phantoms, so the header is what ties `Copy [t139]`
  to the `x` parameter. A reference to any *other* hidden term is annotated
  inline with its name: `Copy [t162(std::sum)]`.
- **State ops show their identity**: `StateInit(count, key=0x5dff…)` /
  `StateRead(...)` / `StateWrite(...)` carry the state's name and key, so a
  write visibly links to its init. The key identifies the *declaration*; which
  slot it lands in also depends on the runtime call path, which no static dump
  can show (see [Dump format conventions](#dump-format-conventions)).
  A `memo(key)` declaration prints as `StateInit memo(name, key=…)`, and its
  bytecode as `state_init memo` / `state_write init memo`.
- **Phi carry-outs** print as a block footer: `phi-out: t126 -> t120 (x)`
  means "when this block's frame pops, copy `t126`'s value into `t120`'s
  register" (the `Phi` term for `x` in the parent block; see `Block.phi_outs`).
- Every term line with a source span ends with a compact `@line:col`
  location (prefixed with the file's display name for non-entry files, e.g.
  `@std:21:1`).

**JSON output** (`--json`) — the full `Program` object, always complete
(phantoms and prelude included): this is the interchange format that
`run --ir` / `check --ir` load, so it is never filtered by default.

```json
{
  "schema": "0.2",
  "id": 0,
  "source": "...",
  "terms": [...],
  "blocks": [...],
  "root_block": 0,
  "constants": {"values": [...]},
  "source_map": {"term_spans": {...}},
  "functions": [...],
  "match_arms": {...},
  "class_names": ["Rect"]
}
```

**Filtered JSON view** (`--json --user-only`) — the same Program JSON shape
with the noise filtered out of the arrays: builtin phantom terms, prelude /
imported-module terms, blocks, and functions, and constants referenced only
by them. Ids are preserved as-is (nothing is renumbered), which is why
`constants.values` becomes an **id-keyed object** here
(`{"0": {"Int": 2}, "6": {"String": "print"}}`) instead of an array;
`source_map.term_spans` and `match_arms` are filtered to the kept terms. The
param/capture/self binding phantoms of user blocks are kept so no kept term
references a missing id. This is a debugging **view**, not an interchange
format — it is **not loadable** by `run --ir`.

#### Program JSON Schema

The IR JSON is the complete compiled `Program` struct — **schema 0.2** (see
[docs/dev/ir-as-target.md](dev/ir-as-target.md) for the full emit-target
contract and the legacy-v0 tolerance rules). All ID newtypes serialize as
their inner integer (e.g. `TermId(5)` becomes `5`). **Defaults are omitted on
the wire**: any field whose value is its default — a `null` option, an empty
array/string/map, a `false` boolean — is simply absent, and loaders treat
absence as the default (the shared rule — see
[Dump format conventions](#dump-format-conventions)).

**Top-level Program**:

| Field | Type | Description |
|-------|------|-------------|
| `schema` | `string` | Wire-format version, `"0.2"`. The loader also accepts documents with no `schema` field (legacy v0 shapes) |
| `id` | `number` | Program ID (always 0 for CLI) |
| `source` | `string` | Original source code (omitted when empty) |
| `terms` | `Term[]` | All terms in the program |
| `blocks` | `Block[]` | All blocks in the program |
| `root_block` | `number` | BlockId of the root/entry block |
| `constants` | `{"values": ConstantValue[]}` | Constant table |
| `source_map` | `{"term_spans": {}, "files": []}` | `term_spans`: TermId → SourceSpan mapping (string keys). `files`: file table for multi-file programs — entry file at index 0, imported modules after (omitted for single-file programs). The whole `source_map` is omitted when empty |
| `has_errors` | `boolean` | Whether the program has parse errors (omitted when `false`) |
| `functions` | `FunctionDef[]` | All function definitions (omitted when empty) |
| `match_arms` | `{[termId: string]: MatchArmMeta[]}` | Match term → arm metadata (string keys); omitted when empty |
| `enums` | `{[name: string]: {"variants": {"name": string, "fields": string[], "data": string[]}[]}}` | Every enum the program declares: each variant's payload field names and constant-data keys (both omitted when empty). Where a field read on a variant finds its payload index. Omitted when empty |
| `class_names` | `string[]` | Sorted names of every class the program declares, built-ins included (the prelude's `Rect` makes it non-empty for any CLI compile). The runtime's answer to "is this value's class label a live class here?" — it gates `MethodCall` hint dispatch. Omitted when empty |

**Term**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique term ID |
| `op` | `TermOp` | The operation (see TermOp table below) |
| `inputs` | `number[]` | TermIds of input dataflow edges (omitted when empty) |
| `block_id` | `number` | BlockId this term belongs to |
| `name` | `string` | Variable name if this is a binding (omitted when none) |
| `register` | `number` | Register index for evaluation. Optional for imports — when any term omits it, the loader recomputes the whole assignment |
| `state_key` | `number` | State key for StateInit/StateRead/StateWrite (omitted otherwise) |
| `child_blocks` | `number[]` | BlockIds of child blocks, for control flow (omitted when empty) |
| `path_pop` | `number` | Omitted when `0`. On a `StateRead`/`StateWrite`: how many enclosing loop steps the access drops from the live call path to address the slot its declaration owns (a `state` declared outside a loop and reassigned inside one). Always `0` on a `StateInit`, which *is* the declaration. |
| `call_site` | `number` | Omitted when absent. On a `Call`/`MethodCall`/`BuiltinCall`: the callsite id (a `u64`) the callee's frame pushes onto its call path, which is what gives each callsite of a function its own `state` slots. Derived from the callee's spelling, its ordinal among identically-spelled callees in the enclosing function, and that function's module/name chain — never from term ids or spans, so it survives a hot reload. Hand-written IR may omit it; a call with no id contributes a shared id of `0`, i.e. one slot per declaration as before call-path keying. |
| `collect` | `boolean` | Omitted when `false`. On a `ForLoop`/`NumericForLoop` term, marks a value-position loop (`x = for …`) that collects each iteration's body result into a list. |
| `is_memo` | `boolean` | Omitted when `false`. On a `StateInit`: the declaration is a `memo(key)`, not a `state`. The slot is addressed by the key value alone, and it is a cache: never named by state dumps, `--state-storage` or a reload. Valid only on a `StateInit` with exactly one input (the key), and no `StateRead`/`StateWrite` may name its `state_key`. |

Execution order within a block is the block's ordered `terms` array (below).
The in-memory `entry`/`block_next`/`block_prev` linked list is not serialized;
legacy documents that carry it (and no block `terms` arrays) still load.

**TermOp** — serde's externally-tagged encoding:

| Op | JSON | Inputs | Child Blocks | Notes |
|----|------|--------|-------------|-------|
| Constant | `{"Constant": cid}` | none | none | Load constant by ConstantId |
| Error | `{"Error": cid}` | none | none | Parse error |
| Add | `"Add"` | [left, right] | none | |
| Sub | `"Sub"` | [left, right] | none | |
| Mul | `"Mul"` | [left, right] | none | |
| Div | `"Div"` | [left, right] | none | |
| Mod | `"Mod"` | [left, right] | none | |
| Neg | `"Neg"` | [operand] | none | Unary minus |
| Eq | `"Eq"` | [left, right] | none | |
| Ne | `"Ne"` | [left, right] | none | |
| Lt | `"Lt"` | [left, right] | none | |
| Le | `"Le"` | [left, right] | none | |
| Gt | `"Gt"` | [left, right] | none | |
| Ge | `"Ge"` | [left, right] | none | |
| Not | `"Not"` | [operand] | none | Logical not |
| And | `"And"` | [left] | [rhs_block] | Short-circuit; rhs_block evaluates right operand |
| Or | `"Or"` | [left] | [rhs_block] | Short-circuit; rhs_block evaluates right operand |
| Coalesce | `"Coalesce"` | [left] | [rhs_block] | `??` — yields the RHS when the left is Nil/Pending |
| Concat | `"Concat"` | [left, right] | none | String concatenation (`++`) |
| Copy | `"Copy"` | [source] or [] | none | Variable reference. No inputs + a `name` = binding phantom (builtin/param/capture/self) — never listed in a block's `terms` array |
| Phi | `"Phi"` | [init] | none | Pure-dataflow join for names rebound inside child blocks. Sits in the parent block before the control-flow term; child frames overwrite via `Block.phi_outs`. |
| Branch | `"Branch"` | [condition] | [then_block, else_block] | if/else |
| ForLoop | `"ForLoop"` | [iterable] | [body_block] | for-in loop. Yields a list of each iteration's body result when `collect` is set (value-position `x = for …`). |
| NumericForLoop | `"NumericForLoop"` | [start, end] | [body_block] | non-allocating `for x in range(a, b)` integer loop. Collects like `ForLoop` when `collect` is set. |
| WhileLoop | `"WhileLoop"` | none | [cond_block, body_block] | while loop |
| Break | `"Break"` | none | none | |
| Continue | `"Continue"` | none | none | |
| Return | `"Return"` | [value] or [] | none | |
| MakeClosure | `{"MakeClosure": fid}` | [captured_values...] | none | Create closure for FunctionId |
| MakeOverloadSet | `"MakeOverloadSet"` | [closure0, closure1, ...] | none | Bundle arity-overloaded closures. See [syntax/function-overloading.md](syntax/function-overloading.md). |
| Call | `"Call"` | [callable, arg0, arg1, ...] | none | |
| MethodCall | `{"MethodCall": {"name": cid, "hint": cid?}}` | [object, arg0, arg1, ...] | none | Method name as ConstantId; tries record field first, then the receiver's class, then a builtin with `object` prepended. `hint` (omitted when absent) names a class for live-edit dispatch. |
| BuiltinCall | `{"BuiltinCall": cid}` | [arg0, arg1, ...] | none | Direct builtin call; `cid` is a String constant holding the builtin's name, resolved by name at lower time. |
| StateInit | `"StateInit"` | [] or [explicit_key] | [init_block] | `state_key` set. Init expression lives in `child_blocks[0]` for lazy evaluation — only entered when the runtime key isn't yet in the persistent state map. Optional `explicit_key` is the value computed for `state(expr) name`. With `is_memo` set it is a `memo(expr) name` declaration and the key is required. |
| StateRead | `"StateRead"` | none | none | `state_key` set |
| StateWrite | `"StateWrite"` | [value] or [value, explicit_key] | none | `state_key` set. Forwards the same `explicit_key` from the matching `StateInit` so the runtime resolves to the same `RuntimeStateKey`. |
| CellNew | `"CellNew"` | [init] | none | Allocate the cell behind a `var`. The one op that produces a `Value::Cell`. |
| CellRead | `"CellRead"` | [cell] | none | Dereference a cell — every source-level read of a `var`. |
| CellWrite | `"CellWrite"` | [cell, value] | none | Write through a cell (`set x = …`). Yields the written value. |
| AllocList | `"AllocList"` | [elem0, elem1, ...] | none | |
| AllocMap | `{"AllocMap": {"fields": [cid, ...], "class": cid?}}` | [val0, val1, ...] | none | Field names as ConstantIds. `class` is present only for a class constructor's allocation, naming the class the record is tagged with (see [language-guide.md](language-guide.md#classes--methods)); it is omitted for a plain record literal. |
| AllocMapSpread | `{"AllocMapSpread": {"entries": [...]}}` | [spread_src..., named_value...] | none | Record literal with `...spread`. Each entry is `Spread(idx)` or `Named{key, idx}` referencing positions in `inputs`. |
| GetField | `{"GetField": cid}` | [object] | none | |
| GetFieldOpt | `{"GetFieldOpt": cid}` | [object] | none | Tolerant read: missing field / Nil object yields Nil (left side of `??`) |
| SetField | `{"SetField": cid}` | [object, value] | none | |
| GetIndex | `"GetIndex"` | [object, index] | none | |
| GetIndexOpt | `"GetIndexOpt"` | [object, index] | none | Tolerant read: missing key / Nil object yields Nil |
| SetIndex | `"SetIndex"` | [object, index, value] | none | |
| AllocElement | `{"AllocElement": {"tag": cid, "prop_keys": [cid, ...]}}` | [prop_val0, ..., child0, ...] | none | JSX-like element. `prop_keys.len()` separates prop values from children in `inputs`. |
| MakeEnumVariant | `{"MakeEnumVariant": cid}` | [field_values...] | none | The variant's tag (`Enum.variant`) as ConstantId; one input per payload field the variant declares in `enums` |
| Match | `"Match"` | [subject] | [arm_body_blocks...] | Arm metadata in `match_arms` |

**Block**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | Unique block ID |
| `parent_term_id` | `number` | TermId that created this block (omitted for root and function bodies) |
| `terms` | `number[]` | The block's TermIds in execution order (omitted when empty). Binding phantoms are not listed. |
| `param_names` | `string[]` | Parameter names (function params, for-loop variable); omitted when empty. These are the block's first registers, in order. For the body of a function with default parameter values the list continues past the parameters with one `name#given` flag per optional parameter (see `FunctionDef.optional_params`) |
| `register_count` | `number` | Total registers needed for this block's frame. Optional for imports — recomputed/filled by the loader |
| `phi_outs` | `PhiOut[]` | Carry-outs: when this block's frame pops, copy each `src_term`'s register into the parent block's `Phi` term register. Drives the rebinding-as-pure-dataflow model. Omitted when empty. |

**FunctionDef**:

| Field | Type | Description |
|-------|------|-------------|
| `id` | `number` | FunctionId |
| `name` | `string` | Function name (omitted for lambdas) |
| `params` | `string[]` | Parameter names (omitted when empty) |
| `optional_params` | `number` | How many of `params` — always the trailing ones — have a default value and may be omitted by a call (omitted when 0). The default *expressions* are not stored here: they are ordinary terms at the head of the body block. Each optional parameter owns a was-it-passed flag, a binding phantom named `name#given` seated in the body block's registers straight after the parameters (so the body block's `param_names` has `params.length + optional_params` entries). A call writes `true`/`false` into each flag, and a placeholder `nil` into the register of a parameter it omitted; the body opens with `name = if name#given then name else <default> end` for each one, in order |
| `body_block` | `number` | BlockId of the function body |
| `capture_names` | `string[]` | Names of captured variables (omitted when empty) |
| `capture_registers` | `number[]` | Which body registers receive captured values (parallel to `capture_names`). Optional for imports — re-derived from the body block's binding phantoms when registers are omitted |
| `self_ref_register` | `number` | Body register for self-reference (enables recursion). Optional for imports, like `capture_registers` |
| `register_count` | `number` | Total registers for function frame. Optional for imports |

**MatchArmMeta**:

| Field | Type | Description |
|-------|------|-------------|
| `pattern` | `Pattern` | The pattern to match (same AST Pattern type) |
| `guard_block` | `number` | BlockId for guard expression (omitted when none) |
| `body_block` | `number` | BlockId for the arm body |

**ConstantValue**: `"Nil"`, `{"Bool": true}`, `{"Int": 42}`, `{"Float": 12345678901234}` (u64 bits), `{"String": "hello"}`

**SourceSpan** — the compact lossless array shared by the IR `source_map`
and the AST JSON, defined in
[Dump format conventions](#dump-format-conventions):
`[startLine, startCol, startOffset, endLine, endCol, endOffset, file?]`. On
input, the loader also accepts the legacy object form
`{"start": {"line", "column", "offset"}, "end": {…}, "file"?}`.

### `show-bytecode` — Bytecode lowering

```
petal show-bytecode [--json] <file.ptl>
petal show-bytecode [--json] -e '<code>'
```

Displays the bytecode the VM executes: the compiled IR lowered — through the
same optimization pipeline a `run` uses — to linear, register-based
instructions. One section per function; the program's top level is the
implicit root function. See [Architecture.md](dev/Architecture.md) for how
the compiler and the VM divide the work.

For `let xs = map([1, 2, 3], fn(x) x * 2 end)`:

```
fn <root>  (144 regs, 0 loop slots)
     0  r138 = const 1
     1  r139 = const 2
     2  r140 = const 3
     3  r141 = list [r138, r139, r140]
     4  r142 = closure f0 caps=[]
     5  r143 = builtin "map" [r141, r142]

fn f0  (4 regs, 0 loop slots)
  params:   r0
     0  r2 = const 2
     1  r3 = r0 * r2
```

Text-form conventions (see
[Dump format conventions](#dump-format-conventions) for the shared prefixes):
`rN` is a register in the function's flat register file, `fN` a FunctionId,
`slotN` a loop-cursor slot, `kN` a state key.
Constant-table operands are resolved inline (`const 1`, `builtin "map"`).
Jump targets (`jump -> 5`) are instruction indexes within the same function —
the left-hand column.

Like `run`, `show-bytecode` shows what a script that runs once executes. A
host that runs a script every frame also keeps its static values between
runs (the `fast` policy's static-value cache), which adds a `static_skip`
in front of each run of static instructions and a `static_store` behind it.
Name the policy to see them: `PETAL_POLICY=fast petal show-bytecode f.ptl`.

#### Bytecode JSON Encoding

`--json` emits `{"functions": [Function]}`, root function first.

**Function**:

| Field | Type | Description |
|-------|------|-------------|
| `fn` | `number \| null` | FunctionId (null for the implicit root function) |
| `name` | `string \| null` | Function name (null for the root and for lambdas) |
| `reg_count` | `number` | Size of the function's flat register file |
| `loop_slots` | `number` | Number of loop-cursor slots the function needs |
| `param_regs` | `number[]` | Registers that receive the parameters, in order, followed by the was-it-passed flag register of each parameter that has a default value |
| `optional_params` | `number` | How many trailing parameters have a default value — the last `optional_params` entries of `param_regs` are their flags (`0` for most functions). The text form prints those registers on a separate `given:` line |
| `capture_regs` | `number[]` | Registers that receive captured values, in capture order |
| `self_ref_reg` | `number \| null` | Register holding the self-reference (recursion), if any |
| `code` | `InstRow[]` | The instruction stream |

**InstRow** — each instruction appears twice, structured for tooling and
rendered for reading:

| Field | Type | Description |
|-------|------|-------------|
| `ip` | `number` | Instruction index within this function (what jump targets refer to) |
| `inst` | `object` | Structured instruction: `{"<Opcode>": {operands}}` |
| `text` | `string` | The disassembled text form of the same instruction |

`inst` is the externally tagged encoding of the instruction enum — one key,
the opcode name, mapping to an object of named operands (the same convention
as a term's `op` in the [IR JSON](#program-json-schema)):

```json
{
  "ip": 5,
  "inst": { "BuiltinCall": { "dst": 143, "name": 3, "args": [141, 142], "in_place": false } },
  "text": "r143 = builtin \"map\" [r141, r142]"
}
```

Operand encoding:

- Registers are plain numbers (`dst`, `a`, `b`, `src`, `args`, …).
- Constant-table operands (`k`, `name`, `field`, `tag`, `class`, `msg`, …)
  are ConstantId indexes into the program's constant table — the same table
  `show-ir --json` emits as `constants`. The `text` form resolves them inline.
- Jump-target operands (`to`, `exit`, `next`, `after`) are `ip` indexes
  within the same function.
- Function operands (`func`) are FunctionIds; state operands (`base`) are
  state keys.
- Optional operands are present as `null` rather than omitted, so each opcode
  has a fixed field set — the one exception to the shared
  [omit-defaults rule](#dump-format-conventions).

The full instruction set is documented in `core/src/backend/bytecode/isa.rs`.

### `show-static` — Static declarations

```
petal show-static [--json] [--all] <file.ptl>
petal show-static [--json] [--all] -e '<code>'
```

Reports, for every top-level declaration, whether its value is **static**:
whether its initializer can only ever produce one value for this source text.
It compiles the program and never runs it. This is the classification the
static-value cache works from: a host that runs a script every frame (the
`fast` and `replay` [policies](#run--execute-a-program)) builds a static
value on the first run and keeps it for the next ones, and this command shows
what is kept and what is not. See
[Static values](features/static-values.md) for what is static and how it is
kept, and
[features/hot-reload.md](features/hot-reload.md#static-values-across-a-reload)
for what a reload does to the values it holds.

```petal
config let SCALE = 2
let W = SCALE * 3
fn hd(a, b)
  {a: a, b: b, w: W}
end
let ROWS = [hd(1, 2), hd(3, 4)]
state counter = 0
let next = counter + 1
let by_sec = [0, 0, 0]
by_sec[1] = 5
print(ROWS, next)
```

```
static    config let SCALE  line 1    read only by static code (W)
static    let W             line 2    read by dynamic code (captured by hd)
function  fn hd             line 3    static-pure
static    let ROWS          line 6    read by dynamic code (line 11)
dynamic   state counter     line 7    its initializer is static
dynamic   let next          line 8
            = next reads counter at line 8
            = counter is state, which changes between runs at line 7
dynamic   let by_sec        line 9
            = by_sec is assigned again (a value that is mutated is never kept) at line 10

7 declarations: 3 static (2 read by dynamic code), 1 function (1 static-pure), 3 dynamic
```

Each declaration is one of three classes.

- **static** — data that depends on nothing dynamic. A literal, a list or
  record of static parts, an operator on static operands, another static
  declaration (a `config let` included, when its own initializer is static
  and it is never reassigned), a call of a constant builtin, or a call of a
  static-pure function with static arguments. The line says who reads it:
  *dynamic code* (an operand of a dynamic term, a closure capture, or a
  free-standing top-level statement such as an `assert`, all of which run on
  every run), only other static declarations, or nothing. Only a static
  declaration that dynamic code reads is kept; one that only feeds
  other static declarations is built and dropped.
- **function** — a function value. It is **static-pure** when its body reads
  no `state`, no `var` but its own, and no dynamic native, prints nothing, and
  calls only static-pure functions. A call of a static-pure function with
  static arguments is static.
- **dynamic** — everything else, including anything the classifier cannot
  resolve. The lines under it are the chain of reasons, one link per
  declaration or function crossed, down to the term that makes it dynamic.
  A `state` or a `var` is dynamic by being one and gets no chain; a `state`
  whose initializer is static says so, since it may be a constant that was
  parked in `state` to avoid rebuilding it.

The rules that most often decide an answer:

| Rule | Example |
|---|---|
| A loop, a branch or a `match` is static only as a whole: its inputs and every term inside it. | `let D = if N > 2 then live else 0 end` is dynamic whichever way `N` points. |
| A name that is assigned again is dynamic at every definition. | `xs[i] = v`, `xs = append(xs, v)`, a rebind inside a top-level `if` or loop. |
| A value that would hold a function is dynamic. | `let HANDLERS = {open: open_file}` |
| A function argument is static only for `map`, `filter`, `reduce`, `sort` and `sort_by`, and only if the function is static-pure. | `apply(double, 2)` is dynamic; `map(XS, double)` is not. |
| A method call is static only if every method of that name is, and every builtin the name can fall back to. | One `fn Square.area` that reads `state` makes every `.area()` dynamic. |
| A field read whose name is a constant-data key of some enum is static only if that enum's data is. | `enum Col name(w: text_width("name")) end` makes every `.w` dynamic. |
| A top-level `var` or `state`, and anything that reads one, is dynamic. | |
| One arity of an overloaded function that is not static-pure makes the whole name not static-pure. | |

A declaration written [`const`](language-guide.md#const--a-value-that-must-be-constant)
is listed with that keyword, and is expected to be static. When it is not, a
`!` line under it says which of two things is the case: `declared const but
not constant: a compile error` (the reason chain that follows is what the
error prints), or `declared const: cannot be verified without its host's
natives` when the only thing in the way is a native this command does not
register. In `--json` the same answer is the declaration's `const_check`:
`constant`, `not_constant` or `unverified`.

A static declaration can also carry a `! not kept` line: its value is handed
to something that may update a container in place (`let MORE = append(BASE,
4)`, an indexed write through another name). The value is still static and so
is what is built from it, but the declaration itself would not be kept.

**Host natives.** A native is static only if its host registered it as a
constant function ([usage/ffi.md](usage/ffi.md#constant-natives)). This command knows the
natives the `petal` CLI registers and no host's, so a `native fn` of a host
(`screen_width`, `text_width`, a game engine's) is dynamic here and its line
reads `a native that is not registered here`. A host asking the same question
of its own `Env` (`Env::static_report`) may report more as static than this
command does, never less.

**The classification never reads a literal's value.** It reads the shape of
the program and the names in it, so editing a literal cannot change any
answer here, including a literal that decides a branch.

#### Checking the answer: `--static-audit`

```
petal run --static-audit <file.ptl>
petal run --static-only <file.ptl>
```

`show-static` is an analysis; `petal run --static-audit` holds it against a
run. It runs the script as usual, and while any top-level term the classifier
called static is being evaluated (however deep the calls it makes) it watches
everything a static value may not depend on:

- a host binding read (`mouse_x`, `time`, the screen size, a font), a
  host-data read, a resource read, output, any effect;
- the random stream;
- `state` being declared, read or written;
- a call of a native that is not registered as a constant function.

Each one touched is reported on stderr with the declaration whose initializer
was running, the input, and the call that touched it, and the command exits
with status 1:

```text
static audit: 14 static declarations, 310 native calls under static terms, 1 failure
  LAB_W (app.ptl:784) read the binding `font`: text_width at app.ptl:382
```

A failure is a bug in the classifier or, far more often, a native registered
`CONST` that is not one ([usage/ffi.md](usage/ffi.md#constant-natives)). A clean run is
evidence for the calls the run made, not a proof.

`--static-only` evaluates **only** the top-level statements that are static
and skips every other one, under the same audit. Static code may call only
constant natives, so the script does not need its host: this builds the
constant tables of a game or a panel from the plain CLI, prints nothing the
script would have printed, and fails if the build raises. `--observe` shows
the values it built.

What the audit cannot see: a read of a `var`, or of a value some other
statement updates in place, moves no counter. And a call the memo replays is
never made, so audit with memoization off (the `petal run` default).

Under a UI host the same flags are on `petal-ui-run`
([headless-ui-run.md](dev/headless-ui-run.md)), which audits every frame of a
scenario; `core-libs/petal-ui/tests/static_audit.rs` does that for the whole
panel corpus.

`PETAL_STATIC_AUDIT=watch` in the environment starts every `Env` of the
process with the audit on and prints each finding on stderr as
`static audit failure: …` when the run that made it ends. That is how a whole
test suite is audited without touching a test: `PETAL_STATIC_AUDIT=watch
cargo test -- --nocapture`, then search the output.

`--all` includes the declarations of imported modules and of the prelude,
under their module-qualified names (`ui::button`). `--json` emits:

```json
{
  "summary": {"declarations": 7, "static": 3, "static_read_by_dynamic": 2,
              "static_mutated": 0, "functions": 1, "functions_static_pure": 1,
              "dynamic": 3},
  "declarations": [
    {"name": "ROWS", "qualified": "ROWS", "kind": "let", "line": 6, "column": 12,
     "class": "static", "read_by_dynamic": true,
     "readers": [{"kind": "dynamic", "line": 11}],
     "direct_depends_on": ["hd"], "depends_on": ["SCALE", "W", "hd"]},
    {"name": "next", "qualified": "next", "kind": "let", "line": 8, "column": 12,
     "class": "dynamic",
     "reason": [{"subject": "next", "text": "reads counter", "line": 8, "column": 12},
                {"subject": "counter", "text": "is state, which changes between runs",
                 "line": 7, "column": 17}],
     "direct_depends_on": ["counter"], "depends_on": ["counter"]}
  ]
}
```

| Field | Meaning |
|---|---|
| `kind` | `let`, `config let`, `var`, `state`, `fn`, `class` or `enum` |
| `qualified`, `module` | The module-qualified name (`lib::BASE`; the bare name in the entry file) and the module, absent for the entry file |
| `class` | `static`, `function` or `dynamic` |
| `static_pure` | Functions only |
| `reason` | The chain for a dynamic declaration, or for a function that is not static-pure: `{subject, text, file, line, column}` per link |
| `read_by_dynamic`, `readers` | Static data only. A reader's `kind` is `dynamic`, `capture`, `statement` or `static`. A read by name from a host or a tool is not visible to the compiler and is not counted |
| `mutated_by` | Static data only, present when its value reaches an in-place update |
| `initializer_static` | Plain `state` only: whether the initializer is static (a `state` holding what is really a constant) |
| `depends_on`, `direct_depends_on` | Every declaration the value can depend on, by qualified name, closed transitively, and the direct ones. A method call depends on every method of that name |

## Dataflow query commands

`graph` and `show-graph` query the compiled dataflow graph without running the
program. `graph` takes `--term <name|id>` with the same resolution rules as
`explain`. [features/provenance.md](features/provenance.md#what-depends-on-what-graph)
is the walkthrough; this section is the reference.

`graph` stops at (or, forward, crosses) every `var` cell. See
[Cells and the frontier](#cells-and-the-frontier): it does not promise an
unqualified "minimal" answer on a program that uses `var`, and it says so in
its output.

### `graph` — One query, one result shape

```
petal graph [--json] --term <t> [--term <t2> ...] [--direction back|forward] [--slice] <file.ptl>
```

| Invocation | Query | Question |
| --- | --- | --- |
| `--direction back` (the default), one `--term` | provenance: the backward slice | What does this value depend on? |
| `--direction back`, several `--term`s, or `--slice` | slice: the subgraph that still produces every target | What part of the program do these need? |
| `--direction forward` | dependents: the forward slice; several `--term`s give the union of their walks | What would a change to this value reach? |

Every query returns the same JSON:

```
{direction: "back"|"forward", targets: TermId[], target_terms: Term[],
 terms: Term[], edges: [{from, to, kind}, ...], frontier: [...],
 complete: bool, minimal: bool}
```

`target_terms` are the terms `--term` named, in order; `terms` is the answer.
`complete` and `minimal` are `false` exactly when `frontier` is non-empty: a
backward walk stopped at a cell, or a forward walk crossed a cell may-edge.

**Provenance** prints the terms that feed into the target, with the edges
connecting them:

```
$ petal graph --term total -e 'let a = 2
let total = a * 3'
Provenance of t141 (total):
  op: Mul
  inputs: [139, 140]

Ancestors (3):
  t139: Copy
  t140: Constant(ConstantId(1))
  t138: Constant(ConstantId(0)) a

Edges (3):
  t139 -> t141
  t140 -> t141
  t138 -> t139
```

`kind` is always `"dataflow"` here: a backward walk answers a *must* question,
so it only crosses value edges.

**Dependents** (`--direction forward`) is the mirror: everything downstream of
the target. This direction answers a *may* question, so it also carries
`"kind": "may"` edges from a `var` declaration and from every `set` to every
read of that cell (`t96 ~> t97 (cell 'x', may)` in text mode). Which write
actually supplied a given read is a runtime fact; over-approximating it is
correct, while under-approximating it would report that a `set` affects
nothing. Each read reached that way is also listed in `frontier`, with
`complete: false`, so a client can tell an exact answer from an
over-approximated one.

**Slice** prints the smallest subgraph that still produces every target.
Useful for focused visualizations and for extracting the interesting part of a
larger program. `edges` are the edges of the subgraph induced on `terms`
(value, dispatch, and cell may-edges whose two ends are both in the slice).
One `--term` gives its provenance unless `--slice` is passed.

On a cell-free program this is the smallest such subgraph. On a program that
reads a `var` it is deliberately **not** minimal: it also pulls in the
declaration and every `set` site, transitively, and reports
`"minimal": false` with a `Not minimal — N cell reads crossed:` block. Too
small would silently compute a different value; too big only loses precision.

### Cells and the frontier

A `var` binds a mutable heap cell (see
[`var` and `set`](syntax/var.md)). The cell operand of a
`CellRead`, a `CellWrite` or a closure capture is an *identity* edge — it names
which box, not which value — so a backward walk never crosses one. Every stop
is reported as a **frontier** entry naming the var, its declaration, and the
complete set of writes that could have supplied the value:

```
Frontier (1):
  t97: read of var 'x' (not traced)
    possible write: t96 [line 2, column 1]
```

In `--json` output each entry is
`{read_term, decl_term, var, summary, captured, host_writable, resolution,
writes: [{term_id, line, column}, ...]}`: `captured` when the read is a
closure capture, `host_writable` for a `state var`, and `resolution`
`"not_traced"` from `graph`. `explain` resolves the boundary to the exact write
(matched on the cell, so one declaration that makes several cells, such as a
`var` in a loop body, does not confuse two of them), lists every recorded write
in order, and reports `truncated` when `max_depth` cuts the chain short.

The write set is complete because no expression evaluates to a cell, so the
only way to reach one is a name lexically bound to its declaration. The
exception is a `state var`, whose slot the host can also write through
`set_state`; that case is printed rather than glossed.

`graph` never runs the program, so it always gives this static
answer (`"resolution": "not_traced"`). `explain` does run it, and so resolves
the boundary to the exact write and continues the chain through it.

`--term x` on a `var` resolves to the *last* `CellWrite` on that name, since
name lookup is last-wins. `explain` from the write shows that write's own
chain, and `graph --direction forward` from it shows every read the write may reach.
Pass an explicit `--term tNN` to start somewhere else.

### `show-graph` — Graphviz DOT export

```
petal show-graph [--all] <file.ptl>
petal show-graph [--all] -e '<code>'
```

Prints the dataflow graph in DOT format, ready to pipe into `dot -Tpng`.
Phantom builtin terms are hidden unless `--all` is given. There is no `--json`
output; the format is DOT.

Nodes are colored by role (constants light blue, state pink, user bindings
white) so the output stays readable on mid-sized programs.

## Builtin Phantom Terms

Every compiled program starts with one phantom term per registered native
function (`t0`, `t1`, …) in the root block: a `Copy` with no inputs whose
`name` is the native's name. They exist for name resolution and for using a
builtin as a value, they are not listed in any block's `terms` array, and
they do not execute. User terms are numbered after them, so the first user
term's id is the number of natives registered, and it moves when a host
registers natives of its own.

In `show-ir --json` output (and `show-ir --all` text) everything before the
first non-phantom term is a phantom; the default text output and the
`--user-only` JSON view hide them.

[dev/Architecture.md](dev/Architecture.md#native-functions) says how natives
are registered and numbered, and
[dev/ir-as-target.md](dev/ir-as-target.md#binding-phantoms-and-builtins) what
an imported IR document needs of them (nothing, unless it wants a builtin as
a value).

## Traversing the IR

JavaScript helpers for walking a `show-ir --json` document (a block's terms,
dataflow inputs, the block tree, constants) are in
[Reading a document](dev/ir-as-target.md#reading-a-document) in
dev/ir-as-target.md.

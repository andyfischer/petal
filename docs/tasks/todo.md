

ISSUE 1

Report: - **`state` cannot be written from a function**, and `state var` turns every
  read into `get`. I kept all state at module scope and all logic inline,
  which is why `app.ptl` is one long script with an "intents" block. It
  reads well, but it does not factor.

Investigate this. I think the correct thing to do is use the `get` keyword
in order to declare the use of mutable state. Verify that this works correctly.
Is there anything wrong with using 'get' ?

Another related report:

- **No in-place mutation across a function boundary** shapes the whole solver.
  `step` is one 300-line function because a helper cannot write `vx[i]`; the
  body/body, body/wall and body/bar contact code is three near-copies for the
  same reason. It is fast enough and it reads top to bottom, but any other
  language would have had `solve_contact(i, j, n)`.

I think this can be solved with `var` or `state var` ?

GROOMED (2026-10-08)

Findings:
- `state var` + `set` + `get` already works from functions, including indexed
  writes (`set vx[i] = get vx[i] + 1.0`). Nothing is wrong with `get`; it stays
  required for cross-function reads (docs/var.md). Plain `state` stays
  unwritable from a function (keep the current error).
- BUG: an indexed `set` through a cell from inside a function copies the whole
  list per write. 20k-element list: 20k helper writes ~1.0s; 100k helper
  read-modify-writes ~31s vs ~1.4s for the same loop inline at module scope.
  (Measured on the Sep 24 release binary; re-measure on a fresh build first.)
- physics.ptl `step(wd, dt, env)` keeps its arrays as locals, so top-level
  helpers cannot see them.

Work:
1. Make `set xs[i] = ...` / `set r.f = ...` through a cell mutate in place when
   the write happens inside a function (nested closure and module-level cell).
2. Prototype both helper shapes for the solver and report readability + timing
   before choosing: (a) nested closures over a local `var` inside `step`,
   (b) module-level `var` arrays with top-level helpers.
3. Refactor physics-playground (`step` contact code into `solve_contact`-style
   helpers; app.ptl intents block into functions) as the proof.
4. Document the pattern in language-guide.md and writing-petal-guide.md.
5. New CLI: `petal apply-change <operation> <file> ...` for refactors that DO
   change program behaviour (unlike lint/suggest). First operation:
   `petal apply-change convert-to-var <file> --target <path>`
   - Target path = slash-separated enclosing declarations ending in the
     binding, `[n]` for repeats, leading `/` for module level:
     `step/vx`, `build_view/row/out[2]`, `/score`. This is a new syntax
     (existing tools only have `--term <name_or_id>`).
   - Accepts `let` -> `var` and `state` -> `state var`.
   - Rewrite: every `=` on the binding -> `set`; `get` inserted only on reads
     inside nested functions (bare reads in the declaring scope stay bare).
   - Exported bindings: follow into importing files. Importers may only read
     an exported `var`; an importer that writes it makes the command refuse.
   - Refuse when an `@x` rebind of the target exists.
   - Writes in place, gated by "result compiles"; `--dry-run` prints a diff.

Open:
- How importers are discovered for an exported target (a `--from <file>` flag
  like `suggest`, or a directory scan).

ISSUE 2

Report: - A `petal bench file.ptl` that prints instructions and milliseconds per
  call of a named function. `--profile` is close but whole-run only.

Go ahead and add `petal bench`

GROOMED (2026-10-08)

`petal run --profile` already has whole-run instruction/time counters and
per-function self instructions (rust/src/profile.rs); build on it.

Spec:
- `petal bench <file> --fn <name> [--fn <name>]...` runs the file normally and
  reports, per named function, over the calls the script itself makes:
  calls, instructions/call, ms/call with min / median / p95.
- Also per call: heap allocations, list/record copies, GC collections + time.
- Opt comparison: also run with the optimizer off and show the delta.
- Iterations: re-run the whole file until a ~1s budget is filled (with a
  warm-up); `--iters N` pins the run count.
- `--json` output.
- Hosts: whatever `petal run` supports, same `--host` flag.
- Docs: CLI.md, help.rs, docs/dev/scripts.md, docs/dev/performance.md.

Open:
- Instruction and time per call: inclusive of callees, self only, or both?
  (`--profile` counts self only.)
- Copy/allocation counters are compiled out of release builds today
  (`dup-stats` feature); decide whether `bench` enables them at runtime.

ISSUE 3

Awkwardness about functions that need to return a generated list from `for`

See this comment:

// A `for` is only captured where its value is *directly* consumed: bound to a
// name, or passed as an argument. As a function's final expression, or as the
// last expression of an `if` branch, it evaluates to nil — so every list built
// by a loop in this file is bound to a local first.
fn zeros(n: num) -> list
  let out = for i in range(0, n) do 0.0 end
  out
end

New behavior:

Let's implement the following logic:

1) If the function is declared `-> nil` then disable implicit value returning


So this:

    fn zeros(n: num) -> nil
      for i in range(0, n) do 0.0 end
    end

Would not return anything (and it would be a useless function)

But this:

    fn zeros(n: num) -> list
      for i in range(0, n) do 0.0 end
    end

Would return a list.

I don't think we need to verify that the return type is a list-like type for
the purposes of implicit return values. The typechecking pass will handle
a return type mismatch. This check is just to determine if the last
expression should implicitly return a value or not.

Additionally an absent / any declaration such as:

    fn zeros(n: num)
      for i in range(0, n) do 0.0 end
    end

WOULD return a value.

As part of this work, update `petal suggest`. If there is
a function with no declared return type and the last expression
has an implicit return, then it should suggest a refactor to
either add a return type or add `-> nil` (this would be a performance
improvement so that the runtime does not construct throwaway lists
for no reason). The suggest logic should check if the function is 
called anywhere and whether the return value is being used, and use
this information when suggesting the refactor.

Add a doc ./docs/implicit-return-values.md which captures this behavior and 
other existing logic details for implicit returns.

GROOMED (2026-10-08)

Findings:
- The quoted comment is stale. A trailing `for` already collects as a
  function's implicit return and as an `if`-branch / `match`-arm tail (commit
  92fef1d, documented in language-guide.md "For loops as mapping expressions").
  Delete the comment and the `let out = ...; out` workarounds in
  examples/dashboards/finance-dashboard/app.ptl.
- Today `-> nil` does not disable anything: `fn c(n) -> nil` ending in a `for`
  still builds and returns the list (no warning); a non-loop tail returns its
  value with a "declares `nil` but returns `num`" warning.

Spec:
- `-> nil`: the function body is compiled with its tail NOT in value position.
  Any tail expression (not only loops) yields nil; trailing loops become
  side-effect loops; the tail mismatch warning no longer fires.
- Absent return type, or any other declared type: unchanged, tail is the value.
- Explicit `return <value>` in a `-> nil` function: unchanged (returns the
  value, type checker warns).
- Lambdas have no return-type syntax, so they always return their tail.
- Audit the 12 existing `-> nil` functions in examples for callers that use
  the result.
- `petal suggest`, new kind, for an unannotated function whose tail is an
  implicit return of a loop:
    - some call uses the result -> suggest `-> list`
    - called, result never used  -> suggest `-> nil` (saves the throwaway list)
    - never called, or exported  -> report both options; `--apply` skips it
  `--apply` may write `-> nil` even though the IR changes; `suggest` is no
  longer required to always preserve IR (update suggestions-plan.md / CLI.md).
- New doc docs/implicit-return-values.md covering this and the existing rules.

Open:
- Does the new suggest kind cover only loop tails, or every unannotated
  function whose result no caller uses?

ISSUE 4

Language syntax change - Lets change the `export` keyword to `pub`

Continue to support `export` during the transition

Update `petal lint` so that it rewrites from export -> pub so
that existing code can be migrated to the new syntax

GROOMED (2026-10-08)

Findings: `export` is one reserved token handled in `parse_export`
(rust/src/parse.rs), plus cst_project.rs, the LSP keyword list, tree-sitter
and vim syntax. 755 `export` lines in 23 .ptl files (examples, petal-libs,
petal-ui); none in ~/garden, ~/.garden or ~/worlds-fair/ui/ptl. No script uses
`pub` as an identifier.

Spec:
- `pub` is a fully reserved keyword, accepted everywhere `export` is
  (fn, let, var, config let, state, enum, class, import).
- `export` keeps working, with no removal date. `petal check` (and the LSP)
  emits a deprecation warning for it.
- `petal lint`: new rule rewriting `export` -> `pub` under `--fix`.
- `petal fmt` also normalizes `export` -> `pub`.
- Migrate all in-repo .ptl files, docs, compiler/parser error text, help text
  and ast_display to `pub`.
- Editor support: tree-sitter grammar (regenerate parser.c), vim syntax, LSP
  keyword list.

# Aspirational Examples

These `.ptl` files are **design sketches**, not runnable programs. They show
how Petal's goal-level features (differentiation via `grad()`, program
projection, provenance tracing, live editing, self-metaprogramming) could
feel to use *from inside a script*. Each calls an in-language API the runtime
does not expose (`grad`, `Program.parse`, `Program.current`, `.slice`,
`.trace`), so none of them compiles.

The features themselves have mostly shipped in another form: through the
host API and the command line rather than as values a script handles. The
table says what exists today for each sketch and where it is documented. See
the [README](../../README.md#goals) for the goals behind them.

When a sketched API lands, rewrite its sketch against the real API and move it
into [`examples/`](../../examples/) so it runs under the test suite.

## Current sketches

| File | What it sketches | What exists today |
|------|------------------|-------------------|
| `differentiation.ptl` | A high-level `grad()` / `gradients()` API for automatic differentiation. | Forward-mode dual numbers: `dual`, `value_of` and `deriv_of` from the `autodiff` package, with derivatives carried through arithmetic and some math builtins. See [features/differentiation.md](../features/differentiation.md) and [`examples/console/differentiation.ptl`](../../examples/console/differentiation.ptl). There is no `grad()` and no reverse mode. |
| `gradient_descent.ptl` | Optimizer sugar on top of `grad()`. | Nothing beyond the dual numbers above. |
| `live_editing.ptl` | Editing a running program from a script (`Program.parse`, `prog.edit`), with state reconciled across the edit. | Hot reload with state carried across the edit, driven by the host: [hot-reload.md](../features/hot-reload.md). The ways to edit a program from outside it are surveyed in [program-modification.md](../features/program-modification.md). |
| `metaprogramming.ptl` | Programs as first-class values: `Program.parse()`, `Program.build()`, `.terms()`, `.functions()`. | A script can read a function value with `fn_info`, `fn_ast` and `code_hash` (experimental): [function-introspection.md](../features/function-introspection.md). The whole program is available outside the script as IR JSON (`petal show-ir --json`, `petal run --ir`): [ir-as-target.md](../dev/ir-as-target.md). |
| `projection.ptl` | Program slicing as a value: `program.slice(target)`, forward and backward slices. | The same queries from the command line: `petal graph` (backward, forward with `--direction forward`, and a slice for several targets). See [CLI.md](../CLI.md#dataflow-query-commands). |
| `provenance.ptl` | Data provenance tracing through the dataflow graph, from inside a script (`prog.trace`). | `petal explain --term <name>` walks the value chain that produced a term, and a runtime error prints the nearest named ancestors of the failing value. See [CLI.md](../CLI.md#explain--show-the-value-chain-that-produced-a-term) and [debugging-visibility.md](../dev/debugging-visibility.md). |

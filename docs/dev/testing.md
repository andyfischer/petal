
## Testing

### Integration tests (vitest)

Uses Vitest. Tests shell out to the compiled `petal` CLI binary and assert on JSON output. Run from the `ts/` directory:

```bash
cd ts

# Run all tests
npx vitest

# Run a specific test file
npx vitest test/ir-basics.test.ts

# Run tests matching a name
npx vitest -t "emits Add"
```

**Test files** (`ts/test/*.test.ts`; a representative list — see the directory
for the full, growing set):
- `ir-basics.test.ts` — constants, arithmetic, variables, registers, comparisons, unary ops
- `ir-control-flow.test.ts` — if/else, for, while, match, short-circuit (&&/||), break, return, continue
- `ir-data-structures.test.ts` — lists, records, enums, field/index access, concat
- `ir-functions.test.ts` — function defs, closures, captures, recursion, lambdas, calls
- `ir-higher-order.test.ts` — map, filter, reduce
- `ir-jsx.test.ts` — JSX-like element syntax
- `ir-state.test.ts` — state init, read, write, state keys
- `bug-state-in-if.test.ts` — regression coverage for state inside conditional branches
- `autodiff.test.ts` — dual numbers and chain-rule propagation
- `provenance.test.ts` / `slicing.test.ts` / `graph.test.ts` — dataflow query commands
- `compound-assign.test.ts` / `pipe-operator.test.ts` / `method-syntax.test.ts` — operators and sugar
- `string-interp.test.ts` / `string-intern.test.ts` / `list-string-builtins.test.ts` / `collection-builtins.test.ts`
- `gc.test.ts` / `loop-state.test.ts` / `loop-carry-limitations.test.ts` / `is-callable.test.ts`
- `lexer.test.ts` / `error-positions.test.ts` / `js-compat.test.ts`
- `modules.test.ts` — multi-file import cases (`fixtures/modules/*.ptl`), `-I`, IR file table
- `test-samples.test.ts` — every `examples/console/*.ptl` file runs without error
- `seed.test.ts` / `error-format.test.ts` — the two determinism knobs (below)

**Helpers** (`ts/test/helpers.ts`):
- The binary is built once per test session by `ts/test/global-setup.ts`
  (wired via `globalSetup` in `vitest.config.ts`); the old `ensureBuild()`
  helper is now a no-op kept for compatibility
- `showIrJson(code)` — compiles Petal code, returns parsed IR JSON (`petal show-ir --json -e '...'`)
- `showAstJson(code)` — returns parsed AST JSON (`petal show-ast --json -e '...'`)
- `showTokensJson(code)` — returns parsed token list (`petal show-tokens --json -e '...'`)
- `runPetal(code)` — executes code, returns stdout (`petal run -e '...'`)
- `userTerms(ir)` — filters out builtin phantom terms
- `termsByOp(ir, op)` — finds terms by operation name
- `termByName(ir, name)` / `termById(ir, id)` — term lookup helpers

### Example-based tests

`ts/test/test-samples.test.ts` runs every `examples/console/*.ptl` file through the `petal` binary
and asserts it exits without error (3 s timeout per file). These are included in the
normal vitest run:

```bash
cd ts
npx vitest test/test-samples.test.ts   # Run just the sample tests
```

For a quick eyeball-check that prints the first few lines of each example's
output, run `./ts/bin/test-examples.ts` (add `--full` for full output).


### Deterministic runs

Two knobs make a run byte-reproducible, which is what before/after diffing of a
mechanical refactor needs (see [refactor-verification.md](refactor-verification.md)):

- **`petal run --seed <n>`**, or `PETAL_SEED=<n>` for any command and any
  embedder, fixes the `random` / `random_int` / `choose` stream. Without one,
  the seed comes from the wall clock and even an *old-vs-old* diff fails.
  Embedders call `Env::set_seed(n)` once before the first run; the PRNG then
  advances naturally across frames and forks.
- **`petal run --error-format bare`** (also on `check`) prints just the error
  message — no `[line N, column M]`, no echoed source line or caret — so a
  re-indenting refactor does not show up as an output diff. Type-checker
  warnings lose their position line and snippet too. What it *cannot* reach is
  a line number quoted inside a message's prose (`written on line 775`); the
  verifier treats a warnings-only difference as a note rather than a failure.

Covered by `ts/test/seed.test.ts`, `ts/test/error-format.test.ts`, and the
`env::tests::seed_tests` / `cli::tests` Rust unit tests.


### Verifying a refactor

A large mechanical change — `petal lint --fix` over the tree, an optimizer
pass, a prelude rewrite — wants proof that it was behavior-preserving.
`ts/bin/verify.ts` is that proof: it runs a *plan* of cheapest-first checks
(`compiles`, `ir-equal`, `control-run`, `run-diff`, `golden`) over a corpus of
`.ptl` files, on two sides that differ along exactly one axis.

```sh
# source A/B — the same binaries over two source trees
./ts/bin/verify.ts --plan lint-fix --before ab3304a~1 --after .

# binary A/B — the same sources under two `petal` builds
./ts/bin/verify.ts --plan compiler --before-bin old/petal --after-bin rust/target/debug/petal
```

`--before` takes a git ref (materialized with `git archive` under the artifacts
dir) or a directory. Each file gets one row — its `kind` (`console`, `ui`,
`module`, `unsupported`, classified by evidence, not a hand list) and a verdict
(`identical-ir`, `identical-trace`, `nondeterministic`, `changed`,
`compile-error`, `unsupported`, `module`). The process exits non-zero if
anything is `changed` or `compile-error`.

A failure leaves a replay bundle under `verify-runs/<plan>-<timestamp>/<file>/`:
the resolved `plan.json`, the `scenario.json` and `seed` used, both traces, and
a `repro.sh` that reproduces the diff from any working directory with no other
context. Useful flags: `--only <substring|glob>`, `--jobs N`, `--frames N`,
`--update-golden`.

`test/ui-golden/index.json` is the UI analogue of `test/example-golden/`: the
sha256 of each UI app's 60-frame `monkey:1`, seed-1 trace. Only the hashes are
checked in — the traces themselves total ~72 MB, so a golden mismatch is a
signal to re-run the app and diff locally, not a stored diff. Re-baseline it
deliberately with `--update-golden`.

Plans live in `test/verify-plans/`. The design, and what is still unbuilt, is
in [refactor-verification.md](refactor-verification.md).

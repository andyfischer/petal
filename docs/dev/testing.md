
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

**Test files** (`ts/test/*.test.ts`):
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
- `test-samples.test.ts` — every `examples/*.ptl` file runs without error

**Helpers** (`ts/test/helpers.ts`):
- `ensureBuild()` — runs `cargo build` once per test session (called in `beforeAll`)
- `showIrJson(code)` — compiles Petal code, returns parsed IR JSON (`petal show-ir --json -e '...'`)
- `showAstJson(code)` — returns parsed AST JSON (`petal show-ast --json -e '...'`)
- `showTokensJson(code)` — returns parsed token list (`petal show-tokens --json -e '...'`)
- `runPetal(code)` — executes code, returns stdout (`petal run -e '...'`)
- `userTerms(ir)` — filters out builtin phantom terms
- `termsByOp(ir, op)` — finds terms by operation name
- `termByName(ir, name)` / `termById(ir, id)` — term lookup helpers

### Example-based tests

`ts/test/test-samples.test.ts` runs every `examples/*.ptl` file through the `petal` binary
and asserts it exits without error (3 s timeout per file). These are included in the
normal vitest run:

```bash
cd ts
npx vitest test/test-samples.test.ts   # Run just the sample tests
```

For a quick eyeball-check that prints the first few lines of each example's
output, run `./ts/bin/test-examples.ts` (add `--full` for full output).


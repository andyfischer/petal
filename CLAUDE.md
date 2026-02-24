# Petal

A custom programming language: Lexer → Parser → AST → Compiler → IR → Step Evaluator.

Source: `rust/src/`

## Testing

### Integration tests (vitest)

Uses Vitest. Tests shell out to the compiled `petal` CLI binary and assert on JSON output.

```bash
# Run all tests
npx vitest

# Run a specific test file
npx vitest test/ir-basics.test.ts

# Run tests matching a name
npx vitest -t "emits Add"
```

**Test files** (`test/*.test.ts`):
- `ir-basics.test.ts` — constants, arithmetic, variables, registers, comparisons, unary ops
- `ir-control-flow.test.ts` — if/else, for, while, match, short-circuit (&&/||), break, return
- `ir-data-structures.test.ts` — lists, records, enums, field/index access, concat
- `ir-functions.test.ts` — function defs, closures, captures, recursion, lambdas, calls
- `ir-state.test.ts` — state init, read, write, state keys

**Helpers** (`test/helpers.ts`):
- `ensureBuild()` — runs `cargo build` once per test session (called in `beforeAll`)
- `showIrJson(code)` — compiles Petal code, returns parsed IR JSON (`petal show-ir --json -e '...'`)
- `showAstJson(code)` — returns parsed AST JSON (`petal show-ast --json -e '...'`)
- `showTokensJson(code)` — returns parsed token list (`petal show-tokens --json -e '...'`)
- `runPetal(code)` — executes code, returns stdout (`petal run -e '...'`)
- `userTerms(ir)` — filters out builtin phantom terms (first 21)
- `termsByOp(ir, op)` — finds terms by operation name
- `termByName(ir, name)` / `termById(ir, id)` — term lookup helpers

### Example-based tests

```bash
./bin/test-each.sh          # Run all 16 examples with timeout
```

Example programs live in `examples/*.ptl`.

## MCP Tool: TestSnippet

An MCP server (`tools/petal-mcp.ts`) provides a `TestSnippet` tool that compiles and runs
Petal code directly. It automatically builds the Rust binary before running. Use this to
quickly test Petal snippets without shelling out manually.

```
TestSnippet({ code: 'print("hello")' })
```

Returns stdout, stderr, and exit code.

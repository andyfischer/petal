# Petal

A custom programming language: Lexer → Parser → AST → Compiler → IR → Step Evaluator.

## Directory Overview

- `rust/` — Main implementation for the core language (lexer, parser, AST, compiler, IR, evaluator)
- `petal-sdl/` — SDL-based native app that integrates the language into a graphical environment
- `petal-diagram-canvas/` — Another integration, web-based diagram renderer
- `petal-web/` — Integration that uses Petal as a React-like page rendering layer.
- `playground/` — Interactive web app for exploring the compiler pipeline (Prism API + React)
- `ts/` — All TypeScript code (Node project with its own `package.json`/`tsconfig.json`):
  - `ts/bin/` — Dev wrappers (`run-petal.ts`, `test-examples.ts`)
  - `ts/tools/` — MCP servers
  - `ts/test/` — Vitest integration tests
- `examples/` — Example `.ptl` programs
- `docs/` — Documentation

Source: `rust/src/`

## Running Petal locally

Use `./ts/bin/run-petal.ts` as the default way to invoke the compiler during
development. It rebuilds the binary if any Rust source is newer than it, then
forwards all arguments to `petal`:

```bash
./ts/bin/run-petal.ts run examples/hello.ptl
./ts/bin/run-petal.ts run -e 'print(1 + 2)'
./ts/bin/run-petal.ts show-ir -e 'let x = 1 + 2'
```

You can still invoke `rust/target/debug/petal` directly if you want to skip
the staleness check.

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

**Test files** (`ts/test/*.test.ts`) — 26 files, 330+ tests:
- `ir-basics.test.ts` — constants, arithmetic, variables, registers, comparisons, unary ops
- `ir-control-flow.test.ts` — if/else, for, while, match, short-circuit (&&/||), break, return, continue
- `ir-data-structures.test.ts` — lists, records, enums, field/index access, concat
- `ir-functions.test.ts` — function defs, closures, captures, recursion, lambdas, calls
- `ir-higher-order.test.ts` — map, filter, reduce
- `ir-jsx.test.ts` — JSX-like element syntax
- `ir-state.test.ts` — state init, read, write, state keys
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

## Playground

An interactive web app (`playground/`) for exploring Petal's compiler pipeline. Built with
Prism Framework (API + React frontend).

```bash
# First-time setup: create playground/.env with a port
echo "PRISM_API_PORT=4027" > playground/.env

cd playground && npm run dev           # Starts the API server
cd playground/web && npm run dev       # Starts the Vite dev server (separate terminal)
```

`PRISM_API_PORT` is required; `VITE_PORT` is optional and defaults to 4007.

**Features:**
- Source code editor with live analysis (tokens, AST, IR, and program output)
- Example file picker — loads examples from `examples/*.ptl` into the editor

**API endpoints** (`playground/src/services/petal-service.ts`):
- `POST /analyze` — returns JSON tokens, AST, IR, and run output
- `POST /analyze-text` — returns human-readable text representations
- `GET /examples` — lists all example files with their contents

**Frontend** (`playground/web/`): React + Vite, proxied to the API via `vite.config.ts`.

## MCP Server

An MCP server (`ts/tools/petal-mcp.ts`) exposes six tools that compile and run Petal code
directly. It automatically builds the Rust binary before running. Use these to
quickly test Petal snippets without shelling out manually.

| Tool | Purpose |
|------|---------|
| `TestSnippet({code, trace?})` | Run a snippet; returns stdout, stderr, exit code. `trace: true` adds a per-term execution trace. |
| `CheckSnippet({code})` | Lex+parse+compile without running. Cheaper than `TestSnippet` for syntax validation. |
| `ExplainTerm({code, term})` | Run with tracing, then walk the dataflow graph backward from `term` to answer "why does X have value Y?". |
| `ShowIR({code})` | Return the compiled IR as JSON. |
| `ShowAST({code})` | Return the parsed AST as JSON. |
| `ShowTokens({code})` | Return the token stream as JSON. |

```
TestSnippet({ code: 'print("hello")' })
```

petal-diagram-canvas exposes a separate MCP server (`ts/tools/petal-diagram-mcp.ts`) with
`Diagram*` tools that speak the debug protocol over WebSocket — see
`docs/debug-protocol.md`.

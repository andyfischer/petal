# Petal

A custom programming language: Lexer → Parser → AST → Compiler → IR → Step Evaluator.

## Directory Overview

- `rust/` — Main implementation for the core language (lexer, parser, AST, compiler, IR, evaluator)
- `petal-sdl/` — SDL-based native app that integrates the language into a graphical environment
- `petal-diagram-canvas/` — Another integration, web-based diagram renderer
- `petal-web/` — Integration that uses Petal as a React-like page rendering layer.
- `playground/` — Interactive web app for exploring the compiler pipeline (Prism API + React)
- `tools/` — MCP server and dev tooling
- `test/` — Vitest integration tests
- `examples/` — Example `.ptl` programs
- `bin/` — Shell scripts
- `docs/` — Documentation

Source: `rust/src/`

## Running Petal locally

Use `./bin/run-petal.ts` as the default way to invoke the compiler during
development. It rebuilds the binary if any Rust source is newer than it, then
forwards all arguments to `petal`:

```bash
./bin/run-petal.ts run examples/hello.ptl
./bin/run-petal.ts run -e 'print(1 + 2)'
./bin/run-petal.ts show-ir -e 'let x = 1 + 2'
```

You can still invoke `rust/target/debug/petal` directly if you want to skip
the staleness check.

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
- `userTerms(ir)` — filters out builtin phantom terms
- `termsByOp(ir, op)` — finds terms by operation name
- `termByName(ir, name)` / `termById(ir, id)` — term lookup helpers

### Example-based tests

`test/test-samples.test.ts` runs every `examples/*.ptl` file through the `petal` binary
and asserts it exits without error (3 s timeout per file). These are included in the
normal vitest run:

```bash
npx vitest test/test-samples.test.ts   # Run just the sample tests
```

## Playground

An interactive web app (`playground/`) for exploring Petal's compiler pipeline. Built with
Prism Framework (API + React frontend).

```bash
cd playground && npm run dev    # Starts API (port 4027) + Vite dev server (port 4028)
```

**Features:**
- Source code editor with live analysis (tokens, AST, IR, and program output)
- Example file picker — loads examples from `examples/*.ptl` into the editor

**API endpoints** (`playground/src/services/petal-service.ts`):
- `POST /analyze` — returns JSON tokens, AST, IR, and run output
- `POST /analyze-text` — returns human-readable text representations
- `GET /examples` — lists all example files with their contents

**Frontend** (`playground/web/`): React + Vite, proxied to the API via `vite.config.ts`.

## MCP Tool: TestSnippet

An MCP server (`tools/petal-mcp.ts`) provides a `TestSnippet` tool that compiles and runs
Petal code directly. It automatically builds the Rust binary before running. Use this to
quickly test Petal snippets without shelling out manually.

```
TestSnippet({ code: 'print("hello")' })
```

Returns stdout, stderr, and exit code.

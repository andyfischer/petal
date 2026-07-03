# Getting Started

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- Node.js **23 or newer** (for running tests and the TypeScript tooling — the
  `ts/bin/*.ts` scripts rely on Node's native TypeScript type-stripping)

## Building

Build the Petal compiler from the project root:

```bash
cd rust
cargo build
```

The binary is output to `rust/target/debug/petal`.

## Running Your First Program

Create a file called `hello.ptl`:

```petal
print("hello, world!")
```

The recommended way to run Petal locally is the `ts/bin/run-petal.ts` wrapper.
It rebuilds the binary if any Rust source is newer than it, then forwards all
arguments to `petal`:

```bash
./ts/bin/run-petal.ts run hello.ptl
./ts/bin/run-petal.ts run -e 'print(1 + 2)'
```

Use this script for day-to-day development and testing — it keeps the binary
in sync with your source changes without paying for a full `cargo build` on
every invocation. You can also call the binary directly if you prefer:

```bash
rust/target/debug/petal run hello.ptl
```

## Running the Examples

The `examples/` directory contains a set of example programs:

```bash
# Run a single example
rust/target/debug/petal run examples/fizzbuzz.ptl

# Run all examples with pass/fail reporting
./ts/bin/test-examples.ts
```

See [examples/README.md](../examples/README.md) for a description of each example.

## CLI Commands

The `petal` binary has several commands for inspecting the compilation pipeline.
Examples below use `./ts/bin/run-petal.ts` (the recommended wrapper); substitute
`rust/target/debug/petal` if you want to skip the staleness check.

```bash
# Run a program
./ts/bin/run-petal.ts run examples/hello.ptl
./ts/bin/run-petal.ts run -e 'print("hi")'

# Show lexer tokens
./ts/bin/run-petal.ts show-tokens -e 'let x = 1'
./ts/bin/run-petal.ts show-tokens --json -e 'let x = 1'

# Show the parsed AST
./ts/bin/run-petal.ts show-ast -e 'let x = 1 + 2'
./ts/bin/run-petal.ts show-ast --json -e 'let x = 1 + 2'

# Show compiled IR (term graph)
./ts/bin/run-petal.ts show-ir -e 'let x = 1 + 2'
./ts/bin/run-petal.ts show-ir --json -e 'let x = 1 + 2'
```

All inspection commands support `--json` for machine-readable output. See
[docs/CLI.md](CLI.md) for the full reference.

## Running Tests

### Integration tests (Vitest)

```bash
cd ts
npm install          # First-time install of test dependencies
npx vitest           # Run all tests
npx vitest -t "name" # Run tests matching a name
```

### Example tests

```bash
./ts/bin/test-examples.ts   # Run all examples with timeout
```

## Using the MCP Tools

If you're using an AI assistant that supports MCP (like Claude Code), the project includes
an MCP server at `ts/tools/petal-mcp.ts` that provides tools — `TestSnippet`,
`CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowBytecode`, `ShowAST`, `ShowTokens`. These let you compile,
run, inspect, and debug Petal code directly from your assistant without shelling out
manually.

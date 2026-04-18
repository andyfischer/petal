# Getting Started

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- Node.js (for running tests and the playground)

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

The recommended way to run Petal locally is the `bin/run-petal.ts` wrapper.
It rebuilds the binary if any Rust source is newer than it, then forwards all
arguments to `petal`:

```bash
./bin/run-petal.ts run hello.ptl
./bin/run-petal.ts run -e 'print(1 + 2)'
```

Use this script for day-to-day development and testing — it keeps the binary
in sync with your source changes without paying for a full `cargo build` on
every invocation. You can also call the binary directly if you prefer:

```bash
rust/target/debug/petal run hello.ptl
```

## Running the Examples

The `examples/` directory contains 22 example programs:

```bash
# Run a single example
rust/target/debug/petal run examples/fizzbuzz.ptl

# Run all examples with pass/fail reporting
./bin/test-examples.sh
```

See [examples/README.md](../examples/README.md) for a description of each example.

## CLI Commands

The `petal` binary has several commands for inspecting the compilation pipeline.
Examples below use `./bin/run-petal.ts` (the recommended wrapper); substitute
`rust/target/debug/petal` if you want to skip the staleness check.

```bash
# Run a program
./bin/run-petal.ts run examples/hello.ptl
./bin/run-petal.ts run -e 'print("hi")'

# Show lexer tokens
./bin/run-petal.ts show-tokens -e 'let x = 1'
./bin/run-petal.ts show-tokens --json -e 'let x = 1'

# Show the parsed AST
./bin/run-petal.ts show-ast -e 'let x = 1 + 2'
./bin/run-petal.ts show-ast --json -e 'let x = 1 + 2'

# Show compiled IR (term graph)
./bin/run-petal.ts show-ir -e 'let x = 1 + 2'
./bin/run-petal.ts show-ir --json -e 'let x = 1 + 2'
```

All inspection commands support `--json` for machine-readable output. See
[docs/CLI.md](CLI.md) for the full reference.

## Running Tests

### Integration tests (Vitest)

```bash
npx vitest           # Run all tests
npx vitest -t "name" # Run tests matching a name
```

### Example tests

```bash
./bin/test-examples.sh   # Run all examples with timeout
```

## Using the Playground

The playground is an interactive web app for exploring the compiler pipeline.
First-time setup requires a port in `playground/.env`:

```bash
echo "PRISM_API_PORT=4027" > playground/.env
cd playground && npm run dev               # starts the API server
cd playground/web && npm run dev            # starts the Vite dev server (separate terminal)
```

Open the Vite URL (default `http://localhost:4007`) to access the editor, where you can
write Petal code and see live tokens, AST, IR, and program output. See
[docs/Playground.md](Playground.md) for more details.

## Using the MCP Tools

If you're using an AI assistant that supports MCP (like Claude Code), the project includes
an MCP server at `tools/petal-mcp.ts` that provides six tools — `TestSnippet`,
`CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowAST`, `ShowTokens`. These let you compile,
run, inspect, and debug Petal code directly from your assistant without shelling out
manually.

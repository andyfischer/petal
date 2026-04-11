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

Run it:

```bash
rust/target/debug/petal run hello.ptl
```

Or run an inline expression with `-e`:

```bash
rust/target/debug/petal run -e 'print(1 + 2)'
```

## Running the Examples

The `examples/` directory contains 16 example programs:

```bash
# Run a single example
rust/target/debug/petal run examples/fizzbuzz.ptl

# Run all examples with pass/fail reporting
./bin/test-examples.sh
```

See [examples/README.md](../examples/README.md) for a description of each example.

## CLI Commands

The `petal` binary has several commands for inspecting the compilation pipeline:

```bash
# Run a program
petal run examples/hello.ptl
petal run -e 'print("hi")'

# Show lexer tokens
petal show-tokens -e 'let x = 1'
petal show-tokens --json -e 'let x = 1'

# Show the parsed AST
petal show-ast -e 'let x = 1 + 2'
petal show-ast --json -e 'let x = 1 + 2'

# Show compiled IR (term graph)
petal show-ir -e 'let x = 1 + 2'
petal show-ir --json -e 'let x = 1 + 2'
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

The playground is an interactive web app for exploring the compiler pipeline:

```bash
cd playground && npm run dev
```

This starts the API server and a Vite dev server. Open the URL printed in the terminal
to access the editor, where you can write Petal code and see live tokens, AST, IR,
and program output. See [docs/Playground.md](Playground.md) for more details.

## Using the MCP Tools

If you're using an AI assistant that supports MCP (like Claude Code), the project includes
an MCP server that provides `TestSnippet`, `ShowIR`, `ShowAST`, and `ShowTokens` tools.
These let you compile and run Petal code directly from your assistant without shelling out
manually.

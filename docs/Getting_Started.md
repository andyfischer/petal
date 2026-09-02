# Getting Started

## Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- Node.js 23 or newer, for the tests and the TypeScript tooling in `ts/`
  (the `ts/bin/*.ts` scripts run as plain TypeScript under Node)

## Building

From the repo root:

```bash
make build
```

This runs `cargo build` in `rust/` and puts the binary at
`rust/target/debug/petal`.

## Running your first program

Create a file called `hello.ptl`:

```petal
print("hello, world!")
```

Run it:

```bash
rust/target/debug/petal run hello.ptl
```

`petal hello.ptl` is shorthand for `petal run hello.ptl`, and `-e` runs code
given on the command line:

```bash
rust/target/debug/petal run -e 'print(1 + 2)'
```

### The `run-petal.ts` wrapper

If you are changing the compiler, use `ts/bin/run-petal.ts` instead of the
binary. It rebuilds the binary when any Rust source is newer than it, then
forwards all arguments to `petal`:

```bash
./ts/bin/run-petal.ts run hello.ptl
./ts/bin/run-petal.ts run -e 'print(1 + 2)'
```

## Running the examples

Console programs live in `examples/console/`. Larger apps (games,
productivity tools, dashboards, custom hosts) live in the other
`examples/` subdirectories, each with its own README.

```bash
# Run one console example
rust/target/debug/petal run examples/console/fizzbuzz.ptl

# Run every console example and report pass/fail
./ts/bin/test-examples.ts
```

See [examples/README.md](../examples/README.md) for a list of the examples.

## Looking inside the compiler

The `petal` binary can show each stage of compilation:

```bash
petal show-tokens -e 'let x = 1'        # lexer tokens
petal show-ast -e 'let x = 1 + 2'       # parsed AST
petal show-ir -e 'let x = 1 + 2'        # compiled IR
petal show-bytecode -e 'let x = 1 + 2'  # bytecode
```

Each of these takes `--json` for machine-readable output. See
[CLI.md](CLI.md) for the full command reference.

## Running the tests

```bash
make test            # build, then run the whole vitest suite
```

Or directly:

```bash
cd ts
npm install          # first time only
npx vitest           # run all tests
npx vitest -t "name" # run tests matching a name
```

The suite also runs every `examples/console/*.ptl` program and checks its
output. See [dev/testing.md](dev/testing.md) for details.

## Using the MCP tools

If you use an AI assistant that supports MCP (such as Claude Code), the repo
includes an MCP server at `ts/tools/petal-mcp.ts`. It exposes tools to
compile, run and inspect Petal snippets (`TestSnippet`, `CheckSnippet`,
`ShowIR`, `ExplainTerm`, and others). See
[dev/mcp-server.md](dev/mcp-server.md).

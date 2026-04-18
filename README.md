# Petal

Petal is a programming language built around **dataflow graphs**, **first-class state**,
and **live editing**.

Every construct in Petal maps to a dataflow graph, making data flow through programs
explicit and traceable. Inline `state` variables persist across function calls and
survive hot reload. The step-based evaluator enables live editing — modify code while
programs are running.

## Quick Start

```bash
# Build the compiler
cd rust && cargo build && cd ..

# Hello world
rust/target/debug/petal run -e 'print("hello, world!")'

# Run an example
rust/target/debug/petal run examples/fizzbuzz.ptl
```

## Language Highlights

```petal
// Functions with implicit return
fn square(x) { x * x }

// Lists and higher-order functions
let nums = [1, 2, 3, 4, 5]
let evens = filter(nums, fn(x) { x % 2 == 0 })

// Enums and pattern matching
enum Shape {
    Circle(radius)
    Rect(w, h)
}

fn area(shape) {
    match shape {
        Circle(r) -> 3.14159 * r * r
        Rect(w, h) -> w * h
    }
}

// Persistent state across calls
fn counter() {
    state count = 0
    count += 1
    count
}

// Pipe operator
[3, 1, 2] |> sort |> reverse |> print

// String interpolation
let name = "Petal"
print("hello, {name}!")
```

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/Getting_Started.md) | Build instructions, running examples, CLI usage |
| [Language Guide](docs/Language_Guide.md) | Complete language reference: types, syntax, control flow, functions, state |
| [Builtins Reference](docs/Builtins.md) | All ~65 built-in functions with signatures and examples |
| [CLI Reference](docs/CLI.md) | Full CLI command reference and JSON output schemas |
| [Architecture](docs/Architecture.md) | Internal design: IR term graph, evaluator, state, provenance |
| [Design Goals](docs/PETAL_GOALS.md) | Language philosophy and the four foundational pillars |

## Tools

| Tool | Description |
|------|-------------|
| [Playground](docs/Playground.md) | Interactive web app for exploring the compiler pipeline (tokens, AST, IR, output) |
| [Game Framework](docs/Game_Framework.md) | SDL2-based 2D game framework with hot reload |
| MCP Server | AI assistant integration — `TestSnippet`, `CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowAST`, `ShowTokens` tools |

## Examples

The [`examples/`](examples/) directory contains 22 programs covering all language features,
from hello world to Conway's Game of Life. See [examples/README.md](examples/README.md)
for the full list.

The [`petal-sdl/examples/`](petal-sdl/examples/) directory contains playable games:
snake, pong, breakout, tetris, invaders, and more.

## Architecture

```
Source Code → Lexer → Parser → AST → Compiler → IR (Term Graph) → Step Evaluator
```

The compiler pipeline lives in `rust/src/`. The IR is a term graph with explicit dataflow
edges — each term represents an operation and references its inputs by ID. Blocks organize
terms into control flow regions. The step evaluator walks the graph one term at a time,
enabling live editing and state preservation across hot reloads.

See [docs/Architecture.md](docs/Architecture.md) for detailed architecture
documentation.

## Testing

```bash
npx vitest               # Integration tests (330+ tests)
./bin/test-examples.sh   # Run all example programs
```

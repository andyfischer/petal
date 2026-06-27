# Petal

Petal is a programming language built around **dataflow graphs**, **first-class state**,
and **live editing**. Every construct in Petal maps to a dataflow graph, making data flow through programs explicit and traceable.

## Quick Start

```bash
# Build the compiler
cd rust && cargo build && cd ..

# Hello world
rust/target/debug/petal run -e 'print("hello, world!")'

# Run an example
rust/target/debug/petal run examples/fizzbuzz.ptl
```

## Language Example

```petal

(TODO: Change this to just a short example instead of a syntax tour)

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
| [Builtins Reference](docs/Builtins.md) | All 68 built-in functions with signatures and examples |
| [CLI Reference](docs/CLI.md) | Full CLI command reference and JSON output schemas |
| [Architecture](docs/Architecture.md) | Internal design: IR term graph, evaluator, state, provenance |
| [Goals](docs/goals.md) | Vision (the four pillars), remaining work, and sequencing |
| [Function Overloading](docs/Function_Overloading.md) | Multi-arity dispatch rules |
| [Mutability Plan](docs/MutabilityPlan.md) | Why the IR is purely immutable (design context) |
| [Debugging & Visibility](docs/debugging-visibility.md) | The three observability stacks (CLI, MCP, vitest) |
| [Debug Protocol](docs/debug-protocol.md) | JSON command/response schema shared by petal-sdl and petal-diagram-canvas |

## Integrations & Tools

| Integration | Description |
|-------------|-------------|
(TODO: Check if all of these are still active and relevant, clean up the list)
| [Game Framework (petal-sdl)](docs/Game_Framework.md) | SDL2-based 2D game framework with hot reload — see also [apps/petal-sdl/docs/game-dev-guide.md](apps/petal-sdl/docs/game-dev-guide.md) and [apps/petal-sdl/docs/agent-protocol.md](apps/petal-sdl/docs/agent-protocol.md) |
| [petal-web](apps/petal-web/README.md) | WebAssembly target that renders JSX element trees as live DOM |
| [petal-diagram-canvas](apps/petal-diagram-canvas/README.md) | Canvas-based diagram visualization with live source editor |
| [petal-fps](apps/petal-fps/README.md) | Hybrid Rust + Petal 3D first-person-shooter experiment with z-buffered rasterizer |
| MCP Server | AI assistant integration — `TestSnippet`, `CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowAST`, `ShowTokens` tools (`ts/tools/petal-mcp.ts`) |

## Testing

```bash
cd ts && npx vitest          # Integration tests (330+ tests)

(TODO: change how tests work, run all the test-examples.ts items inside of
the main vitest, so that it's one command to run them all)
./ts/bin/test-examples.ts    # Run all example programs
```

## License

Petal is released under the [MIT License](LICENSE).

# Petal

Petal is a programming language built around **dataflow graphs**, **first-class state**,
and **live editing**. Every construct in Petal maps to a dataflow graph, making data flow through programs explicit and traceable.

## Quick Start

```bash
# Build the compiler (or run `make build`)
cd rust && cargo build && cd ..

# Hello world
rust/target/debug/petal run -e 'print("hello, world!")'

# Run an example
rust/target/debug/petal run examples/fizzbuzz.ptl
```

Common commands are wrapped in the [Makefile](Makefile) — run `make` to list them
(`make build`, `make test`, `make clean`).

## Language Example

```petal
fn square(x)
  x * x
end

// Persistent state across calls
fn counter()
  state count = 0
  count += 1
  count
end

let name = "Petal"
print([1, 2, 3] |> map(square))   // [1, 4, 9]
print("hello, {name}!")            // hello, Petal!
```

See the [Language Guide](docs/Language_Guide.md) for the full tour: enums and
pattern matching, higher-order functions, and more.

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
| [petal-sdl](apps/petal-sdl/README.md) | SDL2-based 2D game framework with hot reload — see also [apps/petal-sdl/docs/game-dev-guide.md](apps/petal-sdl/docs/game-dev-guide.md) and [apps/petal-sdl/docs/agent-protocol.md](apps/petal-sdl/docs/agent-protocol.md) |
| [petal-web](apps/petal-web/README.md) | WebAssembly target that renders JSX element trees as live DOM |
| [petal-web-canvas](apps/petal-web-canvas/README.md) | Run Petal scripts that draw interactive graphics into an HTML canvas in the browser |
| [petal-diagram-canvas](apps/petal-diagram-canvas/README.md) | Canvas-based diagram visualization with live source editor |
| [petal-fps](apps/petal-fps/README.md) | Hybrid Rust + Petal 3D first-person-shooter experiment with z-buffered rasterizer |
| [side-scroller](apps/side-scroller/README.md) | 2D side-scrolling platformer written almost entirely in Petal |
| MCP Server | AI assistant integration — `TestSnippet`, `CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowAST`, `ShowTokens` tools (`ts/tools/petal-mcp.ts`) |

## Testing

```bash
make test                    # Build, then run the full suite (or: cd ts && npx vitest run)
```

`npx vitest` (and `make test`) runs the integration tests **and** every program in
`examples/` — `ts/test/test-samples.test.ts` executes each `.ptl` file and fails on
any error, so one command covers everything.

```bash
./ts/bin/test-examples.ts    # Optional: print each example's output for manual inspection
```

## License

Petal is released under the [MIT License](LICENSE).

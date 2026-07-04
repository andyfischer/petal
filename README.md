# Petal

Petal is a programming language for creative coding.

### Main features ###

 - Programs are **dataflow graphs** allowing for high levels of introspection.
 - **First-class state** as part of control flow.
 - **Live editing** - modify source while it's running and preserve state.
 - **Speculative execution** - safely run the same program multiple times with alterations for experimentation.
 - **Differentiable** - supports back-propogation, make program modifications based on observed outputs.
 - Various other language features to help quick iteration. Hybrid functional/imperative design, optional type declarations, immutable values.

### Existing Research ###

Some projects and research on the same topics:

 - **Dataflow & reactive languages** — [Lucid](https://en.wikipedia.org/wiki/Lucid_(programming_language)),
   [Lustre](https://en.wikipedia.org/wiki/Lustre_(programming_language)), LabVIEW, and
   FRP (Elm, signal graphs).
 - **Differentiable & automatic programming** — [JAX](https://github.com/jax-ml/jax),
   [PyTorch](https://pytorch.org/), Swift for TensorFlow.
 - **Live coding & hot reloading** — [Sonic Pi](https://sonic-pi.net/),
   [Tidal](https://tidalcycles.org/), Extempore; Smalltalk images, Erlang hot swap,
   [React Fast Refresh](https://reactnative.dev/docs/fast-refresh).
 - **Control flow keyed state** — storing state as part of the control flow graph - 
   React Hooks ([useState](https://overreacted.io/why-do-hooks-rely-on-call-order/))
   and Jetpack Compose's [positional memoization](https://newsletter.jorgecastillo.dev/p/positional-memoization-in-jetpack).

## Quick Language Example

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

See the [Language Guide](docs/Language_Guide.md) for the full tour.

## Quick Start

```bash
# Build the compiler
make build

# Hello world
rust/target/debug/petal run -e 'print("hello, world!")'

# Run an example
rust/target/debug/petal run examples/fizzbuzz.ptl
```

For the full list of developer scripts, see [Developer Scripts & Commands](docs/dev/scripts.md).

## Repository Layout

| Directory | Description |
|-----------|-------------|
| [`rust/`](rust/) | The core language implementation: lexer, parser, AST, compiler, IR, evaluator, bytecode VM |
| [`docs/`](docs/README.md) | Language documentation for using Petal |
| [`docs/dev/`](docs/dev/) | Documentation for developing on Petal |
| [`examples/`](examples/README.md) | Runnable example `.ptl` programs demonstrating language features |
| [`editor-support/`](editor-support/README.md) | Editor/IDE tooling |
| [`ts/`](ts/) | TypeScript tooling, including: dev wrappers, MCP servers, and the vitest integration test suite |
| [`test/`](test/README.md) | Automated tests |
| [`test/benchmarks/`](test/benchmarks/) | Petal programs used to compare backend performance |
| [`apps/`](apps/) | Test apps that integrate/embed Petal (including apps that use SDL, WASM, and HTML canvas) |
| [`petal-ui/`](petal-ui/) | Interactivity layer for embedders: normalized input events, the shared draw-command vocabulary, and the `ui` prelude module |

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/Getting_Started.md) | Build instructions, running examples, CLI usage |
| [Language Guide](docs/Language_Guide.md) | Complete language reference: types, syntax, control flow, functions, state |
| [Builtins Reference](docs/Builtins.md) | All built-in functions with signatures and examples |
| [CLI Reference](docs/CLI.md) | Full CLI command reference and JSON output schemas |
| [Module System](docs/module-system.md) | `import` syntax, module resolution |
| [Architecture](docs/dev/Architecture.md) | Internal design: IR term graph, evaluator, state, provenance |
| [Goals](docs/dev/goals.md) | Vision (the four pillars), remaining work, and sequencing |

## Sample Integration Apps

| Integration | Description |
|-------------|-------------|
| [petal-sdl](apps/petal-sdl/README.md) | SDL2-based 2D game framework with hot reload — see also [apps/petal-sdl/docs/game-dev-guide.md](apps/petal-sdl/docs/game-dev-guide.md) and [apps/petal-sdl/docs/agent-protocol.md](apps/petal-sdl/docs/agent-protocol.md) |
| [petal-web](apps/petal-web/README.md) | WebAssembly target that renders JSX element trees as live DOM |
| [petal-web-canvas](apps/petal-web-canvas/README.md) | Run Petal scripts that draw interactive graphics into an HTML canvas in the browser |
| [petal-diagram-canvas](apps/petal-diagram-canvas/README.md) | Canvas-based diagram visualization with live source editor |
| [petal-fps](apps/petal-fps/README.md) | Hybrid Rust + Petal 3D first-person-shooter experiment with z-buffered rasterizer |
| [side-scroller](apps/side-scroller/README.md) | 2D side-scrolling platformer written almost entirely in Petal |

## License

Petal is released under the [MIT License](LICENSE).

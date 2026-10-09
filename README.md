# Petal

Petal is a programming language for creative coding.

## Goals

The language goals are: Easy to use, supports live coding, good introspection & debuggability, and
supports goal-based program manipulation.

## Design decisions

For those goals, the language has these design decisions:

 - Hybrid functional and imperative. Encourages dataflow based computation with immutable values.
 - Does allows mutable state for practicality. Limited effects.
 - First-class persistent state support with the `state` keyword.
 - Type system with optional type annotations.
 - The runtime VM supports introspection & live editing features including: time-travel debugging, differentiation, provenance tracing, safe speculative execution, and more.

## Project status

This project is in an early, experimental phase. Large backwards-incompatible changes are still happening. Stability not guaranteed.

### Related work

A few references to existing work that Petal is inspired by:

 - **Dataflow and reactive languages:** [Lucid](https://en.wikipedia.org/wiki/Lucid_(programming_language)),
   [Lustre](https://en.wikipedia.org/wiki/Lustre_(programming_language)), LabVIEW, and
   FRP (Elm, signal graphs).
 - **Differentiable programming:** [JAX](https://github.com/jax-ml/jax),
   [PyTorch](https://pytorch.org/), Swift for TensorFlow.
 - **Live coding and hot reloading:** [Sonic Pi](https://sonic-pi.net/),
   [Tidal](https://tidalcycles.org/), Extempore; Smalltalk images, Erlang hot swap,
   [React Fast Refresh](https://reactnative.dev/docs/fast-refresh).
 - **State keyed by control flow:** React Hooks ([useState](https://overreacted.io/why-do-hooks-rely-on-call-order/))
   and Jetpack Compose's [positional memoization](https://newsletter.jorgecastillo.dev/p/positional-memoization-in-jetpack).

## Quick language example

```petal
fn square(x)
  x * x
end

// Persistent state: one slot per call path, kept across runs and hot reloads
fn counter()
  state count = 0
  count += 1
  count
end

let name = "Petal"
print([1, 2, 3] |> map(square))   // [1, 4, 9]
print("hello, {name}!")            // hello, Petal!
```

See the [Language Guide](docs/language-guide.md) for the full tour.

## Install

Install the `petal` CLI as a prebuilt binary (macOS Apple Silicon or Intel,
Linux x86_64 or arm64):

```bash
curl -fsSL https://petal-lang.org/install.sh | sh
```

This puts `petal` in `~/.petal/bin` and adds it to your PATH. It needs no
`sudo` and no dependencies. To uninstall:

```bash
curl -fsSL https://petal-lang.org/uninstall.sh | sh
```

See [docs/dev/releasing.md](docs/dev/releasing.md) for how the binaries are built and published.

## Build from source

```bash
# Build the compiler
make build

# Hello world
core/target/debug/petal run -e 'print("hello, world!")'

# Run an example
core/target/debug/petal run examples/console/fizzbuzz.ptl
```

For the full list of developer commands, see [Developer Scripts & Commands](docs/dev/scripts.md).

## Repository layout

| Directory | Description |
|-----------|-------------|
| [`core/`](core/) | The language implementation: lexer, parser, compiler, IR, evaluator, bytecode VM |
| [`core-libs/petal-ui/`](core-libs/petal-ui/README.md) | Helper library that implements user-interaction primitives |
| [`core-libs/petal-query/`](core-libs/petal-query/README.md) | Helper library that implements asynchronous data fetching |
| [`core-runtime/`](core-runtime/README.md) | Shared libraries written in Petal |
| [`integrations/`](integrations/) | Native bindings for Petal |
| [`garden/`](garden/README.md) | Text editor and IDE built for Petal |
| [`examples/`](examples/README.md) | Collection of runnable examples |
| [`docs/`](docs/README.md) | Documentation |
| [`editor-support/`](editor-support/README.md) | Files for IDEs, syntax definitions, etc. |
| [`tools/`](tools/) | Scripts and tooling for development |
| [`test/`](test/README.md) | Test suite |

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/Getting_Started.md) | Install the CLI, run your first program, and find what to read next |
| [Language Guide](docs/language-guide.md) | The full language reference |
| [Builtins Reference](docs/Builtins.md) | Every built-in function |
| [CLI Reference](docs/CLI.md) | Every `petal` subcommand and flag |
| [Module System](docs/module-system.md) | `import` and module resolution |
| [Architecture](docs/dev/Architecture.md) | How the implementation works |
| [Goals](docs/dev/goals.md) | The vision and the remaining work |

## Building a full app with Petal

The repo comes with "integrations" and "apps", which come together in layers:

```
Petal Core  →  Integrations  →  Apps
```

### Core

"Petal Core" is the core Rust based language implementation in `core/` which includes the compiler, runtime, type checker, and more.

### Integrations

An **integration** is a binding of Petal script execution to a native platform. These can be used as part of building a full application.

The repo contains:

  | Integration | Description |
  |-------------|-------------|
  | [petal-desktop-sdl](integrations/petal-desktop-sdl/README.md) | SDL2 desktop host with hot reload. See the [game dev guide](integrations/petal-desktop-sdl/docs/game-dev-guide.md) and [agent protocol](integrations/petal-desktop-sdl/docs/agent-protocol.md) |
  | [petal-web-html](integrations/petal-web-html/README.md) | WebAssembly host that loads Petal and renders to HTML / DOM |
  | [petal-web-canvas](integrations/petal-web-canvas/README.md) | WebAssembly host that loads Petal and renders graphics to a canvas |
  | [petal-c-bridge](integrations/petal-c-bridge/README.md) | C ABI and C++20 wrapper for embedding Petal. See [embedding-c.md](docs/embedding-c.md) |

### Apps

An "app" is a full runnable application that loads Petal source and does something with it.

  | App | Built with | Description |
  |-----|----------|-------------|
  | [Garden](garden/README.md) | petal-ui | IDE and text editor built for Petal |
  | [diagram-canvas](examples/custom-apps/diagram-canvas/README.md) | petal-web-canvas | Diagram visualization with a live source editor |
  | [petal-fps](examples/custom-apps/petal-fps/README.md) | petal-desktop-sdl | Rust + Petal 3D first-person experiment with a software rasterizer |
  | [petal-fantasy-nes](examples/custom-apps/petal-fantasy-nes/README.md) | petal-desktop-sdl | NES-style fantasy console driven by Petal carts |
  | [side-scroller](examples/custom-apps/side-scroller/README.md) | petal-desktop-sdl | 2D platformer written almost entirely in Petal |

## License

Petal is released under the [MIT License](LICENSE).

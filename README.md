# Petal

Petal is a programming language for creative coding.

### Main features

 - **Programs are dataflow graphs.** Every value knows where it came from, so tools can inspect, trace, and modify a running program.
 - **First-class state.** `state` declares a value that persists across runs, keyed by where in the program it lives.
 - **Live editing.** Change the source while a program runs and keep its state.
 - **Speculative execution.** Re-run a program safely in exploration modes.
 - **Differentiable.** Back-propagation is built in, so a program can be adjusted from observed outputs.
 - **Goal-based editing.** Tools can rewrite source programmatically by stating what a value should become.
 - **Optional type annotations.** Annotate bindings, parameters, and return types where you want them. A shallow checker reports mismatches as warnings and never blocks the run.
 - **Classes as named records.** `class Rect … end` names a record shape and gives it a constructor. `fn Rect.center_x(r: Rect)` declares a method on it. Instances are still plain records: no inheritance, no `self`.
 - **Mutation is opt-in and visible.** `let` bindings are dataflow edges. When you need a mutable slot you declare it with `var` and every write says `set`.
 - A hybrid functional/imperative style with immutable values and other conveniences for quick iteration.

### Project status

This project is in an early, experimental phase. Language stability is not guaranteed.

### Related work

Projects and research on the same topics:

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
rust/target/debug/petal run -e 'print("hello, world!")'

# Run an example
rust/target/debug/petal run examples/console/fizzbuzz.ptl
```

For the full list of developer commands, see [Developer Scripts & Commands](docs/dev/scripts.md).

## Repository layout

| Directory | Description |
|-----------|-------------|
| [`rust/`](rust/) | The language implementation: lexer, parser, compiler, IR, evaluator, bytecode VM |
| [`docs/`](docs/README.md) | Documentation for using Petal, and [`docs/dev/`](docs/dev/) for working on it |
| [`examples/`](examples/README.md) | Runnable examples: console demos, panel apps (games, productivity, dashboards), and custom host integrations |
| [`petal-ui/`](petal-ui/README.md) | The UI layer shared by every host: input events, draw commands, and the `ui` prelude module |
| [`petal-query/`](petal-query/README.md) | Async data layer for UI panels: `query(kind, arg)` handlers with a host-side cache |
| [`petal-libs/`](petal-libs/README.md) | Shared libraries written in Petal itself — [`bloom`](petal-libs/bloom/), the UI component library |
| [`integrations/`](integrations/) | Hosts that embed Petal for a platform: desktop SDL, web HTML, web canvas |
| [`garden/`](garden/README.md) | Garden, a text editor and IDE scripted with Petal |
| [`editor-support/`](editor-support/README.md) | Tree-sitter grammar and Vim syntax files |
| [`ts/`](ts/) | TypeScript tooling: dev scripts, MCP servers, and the vitest integration suite |
| [`test/`](test/README.md) | Test corpora, golden files, and benchmarks |
| [`dist/`](dist/) | The `install.sh` and `uninstall.sh` scripts hosted at petal-lang.org |

## Documentation

| Document | Description |
|----------|-------------|
| [Getting Started](docs/Getting_Started.md) | Install or build, run your first program, find the examples |
| [Language Guide](docs/language-guide.md) | The full language reference |
| [Builtins Reference](docs/Builtins.md) | Every built-in function |
| [CLI Reference](docs/CLI.md) | Every `petal` subcommand and flag |
| [Module System](docs/module-system.md) | `import` and module resolution |
| [Architecture](docs/dev/Architecture.md) | How the implementation works |
| [Goals](docs/dev/goals.md) | The vision and the remaining work |

The full index is in [docs/README.md](docs/README.md).

## How the pieces fit

```
Petal Core  →  Integrations  →  Apps
```

- **Petal Core** is the language ([`rust/`](rust/)) plus the shared UI layer ([`petal-ui/`](petal-ui/)) and data layer ([`petal-query/`](petal-query/README.md)).
- **Integrations** ([`integrations/`](integrations/)) embed the core for one platform:

  | Integration | Description |
  |-------------|-------------|
  | [petal-desktop-sdl](integrations/petal-desktop-sdl/README.md) | SDL2 desktop host with hot reload. See the [game dev guide](integrations/petal-desktop-sdl/docs/game-dev-guide.md) and [agent protocol](integrations/petal-desktop-sdl/docs/agent-protocol.md) |
  | [petal-web-html](integrations/petal-web-html/README.md) | WebAssembly host that renders DOM from JSX-like syntax |
  | [petal-web-canvas](integrations/petal-web-canvas/README.md) | WebAssembly host that renders to an HTML canvas |

- **Apps** ([`examples/`](examples/README.md)) build on an integration rather than on the core directly. Some ship their own host:

  | App | Built on | Description |
  |-----|----------|-------------|
  | [diagram-canvas](examples/custom-integrations/diagram-canvas/README.md) | petal-web-canvas | Diagram visualization with a live source editor |
  | [petal-fps](examples/custom-integrations/petal-fps/README.md) | petal-desktop-sdl | Rust + Petal 3D first-person experiment with a software rasterizer |
  | [petal-fantasy-nes](examples/custom-integrations/petal-fantasy-nes/README.md) | petal-desktop-sdl | NES-style fantasy console driven by Petal carts |
  | [side-scroller](examples/games/side-scroller/README.md) | petal-desktop-sdl | 2D platformer written almost entirely in Petal |

## License

Petal is released under the [MIT License](LICENSE).

# Petal Documentation

## Using the language

Start here if you are learning or writing Petal:

| Document | Description |
|----------|-------------|
| [Getting Started](Getting_Started.md) | Install or build the CLI, run your first program, find the examples |
| [Writing Petal](writing-petal-guide.md) | A short guide for programmers new to Petal: how programs are shaped, what is different, and how to use the tooling |
| [Language Guide](language-guide.md) | The full language reference: values, syntax, control flow, functions, classes, state, type annotations |
| [Syntax Overview](syntax/overview.md) | A compact map of every lexical form, statement, expression, and operator |
| [Builtins Reference](Builtins.md) | Every built-in function, with signatures and examples |
| [CLI Reference](CLI.md) | Every `petal` subcommand and flag, plus the JSON output formats |
| [Module System](module-system.md) | `import`, how modules are found, and hot reload across files |
| [Function Overloading](function-overloading.md) | How calls pick between functions and methods with the same name |
| [Text & Fonts](text-and-fonts.md) | Drawing text in UI apps: faces, sizes, metrics, measurement |
| [Rebind Operator](syntax/rebind-operator.md) | The `@` in-out argument operator: `f(@x)` means `x = f(x)` |
| [Commas](syntax/commas.md) | Where commas are required, and how a leading `-` is read |
| [Line Continuation](syntax/line-continuation.md) | Breaking a long expression across lines |
| [Examples](examples/README.md) | Code snippets used by the docs. Runnable demo programs live in [`../examples/`](../examples/README.md) |

## Embedding and tooling

How to build on Petal from a host program, a tool, or an agent:

| Document | Description |
|----------|-------------|
| [Building Apps](building-apps.md) | The three ways to build an app: a pure Petal script, extending an integration, or embedding a new host |
| [FFI / Embedding](ffi.md) | Embedding Petal in a Rust host: native functions, values, host channels |
| [Embedding Guide](embedding-guide.md) | Patterns for hosts: observing calls, reading named values, feeding inputs, per-run ids |
| [Program Modification](program-modification.md) | Editing programs from code, both source text and running state |
| [Goal-Based Editing](goal-based-editing.md) | Formatting-preserving source edits described as goals |
| [Direct Manipulation](direct-manipulation.md) | Tracing a drawn shape or emitted value back to the call that produced it, then editing that call |
| [Config Files](config-files.md) | Using a `.ptl` file as an app's configuration: read values without running, write them back |
| [`var` / mutable cells](var.md) | How `var`, `set`, and `get` work, and where mutation shows up in the dataflow graph |

## Internals and design

| Document | Description |
|----------|-------------|
| [Architecture](dev/Architecture.md) | How the implementation works: IR term graph, evaluator, state, provenance |
| [Goals](dev/goals.md) | The vision and the remaining unfinished work |
| [IR as a Target](dev/ir-as-target.md) | The IR import format for external emitters (`run --ir`) |
| [Call-path keyed `state`](dev/state-call-paths.md) | The rules for how `state` slots are keyed, and the decisions behind them |
| [Debugging & Visibility](dev/debugging-visibility.md) | The three ways to look inside a running program: CLI, MCP, and tests |
| [Debug Protocol](dev/debug-protocol.md) | The JSON command protocol shared by petal-sdl and diagram-canvas |

## Working on Petal ([dev/](dev/))

Contributor guides first, then plans and design records. The plans are
working documents and may contain point-in-time status.

| Document | Description |
|----------|-------------|
| [Developer Scripts & Commands](dev/scripts.md) | Build, run, test, and benchmark commands |
| [Testing](dev/testing.md) | How to write and run the test suites |
| [MCP Server](dev/mcp-server.md) | Using the MCP tools to compile, run, and inspect snippets |
| [Performance](dev/performance.md) | Profiling tools, what the optimizer does, where the headroom is |
| [Headless UI Runner](dev/headless-ui-run.md) | `petal-ui-run`: driving a UI app without a window and recording a frame trace |
| [Sharing Petal Libraries](dev/sharing-petal-libraries.md) | What writing a pure-Petal library (`petal-libs/bloom`) needs from the language and from hosts |
| [Refactor Verification](dev/refactor-verification.md) | Proving a large mechanical change was behavior-preserving |
| [Releasing](dev/releasing.md) | How prebuilt `petal` binaries are built, published, and installed |
| [Releasing Garden](dev/releasing-garden.md) | How the Garden Homebrew formula is updated |
| [Type Declarations](dev/type-declarations-plan.md) | Design record for optional, warning-only type annotations (shipped) |
| [Typography](dev/typography-plan.md) | Design for fonts, measurement, and flow layout (partly shipped) |
| [Linter](dev/linter-plan.md) | `petal lint` design and the catalogue of possible further rules (shipped) |
| [Pending Values](dev/pending-values-plan.md) | Async pending-value semantics and the petal-query data layer |
| [IR Format Improvements](dev/ir-format-improvements-plan.md) | Audit of the token, AST, IR, and bytecode dumps, with ranked improvements |
| [Experimental: IR-based Editing](dev/experimental-ir-based-editing.md) | An early, unfinished surface for building a program as IR data |
| [Testbed Challenge](dev/testbed-challenge-plan.md) | The target list of panel apps for the Garden testbed, and which are built |

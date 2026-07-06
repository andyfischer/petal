# Petal Documentation

## User reference

Start here if you're learning or using the language:

| Document | Description |
|----------|-------------|
| [Getting Started](Getting_Started.md) | Build instructions, running examples, CLI usage |
| [Language Guide](Language_Guide.md) | Complete language reference: types, syntax, control flow, functions, state |
| [Builtins Reference](Builtins.md) | All built-in functions with signatures and examples |
| [CLI Reference](CLI.md) | Full CLI command reference and JSON output schemas |
| [Module System](module-system.md) | `import` syntax, module resolution, hot reload across files |
| [Function Overloading](Function_Overloading.md) | Multi-arity dispatch rules |
| [Rebind Operator](rebind-operator.md) | The `@` in-out argument operator (`f(@x)` ≡ `x = f(x)`) |
| [Optional Commas](syntax/optional-commas.md) | Comma-less lists and call arguments |
| [Examples](examples/README.md) | Documentation code snippets (runnable demos live in [`../examples/`](../examples/README.md)) |

## Design & internals

How the implementation works and where it's headed:

| Document | Description |
|----------|-------------|
| [Architecture](dev/Architecture.md) | Internal design: IR term graph, evaluator, state, provenance |
| [Goals](dev/goals.md) | Vision (the four pillars), remaining work, and sequencing |
| [IR as a Target](dev/ir-as-target.md) | The IR import-format contract for external emitters (`run --ir`) |
| [Debugging & Visibility](dev/debugging-visibility.md) | The three observability stacks (CLI, MCP, vitest) |
| [Debug Protocol](dev/debug-protocol.md) | JSON command/response schema shared by petal-sdl and petal-diagram-canvas |

## Internal dev notes ([dev/](dev/))

Engineering logs, migration plans, and contributor-facing docs. These are
working documents — expect internal shorthand and point-in-time status:

| Document | Description |
|----------|-------------|
| [Developer Scripts & Commands](dev/scripts.md) | Build, run, test, and benchmark commands for development |
| [Testing](dev/testing.md) | How to write and run the test suites |
| [MCP Server](dev/mcp-server.md) | Using the MCP tools to introspect Petal programs |
| [Bytecode Future Ideas](dev/bytecode-future-ideas.md) | Open follow-ups for the bytecode backend (the backend itself is complete) |
| [Speculative Execution Plan](dev/speculative-execution-plan.md) | Plan/log for speculative execution and immutable-heap work |

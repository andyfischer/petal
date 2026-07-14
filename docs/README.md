# Petal Documentation

## User reference

Start here if you're learning or using the language:

| Document | Description |
|----------|-------------|
| [Getting Started](Getting_Started.md) | Build instructions, running examples, CLI usage |
| [Language Guide](Language_Guide.md) | Complete language reference: types, syntax, control flow, functions, state |
| [Syntax Overview](syntax/overview.md) | Compact map of all lexical forms, statements, expressions, and operators |
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
| [FFI / Embedding](ffi.md) | Embedding Petal in a Rust host: natives, values, host channels |
| [Embedding Guide](embedding-guide.md) | Patterns for embedding without host globals: observing function calls, feeding inputs, per-run ids |
| [Building on Integrations](building-on-integrations.md) | Building your own app on Petal: pure-Petal scripts, extending an integration, or embedding a new host |
| [Program Modification](program-modification.md) | Modifying programs programmatically (source, IR, live) — for tools, agents, and embedders |
| [Goal-Based Editing](goal-based-editing.md) | Declarative, formatting-preserving source edits via `Goal`/`modify_source_with_goals` |
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
| [Linter Plan](dev/linter-plan.md) | `petal lint` design; first slice shipped, normalization catalogue remains |
| [Pending Values Plan](dev/pending-values-plan.md) | Async/pending-value semantics; language+observability shipped, petal-query remains |
| [Refactor-Verification Plan](dev/refactor-verification-plan.md) | Proposal for tooling that verifies refactors are behavior-preserving |
| [Unreal FFI Proposal](dev/unreal-ffi-proposal.md) | Game-engine handle FFI (M1 in progress) |

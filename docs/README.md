# Petal Documentation

## User reference

Start here if you're learning or using the language:

| Document | Description |
|----------|-------------|
| [Getting Started](Getting_Started.md) | Build instructions, running examples, CLI usage |
| [Writing Petal](writing-petal-guide.md) | Intro guide for programmers new to Petal: program shape, the rules that differ, and how to use the tooling |
| [Language Guide](language-guide.md) | Complete language reference: types and optional type annotations, syntax, control flow, functions, classes & methods, state |
| [Syntax Overview](syntax/overview.md) | Compact map of all lexical forms, statements, expressions, and operators |
| [Builtins Reference](Builtins.md) | All built-in functions with signatures and examples |
| [CLI Reference](CLI.md) | Full CLI command reference and JSON output schemas |
| [Module System](module-system.md) | `import` syntax, module resolution, hot reload across files |
| [Function Overloading](function-overloading.md) | Multi-arity dispatch rules, for functions and methods alike |
| [Text & Fonts](text-and-fonts.md) | Drawing text: faces, sizes, metrics, and measurement |
| [Rebind Operator](syntax/rebind-operator.md) | The `@` in-out argument operator (`f(@x)` ≡ `x = f(x)`) |
| [Commas](syntax/commas.md) | Where commas are required, and how `-` is disambiguated |
| [Line Continuation](syntax/line-continuation.md) | Breaking a long expression across lines |
| [Examples](examples/README.md) | Documentation code snippets (runnable demos live in [`../examples/`](../examples/README.md)) |

## Design & internals

How the implementation works and where it's headed:

| Document | Description |
|----------|-------------|
| [Architecture](dev/Architecture.md) | Internal design: IR term graph, evaluator, state, provenance |
| [`var` / mutable cells](var.md) | How `var`, `set` and `get` work: cells, containment, the write/read keyword rules, and the provenance frontier |
| [FFI / Embedding](ffi.md) | Embedding Petal in a Rust host: natives, values, host channels |
| [Embedding Guide](embedding-guide.md) | Patterns for embedding without host globals: observing function calls, reading arbitrary named values, feeding inputs, per-run ids |
| [Direct Manipulation](direct-manipulation.md) | Tracing an emitted value back to the code that produced it: point at a drawn shape, find (and rewrite) its call |
| [Building Apps](building-apps.md) | Building your own app on Petal: pure-Petal scripts, extending an integration, or embedding a new host |
| [Program Modification](program-modification.md) | Modifying programs programmatically — static (source) and live (running-state) editing — for tools, agents, and embedders |
| [Goal-Based Editing](goal-based-editing.md) | Declarative, formatting-preserving source edits via `Goal`/`modify_source_with_goals` |
| [Config Files](config-files.md) | Using a `.ptl` file as an app's configuration format: reading values without running, writing them back |
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
| [Releasing](dev/releasing.md) | How prebuilt `petal` binaries are built, published, and installed |
| [Type Declarations Plan](dev/type-declarations-plan.md) | Design rationale for optional, warning-only type annotations (shipped) |
| [Type Declarations Progress](dev/type-declarations-progress.md) | Chunk-by-chunk status board for the annotation work, through class names as types |
| [Typography Plan](dev/typography-plan.md) | `petal-typography` design: faces, measurement, flow layout |
| [Typography Progress](dev/typography-progress.md) | Status tracker for the typography phases |
| [Bytecode Future Ideas](dev/bytecode-future-ideas.md) | Open follow-ups for the bytecode backend (the backend itself is complete) |
| [Linter Plan](dev/linter-plan.md) | `petal lint` / `lint-fix` design; re-indent + identity-cast rules shipped, normalization catalogue remains |
| [Pending Values Plan](dev/pending-values-plan.md) | Async/pending-value semantics; language+observability shipped, petal-query remains |
| [Refactor-Verification Plan](dev/refactor-verification-plan.md) | Proposal for tooling that verifies refactors are behavior-preserving |
| [Experimental: IR-based Editing](dev/experimental-ir-based-editing.md) | Early, unfinished surface for constructing/transforming a program as IR data |
| [Unreal FFI Proposal](dev/unreal-ffi-proposal.md) | Game-engine handle FFI (M1 in progress) |
| [Testbed Challenge Plan](dev/testbed-challenge-plan.md) | The original 50 target apps for the Garden panel-app testbed; 15 built, 35 remaining |

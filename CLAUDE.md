# Petal

A custom programming language: Lexer → Parser → AST → Compiler → IR (term graph) → Backend. Two backends consume the IR: the **graph** step evaluator (reference) and a **bytecode** register VM (in progress — see [docs/dev/bytecode-status.md](./docs/dev/bytecode-status.md)).

## Repo Layout

- `rust/` — Main implementation for the core language (lexer, parser, AST, compiler, IR, evaluator)
- `petal-ui/` — Standard interactivity layer for embedders: normalized input
  events + edge/level semantics (`InputState`), the shared draw-command
  vocabulary, the Petal-source `ui` prelude module (widgets: `button`,
  `list_update`, …), and a headless test harness. Used by petal-sdl and by
  external embedders.
- `apps/` — Native and web integrations that embed the language:
  - `apps/petal-sdl/` — SDL-based native app that integrates the language into a graphical environment
  - `apps/petal-diagram-canvas/` — Experimental integration using Petal for a web based diagram tool.
  - `apps/petal-web/` — Integration that uses Petal + WASM as a React-like rendering layer.
- `ts/` — TypeScript tooling
  - `ts/bin/` — Dev wrappers (`run-petal.ts`, `test-examples.ts`)
  - `ts/tools/` — MCP servers
  - `ts/test/` — Vitest integration tests
- `editor-support/` — Tooling for editors/IDEs:
  - `editor-support/tree-sitter-petal/` — the reference tree-sitter grammar for
    Petal (syntax highlighting). Ships `grammar.js`, a committed generated
    parser, `queries/highlights.scm`, and a Rust crate (`LANGUAGE` +
    `HIGHLIGHTS_QUERY`).
- `examples/` — Example `.ptl` programs
- `docs/` — Documentation

Source: `rust/src/`

## Build & test

    cd rust && cargo build            # build the petal CLI binary
    cd rust && cargo test             # Rust unit tests
    cd ts && npx vitest               # integration tests (shell out to the compiled binary)
    ts/bin/run-petal.ts <file.ptl>    # build (if needed) + run a Petal program

Details: [docs/dev/testing.md](./docs/dev/testing.md)

## Documentation

 * How to write and run the test suite: [docs/dev/testing.md](./docs/dev/testing.md)
 * Using the MCP server to introspect: [docs/dev/mcp-server.md](./docs/dev/mcp-server.md)
 * Bytecode backend design/status/handoff: [docs/dev/bytecode-status.md](./docs/dev/bytecode-status.md)
 * Module / import system (`import`, resolution, hot reload): [docs/module-system.md](./docs/module-system.md)


# Petal

A custom programming language: Lexer → Parser → AST → Compiler → IR → Step Evaluator.

## Repo Layout

- `rust/` — Main implementation for the core language (lexer, parser, AST, compiler, IR, evaluator)
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
    `HIGHLIGHTS_QUERY`). Consumed by Garden (`~/garden`).
- `examples/` — Example `.ptl` programs
- `docs/` — Documentation

Source: `rust/src/`

## Documentation

 * How to write and run the test suite: [docs/dev/testing.md](./docs/dev/testing.md)
 * Using the MCP server to introspect: [docs/dev/mcp-server.md](./docs/dev/mcp-server.md)


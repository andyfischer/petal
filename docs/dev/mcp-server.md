# MCP Server

`ts/tools/petal-mcp.ts` is an MCP server (`petal-tools`) whose tools compile
and run Petal code directly, so an agent can test a snippet without shelling
out. It builds the Rust binary on first use. Each call has a 10 s timeout.
Every tool writes the snippet to a temp `.ptl` file and runs the matching
`petal` subcommand on it (`runPetalCommand(args, code)`).

| Tool | Purpose |
|------|---------|
| `TestSnippet({code, trace?})` | Run a snippet; returns stdout, stderr, exit code. Non-fatal type-checker warnings appear on stderr. `trace: true` adds a per-term execution trace. |
| `CheckSnippet({code})` | Lex+parse+compile without running. Returns `{ok: true, warnings: [...]}` (each warning `{message, line, column, file}`) or a structured error. Cheaper than `TestSnippet` for validating syntax and type annotations. |
| `ExplainTerm({code, term})` | Run with tracing, then walk the dataflow graph backward from `term` to answer "why does X have value Y?". |
| `ShowStage({code, stage, json?, all?})` | Dump one compilation stage: `stage` is `"tokens"`, `"ast"`, `"ir"`, or `"bytecode"`. JSON by default; `json: false` returns the CLI's text form. For `ir` the default is the user-only view: builtin phantom terms, the std prelude, and imported-module internals are filtered out (ids preserved; in JSON `constants.values` becomes an id-keyed object; not loadable by `run --ir`). `all: true` returns the complete Program, which in JSON is the `run --ir` interchange format. |
| `PendingReport({code})` | Run the code and return the pending report as JSON: every live pending resource with its state, age, origin, and this-frame absorption count. Debugs "why is this region blank". |
| `TraceEmits({code})` | Run with emit tracing; per output channel, every emitted value with the call that produced it and per-argument edit info. The observation half of direct manipulation (see [direct-manipulation.md](../direct-manipulation.md)). |
| `ProposeEdit({code, channel, emit, arg?, to?, goals?, configurable?, static?})` | Propose source edits that make argument `arg` of the call behind emit `emit` on `channel` evaluate to `to`. Pass `goals: [{arg, to}, ...]` for a multi-goal batch resolved consistently. Several proposals come back when several variables feed the value; narrow with `configurable`/`static`, or declare knobs in-source with `config let`. |

`ShowStage` returns exactly what the CLI's dumps emit (`show-tokens` /
`show-ast` / `show-ir` / `show-bytecode`, with `--json` unless `json: false`).
The CLI keeps the four separate subcommands; the MCP surface folds them into
one tool so an agent's tool list stays short. Span encoding, id
conventions, and the omit-defaults rule are documented in
[CLI.md's Dump format conventions](../CLI.md#dump-format-conventions).

```
TestSnippet({ code: 'print("hello")' })
```

A separate server, `ts/tools/petal-diagram-mcp.ts`, exposes `Diagram*` tools
that speak the debug protocol to a running diagram canvas over WebSocket. See
[debug-protocol.md](debug-protocol.md) and
[debugging-visibility.md](debugging-visibility.md).

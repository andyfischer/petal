
## MCP Server

An MCP server (`ts/tools/petal-mcp.ts`) exposes tools that compile and run Petal code
directly. It automatically builds the Rust binary before running. Use these to
quickly test Petal snippets without shelling out manually.

| Tool | Purpose |
|------|---------|
| `TestSnippet({code, trace?})` | Run a snippet; returns stdout, stderr, exit code. Non-fatal type-checker warnings appear on stderr. `trace: true` adds a per-term execution trace. |
| `CheckSnippet({code})` | Lex+parse+compile without running. Returns `{ok: true, warnings: [...]}` (each warning `{message, line, column, file}`) or a structured error. Warnings are non-fatal. Cheaper than `TestSnippet` for validating syntax and type annotations. |
| `ExplainTerm({code, term})` | Run with tracing, then walk the dataflow graph backward from `term` to answer "why does X have value Y?". |
| `ShowIR({code})` | Return the compiled IR as JSON. |
| `ShowBytecode({code})` | Return the bytecode lowering of the IR as JSON. |
| `ShowAST({code})` | Return the parsed AST as JSON. |
| `ShowTokens({code})` | Return the token stream as JSON. |
| `PendingReport({code})` | Run the code and return the frame pending report as JSON: every live pending resource with its state, age, origin, and this-frame absorption count. Debug "why is this region blank". |
| `TraceEmits({code})` | Run with emit tracing; per output channel, every emitted value with the call that produced it and per-argument edit info. The observation half of direct manipulation (see docs/direct-manipulation.md). |
| `ProposeEdit({code, channel, emit, arg?, to?, goals?, configurable?, static?})` | Goal-based edit query: propose source edits that make argument `arg` of the call behind emit `emit` on `channel` evaluate to `to`. Pass `goals: [{arg, to}, …]` instead for a multi-goal batch resolved consistently. Multiple proposals when several variables feed the value; narrow with `configurable`/`static`, or declare knobs in-source with `config let`. |

```
TestSnippet({ code: 'print("hello")' })
```

petal-diagram-canvas exposes a separate MCP server (`ts/tools/petal-diagram-mcp.ts`) with
`Diagram*` tools that speak the debug protocol over WebSocket — see
`docs/dev/debug-protocol.md`.


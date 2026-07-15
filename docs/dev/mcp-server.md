
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

```
TestSnippet({ code: 'print("hello")' })
```

petal-diagram-canvas exposes a separate MCP server (`ts/tools/petal-diagram-mcp.ts`) with
`Diagram*` tools that speak the debug protocol over WebSocket — see
`docs/dev/debug-protocol.md`.


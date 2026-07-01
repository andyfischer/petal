
## MCP Server

An MCP server (`ts/tools/petal-mcp.ts`) exposes six tools that compile and run Petal code
directly. It automatically builds the Rust binary before running. Use these to
quickly test Petal snippets without shelling out manually.

| Tool | Purpose |
|------|---------|
| `TestSnippet({code, trace?})` | Run a snippet; returns stdout, stderr, exit code. `trace: true` adds a per-term execution trace. |
| `CheckSnippet({code})` | Lex+parse+compile without running. Cheaper than `TestSnippet` for syntax validation. |
| `ExplainTerm({code, term})` | Run with tracing, then walk the dataflow graph backward from `term` to answer "why does X have value Y?". |
| `ShowIR({code})` | Return the compiled IR as JSON. |
| `ShowBytecode({code})` | Return the bytecode lowering of the IR as JSON. |
| `ShowAST({code})` | Return the parsed AST as JSON. |
| `ShowTokens({code})` | Return the token stream as JSON. |

```
TestSnippet({ code: 'print("hello")' })
```

petal-diagram-canvas exposes a separate MCP server (`ts/tools/petal-diagram-mcp.ts`) with
`Diagram*` tools that speak the debug protocol over WebSocket — see
`docs/debug-protocol.md`.


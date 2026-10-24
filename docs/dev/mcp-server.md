# MCP Server

`tools/petal-mcp.ts` is an MCP server (`petal-tools`) whose tools compile
and run Petal code directly, so an agent can test a snippet without shelling
out. It builds the Rust binary on first use. Each call has a 10 s timeout.
Every tool writes the snippet to a temp `.ptl` file and runs the matching
`petal` subcommand on it (`runPetalCommand(args, code)`).

The server imports the MCP SDK and zod from `tools/node_modules`, so run
`npm install` in `tools/` once; without it the server exits at start-up and
the client reports the connection closed.

| Tool | Purpose |
|------|---------|
| `TestSnippet({code, trace?})` | Run a snippet; returns stdout, stderr, exit code. Non-fatal type-checker warnings appear on stderr. `trace: true` adds a per-term execution trace. |
| `CheckSnippet({code})` | Lex+parse+compile without running. Returns `{ok: true, warnings: [...]}` (each warning `{message, line, column, file}`) or a structured error. Cheaper than `TestSnippet` for validating syntax and type annotations. |
| `ExplainTerm({code, term})` | Run with tracing, then walk the dataflow graph backward from `term` to answer "why does X have value Y?". |
| `ShowStage({code, stage, json?, all?})` | Dump one compilation stage: `stage` is `"tokens"`, `"ast"`, `"ir"`, or `"bytecode"`. JSON by default; `json: false` returns the CLI's text form. For `ir` the default is the user-only view: builtin phantom terms, the std prelude, and imported-module internals are filtered out (ids preserved; in JSON `constants.values` becomes an id-keyed object; not loadable by `run --ir`). `all: true` returns the complete Program, which in JSON is the `run --ir` interchange format. |
| `PendingReport({code})` | Run the code and return the pending report as JSON: every live pending resource with its state, age, origin, and this-frame absorption count. Debugs "why is this region blank". |
| `TraceEmits({code, seed?})` | Run with emit tracing; per output channel, every emitted value with the call that produced it, per-argument edit info, the emitting native's raw arguments (`native_args`) and the emit's `key`. The observation half of direct manipulation (see [direct-manipulation.md](../features/direct-manipulation.md)). Pass the `seed` you give `ProposeEdit` when the script draws random numbers. |
| `ProposeEdit({code, channel, emit, engine?, arg?, nativeArg?, value?, field?, to?, by?, goals?, allow?, deny?, configurable?, static?, configStrict?, alternatives?, explain?, tol?, maxRuns?, seed?})` | Propose source edits that put an emitted value at a goal. The default engine, `"v2"`, re-runs the program speculatively and solves through function calls; `engine: "v1"` (legacy, only when passed) inverts the call's own arithmetic and accepts only `arg` + `to` goals on one emit (`configurable` / `static` are its filters; under v2 they mean `allow` / `deny`). `goals: [{channel?, emit?, arg? \| nativeArg? \| value?, field?, to? \| by?}, ...]` states several goals at once. The result is the CLI's `propose-edit --json` output, unchanged (see below). |

`ProposeEdit` builds its argv in `tools/petal-mcp-propose.ts`, which has no
MCP dependency so `test/vitest/petal-mcp.test.ts` can drive it against the
built binary. Its v2 result is the object documented under
[CLI.md's `propose-edit`](../CLI.md#propose-edit--propose-source-edits-that-change-an-emitted-value):
`alternatives[]` (ranked; each with `verified`, `description`, `changes`
(per literal: `old_text`, `new_text`, `binding`, `function`, `line`), `edits`
(file, span, `new_text`), `achieved`, `misses`, `max_error`, `collateral`
and `score`), `candidates[]` (every literal considered: why it was excluded,
its measured sensitivity `column`, its cost `weight`), `diagnostics` and,
with `explain`, the pipeline log. The v1 result (`engine: "v1"`) keeps its
old `{goals, proposals, ...}` shape. Until v2 became the default, a call with
only `arg` + `to` (and `configurable` / `static`) got v1 and that shape; it
now gets v2, so a client that reads `proposals` must pass `engine: "v1"`.

An agent's loop for a change request ("move the third circle right by 25"):
`TraceEmits` to find the channel and emit, `ProposeEdit` with
the goal and `explain: true`; if the top alternative moves the wrong thing,
ask again with `allow` naming the literals to move (`"gap"`, `"fn:place"`,
`"line:12"`, `"config"`) or `deny`; then apply a verified alternative's
`edits`.

`ShowStage` returns exactly what the CLI's dumps emit (`show-tokens` /
`show-ast` / `show-ir` / `show-bytecode`, with `--json` unless `json: false`).
The CLI keeps the four separate subcommands; the MCP surface folds them into
one tool so an agent's tool list stays short. Span encoding, id
conventions, and the omit-defaults rule are documented in
[CLI.md's Dump format conventions](../CLI.md#dump-format-conventions).

```
TestSnippet({ code: 'print("hello")' })
```

A separate server, `tools/petal-diagram-mcp.ts`, exposes `Diagram*` tools
that speak the debug protocol to a running diagram canvas over WebSocket. See
[debug-protocol.md](debug-protocol.md) and
[debugging-visibility.md](debugging-visibility.md).

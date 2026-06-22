# Debugging & Agent Visibility

How LLM agents (and humans) can test, validate, and "see" Petal programs.

Petal has **three parallel observability stacks** — CLI (Rust), MCP tools, and
test helpers — all converging on the same IR / draw-command / protocol outputs.
Pick based on execution context (local CLI, running diagram server, vitest).

## Philosophy: values, not state

Petal is a pure dataflow language. Programs are DAGs of terms; each term has
fixed inputs, an operation, and a result that never changes once computed.
Variable names are labels attached to terms — "reassigning" `x` in source
creates a new term and reattaches the label. There is no mutation.

Debugging questions therefore reduce to graph questions. "Why does `total`
have this value?" means "walk provenance backward from the term currently
labelled `total`." `petal explain`, `show-provenance`, `show-dependents`, and
the trace buffer all work off this model.

Rebindings inside a child block lower to pure-dataflow joins: a single
`Phi` op, placed in the parent block *before* its associated control-flow
term (`Branch`, `Match`, `ForLoop`, `WhileLoop`). Each phi initializes from
its `inputs[0]` (the pre-control-flow value) and is updated on child-frame
pops via `Block.phi_outs`. The IR has no register-mutation primitive — see
`docs/MutabilityPlan.md`.

---

## 1. Core CLI (`rust/src/cli.rs`)

Every command takes `-e <code>` or a file path. Most support `--json`.

| Command | Purpose |
|---------|---------|
| `run [--json] [--trace] [--record-trace <path>] [file\|-e code]` | Execute; `--json` = structured errors; `--trace` = per-term stderr events; `--record-trace` writes a JSON trace file |
| `check [--json] [file\|-e code]` | Lex+parse+compile only, exit 0/1 |
| `explain --term <name\|id> [--json] [file\|-e code]` | Run with trace, show value chain (target + ancestors + recorded values). Accepts either a variable name (`total`), a bare numeric term id (`72`), or the `t`-prefixed form (`t72`). |
| `show-tokens [--json]` | Lexer output |
| `show-ast [--json]` | Parser output |
| `show-ir [--json]` | Compiled IR (terms, ops, inputs, blocks) |
| `show-provenance --term <name\|id> [--json]` | Backward dataflow slice (what does this term depend on?) |
| `show-dependents --term <name\|id> [--json]` | Forward dataflow slice (what depends on this term?) |
| `show-slice --term <a> [--term <b>...] [--json]` | Minimal subgraph for multiple targets |
| `show-graph` | Graphviz DOT output for visualization |

Use `./ts/bin/run-petal.ts` to auto-rebuild the binary before invocation.

---

## 2. MCP Tools

### `petal-tools` (`ts/tools/petal-mcp.ts`)

Auto-rebuilds the Rust binary on first use. 10s timeout per call.

| Tool | Input | Output |
|------|-------|--------|
| `TestSnippet` | `{code, trace?}` | `{stdout, stderr, exit_code}`; `trace: true` adds per-term execution events |
| `CheckSnippet` | `{code}` | `{ok}` or `{error}` — lex+parse+compile only, no run |
| `ExplainTerm` | `{code, term}` | Provenance chain with recorded values for the target term |
| `ShowAST` | `{code}` | JSON AST |
| `ShowIR` | `{code}` | JSON IR (terms, ops, inputs, names) |
| `ShowTokens` | `{code}` | JSON token array |

### `petal-diagram-canvas` — frame-by-frame debugger

Connects to a running canvas via WebSocket (`ws://localhost:4012/debug`,
override with `PETAL_DEBUG_URL`). All responses share the shape
`{ok, paused, frame, ...extras}`.

| Tool | Extras | When to use |
|------|--------|-------------|
| `DiagramPause` | — | Freeze frame loop for inspection |
| `DiagramResume` | — | Resume real-time playback |
| `DiagramStep(n)` | `draw_commands[]` | Advance N frames (fixed `dt = 1/60`) |
| `DiagramState` | `state{}` | Dump all runtime state vars as JSON |
| `DiagramSetState(name, value)` | updated `state{}` | Mutate a state var |
| `DiagramCaptureDrawCommands` | `draw_commands[]` | Speculative run, no side effects |
| `DiagramScreenshot` | `screenshot: data:image/png;base64…`, `file` | PNG saved to `./temp/` |
| `DiagramInput({keys_down, mouse})` | — | Inject keyboard/mouse state |

**DrawCommand shape:**
```json
{ "op": "clear|rect|rect_outline|line|circle|text",
  "r": 0-255, "g": 0-255, "b": 0-255,
  "x": int, "y": int, "w": uint, "h": uint,
  "cx": int, "cy": int, "radius": int,
  "x1": int, "y1": int, "x2": int, "y2": int,
  "text": string, "size": uint }
```

Agents can validate visuals *structurally* (exact draw ops) without pixel diffs.

---

## 3. petal-sdl — Four Run Modes

| Mode | Command | Notes |
|------|---------|-------|
| Interactive | `petal-sdl file.ptl` | GUI only, no agent access |
| Agent | `petal-sdl --agent file.ptl` | SDL window + JSON protocol on stdin/stdout |
| Headless | `petal-sdl --headless file.ptl` | No window, starts paused, agent-driven |
| Screenshot | `petal-sdl --screenshot out.png --frames N file.ptl` | One-shot PNG for CI |

### Agent protocol

**stdin → engine:**
```json
{ "cmd": "pause" }
{ "cmd": "resume" }
{ "cmd": "step", "n": 5 }
{ "cmd": "state" }
{ "cmd": "set_state", "name": "player_x", "value": 100.5 }
{ "cmd": "capture_draw_commands" }
{ "cmd": "input", "keys_down": ["w","a"], "mouse": [400,300] }
{ "cmd": "screenshot" }
```

**engine → stdout:**
```json
{ "ok": true, "paused": false, "frame": 42,
  "state": {...}, "draw_commands": [...],
  "output": ["..."], "screenshot": "data:image/png;base64,..." }
```

Command handlers live in `apps/petal-sdl/src/game_loop.rs` (≈ lines 269–450).
Supports hot reload (`--no-hot-reload` to disable).

---

## 4. Test Infrastructure (`ts/test/helpers.ts`)

Vitest-based. Helpers shell out to the compiled `petal` binary.

| Helper | Returns |
|--------|---------|
| `runPetal(code)` | stdout (trimmed) |
| `runPetalError(code)` | stderr; expects failure |
| `showIrJson(code)` | Parsed IR object |
| `showAstJson(code)` | Parsed AST object |
| `showTokensJson(code)` | Token array |
| `userTerms(ir)` | Terms minus builtin phantoms |
| `termByName(ir, name)` / `termById(ir, id)` | Lookup |
| `termsByOp(ir, op)` | Filter by op |

`ts/test/test-samples.test.ts` sanity-runs every `examples/*.ptl` (3s timeout per file).

---

## 5. In-Language Observability

- `print(...)` — space-joined, to stdout
- `str(x)` / `type(x)` — value inspection
- `assert(cond, msg?)` — aborts with `assertion failed: <msg>` + source location
- `assert_eq(a, b)` — aborts with `assert_eq: left=X right=Y`
- Runtime errors carry `"<msg> [line N, column M]"`, a `Caused by:` block of
  nearest named ancestors from the dataflow graph, and stack traces (see
  `rust/src/eval.rs` `build_stack_trace` / `format_provenance`). In JSON mode
  (`petal run --json`) these surface as `{message, line, column, caused_by[], stack[]}`.
- Structured trace buffer (`rust/src/trace.rs`): records every term execution
  (inputs + result) into a ring buffer (default capacity 200,000 events — oldest
  events are dropped once full). Enable via `--record-trace`, `--trace`, or
  `PETAL_DEBUG=1`. Queryable post-run via `Env::trace().explain(...)` or the
  `petal explain` CLI.
- petal-diagram-canvas parses error line info to highlight source
  (`apps/petal-diagram-canvas/src/runtime.ts`)

### Trace JSON schema (`--record-trace <path>`)

```json
{
  "capacity": 200000,
  "count": 42,
  "events": [
    { "seq": 0, "term_id": 68, "name": "x", "op": "Constant(ConstantId(0))",
      "line": 1, "column": 9, "inputs": [], "result": "10" },
    { "seq": 1, "term_id": 70, "name": null, "op": "Add",
      "line": 2, "column": 9, "inputs": ["10", "2"], "result": "12" }
  ]
}
```

`inputs` and `result` are pretty-printed strings (via
`value::value_to_display_string`), not raw values. `name` is the user-visible
variable name when a term's result was bound to one, or `null` otherwise.
`line`/`column` come from the source map; they're `null` for synthetic terms.

---

## Cheat Sheet: Pick the Right Tool

| Goal | Use |
|------|-----|
| Does this snippet compile+run? | `TestSnippet` or `runPetal()` |
| Inspect compilation stages | `ShowIR` / `show-ast` / `show-provenance` |
| Debug a running canvas | `DiagramPause` → `DiagramStep` → `DiagramState` / `DiagramScreenshot` |
| Automate an SDL program | `petal-sdl --agent` JSON protocol |
| CI visual regression | `petal-sdl --screenshot --frames N` |
| Understand data dependencies | `show-provenance` / `show-dependents` / `show-slice` |
| Unit-test IR shape | `showIrJson` + `termByName` / `termsByOp` |
| Validate without running | `petal check` |
| "Why does this variable have this value?" | `petal explain --term <name>` |
| Post-mortem analysis / offline trace review | `petal run --record-trace trace.json` |

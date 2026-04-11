# Debugging & Agent Visibility

How LLM agents (and humans) can test, validate, and "see" Petal programs.

Petal has **three parallel observability stacks** — CLI (Rust), MCP tools, and
test helpers — all converging on the same IR / draw-command / protocol outputs.
Pick based on execution context (local CLI, running diagram server, vitest).

---

## 1. Core CLI (`rust/src/cli.rs`)

Every command takes `-e <code>` or a file path. Most support `--json`.

| Command | Purpose |
|---------|---------|
| `run [file\|-e code]` | Execute, capture stdout/stderr |
| `show-tokens [--json]` | Lexer output |
| `show-ast [--json]` | Parser output |
| `show-ir [--json]` | Compiled IR (terms, ops, inputs, blocks) |
| `show-provenance --term <name\|id> [--json]` | Backward dataflow slice (what does this term depend on?) |
| `show-dependents --term <name\|id> [--json]` | Forward dataflow slice (what depends on this term?) |
| `show-slice --term <a> [--term <b>...] [--json]` | Minimal subgraph for multiple targets |
| `show-graph` | Graphviz DOT output for visualization |

Use `./bin/run-petal.ts` to auto-rebuild the binary before invocation.

---

## 2. MCP Tools

### `petal-tools` (`tools/petal-mcp.ts`)

Auto-rebuilds the Rust binary on first use. 10s timeout per call.

| Tool | Input | Output |
|------|-------|--------|
| `TestSnippet` | `{code}` | `{stdout, stderr, exit_code}` |
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

Command handlers live in `petal-sdl/src/game_loop.rs` (≈ lines 269–450).
Supports hot reload (`--no-hot-reload` to disable).

---

## 4. Test Infrastructure (`test/helpers.ts`)

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

`test/test-samples.test.ts` sanity-runs every `examples/*.ptl` (3s timeout per file).

---

## 5. Playground HTTP API (`playground/`)

| Endpoint | Body | Returns |
|----------|------|---------|
| `POST /analyze` | `{code}` | `{tokens, ast, ir, run}` each with `{json, error}` |
| `POST /analyze-text` | `{code}` | Same but human-readable text |
| `GET /examples` | — | `[{filename, name, content}]` |

---

## 6. In-Language Observability

- `print(...)` — space-joined, to stdout
- `str(x)` / `type(x)` — value inspection
- Runtime errors carry `"<msg> [line N, column M]"` + stack traces (see
  `rust/src/eval.rs` `build_stack_trace`)
- petal-diagram-canvas parses error line info to highlight source
  (`petal-diagram-canvas/src/runtime.ts`)

**Gaps:** no built-in `assert`, no `PETAL_DEBUG` env var / verbose flag,
no tracing hooks.

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
| Web-based exploration | Playground `POST /analyze` |
| Unit-test IR shape | `showIrJson` + `termByName` / `termsByOp` |

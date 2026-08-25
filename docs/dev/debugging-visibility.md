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
the phi-join discussion in `docs/dev/Architecture.md`.

---

## 1. Core CLI (`rust/src/cli/`)

Every command takes `-e <code>` or a file path. Most support `--json`.

| Command | Purpose |
|---------|---------|
| `run [--json] [--trace] [--record-trace <path>] [file\|-e code]` | Execute; `--json` = structured errors; `--trace` = per-term stderr events; `--record-trace` writes a JSON trace file |
| `run --observe [--json] [file\|-e code]` | Execute, then dump the last value bound to every named variable, function-qualified (`list_row.sel`). Survives a runtime error — see §1b |
| `check [--json] [file\|-e code]` | Lex+parse+compile only, exit 0/1 |
| `explain --term <name\|id> [--json] [file\|-e code]` | Run with trace, show value chain (target + ancestors + recorded values). Accepts either a variable name (`total`), a bare numeric term id (`72`), or the `t`-prefixed form (`t72`). |
| `show-tokens [--json]` | Lexer output |
| `show-ast [--json]` | Parser output |
| `show-ir [--json]` | Compiled IR (terms, ops, inputs, blocks) |
| `show-provenance --term <name\|id> [--json]` | Backward dataflow slice (what does this term depend on?) |
| `show-dependents --term <name\|id> [--json]` | Forward dataflow slice (what depends on this term?) |
| `show-slice --term <a> [--term <b>...] [--json]` | Dataflow subgraph for multiple targets (minimal only when no `var` is read — see §1a) |
| `show-graph` | Graphviz DOT output for visualization |
| `pending-report [--json]` | Report pending/loading resource values after a run (see also `--trace-pending`) |
| `lint [--fix\|--check]` | Formatter / source normalizer |

Use `./ts/bin/run-petal.ts` to auto-rebuild the binary before invocation.

### 1a. Cells stop the walk, and the walk says so

A `var` binds a mutable heap cell, and a cell read has no dataflow edge back
to whatever wrote it — that is a *dynamic* fact. All four dataflow queries
therefore treat the cell operand of a `CellRead`, a `CellWrite` and a closure
capture as an **identity** edge (which box) rather than a **value** edge
(which value), and refuse to cross it.

Refusing silently would be no better than crossing: an unannounced truncation
reads as "nothing further influenced this". So every stop produces a
**frontier** record — the var name, its declaration, and the complete static
set of writes that could have supplied the value — and incompleteness is a
field on the result rather than a convention:

- `show-provenance`: `"frontier": [...]` and `"complete": false`. This command
  never runs the program, so it always degrades to the static answer
  (`"resolution": "not_traced"` plus the write sites).
- `show-slice`: `"minimal": false`, and the slice is widened to include the
  declaration and every `set` site transitively. `SliceResult` has no
  "minimal" flag — a caller picks `minimal()` (fallible) or `conservative()`,
  because too-small computes a *different value* while too-big only loses
  precision.
- `show-dependents`: gains `"kind": "may"` edges from the declaration and from
  every `set` to every read. This direction was always a *may* question, so
  over-approximating is the correct answer, and the previous `Downstream (0)`
  on a `set` was not.
- `explain`: runs the program, so it resolves the boundary to the exact write
  (matched on `CellId`, so one declaration term minting several cells does not
  confuse two of them — `state(key) var`, a `var` in a loop body, or a
  `state var` inside a function, which since call-path keying mints one cell
  per callsite and per loop iteration around it)
  and **continues the chain through it**, valuing everything after the hop as
  of that write's execution. It also lists every recorded write in order, and
  reports `truncated` when `max_depth` cuts the chain short.

Runtime error messages ("Caused by:") share the same walk and print the
frontier for the same reason.

See [`var-next-steps.md`](var-next-steps.md) (Provenance) for the argument.

### 1b. Observation: every named value, right now

`run --observe` answers a different question from everything above. The
dataflow queries and the trace answer *"how did this value come to be"* — a
walk backward through a bounded history. Observation answers *"what is
everything, right now"*: one slot per named IR term, overwritten on every
write, dumped after the run.

```
$ petal run --observe app.ptl
row-0
row-3
row-6

Observed values (6):
  cell             = 6
  counter          = 1
  row_label        = "<function>"
  row_label.prefix = "row-"
  scale            = 3
  total            = 30
```

Three rules make the dump readable:

- **Names are function-qualified.** A top-level `sel` and a `sel` inside
  `fn list_row` are `sel` and `list_row.sel` — two keys, not one silently
  shadowing the other. Nesting composes (`outer.inner.x`); an anonymous
  function contributes `fn<id>`. Only *function* bodies qualify: an `if` arm is
  not a scope a reader would confuse, so a `let` inside a top-level `if` still
  reads as its bare name.
- **Last write wins.** A loop temp reports its final iteration, a
  repeatedly-called function's local its last call. History is the trace
  buffer's job.
- **Absent ≠ null.** A binding whose term never executed is missing from the
  dump. "The `else` arm didn't run" and "the `else` arm bound nil" are
  different facts and stay different.

Which to reach for:

| Question | Tool |
|----------|------|
| "What is everything right now?" | `run --observe` |
| "Why does `total` have *this* value?" | `explain --term total` |
| "What happened, in order?" | `--trace` / `--record-trace` |

Observation is off by default and costs a single bool check when off, so a host
can leave it on for a whole session; the trace buffer records every retired
instruction and is bounded for exactly that reason. `--observe` also works when
the program dies partway — the bindings made before the error are the ones a
user debugging it wants, so they are dumped and *then* the error is reported
(under `--json`, as an `observations` field on the error object).

For an embedder this is the supported way to read an arbitrary named value out
of a script **without the program cooperating** — no `push_output` call added
to a script just so the host can see `body_top`. See
[embedding-guide.md](../embedding-guide.md#reading-arbitrary-named-values-observation)
for `Env::observations_mut().enable()` and `Env::get_observations_json`.

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
| `ShowBytecode` | `{code}` | JSON bytecode lowering of the IR |
| `ShowTokens` | `{code}` | JSON token array |
| `PendingReport` | `{code}` | Pending/loading resource values after a run (wraps `pending-report --json`) |

### `petal-diagram-canvas` — frame-by-frame debugger

Connects to a running canvas via WebSocket (`ws://localhost:4012/debug`,
override with `PETAL_DEBUG_URL`). All responses share the shape
`{ok, paused, frame, ...extras}`.

| Tool | Extras | When to use |
|------|--------|-------------|
| `DiagramPause` | — | Freeze frame loop for inspection |
| `DiagramResume` | — | Resume real-time playback |
| `DiagramStep(n)` | `draw_commands[]` | Advance N frames (fixed `dt = 1/60`) |
| `DiagramState` | `state{}` | Dump all runtime state vars as JSON. Top-level vars key by their bare name; a slot reached through a call path keys by that path, name last (`counter/count`) — see [debug-protocol.md](debug-protocol.md) |
| `DiagramSetState(name, value)` | updated `state{}` | Mutate a **top-level** state var (pathed keys are not addressable) |
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

Command handlers live in `integrations/petal-desktop-sdl/src/game_loop.rs` (≈ lines 269–450).
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

`ts/test/test-samples.test.ts` sanity-runs every `examples/console/*.ptl` (3s timeout per file).

---

## 5. In-Language Observability

- `print(...)` — space-joined, to stdout
- `str(x)` / `type(x)` — value inspection
- `assert(cond, msg?)` — aborts with `assertion failed: <msg>` + source location
- `assert_eq(a, b)` — aborts with `assert_eq: left=X right=Y`
- Runtime errors carry `"<msg> [line N, column M]"`, a `Caused by:` block of
  nearest named ancestors from the dataflow graph, and stack traces (see
  `rust/src/backend/errors.rs` `build_stack_trace` / `format_provenance`). In JSON mode
  (`petal run --json`) these surface as `{message, line, column, caused_by[], stack[]}`.
- Structured trace buffer (`rust/src/trace.rs`): records every term execution
  (inputs + result) into a ring buffer (default capacity 200,000 events — oldest
  events are dropped once full). Enable via `--record-trace`, `--trace`, or
  `PETAL_DEBUG=1`. Queryable post-run via `Env::trace().explain(...)` or the
  `petal explain` CLI.
- petal-diagram-canvas surfaces runtime errors in an on-page error panel
  (`sample-apps/diagram-canvas/src/main.ts`)

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
| "What is every variable right now?" (incl. after an error) | `petal run --observe` |
| Post-mortem analysis / offline trace review | `petal run --record-trace trace.json` |

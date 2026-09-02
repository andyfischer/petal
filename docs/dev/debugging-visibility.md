# Debugging & Agent Visibility

How LLM agents (and humans) can test, validate, and "see" Petal programs.

The same facilities are reachable three ways, depending on where you are:
the `petal` CLI, the MCP tools ([mcp-server.md](mcp-server.md)), and the
vitest helpers ([testing.md](testing.md)). They all produce the same IR,
trace, and draw-command output.

## Philosophy: values, not state

Petal is a dataflow language. A program is a graph of terms; each term has
fixed inputs, an operation, and a result that never changes once computed.
Variable names are labels attached to terms: "reassigning" `x` creates a new
term and moves the label. Rebinding inside a child block (an `if` arm, a loop
body) becomes a `Phi` join in the parent block, not a store. The IR has no
register-mutation primitive; see [Architecture.md](Architecture.md).

Debugging questions therefore become graph questions. "Why does `total` have
this value?" means "walk the dataflow graph backward from the term labelled
`total`." `petal explain`, `show-provenance`, `show-dependents`, and the trace
buffer all work off this model.

## 1. Core CLI

Every command takes a file path or `-e <code>`. Most support `--json`. The
full reference is [CLI.md](../CLI.md); `petal help <command>` prints the
options.

| Command | Purpose |
|---------|---------|
| `run [--json] [--trace] [--record-trace <path>]` | Execute. `--json` gives structured errors; `--trace` prints per-term events to stderr; `--record-trace` writes a JSON trace file. |
| `run --observe` | Execute, then dump the last value bound to every named variable (see §1b). Survives a runtime error. |
| `run --trace-emits` | Attribute every emitted value (draw commands, `push_output`) to the call that produced it. |
| `check [--json] [--strict]` | Lex+parse+compile only. Exit 0/1; `--strict` also fails on type warnings. |
| `explain --term <name\|id>` | Run with tracing, then show the value chain that produced the term. Accepts a variable name (`total`), a numeric term id (`72`), or `t72`. |
| `show-tokens` / `show-ast` / `show-ir` / `show-bytecode` | Each compilation stage, as text or `--json`. |
| `show-provenance --term <t>` | Backward dataflow slice: what does this term depend on? |
| `show-dependents --term <t>` | Forward dataflow slice: what depends on this term? |
| `show-slice --term <a> [--term <b>...]` | Dataflow subgraph for several targets (minimal only when no `var` is read; see §1a). |
| `show-graph` | Graphviz DOT output. |
| `pending-report [--json]` | Every pending (loading) resource still live after a run. |
| `propose-edit` | Propose source edits that change an emitted value (the writing half of direct manipulation). |
| `lint [--fix\|--check]` / `lint-fix` | Formatter and source normalizer. |
| `ir-equal <a> <b>` | Are two files the same program, ignoring layout? |

`./ts/bin/run-petal.ts` rebuilds the binary before running it.

### 1a. Cells stop the walk, and the walk says so

A `var` binds a mutable heap cell, and a cell read has no dataflow edge back
to whatever wrote it; which write it saw is a *dynamic* fact. The four
dataflow queries therefore treat the cell operand of a `CellRead`, a
`CellWrite`, and a closure capture as an identity edge (which box), not a
value edge (which value), and refuse to cross it.

Refusing silently would read as "nothing further influenced this", so every
stop produces a **frontier** record: the var name, its declaration, and the
complete static set of writes that could have supplied the value.
Incompleteness is a field on the result, not a convention:

- `show-provenance`: `"frontier": [...]` and `"complete": false`. This command
  never runs the program, so it always gives the static answer
  (`"resolution": "not_traced"` plus the write sites).
- `show-slice`: `"minimal": false`, and the slice widens to include the
  declaration and every `set` site transitively. Too small computes a
  *different value*; too big only loses precision.
- `show-dependents`: gains `"kind": "may"` edges from the declaration and from
  every `set` to every read. This direction was always a "may" question, so
  over-approximating is the correct answer.
- `explain`: runs the program, so it resolves the boundary to the exact write
  (matched on `CellId`, so one declaration that mints several cells, such as a
  `var` in a loop body, does not confuse two of them) and continues the chain
  through it. It also lists every recorded write in order, and reports
  `truncated` when `max_depth` cuts the chain short.

Runtime error messages ("Caused by:") share the same walk and print the
frontier for the same reason. See [var.md](../var.md) for the argument.

### 1b. Observation: every named value, right now

`run --observe` answers a different question from everything above. The
dataflow queries and the trace answer "how did this value come to be", a walk
backward through a bounded history. Observation answers "what is everything,
right now": one slot per named IR term, overwritten on every write, dumped
after the run.

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
  `fn list_row` are `sel` and `list_row.sel`. Nesting composes
  (`outer.inner.x`); an anonymous function contributes `fn<id>`. Only
  function bodies qualify: a `let` inside a top-level `if` reads as its bare
  name.
- **Last write wins.** A loop temp reports its final iteration; a
  repeatedly-called function's local reports its last call. History is the
  trace buffer's job.
- **Absent is not null.** A binding whose term never executed is missing from
  the dump. "The `else` arm didn't run" and "the `else` arm bound nil" stay
  different facts.

Which to reach for:

| Question | Tool |
|----------|------|
| "What is everything right now?" | `run --observe` |
| "Why does `total` have *this* value?" | `explain --term total` |
| "What happened, in order?" | `--trace` / `--record-trace` |

Observation is off by default and costs one bool check when off, so a host
can leave it on for a whole session. `--observe` also works when the program
dies partway: the bindings made before the error are dumped, and then the
error is reported (under `--json`, as an `observations` field on the error
object).

For an embedder this is the supported way to read a named value out of a
script without the script cooperating. See
[embedding-guide.md](../embedding-guide.md#reading-arbitrary-named-values-observation)
for `Env::observations_mut().enable()` and `Env::get_observations_json`.

## 2. MCP tools

`petal-tools` (`ts/tools/petal-mcp.ts`) wraps the CLI above for agents:
`TestSnippet`, `CheckSnippet`, `ExplainTerm`, `ShowIR`, `ShowBytecode`,
`ShowAST`, `ShowTokens`, `PendingReport`, `TraceEmits`, `ProposeEdit`. The
tool reference is [mcp-server.md](mcp-server.md).

`petal-diagram` (`ts/tools/petal-diagram-mcp.ts`) is a frame-by-frame
debugger for a running diagram canvas. It connects over WebSocket
(`ws://localhost:4012/debug`, override with `PETAL_DEBUG_URL`). Every
response has the shape `{ok, paused, frame, ...extras}`.

| Tool | Extras | Use |
|------|--------|-----|
| `DiagramPause` / `DiagramResume` | — | Freeze or resume the frame loop. |
| `DiagramStep(n)` | `draw_commands[]` | Advance N frames at fixed `dt = 1/60`. |
| `DiagramState` | `state{}` | Dump every runtime `state` variable as JSON. See [debug-protocol.md](debug-protocol.md) for how in-function state is keyed. |
| `DiagramSetState(name, value)` | updated `state{}` | Set a **top-level** state var. |
| `DiagramCaptureDrawCommands` | `draw_commands[]` | Speculative run with no side effects. |
| `DiagramScreenshot` | `screenshot` (PNG data URL), `file` | PNG saved under `./temp/`. |
| `DiagramInput({keys_down, mouse})` | — | Inject keyboard and mouse state. |

Agents can validate visuals structurally, by asserting on the exact draw
commands, without pixel diffs. The command shape is in
[debug-protocol.md](debug-protocol.md#drawcommand).

## 3. petal-sdl run modes

| Mode | Command | Notes |
|------|---------|-------|
| Interactive | `petal-sdl file.ptl` | GUI only, no agent access. |
| Agent | `petal-sdl --agent file.ptl` | SDL window plus the JSON protocol on stdin/stdout. |
| Headless | `petal-sdl --headless file.ptl` | No window, starts paused, agent-driven. |
| Screenshot | `petal-sdl --screenshot out.png --frames N file.ptl` | One-shot PNG for CI. The headless rasterizer has no font, so `draw_text` draws nothing. |

The agent protocol is the same JSON schema the diagram canvas speaks over
WebSocket; it is specified in [debug-protocol.md](debug-protocol.md).
Commands are dispatched in
`integrations/petal-desktop-sdl/src/protocol.rs` (`handle_command`). Hot
reload is on by default (`--no-hot-reload` to disable). Building and
testing petal-sdl needs `LIBRARY_PATH=/opt/homebrew/lib` for the SDL2 linker
on macOS.

## 4. Test helpers

The vitest helpers in `ts/test/helpers.ts` shell out to the compiled binary:
`runPetal`, `runPetalError`, `showIrJson`, `showAstJson`, `showTokensJson`,
`explainJson`, `showProvenanceJson`, and the term lookups `userTerms`,
`termByName`, `termById`, `termsByOp`. See [testing.md](testing.md#helpers-tstesthelpersts).

## 5. In-language observability

- `print(...)` — space-joined, to stdout.
- `str(x)` / `type(x)` — value inspection.
- `assert(cond, msg?)` — aborts with `assertion failed: <msg>` and a source
  location. `assert_eq(a, b)` aborts with `assert_eq: left=X right=Y`.
- Runtime errors carry `"<msg> [line N, column M]"`, a `Caused by:` block of
  the nearest named ancestors in the dataflow graph, and a stack trace (see
  `build_stack_trace` / `format_provenance` in `rust/src/backend/errors.rs`).
  Under `petal run --json` these surface as
  `{message, line, column, caused_by[], stack[]}`.
- The trace buffer (`rust/src/trace.rs`) records every term execution (inputs
  and result) into a ring buffer of 200,000 events; the oldest are dropped
  once it is full. Enable it with `--trace`, `--record-trace`, or
  `PETAL_DEBUG=1`. Query it after the run through `Env::trace().explain(...)`
  or `petal explain`.
- The diagram canvas shows runtime errors in an on-page error panel
  (`examples/custom-integrations/diagram-canvas/src/main.ts`).

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

`inputs` and `result` are display strings (`value::value_to_display_string`),
not raw values. `name` is the variable name when the term's result was bound
to one, otherwise `null`. `line`/`column` come from the source map and are
`null` for synthetic terms.

## Cheat sheet

| Goal | Use |
|------|-----|
| Does this snippet compile and run? | `TestSnippet` or `runPetal()` |
| Validate without running | `petal check` / `CheckSnippet` |
| Inspect a compilation stage | `show-tokens` / `show-ast` / `show-ir` / `show-bytecode` |
| "Why does this variable have this value?" | `petal explain --term <name>` |
| "What is every variable right now?" (including after an error) | `petal run --observe` |
| Understand data dependencies | `show-provenance` / `show-dependents` / `show-slice` |
| Offline trace review | `petal run --record-trace trace.json` |
| Unit-test IR shape | `showIrJson` + `termByName` / `termsByOp` |
| Debug a running canvas | `DiagramPause` → `DiagramStep` → `DiagramState` / `DiagramScreenshot` |
| Automate an SDL program | `petal-sdl --agent` |
| CI visual regression | `petal-sdl --screenshot --frames N`, or `petal-ui-run` traces ([headless-ui-run.md](headless-ui-run.md)) |

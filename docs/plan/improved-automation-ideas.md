# Improved automation

Status (2026-09-15): **idea, not started.** Two linked proposals: a scripted
automated user written in Petal, and consolidating today's separate
automation surfaces into one system that the scripted user runs on.

## Why now

Every automated run of a Petal UI today is driven by one of two kinds of
input: a static list of events at fixed frames (a scenario file,
a sequence of `curl` calls) or random noise (`monkey:<seed>`). Neither looks
like a person using the app, and each host has its own way of taking input.

That started to matter with reactive rendering
([reactive-rendering-plan.md](../dev/reactive-rendering-plan.md)). The frame
gate and memoized scopes make the cost of a frame depend on *which* inputs
changed and *which* probes flipped. A pointer that moves 1 px every frame
(`bench_panel --wiggle`) is the worst case for the gate. Input that jumps
straight to a target with no travel is the best case. Neither is what people
actually do. Evaluating the spreadsheet app meant writing the scenario JSON
from a Python script, with sine waves standing in for a hand on a trackpad.
Tests have the same problem: a scenario that clicks `(270, 214)` breaks
silently when the layout moves, and monkey input rarely hits a real button.

## Part 1: a scripted user in Petal

### The idea

A **driver** is a Petal script that plays the user. It runs next to the app
under the headless harness. It can see what the app drew on the last frame
and decide what to do next: find the button labelled "Save", move the pointer
to it over a realistic number of frames, pause, click, type at a human
speed, wait until something appears, and check that it did.

Petal suits this well. The driver author already knows the language and its
records and closures. The driver can also use things only the runtime knows:
emit sites (which call drew this), probe rectangles (where the app
hit-tests), `state` and observations. No DOM-scraping automation tool gets
any of that.

### What a driver looks like

A driver builds a list of **steps**. The runtime schedules them against the
frame clock. **Locators** such as `text("Save")` are plain records, resolved
against the app's frame when the step runs. So a driver never hard-codes
coordinates, and it keeps working after a layout change.

```petal
// examples/productivity/spreadsheet/tests/edit-formula.drive.ptl
import auto: *

let user = persona({seed: 7, pointer: "trackpad", wpm: 55, think: [0.3, 1.2]})

// A cell in the grid, found by the column header and row label the app drew.
fn cell(col: string, row: string) -> record
  intersect(column_of(text(col)), row_of(text(row)))
end

drive(user, [
  wait_for(text("Ledger")),
  hover(cell("Q2", "New MRR")),          // glides there; the row highlight should follow
  expect(fn(app) -> app.state.editing == false, "hovering does not start an edit"),
  click(cell("Q4", "Expansion")),
  type_text("=SUM(B3:E3)"),              // inter-key gaps drawn from 55 wpm
  press("return"),
  wait_for(fn(app) -> app.state.editing == false),
  expect(fn(app) -> !contains(text_in(app, cell("Q4", "Expansion")), "#"),
         "the formula evaluates without an error value"),
  idle(2.0),                             // read the result; the frame gate should skip these frames
])
```

The vocabulary, sketched:

| Kind | Examples | Resolved against |
|---|---|---|
| Locators | `text("Save")`, `text_near("FY total")`, `widget("button", {label: "Save"})` (a draw whose emit chain passes through the function `button` with that argument), `probe_at(x, y)` (the `hovered(r)` rectangle under a point), `region(x, y, w, h)`, `intersect(a, b)` | the last frame's draw commands and emit sites, plus the probe rectangles memoized scopes already record |
| Actions | `hover(loc)`, `click(loc)`, `double_click`, `drag(from, to)`, `scroll(loc, lines)`, `type_text(s)`, `press(key, {shift: true})`, `hold(key, seconds)`, `idle(seconds)` | the persona's timing model |
| Waits | `wait_for(loc)`, `wait_for(fn(app) -> bool)`, with a frame budget | every frame until true |
| Checks | `expect(fn(app) -> bool, message)`, `expect_text(loc, s)`, `expect_frames_skipped(at_least)`, `snapshot("name")` | the frame; `app` is a record `{frame, state, values, commands, gate, memo}` |
| Free-form | `every_frame(fn(app, input) … end)` | per frame, for fuzzers and agents that decide one frame at a time |

Petal has no coroutines, so a driver does not block across frames. Steps are
data, and the scheduler (Rust) turns each one into per-frame input events.
That keeps drivers short and readable, and all the timing lives in one place
where it is deterministic. `every_frame` is the escape hatch for logic that
must look at every frame. It is ordinary immediate-mode Petal, with `state`
for memory.

### Realistic behavior: the persona

A `persona` is a seeded model of a user, applied by the scheduler:

| Behavior | Model |
|---|---|
| Pointer travel | Duration from Fitts's law (distance and target size), a minimum-jerk path with slight curvature, small overshoot and correction on small targets, sub-pixel jitter. A trackpad gives several moves per frame; a mouse gives coarser ones. |
| Dwell | 80–250 ms settled on the target before the press; 60–120 ms between press and release. |
| Typing | Inter-key gaps drawn log-normally around the persona's wpm, with longer pauses at word boundaries. Optionally a typo rate with backspace corrections. |
| Think time | A pause between steps drawn from `think`. Most real frames are idle frames, and that is where the frame gate pays off. |
| Idle | The pointer stays exactly still (no sensor noise) unless the persona says otherwise, so gate behavior matches a real resting hand. |

`persona({instant: true})` turns all of this off, for tests that want the
fewest frames. The same driver then works as both a fast test and a
realistic benchmark.

### Hosting

The first host is the in-process `petal_ui::harness::Headless`.

- **A second `Env`.** The driver compiles into its own `Env` and stack, with
  an `auto` native module registered. It is a separate `Env` rather than a
  second stack in the app's `Env`, so the app's frame gate and memo table
  never see the driver's reads, and a driver `print` never ends up in the
  app's trace.
- **Per frame:** the scheduler advances the current step and emits
  `InputEvent`s → `Headless::event` → `Headless::frame` → a frame snapshot
  (commands, emit origins, `state()`, observations, gate and memo counters,
  probe rectangles) is published to the driver as the `app` record, and to
  locators.
- **Determinism:** the persona has its own seeded RNG, separate from the
  app's `Env::set_seed`. The harness clock is already fixed at 1/60 s. A
  failing run can be replayed from `(app, driver, app seed, persona seed)`.
- **Output:** the same JSONL record `petal-ui-run` writes, plus a `step`
  field (which step was active) and `checks` (what passed or failed on that
  frame), so existing trace tooling keeps working.

### Where it would be used

- **Tests.**
  - Rust: `petal_ui::drive::run(app, driver)` returns the check results, for
    `cargo test` beside `petal-ui/tests/*.rs`.
  - vitest and scripts: `petal-ui-run --drive x.drive.ptl`.
  - Goldens: `test/ui-golden/index.json` gains `drive-<name>-s1` traces next
    to `monkey-1-s1`.
  - Verification: `ts/bin/verify.ts` plans can name drivers.
  - Convention: an app keeps its drivers in `tests/*.drive.ptl` beside
    `app.ptl`.
- **Benchmarks.** `bench_panel --drive x.drive.ptl` times a realistic
  session. That gives frames run vs skipped, run-frame percentiles, and total
  script time under a real input rhythm, rather than wiggle (worst case) or
  hand-generated JSON.
- **Fuzzing, smarter than monkey.** `explore(persona, {steps: 500})` picks
  its targets from what the app shows as interactive: `hovered` and
  `point_in` probe rectangles, keys the app polls with `key_pressed` or
  claims with `claim_key`, and focused text fields. It clicks, types and
  drags on those, not on random pixels. On a failure (runtime error, a
  broken `expect`, a gate-vs-full or memo-on-vs-off trace divergence), it
  shrinks the step list the way `garden/tools/vim-parity/fuzz.ts`
  delta-debugs keystroke programs, then writes the minimal driver out as a
  regression test.
- **Agents.** An MCP session: start an app, send a few steps written in
  Petal, get back the frame (locator matches with their emit sites, state,
  checks). What an agent did while exploring can be saved as a `.drive.ptl`
  file, which is then a test.

## Part 2: what exists today

| System | What it does | Input | What it can observe | Where | Used by |
|---|---|---|---|---|---|
| `Headless` harness | In-process frame loop for a petal-ui script | Rust calls: `event(InputEvent)`, `mouse_move`, `mouse_down`/`up`, `scroll`, `key`, `text`, `click`, `frame`, `frames` | `commands`, `state()`, `state_int`/`float`/`string`, `memo_stats`, `frames_run`/`frames_skipped`, `last_run_reason`; the `Env` directly (observations, profile, emit origins) | `petal-ui/src/harness.rs` | petal-ui tests (`gating.rs`, `memo.rs`, `widgets.rs`, …), `petal-ui-run`, `bench_panel` |
| `petal-ui-run` | CLI: run an app headless, one JSONL record per frame | `--scenario s.json` (edge events keyed by frame: `mouse_move`, `mouse_down`, `click`, `key`, `text`, `scroll`, `modifiers`) or `monkey:<seed>`; `--seed`, `--host-data` fixtures | per frame: `commands`, `state`, `prints`, `result`, `error`; `--gate-stats`, `--memo-stats` on stderr | `petal-ui/src/bin/petal-ui-run.rs`, `petal-ui/src/scenario.rs`, `petal-ui/src/panel_stubs.rs` | `verify.ts`, UI goldens, ad hoc |
| `bench_panel` | Per-frame timing of a panel script | frame count, `--wiggle`, `--no-gate`, `--no-memo`, `--observe`, `--profile`; `--scenario` (added 2026-09-15, uncommitted as of writing) | frame-time percentiles, run-frame percentiles, total script time, memo counters, instruction profile | `petal-ui/examples/bench_panel.rs` | performance work |
| UI goldens | sha256 of each UI app's `petal-ui-run` trace | fixed: `monkey:1`, seed 1, 60 frames, 1280×850 | trace hash only | `test/ui-golden/index.json` | refactor verification |
| `verify.ts` | Before/after proof for mechanical changes, cheapest check first | plans in `test/verify-plans/` (`compiler.json`, `lint-fix.json`) | IR equality, console output, `petal-ui-run` traces | `ts/bin/verify.ts`, [refactor-verification.md](../dev/refactor-verification.md) | large refactors, `lint --fix` sweeps |
| Example golden corpus | Console examples: opts vs `--no-opt`, plus frozen output | none (non-UI programs) | stdout | `ts/bin/test-examples.ts`, `test/example-golden/` | CI |
| Differential oracles | Gate on vs off, memo on vs off, over every panel app | monkey scenario via `Headless` | commands, state, observations frame by frame | `petal-ui/tests/gating.rs`, `petal-ui/tests/memo.rs` | CI |
| Garden debug server | HTTP control of a running (often `--headless`) Garden | `POST /key` (taps or `op: down`/`up`), `/text`, `/mouse` (`click`, `drag`, `down`/`move`/`up`, `scroll`, window-relative logical pixels), `/command`, `/menu`, `/theme`, `/tick` (virtual time), `/seed`, `/panel/reset` | `GET /state` (editor state, `panel.values`, script output), `/scene` (primitives, per pane), `/screenshot` (PNG, per pane), `/frame`, `/buffer/<n>`, `/windows`, `/version`; no gate or memo counters | `garden/garden-app/src/debug.rs`, [garden/docs/debug-server.md](../../garden/docs/debug-server.md) | Garden integration tests, exploration, agents |
| Garden integration tests | Launch Garden headless and assert through the debug server | TypeScript scripts over `DebugClient` | whatever the server exposes | `garden/tools/*-integration-test.ts`, `garden/tools/lib/` | Garden CI |
| Vim-parity fuzzer | Differential keystroke fuzzing of Garden's editor against real `nvim` | generated keystroke programs over the debug server | buffer, cursor, mode; delta-debugs failures | `garden/tools/vim-parity/` | Garden editor |
| Garden script host tests | In-process Garden panel host | `PanelHost` Rust API | panel output | `garden/garden-script/tests/script_host.rs`, `bloom.rs` | Garden CI |
| petal-sdl agent protocol | NDJSON on stdin/stdout (`--agent`, `--headless`) | `input` command as **held state** (`keys_down`, `mouse {x, y, buttons}`, `mouse_delta`, `text`), `step n`, `pause`/`resume`, `set_state` | `state`, `capture_draw_commands`, `screenshot`, `pending_report`, `draw_stats` | `integrations/petal-desktop-sdl/src/protocol.rs`, [debug-protocol.md](../dev/debug-protocol.md) | games, agents |
| diagram-canvas debug API | The same protocol over WebSocket | same | same | `examples/custom-integrations/diagram-canvas/src/debug.ts` | `petal-diagram-mcp` |
| petal-sdl `--screenshot` | Run N frames headless, write a PNG | `--frames N` | pixels | `integrations/petal-desktop-sdl/src/main.rs` | snapshots |
| SDL timeline | Records every frame's input and a heap fork; rewind, and replay recorded input through an edited program | live input, recorded in memory | trails of one draw call across frames | `integrations/petal-desktop-sdl/src/timeline.rs` | live coding (hopper) |
| MCP `petal-tools` | Compile and run **snippets**: `TestSnippet`, `CheckSnippet`, `ExplainTerm`, `TraceEmits`, `ProposeEdit`, `PendingReport`, `Show*` | code strings, one run | stdout, trace, emits, IR | `ts/tools/petal-mcp.ts`, [mcp-server.md](../dev/mcp-server.md) | agents |
| MCP `petal-diagram` | `DiagramInput`, `DiagramStep`, `DiagramState`, `DiagramCaptureDrawCommands`, `DiagramScreenshot`, `DiagramPause`/`Resume`, `DiagramSetState` | the debug protocol | the debug protocol | `ts/tools/petal-diagram-mcp.ts` | agents on diagram-canvas |
| `petal run` introspection | `--observe`, `--trace-emits`, `--record-trace`, `explain` | none (console programs) | bindings, emit attributions, trace | `rust/src/cli/` | debugging |
| Web hosts | petal-web-canvas, petal-web-html | real DOM events only; no injection or stepping API beyond runtime hooks | none exported | `integrations/petal-web-canvas`, `integrations/petal-web-html` | — (browser automation would have to go through a generic tool such as Playwright) |

### Overlap and gaps

- **Four input models for one concept.**
  - `InputEvent` and the scenario format: edge events keyed by frame.
  - The SDL/diagram protocol: a held-state snapshot applied on the next step.
  - The Garden debug server: taps and gestures through the app's own
    dispatch, in window coordinates with Garden's chrome offset.
  - Hand-written Rust calls in tests.

  The key names mostly agree (`petal_ui::input::KEY_NAMES`, the Garden
  `/key` names), but modifier spelling, button numbering and coordinate
  spaces differ.
- **Four ways to read a frame.**
  - `petal-ui-run`'s JSONL record.
  - Garden's `/state`, `/scene` and `/screenshot`.
  - The protocol's `state`, `capture_draw_commands` and `screenshot`.
  - The MCP snippet tools, which cannot see a running app at all.

  Emit sites (which call drew this) are only reachable through `petal run
  --trace-emits` and `TraceEmits`, never for a running UI.
- **Three ways to advance time.** The harness's fixed dt, Garden
  `POST /tick` (virtual time), and protocol `step`.
- **Reactive-rendering counters** (gate reasons, memo stats) are on stderr
  in `petal-ui-run` and `bench_panel` only. Garden and SDL do not expose them.
- **No locators anywhere.** Every test and scenario uses raw coordinates.
- **Web hosts have no automation surface.**
- **Two MCP servers**, split by host rather than by task; neither can drive a
  petal-ui app.
- **What is fine as it is:** the vim-parity fuzzer (editor-specific, with an
  external oracle), `verify.ts`'s plan runner, and the SDL timeline's
  in-memory recording are specialized, and do not need to merge. They should
  consume the common pieces rather than be replaced.

## Part 3: one automation system

### Shape

One model, one session, several transports.

```text
             ┌──────────── Session ────────────┐
 driver.ptl ─┤ scheduler + persona → InputEvent │
 JSON steps ─┤ step / advance time (virtual dt) ├─ Frame snapshot:
 agent (MCP)─┤ locators · checks · recording    │    commands + emit sites, state,
             └───────────────┬─────────────────┘    values, probes, gate/memo, pixels
                             │ AutomationHost trait
        ┌──────────┬─────────┴─────┬──────────────┬──────────────┐
     Headless   Garden panel   petal-sdl      web-canvas     web-html
   (in-process)  (PanelHost)   game loop      (wasm)         (wasm)
```

1. **One input model: `petal_ui::input::InputEvent`**, plus a small gesture
   layer (`click`, `drag`, `tap key`, `type text`) that expands into it. The
   scenario JSON becomes its serialization. The protocol's held-state
   `input` is converted to edge events at the transport boundary. Garden's
   `/key` and `/mouse` bodies parse into the same types. Pick one spelling
   each for keys, modifiers and buttons, and accept the others as aliases.
2. **One frame snapshot**, the superset of today's readers: `frame`,
   `commands` with emit origins, `state`, `values` (observations), probe
   rectangles, `gate` (`skipped`, `reason`), `memo` counters, `prints`,
   `error`, and optional pixels. `petal-ui-run`'s JSONL record is this
   snapshot minus the optional fields, so the goldens keep their format.
3. **One `Session`** (Rust, in petal-ui):
   - `open(app, size, seed)`
   - `send(events)`
   - `step(n, dt)`
   - `snapshot(fields)`
   - `resolve(locator)`
   - `run(driver | steps)`
   - `record()` / `replay(recording)`

   It runs over an `AutomationHost` trait that each host implements:
   - bind input
   - run or skip a frame
   - take the snapshot
   - optionally rasterize

   `Headless` is the reference implementation.
4. **Transports are thin adapters over `Session`.**
   - **In-process:** Rust tests.
   - **CLI:** `petal-ui-run` gains `--drive`; `bench_panel` becomes
     `petal-ui-run --bench`, so timing and trace share one driver.
   - **Wire protocol:** one JSON command set, the current debug-protocol
     commands plus `open`, `drive`, `resolve` and `snapshot` fields, over
     stdio (SDL), WebSocket (diagram-canvas, web hosts) and HTTP (Garden).
     Garden keeps its editor-specific endpoints (`/command`, `/buffer`,
     `/menu`, `/theme`) as extensions.
   - **MCP:** one `petal-tools` server whose `Ui*` tools (`UiOpen`,
     `UiDrive`, `UiSnapshot`, `UiScreenshot`) speak the wire protocol to any
     host, or run `Session` in-process for a file. `Diagram*` becomes an
     alias for the same tools on a WebSocket target, then is retired.

### What becomes what

| Today | Becomes |
|---|---|
| `Scenario` JSON and `monkey:` | a serialization of `Session` input; `monkey` becomes the dumbest `explore` persona |
| `bench_panel` | `petal-ui-run --bench` (or a thin wrapper over the same `Session`) |
| petal-sdl `protocol.rs`, diagram-canvas `debug.ts` | adapters implementing the unified wire protocol over `AutomationHost` |
| Garden `/key`, `/text`, `/mouse`, `/tick`, `/seed`, `/scene`, `/state` | the unified wire protocol over HTTP, for panels; editor-level endpoints stay as Garden extensions |
| `petal-diagram-mcp.ts` | retired into `petal-tools` `Ui*` tools |
| UI goldens, `verify.ts` UI checks, `gating.rs`/`memo.rs` oracles | unchanged in intent; they run drivers as well as monkey |
| SDL timeline input recording | stores `Session` recordings, so a live session can be saved as a replayable test |
| vim-parity fuzzer, Garden integration tests | keep their oracles; move to shared TypeScript client types generated from the protocol |
| web hosts | implement `AutomationHost` in the wasm runtime and expose the WebSocket protocol in dev builds |

### Phases

1. **Unify the model in petal-ui (small, no new features).**
   - Make `Scenario` the one serialization of `InputEvent`, and give it the
     gesture expansions Garden's `/mouse` already has (`drag`, `down`/`move`/`up`,
     double-click).
   - Define the frame snapshot type, and emit it from `petal-ui-run`
     (adding `gate` and `memo` fields per frame, not only on stderr).
   - Fold `bench_panel`'s timing into `petal-ui-run --bench`.
2. **Locators and a JSON step runner.**
   - Keep emit origins per frame in `Headless`, and expose probe rectangles
     from the memo's `Dep::Probe` entries (or a cheap per-frame probe log
     that works with memo off).
   - Implement `resolve(locator)`, and the `Session` step scheduler with
     `persona` timing.
   - Steps are still JSON at this point, which proves out the model before
     any language work.
3. **Petal drivers.**
   - Add the `auto` native module and the second-`Env` hosting, plus
     `petal-ui-run --drive`, `drive::run` for cargo tests, and the vitest
     helper.
   - Write drivers for the spreadsheet, todo and kanban apps, and use them
     for the reactive-rendering benchmarks and the goldens.
4. **One wire protocol and one MCP server.**
   - Specify the protocol in `docs/dev/debug-protocol.md` as a superset.
   - Move petal-sdl and diagram-canvas onto it, then map Garden's panel
     endpoints onto it.
   - Add the `Ui*` MCP tools, and retire `petal-diagram-mcp`.
5. **Exploration and more hosts.**
   - `explore` with shrinking.
   - Divergence checks (gate/memo on vs off) run under drivers.
   - `AutomationHost` for web-canvas and web-html.
   - SDL timeline recordings saved as drivers.

**First concrete steps:** (a) per-frame `gate` and `memo` fields in the
`petal-ui-run` record; (b) `drag` and `down`/`move`/`up` in `Scenario`,
aligned with the Garden `/mouse` op names; (c) a `text("…")` locator over
`DrawCommand::Text`, usable from a scenario as `{"at": 10, "click": {"text":
"Save"}}`. Each is useful on its own and none needs the driver language.

## Open questions

- **Steps as data or blocking calls.** Steps-as-data keeps drivers
  declarative and deterministic, but branching on what the app shows
  ("if a dialog appears, dismiss it") needs `every_frame` or
  `when(loc, steps)`. Is that enough, or does Petal want a
  generator/coroutine form?
- **Locator semantics without a widget tree.** `widget("button", …)` depends
  on emit chains naming a function. Apps that draw inline at the top level
  have no such function. Is text plus probe rectangles enough in practice?
- **Garden panels versus Garden the editor.** Should the unified protocol
  drive a whole Garden window (panes, chrome, focus), or only the panel
  inside a pane, with the editor keeping its own endpoints?
- **Pixels.** `Headless` has no rasterizer. Screenshots in the snapshot come
  only from hosts that render (Garden, SDL). Is a software raster in
  petal-ui worth it for headless visual goldens?
- **Persona calibration.** Which timing numbers are defensible (published
  Fitts's-law and typing studies, or recorded sessions from the SDL
  timeline)? Should benchmarks pin one standard persona so numbers stay
  comparable over time?
- **Cost.** Keeping emit origins and probe rectangles every frame costs
  something. They should be on only under automation, so that benchmark
  runs measure the app and not the recorder.

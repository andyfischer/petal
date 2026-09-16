# Consolidate the debugging and automation surfaces we already have

Status: **proposed**, 2026-09-16. Not started.

A companion to [improved-automation-ideas.md](../plan/improved-automation-ideas.md),
which proposes a *new* automation system. This doc is the other half: what can
be simplified, merged, or made consistent in the systems that exist today.
Most of it is deletion. None of it needs the driver language, locators as a
concept, or `AutomationHost` as a design — and two items (§1, §5) remove
obstacles that plan would otherwise have to work around.

Ordered by payoff per line deleted.

## Contents

- [Tier 1: mechanical duplication](#tier-1-mechanical-duplication)
  - [1. Two embeddings of "run a panel frame"](#1-two-embeddings-of-run-a-panel-frame)
  - [2. Frontend-answered commands, three times](#2-frontend-answered-commands-three-times)
  - [3. Two modifier types and a key-name round trip](#3-two-modifier-types-and-a-key-name-round-trip)
  - [4. Three dataflow queries that are one query](#4-three-dataflow-queries-that-are-one-query)
- [Tier 2: consistency](#tier-2-consistency)
  - [5. One selector grammar for the debug server](#5-one-selector-grammar-for-the-debug-server)
  - [6. `/scene` and `/screenshot` are one capture](#6-scene-and-screenshot-are-one-capture)
  - [7. Four `Show*` MCP tools for one argument](#7-four-show-mcp-tools-for-one-argument)
- [Tier 3: primitives worth adding](#tier-3-primitives-worth-adding)
  - [8. Every command replies with the snapshot](#8-every-command-replies-with-the-snapshot)
  - [9. A text locator on `/scene`, now](#9-a-text-locator-on-scene-now)
  - [10. `POST /batch`](#10-post-batch)
  - [11. Generate the TypeScript client types](#11-generate-the-typescript-client-types)
- [What to leave alone](#what-to-leave-alone)
- [Sequencing](#sequencing)

---

## Tier 1: mechanical duplication

### 1. Two embeddings of "run a panel frame"

**The observation.** `petal_ui::harness::Headless` (`petal-ui/src/harness.rs`)
and `garden_script::PanelHost` (`garden/garden-script/src/panel.rs:1202`) are
two independent implementations of the same object. Each owns an `Env` and a
stack, takes `InputEvent`s, runs a frame, and returns draw commands. Neither
is built on the other.

They have drifted in opposite directions, and the split is not a design
decision — it is two codebases:

| Capability | `Headless` | `PanelHost` |
|---|---|---|
| `InputEvent` in, draw commands out | yes | yes |
| Virtual clock / `advance_clock` | — | `:1438`–`:1474` |
| `set_seed` | — | `:1474` |
| Frame-gate stats | — | `:1542`–`:1554` |
| Memo stats | `memo_stats()` | — |
| `state()` / `state_int` / `state_float` / `state_string` | yes | — |
| Font metrics, variant ratios | — | `:1335`–`:1515` |
| Query provider, edit views, nav arg | — | `:1563`–`:1658` |

This is the direct cause of two gaps the automation plan lists as separate
problems: Garden cannot report gate or memo counters (they live on the side
`PanelHost` lacks), and `petal-ui-run` cannot drive a deterministic clock or
seed (they live on the side `Headless` lacks).

**The change.** `PanelHost` is the superset. Extract the shared core from it
— frame execution, input binding, clock, seed, gate and memo counters,
observation readout — and make `Headless` a thin wrapper over that core
rather than a parallel implementation. The host-specific parts of `PanelHost`
(query providers, edit views, Garden's theme and font plumbing) stay where
they are, as the extension layer.

When [improved-automation-ideas.md](../plan/improved-automation-ideas.md)
reaches its `AutomationHost` trait, **derive it from this extracted core**
rather than designing it fresh. The trait's four operations (bind input, run
or skip a frame, take the snapshot, optionally rasterize) are already
`PanelHost`'s shape.

**How to check it.** `petal-ui/tests/gating.rs` and `memo.rs` keep passing
unchanged; the Garden script-host tests
(`garden/garden-script/tests/script_host.rs`, `bloom.rs`) keep passing
unchanged; then `bench_panel --no-gate`-style counters become readable from a
Garden panel through `/state`, which is the new capability that proves the
merge landed.

### 2. Frontend-answered commands, three times

**The observation.** `DebugCmd::Screenshot` and `DebugCmd::Windows` are
handled once per frontend — `frontend/headless.rs:184`,
`frontend/terminal.rs:104`, `frontend/window.rs:423` — because only the
frontend owns a renderer and a window registry. Consequences:

- The `?window=` guard (`Some(n) if n != 1 => Err(...)`) is written out twice,
  in headless and terminal.
- The single-window `windows` JSON literal is byte-identical in those same two
  files.
- `app/debug_server.rs:125`–`:128` carries placeholder arms
  (`DebugCmd::Screenshot { .. } => …`, `Windows => Err("window listing is
  answered by the frontend")`) whose only job is to make a match total that
  isn't. A new frontend that forgets to intercept them gets a runtime error
  string, not a compile error.
- The settle-then-capture contract is re-stated in each frontend's comments
  and re-implemented in each (`app.settle_panels()` then capture), which is
  precisely the kind of contract that drifts.

**The change.** Give the core a small capability the frontend supplies:

```rust
pub trait Capture {
    /// Rasterize the already-settled scene. `None` for a frontend with no pixels.
    fn rasterize(&mut self, scene: &Scene, crop: Option<Rect>) -> Option<Reply>;
    /// The window registry, as `/windows` reports it.
    fn windows(&self) -> Value;
}
```

`App::handle_debug` then handles **every** command, calls `settle_panels`
once in one place, and the placeholder arms go away. The terminal frontend's
character grid becomes an ordinary `rasterize` implementation instead of a
special case, and the single-window `windows()` body is written once as a
default.

**How to check it.** `tools/screenshot-consistency-test.ts` passes unchanged
(it exists to pin exactly this contract), and
`tools/multi-window-integration-test.ts` still isolates windows. Grep for
`DebugCmd::` under `garden-app/src/frontend/` should return nothing.

### 3. Two modifier types and a key-name round trip

**The observation.** `app::Mods` (`garden-app/src/app/types.rs:30`) and
`petal_ui::input::Modifiers` (`petal-ui/src/input.rs:161`) are the same four
booleans. They are converted by hand at `panel_view.rs:1387`, and `Mods` is
constructed independently from winit, crossterm, muda, and the debug server.

One level down, a key name makes a round trip through an intermediate enum:

```
"pagedown" --debug::parse_key(debug.rs:847)--> Key::PageDown
           --app/input.rs:997 panel_key_name--> "pagedown"   (what the panel actually sees)
```

Both tables are hand-maintained, and `app/input.rs:1247` is a test whose whole
purpose is to assert that the second one agrees with
`garden_script::KEY_NAMES` — a test that exists because the duplication does.

**The change.**

1. Make `app::Mods` an alias for (or `From`/`Into` pair with)
   `petal_ui::input::Modifiers`, and delete the hand conversion at
   `panel_view.rs:1387`.
2. Validate `/key` names against `petal_ui::input::KEY_NAMES` at the route
   boundary, so an unknown name is a 400 with the canonical list rather than a
   silent no-op. `petal-ui/src/scenario.rs:365` already does exactly this for
   scenarios; the error message can be shared.
3. Keep `parse_key` — Garden's `Key` enum is a real abstraction over three
   toolkits — but derive `panel_key_name` from the same table rather than
   writing the inverse by hand, so `input.rs:1247` has nothing left to catch.

**How to check it.** `app/input.rs`'s existing key-name tests, plus
`tools/integration-test.ts`. The modifier change is covered by
`app/tests.rs:4629` (modifiers published as held keys).

### 4. Three dataflow queries that are one query

**The observation.** `handle_show_provenance` (`rust/src/cli/handlers.rs:1142`),
`handle_show_dependents` (`:1270`), and `handle_show_slice` (`:1353`) are
structurally identical: compile, resolve the term query, run one graph walk,
emit `{root, <terms>, edges, frontier, complete}` as JSON or as a text table.

They differ in which walk runs and in **what the terms array is called** —
`ancestors`, `dependents`, `slice`. Three names for one field means no client
can consume them generically, and the inconsistency has already produced a
real gap: `show-dependents` reports neither `frontier` nor `complete`, even
though it is the query that over-approximates most
([debugging-visibility.md §1a](../dev/debugging-visibility.md)).

**The change.** One command with a direction, and one result shape:

```
petal graph --term <t> [--direction back|forward] [--term <t2> ...]
```

- `--direction back` (default, one term) is today's `show-provenance`.
- `--direction forward` is `show-dependents`.
- Several `--term`s is `show-slice`.
- The result is always `{targets, terms, edges, frontier, complete, minimal}`.

Keep `show-provenance` / `show-dependents` / `show-slice` as aliases — they
are in [CLI.md](../CLI.md), the MCP tool descriptions, and
[debugging-visibility.md](../dev/debugging-visibility.md) — but have them
call one handler. `show-graph` (`:1412`) stays separate: DOT is a different
output medium, not a different query.

**How to check it.** The vitest helpers `showProvenanceJson` and the term
lookups in `ts/test/helpers.ts` keep working against the aliases; add a case
asserting `show-dependents` now carries a frontier for a `var` read, which is
the behavior gap the merge closes.

---

## Tier 2: consistency

### 5. One selector grammar for the debug server

**The observation.** Every endpoint invented its own query parameters:

| Selector | Works on | Shape |
|---|---|---|
| `?values=` / `?values_prefix=` | `/state` | exact names, prefixes, tail-matching on function-qualified keys, `all` / `none` |
| `?output=` | `/state` | `new` / `all` / `<cursor>` |
| `?pane=` | `/scene`, `/screenshot` | index |
| `?min=` | `/frame` | integer |
| `?window=` | everything, but must be sole or last | ordinal |

Four idioms. And `/state` is all-or-nothing on everything except `values`: a
client that wants one pane's cursor pays for the whole `panes[]` array
including selection text (capped at 10k chars per pane). That cost is why the
integration tests pipe nearly every call through `jq`.

**The change.** `ValueFilter` (`debug.rs:116`) is the good piece here — exact
names, prefix match, and tail-matching on a qualified key is exactly the right
vocabulary. Generalize it into a field projection that any endpoint accepts:

```
GET /state?select=panes.0.cursor,focus
GET /state?select=panes.*.panel.values.sel
```

with `values=` / `values_prefix=` kept as aliases into it. `?window=` stays a
routing parameter (it selects the *target*, not the *fields*), but the
"sole or last parameter" restriction should go once the path is split
properly — that rule exists only because `parse_target` (`debug.rs:730`)
string-scans for the parameter before the query is parsed.

Add a `state.select` feature flag to `HOST_FEATURES`
(`garden-app/src/version.rs`) in the same commit, per the convention in
[debug-server.md](../../garden/docs/debug-server.md#which-build-am-i-talking-to).

**How to check it.** Rewrite two or three assertions in
`tools/diff-review-integration-test.ts` to use `?select=` and drop their `jq`
equivalents; the assertions must produce identical values.

### 6. `/scene` and `/screenshot` are one capture

**The observation.** Both settle, both build the scene, both take `?pane=`,
and both resolve it through the same `App::pane_capture_rect`
(`app/debug_server.rs:302`). They differ only in how the result is
serialized — PNG bytes versus JSON primitives — and the terminal frontend
already proves the point by answering `/screenshot` with *text*.

**The change.** `GET /capture?format=png|json|text`, with `/scene` and
`/screenshot` kept as aliases. The pane crop, the settle, and the
`X-Garden-Frame` stamping are then written once, and a frontend that cannot
rasterize declines one format rather than special-casing an endpoint.

This is worth doing *after* §2, since `Capture::rasterize` is the seam it
needs.

### 7. Four `Show*` MCP tools for one argument

**The observation.** `ShowTokens`, `ShowAST`, `ShowIR`, `ShowBytecode` in
`ts/tools/petal-mcp.ts` are four tool registrations, four descriptions to keep
accurate, and four entries in the agent's tool list, for four values of one
argument — the compilation stage. `handle_show_tokens` / `_ast` / `_ir` /
`_bytecode` (`cli/handlers.rs:1037`–`:1142`) are similarly parallel.

**The change.** One `ShowStage({ code, stage: "tokens"|"ast"|"ir"|"bytecode", json })`
tool. The CLI subcommands stay as they are — they are typed by humans and the
separate names are the point there — but the MCP surface is read by an agent
that benefits from fewer, more general tools.

While in the file: `ExplainTerm`, `ShowIR`, and the rest all re-implement the
same "write snippet to a temp file, run the binary, read stdout" dance around
`runPetalCommand`. That helper should take the snippet, not the args.

---

## Tier 3: primitives worth adding

### 8. Every command replies with the snapshot

**The observation.** The frame snapshot the automation plan proposes largely
exists: `App::state_json_filtered` (`app/debug_server.rs:348`) is the
superset reader for Garden. What is missing from it is gate and memo counters
(blocked on §1) and emit origins.

Meanwhile, every documented workflow is a two-call pattern: `POST /key`, then
`GET /state` to see what it did. The input endpoints acknowledge with an ad
hoc subset (`{ok, focus, cursor, selection}`) that is neither the snapshot
nor nothing.

**The change.** Let any command take the same `?select=` projection from §5
and return that projection of the snapshot as its reply. `POST /key?select=panes.0.cursor`
is then one round trip, and the ad hoc acknowledgment becomes the default
projection rather than a separate shape. This removes most of the reason
`docs/debug-server.md` has to explain frame-ordering at all.

### 9. A text locator on `/scene`, now

**The observation.** Every integration test and scenario uses raw
coordinates, and the automation plan correctly names this as the reason a
test "breaks silently when the layout moves". But the plan schedules locators
behind the `Session` scheduler, the probe-rectangle work, and the persona
model.

`/scene` already carries every text run with its position, its measured
`advance`, and a `visible` flag that accounts for clipping.
`sceneVisibleTexts` and `sceneTextCount` in `tools/lib/debug-client.ts`
already search it client-side.

**The change.** `GET /scene?find=text:Save` returning the matching primitives
with their rects — roughly twenty lines over the existing `scene_json_view`
(`app/debug_server.rs:900`) — plus a `DebugClient.locate(text)` helper that
returns a clickable point.

That alone lets `diff-review-integration-test.ts` and
`git-panel-integration-test.ts` stop hard-coding geometry. It needs no driver
language, no persona, no `AutomationHost`. **I would promote this ahead of
the other two "first concrete steps"** in the automation plan, since it is
the one that pays before anything else in that plan lands, and it establishes
the locator vocabulary the rest of it will extend.

### 10. `POST /batch`

**The observation.** A Cmd-drag is three HTTP round trips
(`down`, `move`, `up`), each a separate visit to the event loop with
frames possibly running in between. Most documented workflows are four to six
sequential curls, and `tools/lib/debug-client.ts` has grown pane-local
wrappers (`mousePaneLocal`, `rightClickPaneLocal`, `scrollPaneLocal`) largely
to make those sequences less tedious.

**The change.** `POST /batch` taking an array of command bodies, executed in
one event-loop visit, replying with an array of results (and, with §8, one
final snapshot). Gestures become atomic, the client helpers get thinner, and
a driver that later wants to send a whole step's worth of input has the
transport it needs.

### 11. Generate the TypeScript client types

**The observation.** `tools/lib/debug-client.ts:10`–`:108` hand-maintains
`Rect`, `Cursor`, `PaneState`, `AppState`, `VersionReport`, `ScenePrimitive`,
and `WindowInfo` against Rust `json!` literals, with nothing checking that
they agree. Contrast `HOST_FEATURES`, which has a unit test pinning `cli.*`
names to the real argument parser — the response shapes have no equivalent,
even though they are what every test reads.

**The change.** Either derive the reply types with `serde` + `schemars` and
generate the `.d.ts`, or — cheaper, and in keeping with the tools' no-build-step
rule — add a Rust test that serializes one of each reply and compares against
a checked-in JSON sample that the TS types are written from. The second option
catches drift without adding a codegen step to a directory that deliberately
has none.

---

## What to leave alone

- **The vim-parity fuzzer** (`garden/tools/vim-parity/`): an external oracle
  and editor-specific generation. It should consume the shared client types
  from §11, nothing more.
- **`verify.ts`'s plan runner** and the **example golden corpus**: they answer
  a different question (did this refactor change behavior) and their formats
  are frozen on purpose.
- **The SDL timeline's in-memory recording**: specialized for live coding;
  the plan's idea of saving its recordings as drivers is additive, not a merge.
- **`ValueFilter`, `OutputRead`'s three-mode cursor, and the settle-then-capture
  contract**: the strongest parts of the current design. Everything above
  generalizes *from* them rather than replacing them.
- **The GPP protocol** (`garden/docs/gpp.md`): a different contract with a
  different job (a subprocess owning a pane), not another debug surface.

## Sequencing

1. **§3** (modifier type, key tables) and **§4** (one graph query) are
   independent and self-contained. Do them first; neither touches a protocol.
2. **§2** (the `Capture` seam) unblocks **§6**, and makes §8 tractable by
   putting every command in one place.
3. **§9** (the text locator) is independent of all of the above and pays
   immediately. It can run in parallel.
4. **§5** (the selector grammar) then **§8** (snapshot replies) then **§10**
   (`/batch`) are one sequence: each needs the one before it.
5. **§1** (merging the two panel embeddings) is the largest and should be
   scheduled against
   [improved-automation-ideas.md](../plan/improved-automation-ideas.md)
   phase 1, since that plan's `AutomationHost` should be extracted from the
   result rather than designed alongside it.
6. **§7** and **§11** are housekeeping; do them whenever the relevant file is
   already open.

Each of §2–§11 that changes or adds an endpoint appends a name to
`HOST_FEATURES` in `garden-app/src/version.rs` in the same commit, so a test
can say `launchGarden({ requireFeatures: [...] })` and an older binary fails
at launch rather than mid-assertion.

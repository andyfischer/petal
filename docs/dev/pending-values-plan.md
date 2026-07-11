# Plan: Pending values & petal-query

## Goal

Petal scripts should be able to consume async resources (network fetches, host
data, slow computations) **as if they were plain values**, with no `await`, no
callbacks, and no colored functions. A script that fetches a user and paints
their name should read:

```petal
let user = fetch("/api/user/7")
draw_text(user.name, 10, 10)
```

While the fetch is in flight, `user` is a **pending value**. Every ordinary
operation that touches a pending value produces a pending value (or, for
effectful calls like draw commands, a no-op). When the resource resolves, the
next frame's re-run picks up the real value and everything downstream simply
works. A small, enumerated set of **meta functions** (`is_loading`, `is_error`,
`??` fallback, …) can inspect a pending value without being absorbed by it —
this is how scripts render spinners and error states.

Two deliverables:

1. **Language/VM:** the pending-value semantics — a new value kind, its
   propagation rules through every operation, and its interactions with
   control flow, `state`, and collections. Specified by the
   [strict/non-strict table](#the-strictnon-strict-table) below.
2. **`petal-query/`:** a sibling crate to `petal-ui/` providing the
   React-Query-style resource layer — keyed cache, dedup, staleness,
   invalidation — built *on top of* the language-level pending semantics.

A hard, explicit design requirement: **maximum visibility into pending state.**
We expect to build dataflow-visualization tooling that highlights pending
values flowing through a live program, so every pending value must carry
provenance and the runtime must be able to report, per frame, exactly what was
pending, where it originated, and what it absorbed. See
[Observability](#observability-first-class-not-an-afterthought).

## Why this design fits Petal specifically

The load-bearing fact: **Petal hosts re-run the entire script every frame**
(`petal-ui/src/lib.rs` frame contract; `integrations/petal-desktop-sdl/src/game_loop.rs`),
with position-keyed `state` surviving across runs. React Suspense's machinery
(throw a promise, unwind, catch, re-render on resolution) exists because React
*doesn't* re-run everything and needs a way to come back. **Petal's frame loop
is that retry mechanism, for free.** We need:

- no continuations, coroutines, or green threads,
- no new `StepResult` variant — `Continue / Complete / Error` is untouched,
- no abort-and-retry or partial-tree commit rules.

Each frame renders everything resolvable and silently skips the rest; frame
N+k renders more. No frame ever blocks on I/O.

Two existing pieces of the VM are direct precedents:

- **`Value::Dual`** (`rust/src/value.rs`) — a value that looks like a plain
  number but silently threads extra payload (the derivative) through every
  arithmetic op. `Pending` is the same trick with different payload and
  broader coverage.
- **`host_data` returning `Nil` while unavailable** (`petal-ui/src/host_data.rs`)
  — today's manual version of this feature. Scripts must nil-check and
  re-poll; worse, `nil` is ambiguous between "loading" and "legitimately
  absent". Pending values subsume this pattern and remove the ambiguity.

## Prior art (what semantics we're borrowing)

| Precedent | What we take from it |
|---|---|
| **IEEE NaN** | An absorbing value that propagates through strict ops; `isNaN` as the non-strict inspector. |
| **SQL NULL** | Three-valued logic — and its footgun: comparisons silently collapsing to false. We explicitly avoid that (rule: comparisons with pending yield pending, never `false`). |
| **Excel `#N/A` / `#BUSY!`** | The closest match to the whole vision: error values propagate through formulas, `ISNA`/`IFERROR` inspect, async functions show `#BUSY!` and the grid *recalculates* when data lands. Spreadsheet recalc ≡ Petal's frame re-run. Also the proof that visibility is what makes this semantics livable. |
| **Lustre/Lucid clocks** | The formal notion of a stream value being *absent at this tick* — the rigorous version of "pending this frame". Already cited in the README as an influence. |
| **Domain theory / strictness analysis** | `Pending` is a manifest, *inspectable* ⊥. The entire semantics is one sentence: **every operation is strict in Pending except an enumerated non-strict set.** Compositional and checkable. |
| **React Query / Elm `RemoteData`** | The resource state model (`Loading / Error / Ready`), argument-based cache keys, dedup, staleness, invalidation — all in `petal-query`, not the language. |
| **React Suspense / React-tRace (arXiv 2507.05234)** | What *not* to build: abort-and-retry is only needed when re-evaluation isn't free. Also: the purity discipline it imposes is one we largely get from immutable values. |
| **Haxl (Facebook)** | Future work: batch all fetches discovered in one evaluation round. Petal's frame is naturally one "round". |
| **Oz / MultiLisp / AliceML transparent futures** | Rejected alternative: blocking on touch. Blocking mid-frame is the one thing a UI runtime must never do, and the VM has no per-value suspension. |

## Core semantics

### The value

A new value kind, tentatively:

```rust
Value::Pending(ResourceId)   // rust/src/value.rs
```

`ResourceId` is a small index into a per-`Env` (or per-`ExecutionContext`)
**resource table**. The table entry is where all the interesting data lives:

```rust
struct ResourceEntry {
    key: ResourceKey,          // e.g. hash of ("fetch", url) — the cache key
    state: ResourceState,      // Loading | Errored(Value) | Ready(Value)
    origin: Provenance,        // call site (source span), frame started, loop indices
    absorbed_count: u64,       // how many ops absorbed this pending this frame (for viz)
}
```

Keeping `Value::Pending` as a thin id (like `Handle`) keeps `Value: Copy` and
puts provenance/state in one inspectable place — which is exactly what the
visualization tooling wants to query.

An **errored** resource is represented the same way: `Pending(id)` whose entry
is `Errored(err)`. Errors propagate identically to loading (strict ops absorb
them, carrying the same `ResourceId`), and are distinguished only by the meta
functions. This gives error boundaries for free at whatever granularity the
script chooses.

> Naming note: the user-facing docs should probably say "unresolved value"
> or "resource" and reserve *pending* vs *errored* for the two states. This doc
> uses `Pending` for the value kind throughout.

### The one-sentence rule

**Every operation is strict in Pending — it returns the Pending it received —
except the enumerated non-strict meta set.** When an operation receives two
different Pendings, it returns the first (leftmost) one; provenance tooling can
recover the full set from the frame's absorption log.

### The strict/non-strict table

This table *is* the semantics. Anything not listed as non-strict is strict.

| Category | Behavior with a Pending operand |
|---|---|
| Arithmetic (`+ - * / %`, math builtins) | → same Pending |
| Comparison (`== != < <= > >=`) | → same Pending. **Never `false`.** (Avoids the SQL-NULL footgun.) |
| Boolean (`and or not`) | → same Pending. `and`/`or` may short-circuit on a *resolved* first operand as usual; if the decision requires the Pending, result is that Pending. |
| String ops, interpolation (`"hi {x}"`) | → same Pending (the whole string). |
| Field/index access (`x.name`, `x[i]`) | Pending base → same Pending. Resolved list with Pending *element*: access returns that element (collections are element-wise, see below). |
| `if cond` / `while cond` (Pending condition) | **Neither branch executes.** As an expression, the `if` evaluates to the Pending. A `while` with a Pending condition runs zero iterations and evaluates to the Pending. |
| `match` on Pending | No arm executes; result is the Pending. (A dedicated `loading`/`error` arm sugar is possible future work.) |
| `for x in pending_list` | Zero iterations; loop expression value is the Pending. |
| Function call, Pending in arguments | The call **executes normally** — Pending is a first-class value and flows into the body. Strictness applies per-operation inside, not per-call. (This is what lets user code pass resources around and check `is_loading` deep inside.) |
| Calling a Pending *as a function* | → same Pending; body of nothing runs. |
| Effectful natives: draw commands, `print`, host calls | **No-op**, and the absorption is logged for visibility. Frame emits no command for this call. |
| `state x = <pending>` (StateInit) | **Does not commit.** See [state interaction](#interaction-with-state). |
| `state` write (`x = <pending>` where x is state) | Allowed — pending is a legal transient value of a state var — but flagged in the frame report, since it usually indicates rule-above bypass. Open question §Q3. |
| Map key position, `sort` comparator result, list length in `repeat(n)` | **Hard runtime error**, not absorption. These positions corrupt structure silently if absorbed. Enumerated in implementation. |
| **Non-strict meta set** | `is_loading(x)`, `is_error(x)`, `is_ready(x)`, `error_of(x)` (nil if not errored), `x ?? fallback` / `or_else(x, f)`, `resource_key(x)` (for tooling), `settle(x)` (identity on resolved, nil on pending — escape hatch, discouraged). These receive the Pending itself and return real values. On resolved inputs: `is_loading` → false, `is_ready` → true, `x ?? y` → x. |

### Collections are element-wise

`[1, pending, 3]` is a **real list containing a Pending** — not a pending
list. `len` is 3, `map` runs per-element, `list[1]` is the Pending, and any
aggregate that must read every element (`sum`, `join`, `sort`) absorbs and
returns the Pending. Same for maps: a Pending *value* is fine; a Pending *key*
is a hard error.

Rationale: a list of 20 cards where one fetch is slow should render 19 cards.
This matches React Query's per-resource granularity, and it composes: the
element-wise rule plus per-operation strictness reconstructs "whole thing
pending" exactly when the code actually needs every element.

### Interaction with `state`

`StateInit` currently evaluates its init block only on a cache miss and then
commits forever (`docs/dev/Architecture.md`, StateInit/StateRead/StateWrite).
New rule:

> **A Pending result of a StateInit block is not committed.** The state slot
> stays uninitialized; reads of it this frame yield the Pending; the init
> block re-runs on subsequent frames until it produces a non-pending value,
> which then commits normally.

Without this rule, `state user = fetch(url)` would permanently cache the
loading state on frame 1. With it, that line means exactly what it looks like:
"initialize this state from the fetch, once it arrives." This is likely the
single most-used pattern of the whole feature.

Implementation note: this makes `StateInit`'s "miss" path re-enterable across
frames, which interacts with the phi/loop-carry machinery
(`compiler/phi.rs`) — needs a dedicated test alongside the existing
state-in-loop tests.

### Interaction with `Dual` (differentiability)

`Pending` absorbs `Dual`: any arithmetic mixing them yields the Pending. A
backprop pass over a graph containing Pending yields Pending gradients for the
affected paths (and real gradients elsewhere, by element-wise composition).
Trivial, but it's written down now.

### Interaction with speculative execution / forks

Pending values and the resource table live in the `ExecutionContext`
(alongside heap/registries), so `fork_execution` snapshots resource state
consistently and a fork observes the same resolution status as its source at
fork time. Resource *fetching* (the side effect of requesting) must be
context-aware: a speculative fork requesting a new resource should probably
enqueue it in the shared fetcher but is also fine to dedupe against the real
run — fetches are idempotent reads by contract (see petal-query). Open
question §Q4.

## Observability: first-class, not an afterthought

The known failure mode of absorbing values is *silent nothingness* — a blank
region and no idea why. Excel survives this semantics because `#BUSY!` is
**visible in the cell**. Petal's equivalent, and the reason this section is a
requirement rather than a nice-to-have: we intend to build
dataflow-visualization tools that highlight pending values live.

Concretely:

1. **Provenance on every Pending.** The resource table entry carries the
   origin call site (source span), the cache key, loop indices at request
   time, and frame-started. `resource_key(x)` and the debug protocol expose
   it.
2. **Per-frame absorption log — debug-gated.** When enabled, every strict-op
   absorption and every effectful no-op records `(instruction/source span,
   ResourceId)` into a frame-scoped log (a Vec of pairs). This is precisely
   the data a dataflow viz needs to *paint the pending paths* through the
   program: the set of spans a given resource absorbed is that resource's
   downstream cone. **It is off by default** — an unbounded per-absorption Vec
   push is real memory pressure in a hot frame (a large pending list absorbed
   by many draw calls could log thousands of entries/frame), so it is behind a
   runtime debug flag (`--trace-pending` / a debug-protocol toggle / the viz
   tooling turning it on). Steady-state frames with the flag off pay nothing
   beyond the `absorbed_count` counter (see below).
3. **Frame pending report.** At frame end, the host can pull a structured
   summary: every live resource, its state, age in frames, origin, and its
   absorption count/spans this frame. Surfaced via:
   - the debug protocol (`docs/dev/debug-protocol.md`) as a new query,
   - the MCP server (a `PendingReport` tool next to the existing
     introspection tools), so agents can debug "why is this region blank",
   - a `--trace-pending` CLI/env flag printing the report per frame.
4. **Dev overlay hook.** petal-ui exposes the report in a form an integration
   can render as an overlay ("3 pending: fetch(/api/user/7) · 12 frames ·
   absorbed by 4 draw calls at app.ptl:31,32,40,41"). The overlay itself
   lives in integrations/sample apps, not core.
5. **Distinguish "pending" from "nil" in every debugging surface** —
   `ShowIR`/state dumps/`diff_state` must render Pending as
   `<pending fetch("/api/user/7") 12f>`, never as nil or a bare handle.

Design consequence: this is why `Value::Pending` carries a `ResourceId`
rather than being a bare tag — an anonymous absorbing value (NaN-style) would
be *unattributable*, and attribution is the whole visibility story.

## petal-query

A sibling crate `petal-query/` next to `petal-ui/` (workspace member), plus a
`query` prelude module, mirroring petal-ui's layering (Rust core + `.ptl`
prelude). The language ships Pending; petal-query ships cache policy.

### Script-facing API (sketch)

```petal
import query

// Argument-keyed: two call sites with the same key share one request + entry.
let user = query.fetch_json("/api/user/{id}")

// General form: any host-registered fetcher, any key.
let avatar = query.get("avatar", user.avatar_id)   // pending until user resolves — waterfall, see below

if is_loading(user)
  ui.spinner()
elif is_error(user)
  ui.error_banner(error_of(user))
else
  draw_text(user.name, 10, 10)
end

// Or, inline:
draw_text(user.name ?? "…", 10, 10)

query.invalidate("avatar", user.avatar_id)   // force refetch next frame
```

### Semantics and lifecycle

- **`query.get(key…)` returns immediately**: the cached value if fresh, else
  a Pending — and, as a deduped side effect, enqueues the fetch. Keying is
  **by arguments** (React-Query style), not call position, so widgets share
  entries. (Position-keying via the `RuntimeStateKey` machinery remains
  available later if a use case appears.)
- **Resolution lands between frames, never mid-frame.** Host threads (SDL) or
  browser fetch (WASM) deliver into the cache at frame boundaries only. Every
  frame sees a consistent snapshot — a property live-editing and speculative
  execution both depend on.
- **States:** `Loading → Ready(value) | Errored(err)`, plus staleness:
  `Ready` entries older than their `stale_after` refetch in the background
  while continuing to serve the stale value (stale-while-revalidate — the
  script never regresses from Ready back to Pending unless invalidated).
- **Fetchers are host-registered** via a trait (like `host_data`'s
  `DATA_PROVIDER`, but async-shaped):

  ```rust
  trait QueryFetcher {
      fn start(&self, key: &ResourceKey, args: &[Value]) -> FetchTicket;
      // host polls/completes tickets between frames
  }
  ```

  Fetchers are declared to be **idempotent reads** — the contract that makes
  dedup, refetch, and speculative-fork dedup safe.
- **`host_data` subsumption:** `host_data(kind, arg)` becomes a query with a
  host-owned fetcher; its "Nil while unavailable" contract is deprecated in
  favor of a real Pending. Migration is mechanical for existing hosts.

### Known limitation: waterfalls

`query.get("team", query.get("user", id).team_id)` can't start the team fetch
until the user resolves — each frame advances one stage. Acceptable initially
(frames are cheap); the eventual fix is Haxl-style batching: collect all keys
requested during a frame, hand the *batch* to the fetcher. The frame is
naturally one Haxl "round", so this slots in without semantic change. Not in
scope for v1; noted so the fetcher trait leaves room for a batch entry point.

## What we explicitly rejected

- **async/await coloring** — the ceremony is the problem statement.
- **Blocking transparent futures (Oz/MultiLisp/AliceML)** — blocking
  mid-frame freezes rendering; VM has no per-value suspension; wrong model.
- **Suspense-style abort-and-retry (thrown promises)** — solves a problem
  (coming back without full re-evaluation) Petal doesn't have. Would require
  new StepResult machinery and partial-commit rules for zero benefit.
- **`nil` as the pending representation** — ambiguous with legitimate
  absence, unattributable, and already proven painful by `host_data`.

## Open questions

- **Q1 — Pending equality for tooling:** `is_ready`-style meta fns cover
  scripts, but should two Pendings of the same resource be `==` *in the meta
  layer* (e.g. `resource_key(a) == resource_key(b)`)? Proposed: yes, via
  `resource_key`; ordinary `==` stays strict (returns Pending).
- **Q2 — `??` strictness in its right operand:** `x ?? expensive()` — is the
  fallback evaluated when `x` is resolved? Proposed: no (short-circuit, like
  `or`).
- **Q3 — Pending written into committed state:** allow-and-flag (current
  proposal) vs. hard error. Allow-and-flag preserves "Pending is a
  first-class value"; the frame report makes it visible. Revisit after real
  usage.
- **Q4 — fetch requests from speculative forks:** dedupe into the shared
  cache (proposed, safe given the idempotent-read contract) vs. suppress
  entirely.
- **Q5 — non-frame contexts** (`petal run` scripts, tests): no frame loop
  means no retry. Options: (a) the CLI runner loops run-until-no-pending with
  a resolution pump; (b) synchronous fetchers in CLI mode; (c) scripts just
  see Pending. Proposed: (a) behind a flag, (b) for tests via a mock fetcher.
  TestSnippet/MCP tooling needs a deterministic mock-resolution story either
  way.
- **Q6 — surface syntax for boundaries:** is `?? fallback` enough, or do we
  want block sugar (`loading:` / `error:` clauses on `if`/`match`)? Defer
  until the meta functions have been used in anger.

## Incremental roadmap

1. **✅ Done (Chunks A–E).** **`Value::Pending` + resource table + strict
   propagation** in `ops.rs`/`dispatch.rs` (follow the `Dual` pattern), the
   non-strict meta builtins, and the hard-error positions. Unit tests = the
   strict/non-strict table, row by row. No I/O yet: a test-only builtin
   `__pending(key)` / `__resolve(key, value)` drives everything
   deterministically. *(The native pending-disposition tag `NativeClass` uses
   `AllowPending` for the non-strict class; a Pending map key/index is the
   hard-error position.)*
2. **✅ Done (Chunks F–I).** **Control-flow + collections rules**
   (`if`/`while`/`for`/`match`, element-wise lists/maps) + the StateInit
   no-commit rule (with the phi-interaction test). *A single `JumpIfPending`
   opcode (mirroring `JumpIfPresent`) guards the `if`/`while`/`for`/`match`
   lowerings so a Pending condition/iterable/subject runs no branch and the
   expression evaluates to that Pending. `sort`/`join` absorb a Pending
   element (element-wise `len`/index/`map` do not). `Inst::StateWrite` gained
   an `init` flag so only the StateInit commit skips a Pending — the slot
   re-inits each frame until it resolves; ordinary reassignment still commits
   (Q3 allow-and-flag).*
3. **Observability:** provenance, **debug-gated** absorption log, frame
   pending report; debug protocol query + MCP `PendingReport` tool +
   `--trace-pending`. The log is off by default (memory); the always-on
   `absorbed_count` counter covers the cheap case. Do this *before* real
   I/O — it makes step 4 debuggable and it's a stated requirement, not
   polish.
4. **`petal-query` crate:** cache, dedup, fetcher trait, frame-boundary
   delivery; SDL host fetcher (threads) and web-canvas fetcher (browser
   fetch). Sample-app demo (diagram-canvas or a new sample) exercising
   loading/error/stale paths.
5. **`host_data` migration** onto petal-query; deprecate the Nil contract.
6. **Later:** Haxl-style batch fetching; dataflow-viz overlay consuming the
   absorption log; `loading:`/`error:` syntax sugar if warranted.

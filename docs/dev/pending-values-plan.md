# Pending values and petal-query

Status: **the language half is shipped; the data layer shipped in a
different shape than planned.** `Value::Pending`, the strict/non-strict
semantics, the `state` no-commit rule and the observability surfaces are all
in. `petal-query/` exists as a Provider/Cache library over the Garden Pane
Protocol rather than the host-fetcher design sketched here. See
[Status](#status) at the end.

## Goal

Petal scripts should consume async resources (network fetches, host data,
slow computations) **as if they were plain values**, with no `await`, no
callbacks, and no colored functions:

```petal
let user = fetch("/api/user/7")
draw_text(user.name, 10, 10)
```

While the fetch is in flight, `user` is a **pending value**. Every ordinary
operation that touches one produces a pending value (or, for an effectful
call like a draw command, a no-op). When the resource resolves, the next
frame's re-run picks up the real value and everything downstream just works.
A small set of **meta functions** (`is_loading`, `is_error`, `??`, …) can
inspect a pending value without being absorbed by it; that is how scripts
render spinners and error states.

A hard requirement from the start: **maximum visibility into pending
state.** Every pending value carries provenance, and the runtime can report,
per frame, what was pending, where it came from, and what it absorbed.

## Why this fits Petal

Petal hosts re-run the whole script every frame, with `state` surviving
across runs. React Suspense's machinery (throw a promise, unwind, re-render
on resolution) exists because React does *not* re-run everything and needs a
way to come back. Petal's frame loop is that retry mechanism for free. No
continuations, no coroutines, no new `StepResult` variant. Each frame renders
everything resolvable and skips the rest; no frame blocks on I/O.

Two existing pieces of the VM were precedents: `Value::Dual`, a number that
silently threads a derivative through every arithmetic op (Pending is the
same trick with different payload), and `host_data` returning `nil` while
unavailable — the manual version of this feature, whose ambiguity between
"loading" and "absent" is exactly what pending values remove.

## Prior art

| Precedent | What we took |
|---|---|
| IEEE NaN | An absorbing value that propagates through strict ops; `isNaN` as the non-strict inspector. |
| SQL NULL | Its footgun: comparisons silently collapsing to false. We avoid it — a comparison with a pending yields the pending, never `false`. |
| Excel `#N/A` / `#BUSY!` | The closest match: error values propagate through formulas, `ISNA`/`IFERROR` inspect, and the grid recalculates when data lands. Also the proof that *visibility* is what makes this livable. |
| Lustre/Lucid clocks | The formal notion of a stream value being absent at this tick. |
| Strictness analysis | Pending is a manifest, inspectable ⊥. The whole semantics is one sentence (below). |
| React Query / Elm `RemoteData` | The resource state model, argument-keyed caches, dedup, staleness — in petal-query, not the language. |
| React Suspense | What *not* to build: abort-and-retry is only needed when re-evaluation is not free. |
| Haxl | Future work: batch all fetches discovered in one frame. |
| Oz / MultiLisp transparent futures | Rejected: blocking on touch. A UI runtime must never block mid-frame. |

## Core semantics

### The value

`Value::Pending(PendingId)` — a thin id (like `Handle`, so `Value` stays
`Copy`) into a per-context **resource table**. The entry holds the cache
key, the state (`Loading | Errored(value) | Ready(value)`), the origin term,
the frame it started in, and an absorption counter. Keeping all of that in
one inspectable place is what the visualization tooling wants to query.

An **errored** resource is the same value kind with an `Errored` entry. It
propagates exactly like loading and is distinguished only by the meta
functions, which gives error boundaries at whatever granularity the script
chooses.

### The one-sentence rule

**Every operation is strict in Pending — it returns the Pending it received —
except the enumerated non-strict meta set.** With two different Pendings,
the leftmost wins; provenance tooling can recover the full set from the
absorption log.

### The strict/non-strict table

This table *is* the semantics. Anything not listed as non-strict is strict.

| Category | Behavior with a Pending operand |
|---|---|
| Arithmetic, math builtins | → same Pending |
| Comparison (`== != < <= > >=`) | → same Pending. **Never `false`.** |
| Boolean (`and or not`) | → same Pending. `and`/`or` still short-circuit on a *resolved* first operand. |
| String ops, interpolation | → same Pending (the whole string). |
| Field/index access | Pending base → same Pending. A resolved list with a Pending *element* returns that element. |
| `if` / `while` with a Pending condition | Neither branch executes; the expression evaluates to the Pending. A `while` runs zero iterations. |
| `match` on a Pending | No arm executes; the result is the Pending. |
| `for x in <pending>` | Zero iterations; the loop's value is the Pending. |
| A call with a Pending argument | The call **executes normally**. Strictness is per operation inside the body, not per call — this is what lets user code pass resources around and check `is_loading` deep inside. |
| Calling a Pending as a function | → same Pending; nothing runs. |
| Effectful natives (draw commands, `print`, host calls) | **No-op**, logged for visibility. |
| `state x = <pending>` (the init) | **Does not commit.** See below. |
| Ordinary write of a Pending into a `state` | Allowed and flagged in the frame report. |
| Map key, `sort` comparator result, `repeat(n)` count | **Hard runtime error.** These positions corrupt structure silently if absorbed. |
| **Non-strict meta set** | `is_loading(x)`, `is_pending(x)`, `is_error(x)`, `is_ready(x)`, `error_of(x)` (nil if not errored), `x ?? fallback`, `or_else(x, f)`, `resource_key(x)`. These receive the Pending itself and return real values. |

### Collections are element-wise

`[1, pending, 3]` is a real list containing a Pending, not a pending list.
`len` is 3, `map` runs per element, `list[1]` is the Pending, and an
aggregate that must read every element (`sum`, `join`, `sort`) absorbs. Same
for maps: a Pending *value* is fine, a Pending *key* is a hard error.

Rationale: a list of 20 cards where one fetch is slow should render 19
cards. The element-wise rule plus per-operation strictness reconstructs
"whole thing pending" exactly when the code actually needs every element.

### Interaction with `state`

`state x = init` evaluates its initializer only on a cache miss and then
commits forever. New rule:

> **A Pending result of a `state` initializer is not committed.** The slot
> stays uninitialized, reads yield the Pending this frame, and the initializer
> re-runs each frame until it produces a non-pending value.

Without this, `state user = fetch(url)` would cache the loading state on
frame 1 forever. With it, that line means "initialize this state from the
fetch, once it arrives" — probably the single most-used pattern of the
feature.

The rule is per slot. Since `state` is keyed per call path
([state-call-paths.md](state-call-paths.md)), a helper with `state u =
fetch(user_url(id))` called from a `for` over ten ids issues ten fetches into
ten slots, each committing when its own resolves.

### Other interactions

- **`Dual`:** Pending absorbs Dual. A backprop pass over a graph containing a
  Pending yields Pending gradients on the affected paths.
- **Speculative forks:** the resource table lives in the `ExecutionContext`,
  so a fork sees the same resolution status as its source at fork time.
  Fetches are idempotent reads by contract, so a fork's requests can safely
  dedupe into the shared cache.

## Observability

The known failure mode of absorbing values is *silent nothingness* — a blank
region and no idea why. Excel survives because `#BUSY!` is visible in the
cell. Petal's equivalent:

1. **Provenance on every Pending** — origin term, cache key, frame started.
   `resource_key(x)` and the debug protocol expose it.
2. **A per-frame absorption log**, recording `(term, resource)` for every
   strict-op absorption and effectful no-op. This is the data a dataflow
   visualization needs to paint the pending paths. It is **off by default**
   (an unbounded per-absorption push is real memory pressure in a hot frame)
   and enabled by `--trace-pending` / `PETAL_TRACE_PENDING` or the debug
   protocol. The cheap `absorbed_count` counter is always on.
3. **A frame pending report** — every live resource, its state, age in
   frames, origin and absorption count — via the debug protocol's
   `pending_report` query, the `petal pending-report` subcommand, the MCP
   `PendingReport` tool, and a petal-ui host hook for a dev overlay.
4. **Pending is never rendered as nil** in any debugging surface: state
   dumps, `Debug`, JSON and display strings all show `<pending …>` /
   `<errored …>` with origin text.

This is why `Value::Pending` carries an id rather than being a bare tag: an
anonymous absorbing value would be unattributable, and attribution is the
whole visibility story.

## petal-query

The language ships Pending; a data layer ships cache policy. The original
sketch was a sibling crate with a host-registered `QueryFetcher` trait, SDL
thread fetchers and browser `fetch`, and a `query` prelude module:

```petal
import query
let user = query.fetch_json("/api/user/{id}")
if is_loading(user) then ui.spinner()
elsif is_error(user) then ui.error_banner(error_of(user))
else draw_text(user.name, 10, 10) end

draw_text(user.name ?? "…", 10, 10)          // or inline
query.invalidate("avatar", user.avatar_id)   // refetch next frame
```

Principles that still hold: keying is **by arguments**, not call position,
so widgets share entries; resolution lands **between frames, never
mid-frame**, so every frame sees a consistent snapshot; a `Ready` entry that
goes stale refetches in the background while still serving its value
(stale-while-revalidate); fetchers are idempotent reads.

What was built instead (see [petal-query/README.md](../../petal-query/README.md)):
the `petal-query` crate provides `Provider` / `Reply` / `CachePolicy` for the
data-serving side and a generic `Cache` for hosts, running over the Garden
Pane Protocol. A panel script calls `query(kind, arg)` and reads the result
as a pending value; Garden is the host. There is no SDL or web-canvas
fetcher, and `host_data` still uses its synchronous nil-while-unavailable
contract.

**Known limitation: waterfalls.** `query("team", query("user", id).team_id)`
cannot start the team fetch until the user resolves; each frame advances one
stage. The eventual fix is Haxl-style batching of all keys requested in a
frame.

## What was rejected

- **async/await coloring** — the ceremony is the problem statement.
- **Blocking transparent futures** — freezes rendering; the VM has no
  per-value suspension.
- **Suspense-style abort-and-retry** — solves a problem Petal does not have.
- **`nil` as the pending representation** — ambiguous with legitimate
  absence, unattributable, and already proven painful by `host_data`.

## Status

| Step | Status | Notes |
|---|---|---|
| 1. `Value::Pending`, resource table, strict propagation, meta builtins, hard-error positions | done | Test-only `__pending(key)` / `__resolve` / `__reject` builtins drive everything deterministically. |
| 2. Control flow and collections; the `state` no-commit rule | done | One `JumpIfPending` opcode guards the `if`/`while`/`for`/`match` lowerings. `Inst::StateWrite` carries an `init` flag so only the initializer's commit skips a Pending. |
| 3. Observability | done | Provenance, absorption counter, gated absorption log, frame report, `pending-report`, `--trace-pending`, MCP `PendingReport`. |
| 4. `petal-query` | done, different shape | Provider/Cache over GPP; Garden-hosted. No SDL/web fetchers. |
| 5. Migrate `host_data` onto petal-query | not done | `host_data` keeps its synchronous nil contract. |
| 6. Later | open | Haxl-style batching; a dataflow-viz overlay consuming the absorption log; `loading:` / `error:` syntax sugar if the meta functions prove insufficient. |

Open questions still open: whether a Pending written into committed state
should stay allow-and-flag or become a hard error; and the story for
non-frame contexts (`petal run` scripts, tests), where there is no frame
loop to retry — today a script just sees the Pending.

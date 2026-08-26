# petal-query

A **React-Query-style async data layer for Petal UI panels.**

A Petal *panel* draws its whole UI every frame and pulls the data it needs with
`query(kind, arg)`, reading the result as a **pending value** (`is_ready` /
`is_loading` / `is_error` / `??`). `petal-query` is the standard for the two ends
of that channel:

- **Providers** (`Provider`, `Reply`, `CachePolicy`) — the *native* side. A
  `Provider` is a transport-agnostic set of `kind → handler` mappings over a
  per-run state; you declare each answer's value and, per answer, **how cacheable
  it is**. It owns no pane name and no UI script. To run one as a GPP client app
  — ship a Petal UI script (the "page") and answer the script's queries (the
  "data") — hand it to `gpp::serve` with a `gpp::PanelUi`; `petal-query` then
  runs the whole GPP v2 protocol loop for you (handshake, script push, dispatch,
  `mutate`/`navigate`/`emit`, shutdown).
- **Hosts** (`Cache`, `CachePolicy`, `Freshness`) — the embedder side. `Cache` is
  a keyed answer store with in-flight de-duplication, a request outbox, and
  `CachePolicy`-driven freshness (fresh / stale-while-revalidate / expired),
  generic over the stored value type so it links no renderer. Keys are
  `(kind, arg)` where `arg` is **any JSON value** (GPP v2 carries query args
  verbatim), compared by canonical serialization.

`CachePolicy` is the shared vocabulary that crosses the wire between them: a
provider stamps each `Reply` with one, and the host's `Cache` honors it.

## The provider API

Build a `Provider` (the data), then run it over the Garden Pane Protocol with a
`PanelUi` (the page):

```rust
use std::time::Duration;
use petal_query::{CachePolicy, Provider, Reply};
use petal_query::gpp::{self, PanelUi};

const UI: &str = include_str!("git_panel.ptl");

fn main() -> std::io::Result<()> {
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        // The history changes on commit — refresh every few seconds, serving the
        // old list while the refresh runs so the pane never flashes a spinner.
        .query("log", |repo: &mut PathBuf, _ctx| {
            Reply::from(git_log(repo)).cache(
                CachePolicy::max_age(Duration::from_secs(3))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        })
        // A commit addressed by hash never changes — cache it forever.
        .query("commit", |repo: &mut PathBuf, ctx| {
            Reply::json(git_commit(repo, ctx.arg_str()))
        });

    gpp::serve(provider, PanelUi::new("git-log", UI))
}
```

- **State** (`S`) is built from the handshake and handed to every handler by
  `&mut` reference — a repo path, in-memory caches. Stateless providers use
  `Provider::stateless`.
- **`Reply`** carries the value (`Reply::json`), an error (`Reply::error` — a
  JSON-RPC error response on the wire, surfaced to the script via `error_of`),
  or "still loading" (`Reply::loading`), plus a `CachePolicy` (`.max_age(..)`,
  `.no_store()`, `.cache(..)`; default forever).
- **Handler contexts** carry the request's `kind`/`name`/`event`, its **JSON**
  `arg` (`ctx.arg`; `ctx.arg_str()` for the common string case), and the
  handshake `InitializeParams`.
- **`on_emit`** handlers receive the script's `emit(event, arg)` signals — the
  fire-and-forget channel (persist UI state, kick a refresh).
- **`on_mutation`** handlers answer a **mutation** — an effectful, uncached
  request/response call (the fourth quadrant beside `query` and `emit`). Use it
  for GraphQL-style writes.
- **`PanelUi::new(name, script)`** supplies the pane name and UI script;
  **`PanelUi::title`** instead derives the pane name from the built state;
  **`PanelUi::screen(name, source)`** declares an extra navigable screen (the
  declared set is the navigation allowlist). When the panel script calls
  `navigate(name)`, the host fetches that screen's source via GPP v2's
  first-class `navigate` **request** and owns the browser-history stack.
  Register **`on_navigate`** instead to add effects (log the visit, prime the
  target screen's data) — it replaces the built-in declared-screens lookup and
  returns the target's source (or refuses the navigation).

## Cacheability

Because a panel *pulls* every frame, caching is "how often do we re-ask the
provider, and do we show the old value while we wait?".

| Policy | Behavior |
|---|---|
| `CachePolicy::forever()` (default) / `immutable()` | Never re-asked (until an explicit `invalidate`). For a value at an immutable key — a commit hash, a content digest. |
| `CachePolicy::max_age(d)` | Fresh for `d`; then hard-expires — the next query shows a spinner while it refetches. Use when a stale value is worse than a brief spinner. |
| `…​.stale_while_revalidate(s)` | During the `s` window past `max_age`, the stale value is served **and** a background refetch runs (no spinner). |
| `CachePolicy::no_store()` | Never fresh: always served **and** always revalidated. Live data, no spinner flicker after the first load. |

The `CachePolicy` serializes onto the query answer's `cache` field (omitted for
the default), which the host's `Cache` reads to decide, each frame, whether to
serve, background-refresh, or expire an entry.

## Relationship to Garden's `gpp`

The `gpp` crate (`garden/gpp`) is the **single wire definition** of the Garden
Pane Protocol — the JSON-RPC envelope, every message shape, and `CachePolicy`
itself, which this crate re-exports. `petal-query` depends on `gpp` and adds
the client-side machinery: the `Provider` handler API and the `gpp::serve`
protocol loop. There is exactly one definition of the wire; nothing here can
drift from it. Protocol reference: `garden/docs/gpp.md`; app-building guide:
`garden/docs/writing-gpp-apps.md`. The reference providers are Garden's
`gpp-apps/{git-viewers, garden-diff, sqlite-browser, directory-browser,
main-menu, screens-demo, gpp-test-app}`.

## License

MIT

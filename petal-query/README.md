# petal-query

A **React-Query-style async data layer for Petal UI panels.**

A Petal *panel* draws its whole UI every frame and pulls the data it needs with
`query(kind, arg)`, reading the result as a **pending value** (`is_ready` /
`is_loading` / `is_error` / `??`). `petal-query` is the standard for the two ends
of that channel:

- **Providers** (`Provider`, `Reply`, `CachePolicy`) — the *native* side. A
  `Provider` is a transport-agnostic set of `kind → handler` mappings over a
  per-run state; you declare each answer's value and, per answer, **how cacheable
  it is**. It owns no pane name and no UI script. To run one as a panel-mode GPP
  subprocess — ship a Petal UI script (the "page") and answer the script's
  queries (the "data") — hand it to `gpp::serve` with a `gpp::PanelUi`;
  `petal-query` then runs the whole panel-mode protocol loop for you (handshake,
  script push, dispatch, `emit`, shutdown).
- **Hosts** (`Cache`, `CachePolicy`, `Freshness`) — the embedder side. `Cache` is
  a keyed answer store with in-flight de-duplication, a request outbox, and
  `CachePolicy`-driven freshness (fresh / stale-while-revalidate / expired),
  generic over the stored value type so it links no renderer.

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
        .query("commit", |repo: &mut PathBuf, ctx| Reply::json(git_commit(repo, ctx.arg)));

    gpp::serve(provider, PanelUi::new("git-log", UI))
}
```

- **State** (`S`) is built from the handshake and handed to every handler by
  `&mut` reference — a repo path, a parsed transcript, in-memory caches. Stateless
  providers use `Provider::stateless`.
- **`Reply`** carries the value (`Reply::json`), an error (`Reply::error`), or
  "still loading" (`Reply::loading`), plus a `CachePolicy` (`.max_age(..)`,
  `.no_store()`, `.cache(..)`; default forever).
- **`on_emit`** handlers receive the script's `emit(event, arg)` signals — the
  channel for persisting UI state, opening files, etc.
- **`on_mutation`** handlers answer a **mutation** — an effectful, uncached
  request/response call (the fourth quadrant beside `query` and `emit`: JSON-arg
  like `emit`, response-carrying like `query`, but never cached). Use it for
  GraphQL-style writes. The built-in `navigate` mutation powers multi-screen
  panels — see below.
- **`PanelUi::new(name, script)`** supplies the pane name and UI script;
  **`PanelUi::title`** instead derives the pane name from the built state;
  **`PanelUi::screen(name, source)`** declares an extra navigable screen (the
  declared set is the navigation allowlist). When the panel script calls
  `navigate(name)`, the host fetches that screen's source via the built-in
  `navigate` mutation and owns the browser-history stack; override with your own
  `on_mutation("navigate", …)` to add effects.

## Cacheability

Because a panel *pulls* every frame, caching is "how often do we re-ask the
provider, and do we show the old value while we wait?".

| Policy | Behavior |
|---|---|
| `CachePolicy::forever()` (default) / `immutable()` | Never re-asked (until an explicit `invalidate`). For a value at an immutable key — a commit hash, a content digest, a session snapshot. |
| `CachePolicy::max_age(d)` | Fresh for `d`; then hard-expires — the next query shows a spinner while it refetches. Use when a stale value is worse than a brief spinner. |
| `…​.stale_while_revalidate(s)` | During the `s` window past `max_age`, the stale value is served **and** a background refetch runs (no spinner). |
| `CachePolicy::no_store()` | Never fresh: always served **and** always revalidated. Live data, no spinner flicker after the first load. |

The `CachePolicy` serializes onto the query answer's `cacheControl` field
(omitted for the default), which the host's `Cache` reads to decide, each frame,
whether to serve, background-refresh, or expire an entry.

## Relationship to Garden's `gpp`

Garden's `gpp` crate is the canonical Garden Pane Protocol. `petal-query`'s
`wire` module re-implements only the query/panel subset so a provider needs no
dependency on Garden, and the one genuinely shared type — `CachePolicy` — lives
here and is re-used by `gpp`, so the `cacheControl` field cannot drift. The
reference providers are Garden's `gpp-apps/{git-viewers, session-retro,
pr-browser}`.

## License

MIT

# petal-query

A **React-Query-style async data layer for Petal UI panels.**

A Petal *panel* draws its whole UI every frame and pulls the data it needs with
`query(kind, arg)`, reading the result as a **pending value** (`is_ready` /
`is_loading` / `is_error` / `??`). `petal-query` is the standard for the two ends
of that channel:

- **Providers** (`App`, `Reply`, `CachePolicy`) — the *native* side. An `App` is
  a subprocess that ships a Petal UI script (the "page") and answers the script's
  queries (the "data"). You declare `kind → handler` mappings and, per answer,
  **how cacheable it is**; `petal-query` runs the whole panel-mode protocol loop
  for you (handshake, script push, dispatch, `emit`, shutdown). This is the
  elegant surface an app author writes against.
- **Hosts** (`Cache`, `CachePolicy`, `Freshness`) — the embedder side. `Cache` is
  a keyed answer store with in-flight de-duplication, a request outbox, and
  `CachePolicy`-driven freshness (fresh / stale-while-revalidate / expired),
  generic over the stored value type so it links no renderer.

`CachePolicy` is the shared vocabulary that crosses the wire between them: a
provider stamps each `Reply` with one, and the host's `Cache` honors it.

## The provider API

```rust
use std::time::Duration;
use petal_query::{App, CachePolicy, Reply};

const UI: &str = include_str!("git_panel.ptl");

fn main() -> std::io::Result<()> {
    App::new("git-log", UI, |init| PathBuf::from(init.repo_arg()))
        // The history changes on commit — refresh every few seconds, serving the
        // old list while the refresh runs so the pane never flashes a spinner.
        .query("log", |repo, _ctx| {
            Reply::from(git_log(repo)).cache(
                CachePolicy::max_age(Duration::from_secs(3))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        })
        // A commit addressed by hash never changes — cache it forever.
        .query("commit", |repo, ctx| Reply::json(git_commit(repo, ctx.arg)))
        .serve()
}
```

- **State** (`S`) is built from the handshake and handed to every handler by
  `&mut` reference — a repo path, a parsed transcript, in-memory caches. Stateless
  apps use `App::stateless`.
- **`Reply`** carries the value (`Reply::json`), an error (`Reply::error`), or
  "still loading" (`Reply::loading`), plus a `CachePolicy` (`.max_age(..)`,
  `.no_store()`, `.cache(..)`; default forever).
- **`on_emit`** handlers receive the script's `emit(event, arg)` signals — the
  channel for persisting UI state, opening files, etc.
- **`title`** derives the pane name from the built state.

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

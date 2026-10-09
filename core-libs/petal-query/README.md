# petal-query

An async data layer for Petal UI panels, in the style of React Query.

A Petal *panel* draws its whole UI every frame and pulls the data it needs
with `query(kind, arg)`. The script reads the result as a pending value
(`is_ready`, `is_loading`, `is_error`, `??`). `petal-query` standardizes both
ends of that channel:

- **Providers** (`Provider`, `Reply`, `CachePolicy`) are the data side. A
  `Provider` maps each query `kind` to a handler over some per-run state, and
  each answer says how cacheable it is. To run one as a GPP app, pair it with
  a `gpp::PanelUi` (the UI script) and hand both to `gpp::serve`, which runs
  the whole Garden Pane Protocol loop: handshake, script push, dispatch,
  `mutate` / `navigate` / `emit`, shutdown.
- **Hosts** (`Cache`, `CachePolicy`, `Freshness`) are the embedder side.
  `Cache` is a keyed answer store with in-flight de-duplication, a request
  outbox, and freshness driven by `CachePolicy` (fresh, stale-while-
  revalidate, expired). It is generic over the stored value type, so it links
  no renderer. Keys are `(kind, arg)`, where `arg` is any JSON value.

`CachePolicy` is the vocabulary that crosses the wire: a provider stamps each
`Reply` with one, and the host's `Cache` honors it.

## The provider API

Build a `Provider` (the data), then serve it with a `PanelUi` (the page):

```rust
use std::path::PathBuf;
use std::time::Duration;
use petal_query::{CachePolicy, Provider, Reply};
use petal_query::gpp::{self, PanelUi};

const UI: &str = include_str!("git_panel.ptl");

fn main() -> std::io::Result<()> {
    let provider = Provider::new(|init| PathBuf::from(init.repo_arg()))
        // The history changes on commit: refresh every few seconds, serving
        // the old list while the refresh runs so the pane never shows a spinner.
        .query("log", |repo: &mut PathBuf, _ctx| {
            Reply::from(git_log(repo)).cache(
                CachePolicy::max_age(Duration::from_secs(3))
                    .stale_while_revalidate(Duration::from_secs(60)),
            )
        })
        // A commit addressed by hash never changes: cache it forever.
        .query("commit", |repo: &mut PathBuf, ctx| {
            Reply::json(git_commit(repo, ctx.arg_str()))
        });

    gpp::serve(provider, PanelUi::new("git-log", UI))
}
```

- **State** is built once from the handshake by the closure given to
  `Provider::new`, and handed to every handler by `&mut` reference. Use
  `Provider::stateless()` when there is none.
- **`Reply`** carries a value (`Reply::json`, or `Reply::from` on a
  `Result`), an error (`Reply::error`, surfaced to the script via
  `error_of`), or "still loading" (`Reply::loading`), plus a cache policy
  (`.cache(..)`, `.max_age(..)`, `.no_store()`; the default is forever).
- **Handler contexts** carry the request's `kind` / `name` / `event`, its
  JSON `arg` (`ctx.arg`, or `ctx.arg_str()` for the common string case), and
  the handshake `InitializeParams`.
- **`on_emit`** handlers receive the script's `emit(event, arg)` signals: the
  fire-and-forget channel, for persisting UI state or kicking off a refresh.
- **`on_mutation`** handlers answer a mutation: an effectful, uncached
  request/response call. Use it for writes.
- **`PanelUi::new(name, script)`** supplies the pane name and UI script.
  `PanelUi::title` derives the pane name from the built state instead.
  `PanelUi::screen(name, source)` declares an extra navigable screen; the
  declared set is the navigation allowlist. When the script calls
  `navigate(name)`, the host fetches that screen's source through GPP's
  `navigate` request and owns the browser-style history stack. Register
  `on_navigate` to add effects or refuse a navigation; it replaces the
  built-in declared-screens lookup and returns the target's source.
- **`gpp::ScriptSink`** lets a provider push a new script after the handshake
  (`gpp::serve_with_reload`), for live-reload workflows.

## Cacheability

Because a panel pulls every frame, caching answers "how often do we re-ask
the provider, and do we show the old value while we wait?".

| Policy | Behavior |
|---|---|
| `CachePolicy::forever()` (default) / `immutable()` | Never re-asked until an explicit `invalidate`. For values at immutable keys: a commit hash, a content digest. |
| `CachePolicy::max_age(d)` | Fresh for `d`, then expires: the next query shows a spinner while it refetches. Use when a stale value is worse than a brief spinner. |
| `....stale_while_revalidate(s)` | For `s` past `max_age`, the stale value is served and a background refetch runs. No spinner. |
| `CachePolicy::no_store()` | Never fresh: always served and always revalidated. Live data with no flicker after the first load. |

The policy serializes onto the answer's `cache` field (omitted for the
default). The host's `Cache` reads it each frame to decide whether to serve,
background-refresh, or expire an entry.

## Relationship to Garden's `gpp`

The `gpp` crate (`garden/gpp`) is the single wire definition of the Garden
Pane Protocol: the JSON-RPC envelope, every message shape, and `CachePolicy`
itself, which this crate re-exports. `petal-query` adds the client-side
machinery on top: the `Provider` handler API and the `gpp::serve` loop.

- Protocol reference: `garden/docs/gpp.md`
- App-building guide: `garden/docs/writing-gpp-apps.md`
- Reference providers: `garden/gpp-apps/` (`git-viewers`, `garden-diff`,
  `sqlite-browser`, `directory-browser`, `main-menu`, `screens-demo`,
  `gpp-test-app`)

## License

MIT

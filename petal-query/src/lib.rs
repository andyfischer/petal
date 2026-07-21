//! petal-query — a React-Query-style async data layer for Petal UI panels.
//!
//! A Petal *panel* draws its whole UI every frame and pulls the data it needs
//! with `query(kind, arg)`, inspecting the result as a **pending value**
//! (`is_ready` / `is_loading` / `is_error` / `??`). petal-query is the standard
//! for the two ends of that channel:
//!
//! - **Providers** ([`Provider`], [`Reply`], [`CachePolicy`]) — the *native*
//!   side. A [`Provider`] is a transport-agnostic set of `kind` → handler
//!   mappings over a per-run state; you declare each answer's value and how
//!   cacheable it is. It owns no pane name and no UI script — those editor
//!   concerns are the GPP layer's, supplied via [`gpp::PanelUi`](crate::gpp) when
//!   an app runs a provider over the panel-mode GPP loop ([`gpp::serve`]).
//! - **Hosts** ([`Cache`], [`CachePolicy`], [`Freshness`]) — the embedder side.
//!   [`Cache`] is a keyed answer store with in-flight de-duplication, a request
//!   outbox, and [`CachePolicy`]-driven freshness (fresh / stale-while-
//!   revalidate / expired), generic over the stored value type so it links no
//!   renderer. It graduates Garden's hand-rolled `SharedQueryState`.
//!
//! [`CachePolicy`] is the shared vocabulary that crosses the wire between them:
//! a provider stamps each [`Reply`] with one, and the host's [`Cache`] honors it.
//!
//! # Cacheability, in one breath
//!
//! Because a panel *pulls* every frame, caching is "how often do we re-ask, and
//! do we show the old value while waiting?". [`CachePolicy::forever`] (default)
//! never re-asks; [`CachePolicy::max_age`] re-asks after a duration (optionally
//! serving the stale value during the refetch via
//! [`stale_while_revalidate`](CachePolicy::stale_while_revalidate)); and
//! [`CachePolicy::no_store`] always re-asks while still showing the last value.
//!
//! # Relationship to `gpp`
//!
//! Garden's `gpp` crate is the canonical Garden Pane Protocol. petal-query's
//! [`wire`] module re-implements only the query/panel subset so a provider needs
//! no dependency on Garden, and the one genuinely shared type — [`CachePolicy`]
//! — lives here and is re-used by `gpp`, so the `cacheControl` field cannot drift.

pub mod cache;
pub mod cache_control;
pub mod gpp;
pub mod provider;
pub mod wire;

pub use cache::{Cache, Lookup};
pub use cache_control::{CachePolicy, Freshness};
pub use provider::{EmitContext, Provider, QueryContext, Reply};

/// Version of the petal-query provider/cache contract. Bump when the wire shapes
/// or [`CachePolicy`] semantics change incompatibly.
pub const QUERY_VERSION: i64 = 1;

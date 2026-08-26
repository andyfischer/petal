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
//!   renderer.
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
//! # Relationship to the `gpp` crate
//!
//! Garden's `gpp` crate (`garden/gpp`) is the **single** wire definition of the
//! Garden Pane Protocol — the JSON-RPC envelope, every message shape, and
//! [`CachePolicy`] itself, which this crate re-exports. petal-query depends on
//! it and adds the client-side machinery: the [`Provider`] handler API and the
//! [`gpp::serve`](crate::gpp::serve) protocol loop.

pub mod cache;
pub mod gpp;
pub mod provider;

pub use ::gpp::{CachePolicy, Freshness};
pub use cache::{Cache, Lookup};
pub use provider::{
    EmitContext, MutateContext, NavigateContext, Provider, QueryContext, Reply,
};

/// Version of the petal-query provider/cache contract. Bump when the wire shapes
/// or [`CachePolicy`] semantics change incompatibly. Version 2 is the GPP v2
/// protocol: panel-only, id-correlated responses, JSON query args.
pub const QUERY_VERSION: i64 = 2;

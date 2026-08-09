//! `query(kind, arg)` — the async, React-Query-style host→script data channel.
//!
//! Where [`host_data`](petal_ui::host_data) answers *synchronously* inside the
//! frame (so a provider must already hold the value, or return an ambiguous
//! `nil`), `query` is built on Petal's **pending values**: a fetch that has not
//! landed yet surfaces in the script as a real [`Value::Pending`], which the
//! script inspects with the language's meta functions (`is_loading`, `is_error`,
//! `error_of`, `??`). No `nil`-ambiguity, no manual re-poll bookkeeping — the
//! panel's per-frame re-run *is* the retry loop, exactly as
//! Petal's `pending-values` design notes intend.
//!
//! This module is the language-side glue: the `query`/`invalidate` natives and
//! the [`QueryProvider`] trait a host implements. The **cache/fetch policy** it
//! used to carry has graduated into the upstream `petal-query` crate
//! (`../petal-query`) — the same path `host_data` took into `petal-ui`.
//! Garden's pipe-backed provider (`garden-app/src/script_client.rs`) now wraps a
//! `petal_query::Cache`, which honors the `petal_query::CachePolicy` each answer
//! carries (fresh / stale-while-revalidate / expired). What remains here is only
//! the binding to Petal's pending values, which depends on the resource table and
//! so stays in-tree.
//!
//! ## Contract
//!
//! A [`QueryProvider`] (host-owned, attached to the [`PanelHost`](crate::PanelHost)
//! for the duration of each frame) answers `query(kind, arg)` with a
//! [`QueryState`]:
//!
//! - [`Ready`](QueryState::Ready) — the resolved [`HostData`] value tree; the
//!   native returns it as an ordinary Petal value.
//! - [`Loading`](QueryState::Loading) — still in flight; the native returns a
//!   `Value::Pending` (loading) so the script can render a spinner.
//! - [`Errored`](QueryState::Errored) — the fetch failed; the native returns a
//!   `Value::Pending` (errored) carrying the message, readable via `error_of`.
//!
//! The provider is called synchronously each frame and **must not block** — it
//! polls its own background work (threads, channels) and reports the current
//! state. `invalidate(kind, arg)` drops a cached entry so the next `query` for
//! that key refetches — the primitive a "refresh" button is built on.
//!
//! ## Resource-table lifecycle
//!
//! A `Ready` value is returned *directly* from the provider, never routed back
//! through the resource table, so a resource's table entry stays `Loading` for
//! the pane's life. That is deliberate: `get_or_create_loading` dedups by key,
//! so the next `Loading` frame (e.g. after `invalidate`) reuses that same
//! still-`Loading` entry and the script sees a fresh spinner with no per-key
//! reset needed. (A key that has *errored* keeps its errored entry across an
//! invalidate until it next resolves — a minor cosmetic edge on the error path,
//! noted rather than worked around while this lives outside `petal-query`.)

use std::cell::RefCell;
use std::hash::{Hash, Hasher};

use indexmap::IndexMap;
use petal::native_fn::PetalCxt;
use petal::value::Value;

pub use petal_ui::host_data::HostData;

/// The state of one keyed resource, as reported by a [`QueryProvider`] for the
/// current frame. Mirrors React Query / Elm `RemoteData`.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryState {
    /// The fetch is in flight (or has not started). → a loading `Value::Pending`.
    Loading,
    /// The fetch resolved to this value tree. → the value itself.
    Ready(HostData),
    /// The fetch failed with this message. → an errored `Value::Pending`.
    Errored(String),
}

/// A host-side async data source behind the `query(kind, arg)` /
/// `invalidate(kind, arg)` natives. Attached to a [`PanelHost`](crate::PanelHost)
/// via [`set_query_provider`](crate::PanelHost::set_query_provider); the host
/// installs it into this thread's channel around each `env.run`.
///
/// Both methods are called synchronously inside the frame, so implementations
/// must answer from a cache / non-blocking poll of their background work rather
/// than performing the fetch inline.
pub trait QueryProvider {
    /// Report the current [`QueryState`] for `(kind, arg)`, starting the fetch
    /// (as a deduped side effect) on the first request for a key.
    fn query(&mut self, kind: &str, arg: &str) -> QueryState;

    /// Drop any cached entry for `(kind, arg)` so the next [`query`](Self::query)
    /// refetches. A no-arg refresh invalidates every key it cares about.
    fn invalidate(&mut self, kind: &str, arg: &str);
}

thread_local! {
    /// The query provider of the run currently in progress on this thread. A host
    /// installs its provider here for the duration of `env.run` via
    /// [`swap_query_provider`]; the natives read it. `None` → every `query`
    /// answers `Loading` (a script degrades to a permanent spinner, never a crash).
    static QUERY_PROVIDER: RefCell<Option<Box<dyn QueryProvider>>> =
        const { RefCell::new(None) };
}

/// Install `provider` as this thread's active query provider, returning whatever
/// was installed before — the same swap dance as
/// [`host_data::swap_data_provider`](petal_ui::host_data::swap_data_provider),
/// so a panic mid-run leaves the provider in the channel for the host to reclaim.
pub fn swap_query_provider(
    provider: Option<Box<dyn QueryProvider>>,
) -> Option<Box<dyn QueryProvider>> {
    QUERY_PROVIDER.with(|p| std::mem::replace(&mut *p.borrow_mut(), provider))
}

/// Register the `query(kind, arg)` and `invalidate(kind, arg)` natives. Called
/// from [`register_panel_natives`](crate::panel) alongside the petal-ui set.
pub fn register_query(env: &mut petal::env::Env) {
    env.register_native("query", native_query);
    env.register_native("invalidate", native_invalidate);
}

/// Cache/resource key for a `(kind, arg)` pair — a `u64` hash the resource table
/// is keyed by. Distinct kinds/args never collide (the NUL separator can't
/// appear in the interned strings).
fn resource_key(kind: &str, arg: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut hasher);
    0u8.hash(&mut hasher);
    arg.hash(&mut hasher);
    hasher.finish()
}

/// `query(kind, arg)` — ask the host's [`QueryProvider`] for a resource. Returns
/// the resolved value when [`Ready`](QueryState::Ready), or a `Value::Pending`
/// (loading or errored) otherwise, so the script drives its own loading/error UI
/// through the pending meta functions. Without a provider it answers a loading
/// Pending (graceful degradation — never a crash).
fn native_query(cxt: &mut PetalCxt) -> Result<u32, String> {
    let kind = cxt.get_string(1)?;
    let arg = cxt.get_string(2)?;
    let state = QUERY_PROVIDER
        .with(|p| {
            p.borrow_mut()
                .as_mut()
                .map(|provider| provider.query(&kind, &arg))
        })
        .unwrap_or(QueryState::Loading);

    match state {
        QueryState::Ready(data) => {
            let value = data_to_value(cxt, &data);
            cxt.push_value(value);
        }
        QueryState::Loading => {
            // Stamp the resource with the requesting call site + current frame so
            // the pending-observability tooling can render provenance and age.
            let key = resource_key(&kind, &arg);
            let origin = cxt.origin();
            let frame = cxt.frame();
            let id = cxt
                .resources_mut()
                .get_or_create_loading(key, origin, frame);
            cxt.push_value(Value::Pending(id));
        }
        QueryState::Errored(msg) => {
            let key = resource_key(&kind, &arg);
            let origin = cxt.origin();
            let frame = cxt.frame();
            let id = cxt
                .resources_mut()
                .get_or_create_loading(key, origin, frame);
            let err = Value::String(cxt.heap_mut().alloc_string(msg));
            cxt.resources_mut().reject(key, err, frame);
            cxt.push_value(Value::Pending(id));
        }
    }
    Ok(1)
}

/// `invalidate(kind, arg)` — tell the host provider to drop `(kind, arg)`'s
/// cached entry so the next frame's `query` refetches. Returns nil. Without a
/// provider it is a no-op.
fn native_invalidate(cxt: &mut PetalCxt) -> Result<u32, String> {
    let kind = cxt.get_string(1)?;
    let arg = cxt.get_string(2)?;
    QUERY_PROVIDER.with(|p| {
        if let Some(provider) = p.borrow_mut().as_mut() {
            provider.invalidate(&kind, &arg);
        }
    });
    cxt.push_nil();
    Ok(1)
}

/// Convert a [`HostData`] tree into a heap-allocated Petal [`Value`] — the same
/// projection [`host_data`](petal_ui::host_data) uses (kept in sync; the upstream
/// one is private to that module).
pub(crate) fn data_to_value(cxt: &mut PetalCxt, data: &HostData) -> Value {
    match data {
        HostData::Nil => Value::Nil,
        HostData::Bool(b) => Value::Bool(*b),
        HostData::Int(n) => Value::Int(*n),
        HostData::Float(f) => Value::Float(*f),
        HostData::Str(s) => Value::String(cxt.heap_mut().alloc_string(s.clone())),
        HostData::List(items) => {
            let values: Vec<Value> = items.iter().map(|d| data_to_value(cxt, d)).collect();
            Value::List(cxt.heap_mut().alloc_list(values))
        }
        HostData::Record(fields) => {
            let mut map = IndexMap::new();
            for (name, d) in fields {
                let v = data_to_value(cxt, d);
                map.insert(name.clone(), v);
            }
            Value::Map(cxt.heap_mut().alloc_map(map))
        }
    }
}

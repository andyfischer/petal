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
//! the [`DynQueryProvider`] a host implements (core's
//! [`ResourceProvider`], keyed by [`QueryKey`]). The **cache/fetch policy** it
//! used to carry has graduated into the upstream `petal-query` crate
//! (`../core-sdk/petal-query`) — the same path `host_data` took into `petal-ui`.
//! Garden's pipe-backed provider (`garden-app/src/script_client.rs`) now wraps a
//! `petal_query::Cache`, which honors the `petal_query::CachePolicy` each answer
//! carries (fresh / stale-while-revalidate / expired). What remains here is only
//! the binding to Petal's pending values, which depends on the resource table and
//! so stays in-tree.
//!
//! ## Contract
//!
//! A [`DynQueryProvider`] (host-owned, attached to the [`PanelHost`](crate::PanelHost)
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

use petal::record::RecordMap;
use petal::resource_table::hash_key;
use petal::native_fn::{InputClasses, NativeEffects, PetalCxt};
use petal::value::Value;

pub use petal::resource_provider::{ResourceAnswer, ResourceProvider};
pub use petal_ui::host_data::HostData;

/// What a query is named by: the `(kind, arg)` pair of `query(kind, arg)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryKey {
    pub kind: String,
    pub arg: String,
}

impl QueryKey {
    pub fn new(kind: impl Into<String>, arg: impl Into<String>) -> QueryKey {
        QueryKey { kind: kind.into(), arg: arg.into() }
    }
}

/// The state of one query, as its provider reports it for the current frame:
/// core's [`ResourceAnswer`] over [`HostData`].
pub type QueryState = ResourceAnswer<HostData>;

/// A host-side async data source behind the `query(kind, arg)` /
/// `invalidate(kind, arg)` natives: core's [`ResourceProvider`] keyed by
/// [`QueryKey`] and answering [`HostData`]. Attached to a
/// [`PanelHost`](crate::PanelHost) via
/// [`set_query_provider`](crate::PanelHost::set_query_provider); the host
/// installs it into this thread's channel around each `env.run`.
///
/// Both methods are called synchronously inside the frame, so implementations
/// must answer from a cache / non-blocking poll of their background work rather
/// than performing the fetch inline; [`answer`](ResourceProvider::answer) starts
/// the fetch (as a deduped side effect) on the first request for a key.
pub type DynQueryProvider = dyn ResourceProvider<Key = QueryKey, Value = HostData>;

thread_local! {
    /// The query provider of the run currently in progress on this thread. A host
    /// installs its provider here for the duration of `env.run` via
    /// [`swap_query_provider`]; the natives read it. `None` → every `query`
    /// answers `Loading` (a script degrades to a permanent spinner, never a crash).
    static QUERY_PROVIDER: RefCell<Option<Box<DynQueryProvider>>> =
        const { RefCell::new(None) };
}

/// Install `provider` as this thread's active query provider, returning whatever
/// was installed before — the same swap dance as
/// [`host_data::swap_data_provider`](petal_ui::host_data::swap_data_provider),
/// so a panic mid-run leaves the provider in the channel for the host to reclaim.
pub fn swap_query_provider(
    provider: Option<Box<DynQueryProvider>>,
) -> Option<Box<DynQueryProvider>> {
    QUERY_PROVIDER.with(|p| std::mem::replace(&mut *p.borrow_mut(), provider))
}

/// Register the `query(kind, arg)` and `invalidate(kind, arg)` natives. Called
/// from [`register_panel_natives`](crate::panel) alongside the petal-ui set.
pub fn register_query(env: &mut petal::env::Env) {
    // `query` answers from the host's cache, a host-data read. Its loading
    // path also creates a resource — an effect, but one on a call that
    // answers `Pending`, which the memo treats as effectful whatever the row
    // says; so the row leaves it out and a scope that queries ready data
    // stays memoizable (see `petal::effect_audit`).
    env.register_native(
        "query",
        native_query,
        NativeEffects::reads(InputClasses::HOST_DATA),
    );
    env.register_native("invalidate", native_invalidate, NativeEffects::EFFECT);
}

/// `query(kind, arg)` — ask the host's [`DynQueryProvider`] for a resource. Returns
/// the resolved value when [`Ready`](QueryState::Ready), or a `Value::Pending`
/// (loading or errored) otherwise, so the script drives its own loading/error UI
/// through the pending meta functions. Without a provider it answers a loading
/// Pending (graceful degradation — never a crash).
fn native_query(cxt: &mut PetalCxt) -> Result<u32, String> {
    let key = QueryKey::new(cxt.get_string(1)?, cxt.get_string(2)?);
    // The answer lives in the host's cache: tell the frame gate, so the frame
    // re-runs when the pane reports the cache moved (and not otherwise).
    cxt.note_host_read();
    let state = QUERY_PROVIDER
        .with(|p| p.borrow_mut().as_mut().map(|provider| provider.answer(&key)))
        .unwrap_or(QueryState::Loading);
    // A loading or errored answer is stamped with the requesting call site +
    // current frame, so the pending-observability tooling can render its
    // provenance and age.
    cxt.push_answer(hash_key(&key), state, |cxt, data| data_to_value(cxt, &data));
    Ok(1)
}

/// `invalidate(kind, arg)` — tell the host provider to drop `(kind, arg)`'s
/// cached entry so the next frame's `query` refetches. Returns nil. Without a
/// provider it is a no-op.
fn native_invalidate(cxt: &mut PetalCxt) -> Result<u32, String> {
    let key = QueryKey::new(cxt.get_string(1)?, cxt.get_string(2)?);
    // Dropping a cache entry is an effect no replay reproduces. Without this
    // the call leaves the activity counters untouched, so a scope containing
    // it memoizes as a pure function of its arguments and is replayed — and a
    // replayed `invalidate` never runs, which means the pane that asked for
    // fresh data keeps being served the stale entry, forever.
    cxt.note_effect();
    QUERY_PROVIDER.with(|p| {
        if let Some(provider) = p.borrow_mut().as_mut() {
            provider.invalidate(&key);
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
            let mut map = RecordMap::default();
            for (name, d) in fields {
                let v = data_to_value(cxt, d);
                map.insert(name.clone(), v);
            }
            Value::Map(cxt.heap_mut().alloc_map(map))
        }
    }
}

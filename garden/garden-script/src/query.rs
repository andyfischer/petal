//! Garden's answer to `query(kind, arg)`: the [`QueryHost`] behind the
//! `query` package (core-stdlib/query) in a panel.
//!
//! Where [`host_data`](petal_ui::host_data) answers *synchronously* inside the
//! frame (so a provider must already hold the value, or return an ambiguous
//! `nil`), `query` answers from a cache and says when it doesn't have the
//! value yet: the script gets a *pending answer* it reads with `is_ready`,
//! `is_loading`, `is_error` and `error_of`. The panel's per-frame re-run is
//! the retry loop. The natives, the pending answers and their readers are the
//! `query` package's (`petal_stdlib_query`); a panel script reaches them
//! through `import garden`, which re-exports `query`.
//!
//! This module is the binding to Garden's data: the [`DynQueryProvider`] a host
//! implements (a [`ResourceProvider`] keyed by [`QueryKey`]), the per-thread
//! channel that holds the current run's provider, and the [`QueryHost`] that
//! answers from it. The **cache/fetch policy** lives in the `petal-query`
//! crate (`../core-sdk/petal-query`): Garden's pipe-backed provider
//! (`garden-app/src/script_client.rs`) wraps a `petal_query::Cache`, which
//! honors the `petal_query::CachePolicy` each answer carries.
//!
//! ## Contract
//!
//! A [`DynQueryProvider`] (host-owned, attached to the [`PanelHost`](crate::PanelHost)
//! for the duration of each frame) answers `query(kind, arg)` with a
//! [`QueryState`]:
//!
//! - [`Ready`](QueryState::Ready): the resolved [`HostData`] value tree; the
//!   script gets it as an ordinary Petal value.
//! - [`Loading`](QueryState::Loading): still in flight; the script gets a
//!   loading pending answer and can draw a spinner.
//! - [`Errored`](QueryState::Errored): the fetch failed; the script gets a
//!   failed pending answer whose `error_of` is the message.
//!
//! The provider is called synchronously each frame and **must not block**: it
//! polls its own background work (threads, channels) and reports the current
//! state. `invalidate(kind, arg)` drops a cached entry so the next `query` for
//! that key refetches: the primitive a "refresh" button is built on.

use std::cell::RefCell;

use petal::record::RecordMap;
use petal::native_fn::PetalCxt;
use petal::value::Value;
use petal_stdlib_query::QueryHost;

pub use petal_stdlib_query::{ResourceAnswer, ResourceProvider};
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
    install_host();
    QUERY_PROVIDER.with(|p| std::mem::replace(&mut *p.borrow_mut(), provider))
}

/// Register the `query` package on a panel's `env` and make Garden's channel
/// this thread's [`QueryHost`]. Called from
/// [`register_panel_natives`](crate::panel) alongside the petal-ui set.
pub fn register_query(env: &mut petal::env::Env) {
    petal_stdlib_query::register(env);
    install_host();
}

/// Make [`GardenQueryHost`] this thread's [`QueryHost`]. Idempotent; done on
/// registration and again whenever a run swaps its provider in, so the run's
/// thread has it whichever thread registered the panel.
fn install_host() {
    petal_stdlib_query::swap_host(Some(Box::new(GardenQueryHost)));
}

/// The [`QueryHost`] of a Garden panel: answers from the provider in this
/// thread's channel ([`swap_query_provider`]), and answers loading when there
/// is none, so a script degrades to a permanent spinner, never a crash.
struct GardenQueryHost;

impl QueryHost for GardenQueryHost {
    fn answer(
        &mut self,
        cx: &mut PetalCxt,
        kind: &str,
        arg: Value,
    ) -> Result<ResourceAnswer<Value>, String> {
        let key = QueryKey::new(kind, arg_string(cx, arg)?);
        let state = QUERY_PROVIDER
            .with(|p| p.borrow_mut().as_mut().map(|provider| provider.answer(&key)))
            .unwrap_or(QueryState::Loading);
        Ok(state.map(|data| data_to_value(cx, &data)))
    }

    fn invalidate(&mut self, cx: &mut PetalCxt, kind: &str, arg: Value) -> Result<(), String> {
        let key = QueryKey::new(kind, arg_string(cx, arg)?);
        QUERY_PROVIDER.with(|p| {
            if let Some(provider) = p.borrow_mut().as_mut() {
                provider.invalidate(&key);
            }
        });
        Ok(())
    }
}

/// A query's argument as Garden keys it: a string.
fn arg_string(cx: &PetalCxt, arg: Value) -> Result<String, String> {
    match arg {
        Value::String(id) => Ok(cx.heap().get_string(id).to_string()),
        other => Err(format!("Expected string at arg 2, got {}", other.type_name())),
    }
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

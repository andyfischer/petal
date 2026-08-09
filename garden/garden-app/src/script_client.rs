//! The host side of a **panel-mode GPP client** (the script-push protocol; see
//! `docs/gpp.md`).
//!
//! A panel-mode client pushes a Petal UI script the host runs in its in-process
//! [`PanelHost`](garden_script::PanelHost), and answers the script's
//! `query(kind, arg)` calls over the pipe. This module is the bridge: a
//! [`ProcessQueryProvider`] implements the synchronous, in-frame
//! [`QueryProvider`] contract by reading a shared cache and **enqueuing** a pipe
//! request for any key it needs; the owning [`PanelView`](crate::panel_view)
//! flushes that queue to the subprocess and feeds `queryResult` answers back into
//! the cache on the poll tick.
//!
//! The cache itself is [`petal_query::Cache`], the reusable graduation of what
//! used to be a hand-rolled `HashMap` here. It honors the
//! [`CachePolicy`](petal_query::CachePolicy) each answer carries on its
//! `cacheControl` field: a fresh answer is served without a refetch, a stale one
//! is served *and* re-requested in the background (stale-while-revalidate), and
//! an expired one falls back to a spinner while it refetches. An answer with no
//! policy is fresh forever (cache until `invalidate`) — the historical behavior.
//!
//! Everything here runs on the main thread (the `PanelHost` and its provider are
//! not `Send`, and the pipe is drained on the main thread), so the shared state
//! is an [`Rc<RefCell<_>>`] rather than an `Arc<Mutex<_>>`. Freshness is measured
//! against [`Instant::now`], which the main thread can read directly.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use garden_script::{HostData, QueryProvider, QueryState};
use petal_query::{Cache, CachePolicy, Lookup};

/// The cache + request bookkeeping shared between a [`ProcessQueryProvider`]
/// (which reads the cache and enqueues requests inside a frame) and the owning
/// [`PanelView`](crate::panel_view) (which flushes requests to the pipe and
/// records answers). A thin wrapper over [`petal_query::Cache`] that adapts its
/// generic API to this module's call sites. Wrap it with [`new_shared`].
#[derive(Default)]
pub struct SharedQueryState {
    cache: Cache<HostData>,
}

/// A shared handle to a [`SharedQueryState`]. Cloned once for the provider (moved
/// into the `PanelHost`) and once for the `PanelView` that owns the pipe.
pub type Shared = Rc<RefCell<SharedQueryState>>;

/// Create a fresh shared query state.
pub fn new_shared() -> Shared {
    Rc::new(RefCell::new(SharedQueryState::default()))
}

impl SharedQueryState {
    /// Report the current [`QueryState`] for `(kind, arg)` and schedule any fetch
    /// the cache's freshness policy calls for. Called synchronously inside a
    /// frame by [`ProcessQueryProvider::query`].
    pub fn lookup(&mut self, kind: &str, arg: &str) -> QueryState {
        match self.cache.lookup(kind, arg, Instant::now()) {
            Lookup::Ready(v) => QueryState::Ready(v),
            Lookup::Errored(e) => QueryState::Errored(e),
            Lookup::Loading => QueryState::Loading,
        }
    }

    /// Record a `queryResult` answer from the client. A `value` resolves the key
    /// to a ready entry; an `error` (with no value) to an errored entry; neither
    /// leaves it pending (the client is still working — we keep waiting without
    /// re-requesting). The answer is stamped with `cache_control` (or
    /// [`CachePolicy::forever`] when the client sent none) so later frames apply
    /// its freshness.
    pub fn resolve(
        &mut self,
        kind: String,
        arg: String,
        value: Option<serde_json::Value>,
        error: Option<String>,
        cache_control: Option<CachePolicy>,
    ) {
        let policy = cache_control.unwrap_or_default();
        let now = Instant::now();
        match (value, error) {
            (Some(v), _) => {
                self.cache
                    .resolve(kind, arg, Ok(json_to_host_data(&v)), policy, now);
            }
            (None, Some(e)) => {
                self.cache.resolve(kind, arg, Err(e), policy, now);
            }
            (None, None) => { /* still loading; the key stays in flight */ }
        }
    }

    /// Drop a cached/requested key so the next `query` re-requests it — the
    /// client-pushed `invalidate` and the script's own `invalidate` both land here.
    pub fn invalidate(&mut self, kind: &str, arg: &str) {
        self.cache.invalidate(kind, arg);
    }

    /// Take the queued `(kind, arg)` requests to send to the client this tick.
    pub fn take_outbox(&mut self) -> Vec<(String, String)> {
        self.cache.take_outbox()
    }
}

/// A [`QueryProvider`] backed by a subprocess over the GPP pipe. `query` never
/// blocks: it answers from the shared cache, and on a miss (or a policy-expired
/// entry) enqueues a request (deduped) and reports [`Loading`](QueryState::Loading);
/// a stale-but-serveable entry is returned while a background refetch is enqueued.
/// The pipe round-trip is driven by the owning `PanelView`, not here.
pub struct ProcessQueryProvider {
    shared: Shared,
}

impl ProcessQueryProvider {
    pub fn new(shared: Shared) -> ProcessQueryProvider {
        ProcessQueryProvider { shared }
    }
}

impl QueryProvider for ProcessQueryProvider {
    fn query(&mut self, kind: &str, arg: &str) -> QueryState {
        self.shared.borrow_mut().lookup(kind, arg)
    }

    fn invalidate(&mut self, kind: &str, arg: &str) {
        self.shared.borrow_mut().invalidate(kind, arg);
    }
}

/// Project a `serde_json::Value` (a `queryResult`'s payload) onto the
/// [`HostData`] tree the panel data channel speaks. JSON writes both integers
/// and reals as `number`, so the split is recovered from the literal: a value
/// that is exactly an integer becomes an [`Int`](HostData::Int), anything
/// fractional (or beyond `i64`) a [`Float`](HostData::Float). Truncating here
/// would be unrecoverable — a client sending a `0.42` ratio for a meter would
/// have the script draw it as `0`.
fn json_to_host_data(v: &serde_json::Value) -> HostData {
    match v {
        serde_json::Value::Null => HostData::Nil,
        serde_json::Value::Bool(b) => HostData::Bool(*b),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => HostData::Int(i),
            (None, Some(f)) => HostData::Float(f),
            // Neither projection fits (a u64 above i64::MAX has no exact f64
            // either); saturate rather than drop the field.
            (None, None) => HostData::Int(i64::MAX),
        },
        serde_json::Value::String(s) => HostData::Str(s.clone()),
        serde_json::Value::Array(items) => {
            HostData::List(items.iter().map(json_to_host_data).collect())
        }
        serde_json::Value::Object(fields) => HostData::Record(
            fields
                .iter()
                .map(|(k, val)| (k.clone(), json_to_host_data(val)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn miss_enqueues_once_and_reports_loading() {
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());

        // First query for an unknown key: Loading, and enqueued for the pipe.
        assert_eq!(provider.query("log", ""), QueryState::Loading);
        // Re-querying the same key every frame does NOT re-enqueue.
        assert_eq!(provider.query("log", ""), QueryState::Loading);
        let outbox = shared.borrow_mut().take_outbox();
        assert_eq!(outbox, vec![("log".to_string(), "".to_string())]);
        // After draining, still no duplicate request (it's marked requested).
        assert_eq!(provider.query("log", ""), QueryState::Loading);
        assert!(shared.borrow_mut().take_outbox().is_empty());
    }

    #[test]
    fn resolve_makes_the_next_query_ready() {
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("commit", "abc");
        shared.borrow_mut().take_outbox();

        shared.borrow_mut().resolve(
            "commit".into(),
            "abc".into(),
            Some(json!({ "body": "diff", "n": 3 })),
            None,
            None,
        );

        match provider.query("commit", "abc") {
            QueryState::Ready(HostData::Record(fields)) => {
                assert!(fields
                    .iter()
                    .any(|(k, v)| k == "body" && matches!(v, HostData::Str(s) if s == "diff")));
                assert!(fields
                    .iter()
                    .any(|(k, v)| k == "n" && matches!(v, HostData::Int(3))));
            }
            other => panic!("expected Ready record, got {other:?}"),
        }
    }

    #[test]
    fn error_answer_reports_errored() {
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("log", "");
        shared
            .borrow_mut()
            .resolve("log".into(), "".into(), None, Some("no repo".into()), None);
        assert_eq!(
            provider.query("log", ""),
            QueryState::Errored("no repo".into())
        );
    }

    #[test]
    fn invalidate_forces_a_refetch() {
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("log", "");
        shared.borrow_mut().take_outbox();
        shared
            .borrow_mut()
            .resolve("log".into(), "".into(), Some(json!({})), None, None);
        assert!(matches!(provider.query("log", ""), QueryState::Ready(_)));

        // Invalidate (as a client push or a script call) drops it; next query
        // re-requests over the pipe.
        provider.invalidate("log", "");
        assert_eq!(provider.query("log", ""), QueryState::Loading);
        assert_eq!(
            shared.borrow_mut().take_outbox(),
            vec![("log".to_string(), "".to_string())]
        );
    }

    #[test]
    fn no_store_answer_serves_value_and_re_requests() {
        // A no_store answer stays serveable (no spinner) but re-enqueues a
        // background refetch on the very next query — the wire path for
        // CachePolicy::no_store carried through resolve().
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("live", "");
        shared.borrow_mut().take_outbox();
        shared.borrow_mut().resolve(
            "live".into(),
            "".into(),
            Some(json!({ "n": 1 })),
            None,
            Some(CachePolicy::no_store()),
        );
        assert!(matches!(provider.query("live", ""), QueryState::Ready(_)));
        // Served AND a refetch was enqueued.
        assert_eq!(
            shared.borrow_mut().take_outbox(),
            vec![("live".to_string(), "".to_string())]
        );
    }

    #[test]
    fn max_age_zero_answer_expires_immediately() {
        // A max_age(0)-with-no-swr answer hard-expires the moment any time
        // passes, so a later query re-requests over the pipe. Uses a real sleep
        // since the cache reads Instant::now() internally.
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("now", "");
        shared.borrow_mut().take_outbox();
        shared.borrow_mut().resolve(
            "now".into(),
            "".into(),
            Some(json!(1)),
            None,
            Some(CachePolicy::max_age(Duration::from_millis(0))),
        );
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(provider.query("now", ""), QueryState::Loading);
        assert_eq!(
            shared.borrow_mut().take_outbox(),
            vec![("now".to_string(), "".to_string())]
        );
    }

    #[test]
    fn json_projection_covers_all_kinds() {
        let v = json!({ "s": "x", "b": true, "i": 7, "nil": null, "list": [1, "a"] });
        let HostData::Record(fields) = json_to_host_data(&v) else {
            panic!("expected record");
        };
        let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        assert!(matches!(get("s"), Some(HostData::Str(s)) if s == "x"));
        assert!(matches!(get("b"), Some(HostData::Bool(true))));
        assert!(matches!(get("i"), Some(HostData::Int(7))));
        assert!(matches!(get("nil"), Some(HostData::Nil)));
        assert!(matches!(get("list"), Some(HostData::List(items)) if items.len() == 2));
    }

    #[test]
    fn json_projection_keeps_fractions() {
        // The channel's one unrecoverable loss: a client's fractional number
        // arriving as an Int. Every value below is one a real client sends —
        // a ratio, a negative delta, a rate — and each must survive as a Float.
        let v = json!({ "ratio": 0.42, "delta": -1.5, "rate": 2.5e-3, "whole": 3.0 });
        let HostData::Record(fields) = json_to_host_data(&v) else {
            panic!("expected record");
        };
        let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
        assert_eq!(get("ratio"), Some(&HostData::Float(0.42)));
        assert_eq!(get("delta"), Some(&HostData::Float(-1.5)));
        assert_eq!(get("rate"), Some(&HostData::Float(2.5e-3)));
        // A float-typed whole number stays a Float — the JSON literal `3.0`
        // records the sender's intent, and `3` is still an Int.
        assert_eq!(get("whole"), Some(&HostData::Float(3.0)));
        assert_eq!(json_to_host_data(&json!(3)), HostData::Int(3));
    }

    #[test]
    fn resolved_floats_reach_the_script_side_unrounded() {
        // End of the client->host leg: what a `queryResult` carrying a float
        // hands the panel runtime. The HUD bug was this exact shape — a meter
        // fraction that read back as 0.
        let shared = new_shared();
        let mut provider = ProcessQueryProvider::new(shared.clone());
        provider.query("hud", "");
        shared.borrow_mut().take_outbox();
        shared.borrow_mut().resolve(
            "hud".into(),
            "".into(),
            Some(json!({ "meters": [{ "label": "cpu", "fill": 0.42 }] })),
            None,
            None,
        );

        let QueryState::Ready(HostData::Record(fields)) = provider.query("hud", "") else {
            panic!("expected a ready record");
        };
        let Some((_, HostData::List(meters))) = fields.iter().find(|(k, _)| k == "meters") else {
            panic!("expected a meters list");
        };
        let HostData::Record(meter) = &meters[0] else {
            panic!("expected a meter record");
        };
        assert_eq!(
            meter.iter().find(|(k, _)| k == "fill").map(|(_, v)| v),
            Some(&HostData::Float(0.42))
        );
    }
}

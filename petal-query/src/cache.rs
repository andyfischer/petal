//! [`Cache`] — the host-side `(kind, arg)` answer store that honors
//! [`CachePolicy`].
//!
//! This is the piece a host embeds behind its `query(kind, arg)` data channel.
//! It graduates Garden's hand-rolled `SharedQueryState` into a reusable,
//! renderer-agnostic component: a keyed cache with in-flight de-duplication, an
//! outbox of requests to send, and — new — [`CachePolicy`]-driven freshness so
//! stale answers refetch in the background and expired ones fall back to a
//! spinner.
//!
//! It is generic over the stored value type `V` so it carries no dependency on
//! any renderer or on Petal itself: Garden instantiates `Cache<HostData>` and
//! converts JSON to `HostData` before calling [`resolve`](Cache::resolve). A
//! plain `Cache<serde_json::Value>` works too.
//!
//! # The frame loop a host runs
//!
//! ```text
//! // inside the panel frame, for each query(kind, arg) the script calls:
//! match cache.lookup(kind, arg, now) {
//!     Lookup::Ready(v)   => // hand v to the script
//!     Lookup::Errored(e) => // surface e as an errored pending value
//!     Lookup::Loading    => // hand the script a loading pending value (spinner)
//! }
//!
//! // on the poll tick, before the next frame:
//! for (kind, arg) in cache.take_outbox() { /* send a query request */ }
//! // when an answer arrives:
//! cache.resolve(kind, arg, Ok(value), policy, now);
//! ```
//!
//! `lookup` both reports the current state *and* schedules any needed fetch
//! (first miss, or a stale/expired entry), de-duplicated so a key the script
//! asks for every frame is requested at most once while in flight.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::cache_control::{CachePolicy, Freshness};

/// One resolved entry: the provider's answer plus the policy and time it landed.
struct Entry<V> {
    outcome: Result<V, String>,
    policy: CachePolicy,
    resolved_at: Instant,
}

/// What a [`lookup`](Cache::lookup) found for a `(kind, arg)` this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup<V> {
    /// No fresh/stale answer available; a fetch is (now) in flight. The script
    /// should render a loading state.
    Loading,
    /// A value to hand the script. May be a *stale* value being revalidated in
    /// the background — from the script's point of view it is simply the data.
    Ready(V),
    /// The provider reported this error for the key.
    Errored(String),
}

/// A keyed answer cache with in-flight de-duplication, a request outbox, and
/// [`CachePolicy`]-driven freshness. See the [module docs](self).
pub struct Cache<V> {
    entries: HashMap<(String, String), Entry<V>>,
    /// Keys with a request in flight (sent, awaiting an answer). Dedups so a key
    /// asked for every frame is requested once.
    requested: HashSet<(String, String)>,
    /// Keys needing a request that have not been sent yet. Drained by
    /// [`take_outbox`](Cache::take_outbox).
    outbox: Vec<(String, String)>,
}

impl<V> Default for Cache<V> {
    fn default() -> Cache<V> {
        Cache {
            entries: HashMap::new(),
            requested: HashSet::new(),
            outbox: Vec::new(),
        }
    }
}

impl<V: Clone> Cache<V> {
    /// A fresh, empty cache.
    pub fn new() -> Cache<V> {
        Cache::default()
    }

    /// Report the current state for `(kind, arg)` at time `now`, scheduling a
    /// fetch when one is needed:
    ///
    /// - **Fresh** cached answer → [`Ready`](Lookup::Ready) /
    ///   [`Errored`](Lookup::Errored), no fetch.
    /// - **Stale** answer (past `max_age`, within the SWR window, or
    ///   `no_store`) → the value is still served, *and* a background refetch is
    ///   enqueued (deduped).
    /// - **Expired** or **missing** answer → the entry is dropped and a fetch is
    ///   enqueued (deduped); returns [`Loading`](Lookup::Loading).
    ///
    /// Enqueuing is idempotent per key while a request is in flight.
    pub fn lookup(&mut self, kind: &str, arg: &str, now: Instant) -> Lookup<V> {
        let key = (kind.to_string(), arg.to_string());
        if let Some(entry) = self.entries.get(&key) {
            let age = now.saturating_duration_since(entry.resolved_at);
            match entry.policy.freshness_at(age) {
                Freshness::Fresh => return self.ready_lookup(&key),
                Freshness::Stale => {
                    // Serve the value now; refetch in the background.
                    self.enqueue(key.clone());
                    return self.ready_lookup(&key);
                }
                Freshness::Expired => {
                    self.entries.remove(&key);
                    // fall through to the miss path
                }
            }
        }
        // Miss (or just-expired): request once, report loading.
        self.enqueue(key);
        Lookup::Loading
    }

    /// Turn the cached entry for `key` into a [`Lookup`]. The key must exist.
    fn ready_lookup(&self, key: &(String, String)) -> Lookup<V> {
        match &self.entries[key].outcome {
            Ok(v) => Lookup::Ready(v.clone()),
            Err(e) => Lookup::Errored(e.clone()),
        }
    }

    /// Enqueue a request for `key` unless one is already in flight.
    fn enqueue(&mut self, key: (String, String)) {
        if self.requested.insert(key.clone()) {
            self.outbox.push(key);
        }
    }

    /// Record a provider's answer for `(kind, arg)`, landing at `now` with the
    /// given `policy`. Clears the in-flight mark. A [`no_store`](CachePolicy::no_store)
    /// answer is still stored (so it can be served while the next revalidation
    /// runs); its policy keeps it perpetually stale.
    pub fn resolve(
        &mut self,
        kind: String,
        arg: String,
        outcome: Result<V, String>,
        policy: CachePolicy,
        now: Instant,
    ) {
        let key = (kind, arg);
        self.requested.remove(&key);
        self.entries.insert(
            key,
            Entry {
                outcome,
                policy,
                resolved_at: now,
            },
        );
    }

    /// Drop any cached/in-flight state for `(kind, arg)` so the next
    /// [`lookup`](Cache::lookup) re-requests it — the script's `invalidate(...)`
    /// and a client-pushed invalidate both land here.
    pub fn invalidate(&mut self, kind: &str, arg: &str) {
        let key = (kind.to_string(), arg.to_string());
        self.entries.remove(&key);
        self.requested.remove(&key);
    }

    /// Take the queued `(kind, arg)` requests to send this tick, clearing the
    /// outbox. Each returned key is marked in-flight until it
    /// [`resolve`](Cache::resolve)s or is [`invalidate`](Cache::invalidate)d.
    pub fn take_outbox(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.outbox)
    }

    /// Whether a request for `(kind, arg)` is currently in flight. Mostly for
    /// tests and diagnostics.
    pub fn is_in_flight(&self, kind: &str, arg: &str) -> bool {
        self.requested
            .contains(&(kind.to_string(), arg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn drain(cache: &mut Cache<i64>) -> Vec<(String, String)> {
        cache.take_outbox()
    }

    #[test]
    fn miss_enqueues_once_and_reports_loading() {
        let mut cache: Cache<i64> = Cache::new();
        let t = Instant::now();
        assert_eq!(cache.lookup("log", "", t), Lookup::Loading);
        // Re-querying the same key every frame does not re-enqueue.
        assert_eq!(cache.lookup("log", "", t), Lookup::Loading);
        assert_eq!(drain(&mut cache), vec![("log".into(), "".into())]);
        // After draining, still no duplicate request (it's in flight).
        assert_eq!(cache.lookup("log", "", t), Lookup::Loading);
        assert!(drain(&mut cache).is_empty());
    }

    #[test]
    fn resolve_makes_the_next_lookup_ready() {
        let mut cache: Cache<i64> = Cache::new();
        let t = Instant::now();
        cache.lookup("commit", "abc", t);
        drain(&mut cache);
        cache.resolve("commit".into(), "abc".into(), Ok(3), CachePolicy::forever(), t);
        assert_eq!(cache.lookup("commit", "abc", t), Lookup::Ready(3));
        // Forever policy never refetches.
        assert!(drain(&mut cache).is_empty());
    }

    #[test]
    fn errored_answer_reports_errored() {
        let mut cache: Cache<i64> = Cache::new();
        let t = Instant::now();
        cache.lookup("log", "", t);
        cache.resolve(
            "log".into(),
            "".into(),
            Err("no repo".into()),
            CachePolicy::forever(),
            t,
        );
        assert_eq!(cache.lookup("log", "", t), Lookup::Errored("no repo".into()));
    }

    #[test]
    fn invalidate_forces_a_refetch() {
        let mut cache: Cache<i64> = Cache::new();
        let t = Instant::now();
        cache.lookup("log", "", t);
        drain(&mut cache);
        cache.resolve("log".into(), "".into(), Ok(1), CachePolicy::forever(), t);
        assert!(matches!(cache.lookup("log", "", t), Lookup::Ready(1)));

        cache.invalidate("log", "");
        assert_eq!(cache.lookup("log", "", t), Lookup::Loading);
        assert_eq!(drain(&mut cache), vec![("log".into(), "".into())]);
    }

    #[test]
    fn max_age_expiry_refetches_with_a_spinner() {
        let mut cache: Cache<i64> = Cache::new();
        let t0 = Instant::now();
        cache.lookup("log", "", t0);
        drain(&mut cache);
        cache.resolve(
            "log".into(),
            "".into(),
            Ok(7),
            CachePolicy::max_age(Duration::from_secs(5)),
            t0,
        );
        // Fresh within max_age.
        assert_eq!(cache.lookup("log", "", t0 + Duration::from_secs(4)), Lookup::Ready(7));
        assert!(drain(&mut cache).is_empty());
        // Past max_age with no SWR window: hard-expire -> Loading + refetch.
        assert_eq!(
            cache.lookup("log", "", t0 + Duration::from_secs(6)),
            Lookup::Loading
        );
        assert_eq!(drain(&mut cache), vec![("log".into(), "".into())]);
    }

    #[test]
    fn stale_while_revalidate_serves_stale_and_refetches() {
        let mut cache: Cache<i64> = Cache::new();
        let t0 = Instant::now();
        cache.lookup("log", "", t0);
        drain(&mut cache);
        cache.resolve(
            "log".into(),
            "".into(),
            Ok(7),
            CachePolicy::max_age(Duration::from_secs(5))
                .stale_while_revalidate(Duration::from_secs(60)),
            t0,
        );
        // In the stale window: the OLD value is served AND a refetch enqueued.
        assert_eq!(
            cache.lookup("log", "", t0 + Duration::from_secs(10)),
            Lookup::Ready(7)
        );
        assert_eq!(drain(&mut cache), vec![("log".into(), "".into())]);
        // While that refetch is in flight, no duplicate request.
        assert_eq!(
            cache.lookup("log", "", t0 + Duration::from_secs(11)),
            Lookup::Ready(7)
        );
        assert!(drain(&mut cache).is_empty());
        // The refetch lands with a newer value; now fresh again.
        let t1 = t0 + Duration::from_secs(11);
        cache.resolve(
            "log".into(),
            "".into(),
            Ok(9),
            CachePolicy::max_age(Duration::from_secs(5))
                .stale_while_revalidate(Duration::from_secs(60)),
            t1,
        );
        assert_eq!(cache.lookup("log", "", t1), Lookup::Ready(9));
        assert!(drain(&mut cache).is_empty());
    }

    #[test]
    fn no_store_always_serves_and_always_revalidates() {
        let mut cache: Cache<i64> = Cache::new();
        let t0 = Instant::now();
        cache.lookup("live", "", t0);
        drain(&mut cache);
        cache.resolve("live".into(), "".into(), Ok(1), CachePolicy::no_store(), t0);
        // Serves the value AND enqueues a refetch on the very next lookup.
        assert_eq!(cache.lookup("live", "", t0), Lookup::Ready(1));
        assert_eq!(drain(&mut cache), vec![("live".into(), "".into())]);
    }
}

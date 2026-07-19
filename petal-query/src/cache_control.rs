//! [`CachePolicy`] — how cacheable one query answer is.
//!
//! This is the metadata a provider attaches to each answer so the host can
//! decide, on later frames, whether to serve the cached value, quietly refetch
//! it in the background, or drop it and show a spinner. It is the pull-model
//! cousin of an HTTP `Cache-Control` header: the value crosses the wire on a
//! [`QueryResult`](crate::wire::QueryResult)'s `cacheControl` field and is
//! interpreted by the host's [`Cache`](crate::cache::Cache).
//!
//! # The model
//!
//! A panel script *pulls* `query(kind, arg)` every frame, so "caching" is really
//! "how often do we re-ask the provider, and do we show the old value while we
//! wait?". Two knobs express every useful policy:
//!
//! - **`max_age`** — how long a freshly-fetched answer stays *fresh*. While
//!   fresh, the host never re-asks. `None` (the default) means *fresh forever*:
//!   cache until an explicit `invalidate`, the historical Garden behavior.
//! - **`stale_while_revalidate`** — how long *past* `max_age` the host keeps
//!   serving the stale answer while a background refetch runs. Within this
//!   window the script sees the old value (no spinner); once it elapses the
//!   entry hard-expires and the next `query` returns loading (a spinner) until
//!   the refetch lands.
//!
//! [`no_store`](CachePolicy::no_store) is the special case "never fresh, never
//! expired": the answer is always served *and* always revalidated — the right
//! choice for genuinely live data, with no spinner flicker after the first load.
//!
//! # Examples
//!
//! ```
//! use std::time::Duration;
//! use petal_query::CachePolicy;
//!
//! // A commit diff addressed by hash never changes — cache it forever.
//! let immutable = CachePolicy::forever();
//!
//! // A git log is cheap but changes on commit — refresh every few seconds,
//! // showing the old list while the refresh runs.
//! let log = CachePolicy::max_age(Duration::from_secs(3))
//!     .stale_while_revalidate(Duration::from_secs(60));
//!
//! // A live tail — always revalidate, never show a spinner after load.
//! let live = CachePolicy::no_store();
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How cacheable one query answer is. Attached by a provider to each answer and
/// honored by the host's [`Cache`](crate::cache::Cache). See the [module
/// docs](self) for the model.
///
/// The default ([`forever`](Self::forever)) preserves Garden's historical
/// cache-until-invalidate behavior, so an answer with no policy is unchanged.
/// Durations are carried on the wire as whole milliseconds (`u64`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CachePolicy {
    /// Never treat this answer as fresh: always serve the last value *and*
    /// trigger a background refetch. Live data. When set, `max_age` /
    /// `stale_while_revalidate` are ignored.
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_store: bool,

    /// How long (ms) the answer stays fresh after it lands. `None` = fresh
    /// forever (cache until `invalidate`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_ms: Option<u64>,

    /// How long (ms) *past* `max_age` a stale answer may still be served while a
    /// background refetch runs. `None` = no stale window: at `max_age` the entry
    /// hard-expires and the next query shows a spinner until the refetch lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_while_revalidate_ms: Option<u64>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl CachePolicy {
    /// Fresh forever: never refetched until an explicit `invalidate`. The
    /// default, and the right choice for a value addressed by an immutable key
    /// (a commit hash, a content digest). [`immutable`](Self::immutable) is a
    /// self-documenting alias.
    pub fn forever() -> CachePolicy {
        CachePolicy::default()
    }

    /// Self-documenting alias for [`forever`](Self::forever): the resource at
    /// this `(kind, arg)` can never change, so the answer is cached for the
    /// pane's life.
    pub fn immutable() -> CachePolicy {
        CachePolicy::forever()
    }

    /// Fresh for `max_age`, then stale. Without a
    /// [`stale_while_revalidate`](Self::stale_while_revalidate) window the entry
    /// hard-expires at `max_age` (the next query shows a spinner while it
    /// refetches) — use that when a stale value is worse than a brief spinner.
    pub fn max_age(max_age: Duration) -> CachePolicy {
        CachePolicy {
            no_store: false,
            max_age_ms: Some(max_age.as_millis() as u64),
            stale_while_revalidate_ms: None,
        }
    }

    /// Never fresh: always serve the cached value *and* revalidate it in the
    /// background. Live data with no spinner flicker after the first load. Note
    /// that in a pull-per-frame model this refetches as fast as the host polls,
    /// so prefer [`max_age`](Self::max_age) with a small duration to bound the
    /// refetch rate for merely-frequently-changing data.
    pub fn no_store() -> CachePolicy {
        CachePolicy {
            no_store: true,
            max_age_ms: None,
            stale_while_revalidate_ms: None,
        }
    }

    /// Set the stale-while-revalidate window: how long past `max_age` the stale
    /// answer is served while a background refetch runs before the entry
    /// hard-expires. A builder on top of [`max_age`](Self::max_age).
    pub fn stale_while_revalidate(mut self, window: Duration) -> CachePolicy {
        self.stale_while_revalidate_ms = Some(window.as_millis() as u64);
        self
    }

    /// The freshness of an answer of this policy at `age` (time since it was
    /// fetched). Pure; the [`Cache`](crate::cache::Cache) calls it with a real
    /// elapsed time.
    pub fn freshness_at(&self, age: Duration) -> Freshness {
        if self.no_store {
            // Never fresh, never expired: always serve + always revalidate.
            return Freshness::Stale;
        }
        let Some(max_age_ms) = self.max_age_ms else {
            // No max_age: fresh forever.
            return Freshness::Fresh;
        };
        let age_ms = age.as_millis() as u64;
        if age_ms <= max_age_ms {
            Freshness::Fresh
        } else {
            match self.stale_while_revalidate_ms {
                Some(swr) if age_ms <= max_age_ms.saturating_add(swr) => Freshness::Stale,
                _ => Freshness::Expired,
            }
        }
    }
}

/// The freshness verdict for a cached answer at some age — the output of
/// [`CachePolicy::freshness_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Within `max_age`: serve the cached value, do not refetch.
    Fresh,
    /// Past `max_age` but inside the stale-while-revalidate window (or
    /// `no_store`): serve the cached value *and* trigger a background refetch.
    Stale,
    /// Past `max_age + stale_while_revalidate`: drop the value; the next query
    /// is a miss (loading) that refetches.
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn forever_is_always_fresh() {
        let p = CachePolicy::forever();
        assert_eq!(p.freshness_at(Duration::ZERO), Freshness::Fresh);
        assert_eq!(
            p.freshness_at(Duration::from_secs(365 * 24 * 3600)),
            Freshness::Fresh
        );
    }

    #[test]
    fn max_age_without_swr_hard_expires() {
        let p = CachePolicy::max_age(Duration::from_secs(5));
        assert_eq!(p.freshness_at(Duration::from_secs(4)), Freshness::Fresh);
        assert_eq!(p.freshness_at(Duration::from_secs(5)), Freshness::Fresh);
        // One tick past max_age with no stale window -> straight to Expired.
        assert_eq!(p.freshness_at(Duration::from_millis(5001)), Freshness::Expired);
    }

    #[test]
    fn max_age_with_swr_serves_stale_then_expires() {
        let p = CachePolicy::max_age(Duration::from_secs(5))
            .stale_while_revalidate(Duration::from_secs(10));
        assert_eq!(p.freshness_at(Duration::from_secs(4)), Freshness::Fresh);
        assert_eq!(p.freshness_at(Duration::from_secs(9)), Freshness::Stale);
        assert_eq!(p.freshness_at(Duration::from_secs(15)), Freshness::Stale);
        assert_eq!(p.freshness_at(Duration::from_secs(16)), Freshness::Expired);
    }

    #[test]
    fn no_store_is_always_stale() {
        let p = CachePolicy::no_store();
        assert_eq!(p.freshness_at(Duration::ZERO), Freshness::Stale);
        assert_eq!(p.freshness_at(Duration::from_secs(999)), Freshness::Stale);
    }

    #[test]
    fn default_answer_omits_all_fields_on_the_wire() {
        // A forever policy serializes to `{}` so it adds nothing to the wire.
        let v = serde_json::to_value(CachePolicy::forever()).unwrap();
        assert_eq!(v, json!({}));
    }

    #[test]
    fn policy_round_trips_camel_case_ms() {
        let p = CachePolicy::max_age(Duration::from_secs(3))
            .stale_while_revalidate(Duration::from_secs(60));
        let v = serde_json::to_value(p).unwrap();
        assert_eq!(v["maxAgeMs"], 3000);
        assert_eq!(v["staleWhileRevalidateMs"], 60000);
        assert!(v.get("noStore").is_none());
        let back: CachePolicy = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn no_store_round_trips() {
        let v = serde_json::to_value(CachePolicy::no_store()).unwrap();
        assert_eq!(v["noStore"], true);
        let back: CachePolicy = serde_json::from_value(v).unwrap();
        assert!(back.no_store);
    }
}

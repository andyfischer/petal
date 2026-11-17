//! The host side of pending resources: the shape a host's data source answers
//! in ([`ResourceAnswer`], [`ResourceProvider`]) and the report of a batch
//! host's settle loop ([`crate::env::Env::run_until_settled`]).
//!
//! A native that fetches something asks its host for the key and returns what
//! the host says: the value, a loading pending value, or an errored one
//! ([`PetalCxt::push_answer`](crate::native_fn::PetalCxt::push_answer) does
//! the three cases). How the host gets the value — a queue, a thread pool, a
//! cache with expiry, retries, politeness delays — is its own business; core
//! owns only the answer's shape, which keys a run asked for and didn't get
//! ([`crate::resource_table::ResourceTable::unanswered_since`]), and the loop
//! that re-runs until a run asks for nothing new.

/// A host's answer for one keyed resource, as of now. Mirrors React Query /
/// Elm `RemoteData`. `T` is the host's own value type (a JSON tree, a
/// `HostData`, a heap [`Value`](crate::value::Value)); a native turns a
/// `Ready` one into a Petal value.
#[derive(Clone, Debug, PartialEq)]
pub enum ResourceAnswer<T> {
    /// Not there yet (in flight, or not started). → a loading `Value::Pending`.
    Loading,
    /// Landed with this value. → the value itself.
    Ready(T),
    /// Failed with this message. → an errored `Value::Pending` whose
    /// `error_of` is the message.
    Errored(String),
}

/// A host's keyed data source, read by its natives inside a run. Answers from
/// what the host already has and never blocks: a key it doesn't have yet is
/// [`Loading`](ResourceAnswer::Loading), and fetching it is the host's
/// business (start it as a side effect of the first ask, or leave it to a
/// settle loop, which hands over the keys a run left loading).
///
/// Garden's `query(kind, arg)` is one (its `Key` is the `(kind, arg)` pair);
/// a scraper's fetch natives are another (`Key = str`, a URL).
pub trait ResourceProvider {
    /// What a resource is named by. `?Sized`, so a provider keyed by URL can
    /// take `&str`.
    type Key: ?Sized;
    /// What a ready resource holds.
    type Value;

    /// The current answer for `key`.
    fn answer(&mut self, key: &Self::Key) -> ResourceAnswer<Self::Value>;

    /// Drop what is held for `key`, so the next [`answer`](Self::answer)
    /// fetches it again. Does nothing by default.
    fn invalidate(&mut self, _key: &Self::Key) {}
}

/// What a settle loop's host did with one requested key, as its resolve
/// callback reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// The host has the value; the next run's native answers it.
    Resolved,
    /// The fetch failed; the next run's native answers an errored pending
    /// value with this message.
    Errored(String),
}

/// One run of [`crate::env::Env::run_until_settled`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunReport {
    /// The run's index, from 0.
    pub run: usize,
    /// The resource keys the run asked for and was answered loading, each once
    /// — what the host was handed.
    pub requested: Vec<u64>,
    /// Of those, the ones the host resolved.
    pub resolved: Vec<u64>,
    /// Of those, the ones whose fetch failed, with the message.
    pub errored: Vec<(u64, String)>,
    /// The run's script error, if it failed. A run that failed while it still
    /// had requests out is retried once they are answered (an index past the
    /// end of a list that was still loading, say); only the last run's error
    /// is the loop's.
    pub error: Option<String>,
}

/// What [`crate::env::Env::run_until_settled`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct SettleReport {
    /// Every run, in order. Never empty.
    pub runs: Vec<RunReport>,
    /// Whether the last run asked for nothing new. False when the loop stopped
    /// at its run limit, or because the host left a key loading.
    pub settled: bool,
    /// Keys the last run still left loading: the host was handed them and
    /// didn't resolve them, or the loop hit its run limit first. Empty when
    /// [`settled`](Self::settled).
    pub unanswered: Vec<u64>,
    /// The last run's result, `None` when it failed (its
    /// [`error`](RunReport::error) says why). Lives in the stack's heap.
    pub value: Option<crate::value::Value>,
}

impl SettleReport {
    /// The last run's report.
    pub fn last(&self) -> &RunReport {
        self.runs.last().expect("a settle loop runs at least once")
    }
}

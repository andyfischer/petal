//! The natives of Petal's `query` package (`core-stdlib/query`): the script
//! side of petal-query.
//!
//! `query(kind, arg)` asks the host for data and answers either the value or
//! a **pending answer**, a `Value::Handle` of this crate's handle class that
//! records the data as still loading or as failed. `is_ready`, `is_loading`,
//! `is_error` and `error_of` read one. Anything else a script does with a
//! pending answer — a field read, arithmetic, drawing it — is the ordinary
//! error any handle gets, so a script that forgets to test sees why, where it
//! happened, instead of a blank region.
//!
//! ```
//! let mut env = petal::env::Env::new();
//! petal_stdlib_query::register(&mut env);
//! env.set_echo(false);
//! let pid = env.load_program("import query\nprint(is_loading(query(\"user\", 7)))").unwrap();
//! let stack = env.create_stack(pid).unwrap();
//! env.run(stack).unwrap();
//! assert_eq!(env.take_output(), ["true"]);
//! ```
//!
//! **Where the data comes from is the host's.** It installs a [`QueryHost`]
//! for the thread its runs happen on ([`swap_host`]); with none installed,
//! every query answers loading, so a script degrades to its loading path
//! rather than crashing. A host's other natives can hand a script a pending
//! answer too, with [`push_answer`] / [`push_loading`] / [`push_failed`].
//!
//! **A batch host** (a report builder, a test), which has no frame loop to
//! retry for it, runs a script with [`run_until_settled`]: run, hand the host
//! what the run asked for and didn't get, run again, until a run asks for
//! nothing new.
//!
//! A pending answer is immutable: each distinct `(name, state)` gets its own
//! entry in a per-thread table, so the readers are constant functions of the
//! handle they are given. The table holds one entry per distinct pending
//! state seen on the thread, which is bounded by the queries a script makes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use petal::env::Env;
use petal::stack::StackKey;
use petal::handle::{HandleClass, HandleClassId, HandleVal};
use petal::native_fn::{InputClasses, NativeEffects, NativeResult, PetalCxt};
use petal::value::Value;

/// The package name: scripts write `import query`.
pub const PACKAGE_NAME: &str = "query";

/// The package's manifest, `core-stdlib/query/petal.toml`.
pub const MANIFEST: &str = include_str!("../../petal.toml");

/// The package's one module, `core-stdlib/query/src/query.ptl`: the
/// `native fn` declarations and their docs.
pub const SOURCE: &str = include_str!("../../src/query.ptl");

/// The name of the handle class a pending answer belongs to.
pub const HANDLE_CLASS: &str = "pending answer";

/// The package's files as [`Env::register_package`] takes them.
pub fn package_modules() -> [(&'static str, &'static str); 2] {
    [
        (PACKAGE_NAME, SOURCE),
        (petal::package::MANIFEST_FILE, MANIFEST),
    ]
}

// ── Answers ──────────────────────────────────────────────────────────────

/// A host's answer for one keyed resource, as of now. Mirrors React Query /
/// Elm `RemoteData`. `T` is whatever the host holds; [`push_answer`] takes
/// one over Petal values.
#[derive(Clone, Debug, PartialEq)]
pub enum ResourceAnswer<T> {
    /// Not there yet (in flight, or not started): a loading pending answer.
    Loading,
    /// Landed with this value: the value itself.
    Ready(T),
    /// Failed with this message: a failed pending answer whose `error_of` is
    /// the message.
    Errored(String),
}

impl<T> ResourceAnswer<T> {
    /// The same answer with its ready value mapped.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> ResourceAnswer<U> {
        match self {
            ResourceAnswer::Loading => ResourceAnswer::Loading,
            ResourceAnswer::Ready(v) => ResourceAnswer::Ready(f(v)),
            ResourceAnswer::Errored(m) => ResourceAnswer::Errored(m),
        }
    }
}

/// A host's keyed data source, read by its natives inside a run. Answers from
/// what the host already has and never blocks: a key it doesn't have yet is
/// [`Loading`](ResourceAnswer::Loading), and fetching it is the host's
/// business (start it as a side effect of the first ask, or leave it to a
/// settle loop, which hands over the names a run left loading).
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

/// What answers `query(kind, arg)` and `invalidate(kind, arg)` on this
/// thread. Called synchronously inside a run, so it answers from a cache or a
/// non-blocking poll of its background work; it never fetches inline.
pub trait QueryHost {
    /// The answer for `(kind, arg)` now. A ready answer's value is built on
    /// `cx`'s heap. An `Err` is a script error at the `query` call (an
    /// argument the host does not take, say), not a failed load.
    fn answer(
        &mut self,
        cx: &mut PetalCxt,
        kind: &str,
        arg: Value,
    ) -> Result<ResourceAnswer<Value>, String>;

    /// Drop what is cached for `(kind, arg)`, so the next query asks again.
    fn invalidate(&mut self, _cx: &mut PetalCxt, _kind: &str, _arg: Value) -> Result<(), String> {
        Ok(())
    }
}

thread_local! {
    /// The [`QueryHost`] of the runs on this thread.
    static HOST: RefCell<Option<Box<dyn QueryHost>>> = const { RefCell::new(None) };
    /// Every pending answer minted on this thread.
    static TABLE: RefCell<Table> = RefCell::new(Table::default());
}

/// Install `host` as this thread's [`QueryHost`], returning the one installed
/// before. `None` takes it out, after which every query answers loading.
pub fn swap_host(host: Option<Box<dyn QueryHost>>) -> Option<Box<dyn QueryHost>> {
    HOST.with(|h| std::mem::replace(&mut *h.borrow_mut(), host))
}

// ── The pending-answer table ─────────────────────────────────────────────

/// One pending answer: what it is for, and whether it failed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Entry {
    /// `query("user", 7)`, or whatever name a host native gave it.
    name: String,
    /// `None` while loading; the message once failed.
    error: Option<String>,
}

#[derive(Default)]
struct Table {
    entries: Vec<Entry>,
    slots: HashMap<Entry, u32>,
    /// Names answered loading since the last [`take_asked`], each once, in
    /// order: what a settle loop hands its host.
    asked: Vec<String>,
    asked_set: HashSet<String>,
}

impl Table {
    fn slot(&mut self, entry: Entry) -> u32 {
        if let Some(&slot) = self.slots.get(&entry) {
            return slot;
        }
        let slot = self.entries.len() as u32;
        self.entries.push(entry.clone());
        self.slots.insert(entry, slot);
        slot
    }
}

fn entry(slot: u32) -> Option<Entry> {
    TABLE.with(|t| t.borrow().entries.get(slot as usize).cloned())
}

fn describe(slot: u32) -> String {
    match entry(slot) {
        Some(Entry { name, error: None }) => format!("{name}, still loading"),
        Some(Entry { name, error: Some(msg) }) => format!("{name}, failed: {msg}"),
        None => "an unknown pending answer".to_string(),
    }
}

/// The names answered loading on this thread since the last call, each once:
/// what a run asked for and didn't get.
pub fn take_asked() -> Vec<String> {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.asked_set.clear();
        std::mem::take(&mut t.asked)
    })
}

/// This crate's handle class in the `Env` a native runs in.
fn class_id(cx: &PetalCxt) -> Result<HandleClassId, String> {
    cx.handle_class_id(HANDLE_CLASS).ok_or_else(|| {
        "the query package's natives are registered without its handle class; \
         call petal_stdlib_query::register"
            .to_string()
    })
}

/// The pending answer `v` is, if it is one.
fn pending_entry(cx: &PetalCxt, v: Value) -> Option<Entry> {
    let Value::Handle(h) = v else { return None };
    if cx.handle_class_id(HANDLE_CLASS) != Some(h.class) {
        return None;
    }
    entry(h.slot)
}

/// Push a loading pending answer named `name` (`query("user", 7)`, an image
/// path): what a host native returns for data it does not have yet.
pub fn push_loading(cx: &mut PetalCxt, name: &str) -> Result<(), String> {
    let class = class_id(cx)?;
    let slot = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if t.asked_set.insert(name.to_string()) {
            t.asked.push(name.to_string());
        }
        t.slot(Entry { name: name.to_string(), error: None })
    });
    cx.push_value(Value::Handle(HandleVal { class, slot, serial: 0 }));
    Ok(())
}

/// Push a failed pending answer named `name`, whose `error_of` is `message`.
pub fn push_failed(cx: &mut PetalCxt, name: &str, message: String) -> Result<(), String> {
    let class = class_id(cx)?;
    let slot = TABLE.with(|t| {
        t.borrow_mut().slot(Entry { name: name.to_string(), error: Some(message) })
    });
    cx.push_value(Value::Handle(HandleVal { class, slot, serial: 0 }));
    Ok(())
}

/// Push `answer`: a ready value as itself, otherwise a pending answer named
/// `name`. The body of a host native over a [`ResourceProvider`].
pub fn push_answer(
    cx: &mut PetalCxt,
    name: &str,
    answer: ResourceAnswer<Value>,
) -> Result<(), String> {
    match answer {
        ResourceAnswer::Ready(v) => {
            cx.push_value(v);
            Ok(())
        }
        ResourceAnswer::Loading => push_loading(cx, name),
        ResourceAnswer::Errored(message) => push_failed(cx, name, message),
    }
}

// ── Registration ─────────────────────────────────────────────────────────

/// Register the natives and the handle class only, for a host that registers
/// the package source some other way. Registering twice is harmless.
pub fn register_natives(env: &mut Env) {
    if !env.handle_classes().iter().any(|c| c.name == HANDLE_CLASS) {
        let id = env.register_handle_class(HandleClass {
            name: HANDLE_CLASS.to_string(),
            is_valid: Box::new(|slot, _| entry(slot).is_some()),
            describe: Box::new(|slot, _| describe(slot)),
            call_method: Box::new(|cx, method| {
                let what = match cx.get_value(1) {
                    Ok(Value::Handle(h)) => describe(h.slot),
                    _ => "a pending answer".to_string(),
                };
                Err(format!(
                    "cannot call .{method}() on {what}: test it with is_ready first"
                ))
            }),
        });
        // The readers only look at the table, which a run cannot change.
        env.mark_handle_class_spec_safe(id);
    }
    // `query` answers from the host's cache: a host-data read, so the frame
    // gate re-runs when the host reports the cache moved. A loading answer
    // adds to the table, which no run reads back: not an effect.
    env.register_native("query", n_query, NativeEffects::reads(InputClasses::HOST_DATA));
    env.register_native("invalidate", n_invalidate, NativeEffects::EFFECT);
    // A pending answer is immutable, so each reader is a constant function
    // of its argument.
    env.register_native("is_ready", n_is_ready, NativeEffects::CONST);
    env.register_native("is_loading", n_is_loading, NativeEffects::CONST);
    env.register_native("is_error", n_is_error, NativeEffects::CONST);
    env.register_native("error_of", n_error_of, NativeEffects::CONST);
}

/// Register everything `import query` needs: the natives, then the package
/// (so the natives take its declared parameter names), and announce that this
/// `Env` provides `query` at the manifest's version. Call it before
/// `load_program`.
pub fn register(env: &mut Env) {
    register_natives(env);
    env.register_package(PACKAGE_NAME, package_modules())
        .expect("the query package registers");
    env.provides_package(PACKAGE_NAME)
        .expect("the query package has a version");
}

// ── Natives ──────────────────────────────────────────────────────────────

/// `query("user", 7)`: the name of a query's pending answer.
fn query_name(cx: &PetalCxt, kind: &str, arg: Value) -> String {
    let arg = petal::value::value_to_json_lossy(&arg, cx.heap());
    format!("query({:?}, {arg})", kind)
}

fn n_query(cx: &mut PetalCxt) -> NativeResult {
    let kind = cx.get_string(1)?;
    let arg = cx.get_value(2)?;
    cx.note_host_read();
    // Take the host out while it answers, so it can use `cx` freely.
    let mut host = swap_host(None);
    let answer = match host.as_mut() {
        Some(h) => h.answer(cx, &kind, arg),
        None => Ok(ResourceAnswer::Loading),
    };
    swap_host(host);
    let name = query_name(cx, &kind, arg);
    push_answer(cx, &name, answer?)?;
    Ok(1)
}

fn n_invalidate(cx: &mut PetalCxt) -> NativeResult {
    let kind = cx.get_string(1)?;
    let arg = cx.get_value(2)?;
    // Dropping a cache entry is an effect no replay reproduces: a memoized
    // scope that replayed it would leave the stale entry in place.
    cx.note_effect();
    let mut host = swap_host(None);
    let result = match host.as_mut() {
        Some(h) => h.invalidate(cx, &kind, arg),
        None => Ok(()),
    };
    swap_host(host);
    result?;
    cx.push_nil();
    Ok(1)
}

fn n_is_ready(cx: &mut PetalCxt) -> NativeResult {
    let v = cx.get_value(1)?;
    let ready = pending_entry(cx, v).is_none();
    cx.push_bool(ready);
    Ok(1)
}

fn n_is_loading(cx: &mut PetalCxt) -> NativeResult {
    let v = cx.get_value(1)?;
    let loading = matches!(pending_entry(cx, v), Some(Entry { error: None, .. }));
    cx.push_bool(loading);
    Ok(1)
}

fn n_is_error(cx: &mut PetalCxt) -> NativeResult {
    let v = cx.get_value(1)?;
    let failed = matches!(pending_entry(cx, v), Some(Entry { error: Some(_), .. }));
    cx.push_bool(failed);
    Ok(1)
}

fn n_error_of(cx: &mut PetalCxt) -> NativeResult {
    let v = cx.get_value(1)?;
    match pending_entry(cx, v) {
        Some(Entry { error: Some(msg), .. }) => cx.push_string(msg),
        _ => cx.push_nil(),
    }
    Ok(1)
}

// ── Settle loop ──────────────────────────────────────────────────────────

/// What a settle loop's host did with one requested name, as its resolve
/// callback reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// The host has the value; the next run's native answers it.
    Resolved,
    /// The fetch failed; the next run's native answers a failed pending
    /// answer with this message.
    Errored(String),
}

/// One run of [`run_until_settled`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunReport {
    /// The run's index, from 0.
    pub run: usize,
    /// The names the run asked for and was answered loading, each once, that
    /// the host had not been handed before: what it was handed.
    pub requested: Vec<String>,
    /// Of those, the ones the host resolved.
    pub resolved: Vec<String>,
    /// Of those, the ones whose fetch failed, with the message.
    pub errored: Vec<(String, String)>,
    /// The run's script error, if it failed. A run that failed while it still
    /// had requests out is retried once they are answered; only the last
    /// run's error is the loop's.
    pub error: Option<String>,
}

/// What [`run_until_settled`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct SettleReport {
    /// Every run, in order. Never empty.
    pub runs: Vec<RunReport>,
    /// Whether the last run asked for nothing it didn't get.
    pub settled: bool,
    /// Names the last run still got loading answers for.
    pub unanswered: Vec<String>,
    /// The last run's result, `None` when it failed. Lives in the stack's
    /// heap.
    pub value: Option<Value>,
}

impl SettleReport {
    /// The last run's report.
    pub fn last(&self) -> &RunReport {
        self.runs.last().expect("a settle loop runs at least once")
    }
}

/// Run `stack` until it stops asking for data it doesn't have: the loop a
/// host with no frame loop (a report builder, a scraper, a test) would
/// otherwise write itself.
///
/// Each run starts the stack from the top. After it, the names it was
/// answered loading for that the host hasn't been handed before go to
/// `resolve`, all at once, so the host can fetch them however it likes; it
/// returns what became of each, and a name it leaves out is still loading.
/// The host's natives answer the resolved names in the next run.
///
/// The loop stops after a run that asks for nothing new, or after `max_runs`
/// runs (at least one). Between runs the previous run's `print` output and
/// output buffers are dropped, so afterwards they hold the last run's.
/// `Err` only for an unknown stack.
pub fn run_until_settled<F>(
    env: &mut Env,
    stack: StackKey,
    max_runs: usize,
    mut resolve: F,
) -> Result<SettleReport, String>
where
    F: FnMut(&mut Env, &[String]) -> Vec<(String, Resolution)>,
{
    let max_runs = max_runs.max(1);
    let mut handed: HashSet<String> = HashSet::new();
    let mut runs = Vec::new();
    for run in 0..max_runs {
        if run > 0 {
            env.clear_run_output(stack)?;
        }
        env.reset_stack(stack)?;
        take_asked();
        let result = env.run(stack);
        let unanswered = take_asked();
        let fresh: Vec<String> = unanswered
            .iter()
            .filter(|k| !handed.contains(*k))
            .cloned()
            .collect();
        let mut report = RunReport {
            run,
            requested: fresh.clone(),
            error: result.as_ref().err().cloned(),
            ..RunReport::default()
        };
        if fresh.is_empty() || run + 1 == max_runs {
            runs.push(report);
            return Ok(SettleReport {
                runs,
                settled: unanswered.is_empty(),
                unanswered,
                value: result.ok(),
            });
        }
        for (name, resolution) in resolve(env, &fresh) {
            match resolution {
                Resolution::Resolved => report.resolved.push(name),
                Resolution::Errored(message) => report.errored.push((name, message)),
            }
        }
        handed.extend(fresh);
        // The natives answer from the host's data, which just moved: a
        // memoized scope that read it must not replay.
        env.note_host_data_changed(stack);
        runs.push(report);
    }
    unreachable!("the last run returns")
}

//! Placeholder passes: how a batch host (a scraper, a report builder, a test)
//! runs a script whose data arrives between runs, and fetches every key a
//! run reaches together instead of one stage per run.
//!
//! A host native that does not have its data yet hands the script a
//! **placeholder** with [`push_placeholder`]: a harmless stand-in of the real
//! value's shape (a page record with an empty body, `nil`, an empty list).
//! The script runs on past it, field reads and loops included, and reaches
//! the other things it would ask for. Handing one out marks the run
//! **speculative** and records the name as asked.
//!
//! [`settle`] runs the script until a run hands out no placeholders:
//!
//! - after each run, every name it asked for that the host has not been
//!   handed before goes to the host's `resolve`, all at once;
//! - a speculative run's script error is tolerated (it may have used a
//!   placeholder where real data would not fail), and its outputs (`print`,
//!   output buffers) and the `state` it committed are dropped before the next
//!   run;
//! - the first run that asked for nothing used only real data, so its outputs
//!   are the result, and its error, if any, is a real one.
//!
//! A load that failed is not a placeholder: a native answers it with
//! [`petal_stdlib_query::push_failed`], so the script tests it with
//! `is_error` / `error_of`. A [`petal_stdlib_query::push_loading`] answer
//! counts as asked as well, so a host can mix both kinds of native.
//!
//! ```
//! use petal::env::Env;
//! use petal::native_fn::{InputClasses, NativeEffects};
//! use petal::value::Value;
//! use std::cell::RefCell;
//! use std::rc::Rc;
//!
//! let have: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(None));
//! let mut env = Env::new();
//! env.set_echo(false);
//! let h = have.clone();
//! env.register_native_boxed("price", move |cx| {
//!     match *h.borrow() {
//!         Some(p) => cx.push_value(Value::Int(p)),
//!         None => petal_batch::push_placeholder(cx, "price", Value::Int(0))?,
//!     }
//!     Ok(1)
//! }, NativeEffects::reads(InputClasses::HOST_DATA));
//! let pid = env.load_program("native fn price()\nprint(price() * 2)").unwrap();
//! let stack = env.create_stack(pid).unwrap();
//!
//! let report = petal_batch::settle(&mut env, stack, 10, |_, names| {
//!     assert_eq!(names, ["price"]);
//!     *have.borrow_mut() = Some(21);
//!     vec![("price".to_string(), petal_batch::Resolution::Resolved)]
//! }).unwrap();
//! assert!(report.settled);
//! assert_eq!(report.runs.len(), 2);
//! assert!(report.runs[0].speculative && !report.runs[1].speculative);
//! assert_eq!(env.take_output(), ["42"]);
//! ```

use std::cell::RefCell;
use std::collections::HashSet;

use petal::env::Env;
use petal::native_fn::PetalCxt;
use petal::stack::StackKey;
use petal::value::Value;

/// What the host did with one requested name: the query package's own type,
/// so one resolve step serves placeholders and loading answers alike.
pub use petal_stdlib_query::Resolution;

// ── Placeholders ─────────────────────────────────────────────────────────

/// The placeholders handed out on this thread since the last
/// [`take_placeholders`]: each name once, in order.
#[derive(Default)]
struct Noted {
    names: Vec<String>,
    seen: HashSet<String>,
}

thread_local! {
    static NOTED: RefCell<Noted> = RefCell::new(Noted::default());
}

/// Record that a placeholder named `name` was handed out: the current run is
/// speculative, and `name` is asked for. For a native that pushes the
/// stand-in itself (a record built with its own helpers);
/// [`push_placeholder`] is this plus the push.
///
/// The native should also call `cx.note_host_read()` (and be registered as a
/// `HOST_DATA` reader), so a memoized scope that read a placeholder is not
/// replayed once the real data lands.
pub fn note_placeholder(name: &str) {
    NOTED.with(|n| {
        let mut n = n.borrow_mut();
        if n.seen.insert(name.to_string()) {
            n.names.push(name.to_string());
        }
    });
}

/// Push `value` as the answer of a call for data named `name` that is not
/// here yet, and [note](note_placeholder) it: the run is speculative, and
/// `name` goes to the host's `resolve` after it. `value` is a harmless
/// stand-in the script can read like the real thing (same shape, empty
/// contents), built on `cx`'s heap. Also notes a host-data read. Never fails
/// today; `Result` matches the query package's `push_*`.
pub fn push_placeholder(cx: &mut PetalCxt, name: &str, value: Value) -> Result<(), String> {
    note_placeholder(name);
    cx.note_host_read();
    cx.push_value(value);
    Ok(())
}

/// Whether a placeholder has been handed out on this thread since the last
/// [`take_placeholders`] (which [`settle`] calls before each run): inside a
/// run, whether the run is speculative so far. A
/// [`push_loading`](petal_stdlib_query::push_loading) answer also makes a
/// run speculative in its [`RunReport`], but is not seen here.
pub fn is_speculative() -> bool {
    NOTED.with(|n| !n.borrow().names.is_empty())
}

/// The names of the placeholders handed out on this thread since the last
/// call, each once, in the order first handed out. [`settle`] calls it
/// around each run; a host running its own loop calls it the same way.
pub fn take_placeholders() -> Vec<String> {
    NOTED.with(|n| {
        let mut n = n.borrow_mut();
        n.seen.clear();
        std::mem::take(&mut n.names)
    })
}

// ── Reports ──────────────────────────────────────────────────────────────

/// One run of [`settle`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunReport {
    /// The run's index, from 0.
    pub run: usize,
    /// Whether the run asked for anything (a placeholder or a loading
    /// answer): it may have used stand-ins, so its outputs are not the
    /// result and its error is not a real one. Only the last run can be
    /// non-speculative.
    pub speculative: bool,
    /// The names of the placeholders the run handed out, each once, in order.
    pub placeholders: Vec<String>,
    /// The names the run was answered loading for (the query package's
    /// [`take_asked`](petal_stdlib_query::take_asked)), each once, in order.
    pub loading: Vec<String>,
    /// Of `placeholders` then `loading`, the names the host had not been
    /// handed before: what `resolve` was handed. The last run's are only
    /// reported, never resolved (on a run that hit `max_runs`).
    pub requested: Vec<String>,
    /// Of `requested`, the ones the host resolved.
    pub resolved: Vec<String>,
    /// Of `requested`, the ones whose fetch failed, with the message.
    pub errored: Vec<(String, String)>,
    /// The run's script error, if it failed. Tolerated when the run is
    /// speculative; a real error on the settled run.
    pub error: Option<String>,
}

impl RunReport {
    /// Every name the run asked for: `placeholders` then `loading`, each once.
    pub fn asked(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.placeholders
            .iter()
            .chain(&self.loading)
            .filter(|n| seen.insert(n.as_str()))
            .cloned()
            .collect()
    }
}

/// What [`settle`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct SettleReport {
    /// Every run, in order. Never empty.
    pub runs: Vec<RunReport>,
    /// Whether the last run asked for nothing: it used only real data, so
    /// the outputs the `Env` holds are its, and they are the result. False
    /// when `max_runs` came first, or when a run asked only for names the
    /// host had been handed and left unresolved.
    pub settled: bool,
    /// What the last run still asked for (empty when settled).
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

    /// The settled run's script error: a real error, since that run used no
    /// placeholders. `None` when it succeeded, or when the loop did not
    /// settle (the last run's error, if any, is then in [`last`](Self::last)).
    pub fn error(&self) -> Option<&str> {
        if self.settled { self.last().error.as_deref() } else { None }
    }
}

// ── The settle loop ──────────────────────────────────────────────────────

/// The host side of [`settle_with`]: fetch what a run asked for, and see
/// each run before its outputs are dropped.
pub trait BatchHost {
    /// Fetch `names` (the run's [`requested`](RunReport::requested)), all at
    /// once, however the host likes. Returns what became of each; a name
    /// left out stays unanswered, and the next run's native hands out a
    /// placeholder (or a loading answer) for it again. The natives answer
    /// the resolved names in the next run, and a failed one with
    /// [`push_failed`](petal_stdlib_query::push_failed).
    fn resolve(&mut self, env: &mut Env, names: &[String]) -> Vec<(String, Resolution)>;

    /// Called just before run `run` starts, after the previous run's outputs
    /// were dropped (to turn tracing on, reset per-run counters).
    fn before_run(&mut self, _env: &mut Env, _stack: StackKey, _run: usize) {}

    /// Called after every run, the last included, with its finished report
    /// (resolve has run). The run's `print` output and output buffers are
    /// still in `env`: read them here (peek, or drain what you need) to see
    /// a speculative run's. The next run drops them; the last run's stay in
    /// `env` after [`settle_with`] returns.
    fn after_run(&mut self, _env: &mut Env, _stack: StackKey, _report: &RunReport) {}
}

/// A closure as a [`BatchHost`] with no per-run hooks.
struct ResolveFn<F>(F);

impl<F> BatchHost for ResolveFn<F>
where
    F: FnMut(&mut Env, &[String]) -> Vec<(String, Resolution)>,
{
    fn resolve(&mut self, env: &mut Env, names: &[String]) -> Vec<(String, Resolution)> {
        (self.0)(env, names)
    }
}

/// Run `stack` until a run asks for nothing: [`settle_with`] with a resolve
/// closure and no per-run hooks. Like
/// [`petal_stdlib_query::run_until_settled`], but a run that handed out
/// placeholders is speculative: its error is tolerated and its outputs and
/// state are dropped.
pub fn settle<F>(
    env: &mut Env,
    stack: StackKey,
    max_runs: usize,
    resolve: F,
) -> Result<SettleReport, String>
where
    F: FnMut(&mut Env, &[String]) -> Vec<(String, Resolution)>,
{
    settle_with(env, stack, max_runs, &mut ResolveFn(resolve))
}

/// Run `stack` until a run asks for nothing new, handing `host` what each
/// run asked for in between.
///
/// Each run starts the stack from the top, with the `state` the stack had
/// when the loop began (a speculative run's commits are undone; `memo(key)`
/// entries are dropped with them) and with the previous run's `print` output
/// and output buffers dropped. After a run, `host.resolve` gets the names it
/// asked for (placeholders, then loading answers) that the host has not been
/// handed before; then `host.after_run` sees the run. Resolving reports the
/// host data as changed, so a memoized scope does not replay a placeholder.
///
/// The loop stops after a run that asked for nothing new, or after
/// `max_runs` runs (at least one). It has settled when that last run asked
/// for nothing at all. `Err` only for an unknown stack.
pub fn settle_with(
    env: &mut Env,
    stack: StackKey,
    max_runs: usize,
    host: &mut dyn BatchHost,
) -> Result<SettleReport, String> {
    let max_runs = max_runs.max(1);
    let start_state = env.snapshot_state(stack).ok_or("Stack not found")?;
    let mut handed: HashSet<String> = HashSet::new();
    let mut runs = Vec::new();
    for run in 0..max_runs {
        if run > 0 {
            env.clear_run_output(stack)?;
            env.restore_state(stack, start_state.clone());
        }
        env.reset_stack(stack)?;
        take_placeholders();
        petal_stdlib_query::take_asked();
        host.before_run(env, stack, run);

        let result = env.run(stack);

        let mut report = RunReport {
            run,
            placeholders: take_placeholders(),
            loading: petal_stdlib_query::take_asked(),
            error: result.as_ref().err().cloned(),
            ..RunReport::default()
        };
        let asked = report.asked();
        report.speculative = !asked.is_empty();
        let fresh: Vec<String> = asked.iter().filter(|n| !handed.contains(*n)).cloned().collect();
        let last = fresh.is_empty() || run + 1 == max_runs;
        report.requested = fresh.clone();
        if !last {
            for (name, resolution) in host.resolve(env, &fresh) {
                match resolution {
                    Resolution::Resolved => report.resolved.push(name),
                    Resolution::Errored(message) => report.errored.push((name, message)),
                }
            }
            handed.extend(fresh);
            // The natives answer from the host's data, which just moved.
            env.note_host_data_changed(stack);
        }
        host.after_run(env, stack, &report);
        runs.push(report);
        if last {
            return Ok(SettleReport {
                runs,
                settled: asked.is_empty(),
                unanswered: asked,
                value: result.ok(),
            });
        }
    }
    unreachable!("the last run returns")
}

# petal-batch

Placeholder passes: how a **batch host** (a scraper, a report builder, a
test) runs a Petal script whose data arrives between runs, and fetches every
key a run reaches *together*, instead of one stage per run.

```toml
petal-batch = { path = "<petal>/core-sdk/petal-batch" }
```

## Why

A value the host has not got yet used to be an absorbing pending value in
the core language: `page.url` on a loading page was itself pending, a `for`
over a pending list ran zero times, and the script ran on to its other
requests. That is gone ([core-stdlib/query](../../core-stdlib/query/README.md#why-this-is-a-library)):
an answer not loaded yet is now an opaque `query` handle, and using it as
data is an error at that line. The language will not get absorption back,
and a library cannot rebuild it (a handle class only intercepts method
calls).

So with plain `query` handles a batch script either tests every answer
before using it or fails at the first one, and either way a run stops
asking at the first thing it lacks. That is the cost the query README's
"Still open" note names: `query("team", query("user", id).team_id)` takes one
run per stage, and a page linking 50 others takes 50 stages if the script
reads each before asking for the next.

This crate moves the stand-in to the **host**. A native that does not have
its data yet answers a **placeholder**: an ordinary value of the real one's
shape with empty contents (a page record with `status: 0` and an empty body,
`nil`, an empty list). The script runs on with it and reaches the rest of
what it would ask for, so a loop over 50 links asks for all 50 in one run.
Handing one out marks the run **speculative**: its results may be built from
stand-ins, so they are thrown away, and an error it hit (`fetch(nil)` on a
placeholder's empty `next`) does not count. The loop **settles** on the first
run that handed out no placeholders. That run used only real data, so its
outputs are the result and its error, if any, is real.

## API

```rust
// In a native:
pub fn push_placeholder(cx: &mut PetalCxt, name: &str, value: Value) -> Result<(), String>;
pub fn note_placeholder(name: &str);          // mark + record, the native pushes the stand-in itself
pub fn is_speculative() -> bool;              // has this run handed out a placeholder so far
pub fn take_placeholders() -> Vec<String>;    // names noted since the last call (settle calls it)

// The loop:
pub fn settle<F>(env: &mut Env, stack: StackKey, max_runs: usize, resolve: F)
    -> Result<SettleReport, String>
where F: FnMut(&mut Env, &[String]) -> Vec<(String, Resolution)>;
pub fn settle_with(env: &mut Env, stack: StackKey, max_runs: usize, host: &mut dyn BatchHost)
    -> Result<SettleReport, String>;

pub trait BatchHost {
    fn resolve(&mut self, env: &mut Env, names: &[String]) -> Vec<(String, Resolution)>;
    fn before_run(&mut self, env: &mut Env, stack: StackKey, run: usize) {}
    fn after_run(&mut self, env: &mut Env, stack: StackKey, report: &RunReport) {}
}
pub use petal_stdlib_query::Resolution;       // Resolved | Errored(String)
```

`RunReport { run, speculative, placeholders, loading, requested, resolved,
errored, error }` (plus `asked()`), and `SettleReport { runs, settled,
unanswered, value }` (plus `last()` and `error()`, the settled run's real
error).

A native over a host cache:

```rust
fn n_fetch(cx: &mut PetalCxt) -> NativeResult {
    let url = cx.get_string(1)?;
    match cache().get(&url) {
        Some(Ok(page)) => cx.push_value(page_value(cx, page)),
        // A failed load is a query failed answer: `is_error` / `error_of` read it.
        Some(Err(msg)) => petal_stdlib_query::push_failed(cx, &url, msg)?,
        None => {
            let stand_in = page_value(cx, &Page::placeholder(&url));
            petal_batch::push_placeholder(cx, &url, stand_in)?;
        }
    }
    Ok(1)
}
// Registered as a host-data read, so a memoized scope never replays a stand-in:
env.register_native("fetch", n_fetch, NativeEffects::reads(InputClasses::HOST_DATA));
```

## The loop

`settle_with` runs the stack from the top until a run asks for nothing new:

1. Before every run but the first, the previous run's `print` output and
   output buffers are dropped, and the stack's `state` is put back to what it
   was when the loop began (`memo(key)` entries go with it). A speculative run
   leaves nothing behind, so a `state x = fetch(url).title` initializer is
   committed from the real page, not the stand-in.
2. `before_run`, then the run.
3. The names it asked for: its placeholders, then the
   [`push_loading`](../../core-stdlib/query/README.md#hosts) answers it got
   (`petal_stdlib_query::take_asked`), so natives of both kinds mix. The run
   is speculative if there are any.
4. The names not handed to the host before go to `resolve`, all at once
   (not on the last run). A name `resolve` leaves out stays unanswered; the
   next run's native hands out a placeholder for it again, which is nothing
   new, so the loop stops there.
5. `after_run` with the finished report. **The run's outputs are still in
   the `Env` here**: this is where a host reads a speculative run's emits or
   per-run trace stats. The final run's outputs stay in the `Env` after the
   loop returns.

It stops after a run that asked for nothing new, or after `max_runs` runs
(at least one). `settled` says the last run asked for nothing at all.

**Cost.** Breadth is free: everything one run can reach is one batch.
Depth is not: each level of a waterfall (a page whose link is on a page not
fetched yet) is one more run, since the placeholder's link is empty. A
speculative run does all the script's work on stand-ins, so a host keeps
placeholders cheap and the script's work on them harmless.

**Placeholders must be harmless.** A run on stand-ins still calls every
native the script reaches. An effect outside the run (writing a file,
sending a message) belongs in the run's outputs, applied by the host from
the settled run's only. `is_speculative()` in a native says only whether a
placeholder was handed out *so far*: a run can turn speculative later.

The tests in [`tests/settle.rs`](tests/settle.rs) cover breadth, waterfalls,
tolerated and real errors, failed answers, the run cap, the hooks, state
undo and mixing with loading answers.

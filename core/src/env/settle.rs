//! The batch host's loop over pending resources: run, hand the host what the
//! run asked for and didn't get, run again — until a run asks for nothing new.
//! See [`Env::run_until_settled`] and [`crate::resource_provider`].

use std::collections::HashSet;

use super::*;

use crate::resource_provider::{Resolution, RunReport, SettleReport};
use crate::resource_table::ResourceTable;

impl Env {
    /// The resource table of the context `stack_id` runs in: every pending
    /// resource its natives created, with state and provenance. `None` for an
    /// unknown stack.
    pub fn resources(&self, stack_id: StackKey) -> Option<&ResourceTable> {
        self.ctx_for(stack_id).map(|ck| &self.ctx(ck).resources)
    }

    /// [`resources`](Self::resources), writable: a host that keeps its answers
    /// in the table itself lands them with
    /// [`ResourceTable::resolve`] / [`ResourceTable::reject`].
    pub fn resources_mut(&mut self, stack_id: StackKey) -> Option<&mut ResourceTable> {
        let ck = self.ctx_for(stack_id)?;
        Some(&mut self.ctx_mut(ck).resources)
    }

    /// The current frame of the context `stack_id` runs in (see
    /// [`advance_frame`](Self::advance_frame)); 0 for an unknown stack.
    pub fn frame(&self, stack_id: StackKey) -> u64 {
        self.ctx_for(stack_id).map(|ck| self.ctx(ck).frame()).unwrap_or(0)
    }

    /// The keys `stack_id`'s natives asked for in the current frame and were
    /// answered loading, each once
    /// ([`ResourceTable::unanswered_since`] at [`frame`](Self::frame)). Read
    /// after a run, with the frame advanced before it, these are the requests
    /// that run left for the host. Empty for an unknown stack.
    pub fn unanswered_requests(&self, stack_id: StackKey) -> Vec<u64> {
        let frame = self.frame(stack_id);
        self.resources(stack_id)
            .map(|t| t.unanswered_since(frame))
            .unwrap_or_default()
    }

    /// Run `stack_id` until it stops asking for resources it doesn't have —
    /// the loop a batch host of pending values (a scraper, a report builder)
    /// would otherwise write itself.
    ///
    /// Each run starts the stack from the top in a new frame
    /// ([`reset_stack`](Self::reset_stack), [`advance_frame`](Self::advance_frame)).
    /// After it, the keys it was answered loading for and that the host hasn't
    /// been handed before go to `resolve`, all at once, so the host can fetch
    /// them however it likes (in parallel, through a rate limiter, from a
    /// cache). `resolve` gets the `Env` too, for a host that keeps answers in
    /// the [resource table](Self::resources_mut) or wants the run's output,
    /// and returns what became of each key; a key it leaves out is still
    /// loading. The host's natives answer the resolved keys in the next run
    /// (see [`crate::resource_provider::ResourceProvider`]).
    ///
    /// The loop stops after a run that asks for nothing new — settled, or
    /// stuck on a key the host left loading — or after `max_runs` runs (at
    /// least one). A run that fails while it has new requests out is retried
    /// after they are answered (a script may index past the end of a list
    /// that was still loading); the last run's error is the loop's.
    ///
    /// Between runs the previous run's `print` output and output buffers are
    /// dropped, so after the loop they hold the last run's; `resolve` can
    /// drain a run's before that. `Err` only for an unknown stack.
    pub fn run_until_settled<F>(
        &mut self,
        stack_id: StackKey,
        max_runs: usize,
        mut resolve: F,
    ) -> Result<SettleReport, String>
    where
        F: FnMut(&mut Env, &[u64]) -> Vec<(u64, Resolution)>,
    {
        let ck = self.ctx_for(stack_id).ok_or("Stack not found")?;
        let max_runs = max_runs.max(1);
        let mut handed: HashSet<u64> = HashSet::new();
        let mut runs = Vec::new();
        for run in 0..max_runs {
            if run > 0 {
                self.ctx_mut(ck).clear_run_output();
            }
            self.reset_stack(stack_id)?;
            self.advance_frame(stack_id);
            let frame = self.ctx(ck).frame();
            let result = self.run(stack_id);
            let unanswered = self.ctx(ck).resources.unanswered_since(frame);
            let fresh: Vec<u64> = unanswered
                .iter()
                .copied()
                .filter(|k| !handed.contains(k))
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
            for (key, resolution) in resolve(self, &fresh) {
                match resolution {
                    Resolution::Resolved => report.resolved.push(key),
                    Resolution::Errored(message) => report.errored.push((key, message)),
                }
            }
            handed.extend(fresh);
            // The natives answer from the host's data, which just moved: a
            // memoized scope that read it must not replay.
            self.note_host_data_changed(stack_id);
            runs.push(report);
        }
        unreachable!("the last run returns")
    }
}

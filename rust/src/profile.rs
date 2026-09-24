//! Opt-in execution profiling: what the VM actually spent its instructions on.
//!
//! [`DupStats`](crate::stats::DupStats) answers "how much did immutability cost
//! us?"; this answers the prior question — "which opcodes and which builtins is
//! a program running at all?" — which is where an optimization effort starts.
//!
//! Unlike the duplication counters, collection is a **runtime** switch rather
//! than a compile-time one, so a shipped release binary can profile a slow
//! script without a rebuild: `petal run --profile <file>`. When
//! [`enabled`](VmProfile::enabled) is false every `record_*` is one
//! predictable branch, which does not measurably move the benchmarks.
//!
//! The counts are exact, but note what a *count* can and cannot tell you: it
//! says a program executed 4 M `GetField`s, not that `GetField` is slow. Pair it
//! with a sampling profiler (`cargo build --profile profiling`, then `sample`)
//! to turn a large count into a time attribution.

use std::fmt;
use std::time::Duration;

use crate::program::FunctionId;

use crate::backend::bytecode::isa::{Inst, Opcode};

/// Execution counters for one profiled session. Lives on the
/// [`Env`](crate::env::Env) and accumulates across every run on it until
/// [`reset`](VmProfile::reset) — so a host driving 60 frames a second gets
/// totals over all of them, and the per-frame figure is that divided by the
/// frame count.
#[derive(Debug, Clone)]
pub struct VmProfile {
    /// Master switch. Every recording path early-returns when this is false.
    pub enabled: bool,
    /// Instructions retired, per opcode.
    by_opcode: [u64; Opcode::COUNT],
    /// Builtin/native invocations, indexed by `NativeFnId`. Grown on demand
    /// because the table's size is a host decision (embedders register their
    /// own natives), not a constant.
    by_native: Vec<u64>,
    /// Instructions retired per function, indexed by [`fn_slot`]: slot 0 is the
    /// implicit root function, slot `n + 1` is `FunctionId(n)`. Counted where
    /// the instruction executes, so it is *self* work — a function's callees
    /// are charged to the callees. Grown on demand like `by_native`.
    by_function: Vec<u64>,
    /// Wall time spent inside each native (host callbacks included), by
    /// `NativeFnId`. Intrinsics that call back into Petal (`map`, `filter`,
    /// ...) are not timed: their time is the closures' instructions.
    native_time: Vec<Duration>,
    /// Wall time spent in natives called directly from each function, by
    /// [`fn_slot`] — the part of a function's cost that its instruction count
    /// cannot show.
    fn_native_time: Vec<Duration>,
    /// User-function calls (`Call`/`MethodCall` reaching a Petal function),
    /// i.e. how many VM frames were pushed.
    pub calls: u64,
    /// Garbage collections run, and the wall time they took.
    pub collections: u64,
    pub gc_time: Duration,
}

impl Default for VmProfile {
    fn default() -> Self {
        VmProfile {
            enabled: false,
            // `[u64; N]` only derives Default up to N = 32.
            by_opcode: [0; Opcode::COUNT],
            by_native: Vec::new(),
            by_function: Vec::new(),
            native_time: Vec::new(),
            fn_native_time: Vec::new(),
            calls: 0,
            collections: 0,
            gc_time: Duration::ZERO,
        }
    }
}

impl VmProfile {
    pub fn new() -> Self {
        Self::default()
    }

    /// Turn collection on (or off), clearing whatever was collected before —
    /// enabling is the start of a measurement, so it should not inherit counts.
    pub fn set_enabled(&mut self, on: bool) {
        self.reset();
        self.enabled = on;
    }

    /// Record one retired instruction. Inlined and branch-first so a
    /// non-profiling run pays a single predictable test.
    #[inline(always)]
    pub fn record_inst(&mut self, inst: &Inst) {
        if !self.enabled {
            return;
        }
        self.by_opcode[inst.opcode() as usize] += 1;
    }

    /// Record one retired instruction of function `func` (`None` = root). The
    /// per-opcode count is [`record_inst`](Self::record_inst)'s; this adds the
    /// per-function attribution. Only reached with hooks on.
    #[inline]
    pub fn record_inst_in(&mut self, inst: &Inst, func: Option<FunctionId>) {
        if !self.enabled {
            return;
        }
        self.by_opcode[inst.opcode() as usize] += 1;
        let slot = fn_slot(func);
        if slot >= self.by_function.len() {
            self.by_function.resize(slot + 1, 0);
        }
        self.by_function[slot] += 1;
    }

    /// Record the wall time one call of native `nid` took, made from function
    /// `caller` (`None` = root).
    pub fn record_native_time(&mut self, nid: u32, caller: Option<FunctionId>, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        let idx = nid as usize;
        if idx >= self.native_time.len() {
            self.native_time.resize(idx + 1, Duration::ZERO);
        }
        self.native_time[idx] += elapsed;
        let slot = fn_slot(caller);
        if slot >= self.fn_native_time.len() {
            self.fn_native_time.resize(slot + 1, Duration::ZERO);
        }
        self.fn_native_time[slot] += elapsed;
    }

    /// Record one native/builtin invocation by table index.
    #[inline(always)]
    pub fn record_native(&mut self, nid: u32) {
        if !self.enabled {
            return;
        }
        let idx = nid as usize;
        if idx >= self.by_native.len() {
            self.by_native.resize(idx + 1, 0);
        }
        self.by_native[idx] += 1;
    }

    /// Record one user-function call (a pushed VM frame).
    #[inline(always)]
    pub fn record_call(&mut self) {
        if !self.enabled {
            return;
        }
        self.calls += 1;
    }

    /// Record one completed garbage collection and what it cost.
    pub fn record_gc(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.collections += 1;
        self.gc_time += elapsed;
    }

    /// Total instructions retired across all opcodes.
    pub fn total_insts(&self) -> u64 {
        self.by_opcode.iter().sum()
    }

    /// Total native/builtin invocations.
    pub fn total_natives(&self) -> u64 {
        self.by_native.iter().sum()
    }

    /// `(opcode, count)` for every opcode that ran, most-frequent first.
    pub fn opcodes_by_count(&self) -> Vec<(Opcode, u64)> {
        let mut rows: Vec<(Opcode, u64)> = Opcode::ALL
            .iter()
            .copied()
            .zip(self.by_opcode)
            .filter(|(_, n)| *n > 0)
            .collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        rows
    }

    /// `(native fn id, count)` for every native that ran, most-frequent first.
    /// The id is resolved to a name by the caller, which holds the table.
    pub fn natives_by_count(&self) -> Vec<(u32, u64)> {
        let mut rows: Vec<(u32, u64)> = self
            .by_native
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| (i as u32, *n))
            .collect();
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        rows
    }

    /// `(fn slot, self instructions, time in natives it called)` for every
    /// function that ran, by instruction count, most first. Slot 0 is the root
    /// function, slot `n + 1` is `FunctionId(n)` (see [`fn_slot`]).
    pub fn functions_by_count(&self) -> Vec<(usize, u64, Duration)> {
        let n = self.by_function.len().max(self.fn_native_time.len());
        let mut rows: Vec<(usize, u64, Duration)> = (0..n)
            .map(|i| {
                (
                    i,
                    self.by_function.get(i).copied().unwrap_or(0),
                    self.fn_native_time.get(i).copied().unwrap_or(Duration::ZERO),
                )
            })
            .filter(|&(_, c, t)| c > 0 || !t.is_zero())
            .collect();
        rows.sort_by_key(|&(_, n, _)| std::cmp::Reverse(n));
        rows
    }

    /// `(native fn id, calls, total wall time)` for every timed native, by
    /// time, most first.
    pub fn natives_by_time(&self) -> Vec<(u32, u64, Duration)> {
        let mut rows: Vec<(u32, u64, Duration)> = self
            .native_time
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_zero())
            .map(|(i, t)| (i as u32, self.by_native.get(i).copied().unwrap_or(0), *t))
            .collect();
        rows.sort_by_key(|&(_, _, t)| std::cmp::Reverse(t));
        rows
    }

    /// Clear every counter, leaving `enabled` alone.
    pub fn reset(&mut self) {
        let enabled = self.enabled;
        *self = Self::default();
        self.enabled = enabled;
    }

    /// Render the report, resolving native ids through `native_name`. `elapsed`
    /// is the wall time the measured work took, used for the rate lines; pass
    /// `None` when there is no meaningful span to divide by.
    pub fn report(
        &self,
        elapsed: Option<Duration>,
        native_name: impl Fn(u32) -> String,
        top_n: usize,
    ) -> String {
        self.report_with_functions(elapsed, native_name, |_| None, top_n)
    }

    /// [`report`](Self::report) plus the per-function and per-native-time
    /// sections, resolving a function slot (see [`fn_slot`]) to its name with
    /// `fn_name`. A `None` name drops that section (the caller has no program
    /// to resolve against).
    pub fn report_with_functions(
        &self,
        elapsed: Option<Duration>,
        native_name: impl Fn(u32) -> String,
        fn_name: impl Fn(usize) -> Option<String>,
        top_n: usize,
    ) -> String {
        use fmt::Write as _;
        let mut s = String::new();
        let total = self.total_insts();
        let _ = writeln!(s, "vm profile:");
        let _ = writeln!(s, "  instructions   {}", commas(total));
        if let Some(d) = elapsed {
            let secs = d.as_secs_f64();
            let _ = writeln!(s, "  wall time      {:.1} ms", secs * 1e3);
            if secs > 0.0 {
                let _ = writeln!(
                    s,
                    "  rate           {:.1} M inst/s",
                    total as f64 / secs / 1e6
                );
            }
        }
        let _ = writeln!(s, "  user calls     {}", commas(self.calls));
        let _ = writeln!(s, "  native calls   {}", commas(self.total_natives()));
        let _ = writeln!(
            s,
            "  collections    {} ({:.1} ms)",
            self.collections,
            self.gc_time.as_secs_f64() * 1e3
        );

        let opcodes = self.opcodes_by_count();
        histogram(
            &mut s,
            "top opcodes",
            opcodes.iter().map(|&(op, n)| (op.name().to_string(), n)),
            total,
            top_n,
        );

        let natives = self.natives_by_count();
        histogram(
            &mut s,
            "top builtins",
            natives.iter().map(|&(nid, n)| (native_name(nid), n)),
            self.total_natives(),
            top_n,
        );

        let functions = self.functions_by_count();
        if !functions.is_empty() && fn_name(0).is_some() {
            let _ = writeln!(
                s,
                "\n  top functions (self instructions, then time in the natives they call):"
            );
            for &(slot, n, t) in functions.iter().take(top_n) {
                let name = fn_name(slot).unwrap_or_else(|| format!("fn#{slot}"));
                let pct = if total == 0 { 0.0 } else { n as f64 * 100.0 / total as f64 };
                let _ = writeln!(
                    s,
                    "    {:<44} {:>12}  {:>5.1}%  {:>9.2} ms",
                    name,
                    commas(n),
                    pct,
                    t.as_secs_f64() * 1e3
                );
            }
        }

        let timed = self.natives_by_time();
        if !timed.is_empty() {
            let _ = writeln!(s, "\n  natives by time:");
            for &(nid, calls, t) in timed.iter().take(top_n) {
                let ms = t.as_secs_f64() * 1e3;
                let per = if calls == 0 { 0.0 } else { t.as_secs_f64() * 1e9 / calls as f64 };
                let _ = writeln!(
                    s,
                    "    {:<18} {:>9.2} ms  {:>10} calls  {:>7.0} ns/call",
                    native_name(nid),
                    ms,
                    commas(calls),
                    per
                );
            }
        }
        s
    }
}

/// The per-function counters' index for a function: 0 for the implicit root,
/// `id + 1` for `FunctionId(id)`.
#[inline(always)]
pub fn fn_slot(func: Option<FunctionId>) -> usize {
    match func {
        None => 0,
        Some(FunctionId(i)) => i as usize + 1,
    }
}

/// Append one `label: rows` histogram section — `name  count  share-of-total`,
/// truncated to `top_n`. Skipped entirely when there is nothing to report, so a
/// program that called no builtins gets no empty "top builtins" heading.
fn histogram(
    out: &mut String,
    label: &str,
    rows: impl Iterator<Item = (String, u64)>,
    total: u64,
    top_n: usize,
) {
    use fmt::Write as _;
    let mut rows = rows.take(top_n).peekable();
    if rows.peek().is_none() {
        return;
    }
    let _ = writeln!(out, "\n  {label}:");
    for (name, n) in rows {
        let pct = if total == 0 {
            0.0
        } else {
            n as f64 * 100.0 / total as f64
        };
        let _ = writeln!(out, "    {:<18} {:>12}  {:>5.1}%", name, commas(n), pct);
    }
}

/// `1234567` → `"1,234,567"`. Big counts are the norm here and are unreadable
/// undelimited.
fn commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_profile_records_nothing() {
        let mut p = VmProfile::new();
        p.record_inst(&Inst::LoadNil { dst: 0 });
        p.record_native(3);
        p.record_call();
        assert_eq!(p.total_insts(), 0);
        assert_eq!(p.total_natives(), 0);
        assert_eq!(p.calls, 0);
    }

    #[test]
    fn enabled_profile_counts_per_opcode_and_native() {
        let mut p = VmProfile::new();
        p.set_enabled(true);
        p.record_inst(&Inst::LoadNil { dst: 0 });
        p.record_inst(&Inst::LoadNil { dst: 1 });
        p.record_inst(&Inst::Jump { to: 0 });
        p.record_native(2);
        p.record_native(2);
        p.record_native(5);
        assert_eq!(p.total_insts(), 3);
        assert_eq!(
            p.opcodes_by_count(),
            vec![(Opcode::LoadNil, 2), (Opcode::Jump, 1)]
        );
        assert_eq!(p.natives_by_count(), vec![(2, 2), (5, 1)]);
    }

    #[test]
    fn per_function_counts_and_native_time() {
        let mut p = VmProfile::new();
        p.set_enabled(true);
        p.record_inst_in(&Inst::LoadNil { dst: 0 }, None);
        p.record_inst_in(&Inst::LoadNil { dst: 0 }, Some(FunctionId(2)));
        p.record_inst_in(&Inst::LoadNil { dst: 0 }, Some(FunctionId(2)));
        p.record_native(7);
        p.record_native_time(7, Some(FunctionId(2)), Duration::from_micros(5));
        assert_eq!(p.total_insts(), 3);
        assert_eq!(
            p.functions_by_count(),
            vec![(3, 2, Duration::from_micros(5)), (0, 1, Duration::ZERO)]
        );
        assert_eq!(p.natives_by_time(), vec![(7, 1, Duration::from_micros(5))]);
        let r = p.report_with_functions(None, |n| format!("n{n}"), |s| Some(format!("f{s}")), 5);
        assert!(r.contains("top functions"), "{r}");
        assert!(r.contains("f3"), "{r}");
        assert!(r.contains("natives by time"), "{r}");
    }

    #[test]
    fn enabling_clears_earlier_counts() {
        let mut p = VmProfile::new();
        p.set_enabled(true);
        p.record_call();
        p.set_enabled(true);
        assert_eq!(p.calls, 0);
    }

    #[test]
    fn commas_groups_digits() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_234_567), "1,234,567");
    }
}

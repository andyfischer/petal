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
        s
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

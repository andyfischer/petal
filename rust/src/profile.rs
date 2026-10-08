//! Opt-in execution profiling: what the VM actually spent its instructions on.
//!
//! [`DupStats`](crate::stats::DupStats) answers "how much did immutability cost
//! us?"; this answers the prior question — "which opcodes and which builtins is
//! a program running at all?" — which is where an optimization effort starts.
//!
//! Collection is a **runtime** switch rather than a compile-time one, so a
//! shipped release binary can profile a slow script without a rebuild:
//! `petal run --profile <file>`. When [`enabled`](VmProfile::enabled) is false
//! every `record_*` is one predictable branch, which does not measurably move
//! the benchmarks.
//!
//! [`CallBench`] is the second tool here: where the profile describes a whole
//! run, it measures individual calls of chosen functions (`petal bench`).
//!
//! The counts are exact, but note what a *count* can and cannot tell you: it
//! says a program executed 4 M `GetField`s, not that `GetField` is slow. Pair it
//! with a sampling profiler (`cargo build --profile profiling`, then `sample`)
//! to turn a large count into a time attribution.

use std::fmt;
use std::time::{Duration, Instant};

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
    /// Per-call measurement of chosen functions (`petal bench`). It has its
    /// own switch and is independent of `enabled`: it is consulted where a
    /// frame is pushed and popped rather than per instruction, so turning it
    /// on does not take the VM off its fast dispatch loop. Not touched by
    /// [`reset`](VmProfile::reset) / [`set_enabled`](VmProfile::set_enabled).
    pub bench: CallBench,
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
            bench: CallBench::default(),
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

    /// Clear every counter, leaving `enabled` and the call bench alone.
    pub fn reset(&mut self) {
        let enabled = self.enabled;
        let bench = std::mem::take(&mut self.bench);
        *self = Self::default();
        self.enabled = enabled;
        self.bench = bench;
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

// ---------------------------------------------------------------------------
// Per-call measurement (`petal bench`)
// ---------------------------------------------------------------------------

/// The cumulative counters a call is measured against: read when a measured
/// frame is pushed and again when it pops, and the difference is what the call
/// cost. The VM supplies them (instructions from the stack, the rest from the
/// heap's [`stats`](crate::stats)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallCounters {
    /// Instructions retired on the stack.
    pub insts: u64,
    /// Heap objects allocated, all kinds.
    pub allocs: u64,
    /// Copy-on-write duplications of a list, record or f64 array.
    pub copies: u64,
    /// Bytes those duplications copied.
    pub copy_bytes: u64,
}

/// How many per-call wall times a [`CallStats`] keeps for its percentiles.
/// Up to this many calls the median and p95 are exact; past it they are taken
/// over a uniform random sample of this size (min, max and every mean stay
/// exact regardless).
pub const BENCH_SAMPLE_CAP: usize = 1 << 16;

/// What one benched function cost, summed over the calls measured so far.
///
/// Two populations are kept apart so recursion cannot double count:
///
/// - **Inclusive** figures (the function plus everything it calls) are summed
///   over *outermost* activations only — a call made while no other call of
///   the same function is on the stack. A recursive call's time is already
///   inside its outermost caller's.
/// - **Self** figures (minus the user functions it calls; natives it calls
///   stay in) are summed over *every* activation, recursive ones included.
#[derive(Debug, Clone)]
pub struct CallStats {
    /// The function measured.
    pub func: FunctionId,
    /// Every activation that ran, recursive ones included.
    pub calls: u64,
    /// Activations with no other activation of this function below them.
    pub outer_calls: u64,
    /// Calls the memo replayed from a record instead of running. They pushed
    /// no frame, so they are in none of the other figures.
    pub replayed: u64,
    /// Inclusive wall time and instructions, over `outer_calls`.
    pub incl_ns: u64,
    pub incl_insts: u64,
    /// Self wall time and instructions, over `calls`.
    pub self_ns: u64,
    pub self_insts: u64,
    /// Heap allocations and copy-on-write duplications, inclusive, over
    /// `outer_calls`.
    pub allocs: u64,
    pub copies: u64,
    pub copy_bytes: u64,
    /// Collections that ran during an outermost activation, and their time
    /// (which is also inside `incl_ns`).
    pub collections: u64,
    pub gc_ns: u64,
    /// Extremes of one outermost activation's inclusive time and instructions.
    pub min_ns: u64,
    pub max_ns: u64,
    pub min_insts: u64,
    pub max_insts: u64,
    /// Inclusive nanoseconds of individual outermost activations, for the
    /// percentiles: all of them up to [`BENCH_SAMPLE_CAP`], a uniform sample
    /// past it.
    samples: Vec<u64>,
    /// Activations of this function on the stack right now.
    active: u32,
    /// Reservoir-sampling PRNG state (xorshift; fixed seed, so two runs over
    /// the same calls sample the same ones).
    rng: u64,
}

impl CallStats {
    fn new(func: FunctionId) -> Self {
        CallStats {
            func,
            calls: 0,
            outer_calls: 0,
            replayed: 0,
            incl_ns: 0,
            incl_insts: 0,
            self_ns: 0,
            self_insts: 0,
            allocs: 0,
            copies: 0,
            copy_bytes: 0,
            collections: 0,
            gc_ns: 0,
            min_ns: u64::MAX,
            max_ns: 0,
            min_insts: u64::MAX,
            max_insts: 0,
            samples: Vec::new(),
            active: 0,
            rng: 0x9E37_79B9_7F4A_7C15 ^ (func.0 as u64 + 1),
        }
    }

    /// Keep `ns` as a percentile sample (algorithm R once the cap is reached).
    fn push_sample(&mut self, ns: u64) {
        if self.samples.len() < BENCH_SAMPLE_CAP {
            self.samples.push(ns);
            return;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        // `outer_calls` already counts this one.
        let slot = (self.rng % self.outer_calls) as usize;
        if slot < BENCH_SAMPLE_CAP {
            self.samples[slot] = ns;
        }
    }

    /// The `q` quantile (0.0 ..= 1.0) of one outermost call's inclusive
    /// nanoseconds, by nearest rank. `None` when nothing was measured.
    pub fn quantile_ns(&self, q: f64) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let rank = (q * sorted.len() as f64).ceil() as usize;
        Some(sorted[rank.clamp(1, sorted.len()) - 1])
    }

    /// Whether the percentiles come from a sample rather than every call.
    pub fn sampled(&self) -> bool {
        self.outer_calls as usize > self.samples.len()
    }
}

/// A frame being measured: a benched function's, or one a benched function
/// called directly (timed only so the caller's self figures can subtract it).
#[derive(Debug, Clone)]
struct OpenCall {
    /// Index of the frame in the VM's frame stack.
    depth: usize,
    /// Index into [`CallBench::targets`] plus one; 0 for a callee that is not
    /// itself benched.
    target: u32,
    /// Whether this is the outermost activation of its target.
    outermost: bool,
    start: CallCounters,
    gc_count: u64,
    gc_ns: u64,
    /// Inclusive time and instructions of the frames this one called.
    child_ns: u64,
    child_insts: u64,
    t0: Instant,
}

/// Per-call measurement of a chosen set of functions: how many times each ran
/// and what a call cost in instructions, wall time, allocations, copies and
/// collections. This is what `petal bench` reads.
///
/// It lives on the [`VmProfile`] but is deliberately not a per-instruction
/// hook: the VM consults it only where a user-function frame is pushed
/// ([`enter`](Self::enter)) and popped ([`leave`](Self::leave)), so a benched
/// run executes on the same fast dispatch loop as an ordinary one and the
/// timings are of the code a shipped host runs. Disabled, it costs one branch
/// per call and one per return.
///
/// Only benched functions and their direct callees are timed, so the cost of
/// measuring — two clock reads per timed frame — lands inside a benched call
/// in proportion to the user functions it calls directly.
#[derive(Debug, Clone, Default)]
pub struct CallBench {
    /// Master switch; [`enter`](Self::enter) returns at once when false.
    pub enabled: bool,
    /// `FunctionId` → index into `targets` plus one, 0 for "not benched".
    target_of: Vec<u32>,
    targets: Vec<CallStats>,
    /// The measured frames live on the stack, innermost last.
    open: Vec<OpenCall>,
    /// Collections seen while enabled, and their time. Cumulative, like
    /// [`CallCounters`]: a call's share is the difference across it.
    collections: u64,
    gc_ns: u64,
}

impl CallBench {
    /// A bench over `funcs`, enabled. Duplicates are measured once.
    pub fn new(funcs: &[FunctionId]) -> Self {
        let mut b = CallBench {
            enabled: true,
            ..Default::default()
        };
        for &f in funcs {
            let slot = f.0 as usize;
            if slot >= b.target_of.len() {
                b.target_of.resize(slot + 1, 0);
            }
            if b.target_of[slot] == 0 {
                b.targets.push(CallStats::new(f));
                b.target_of[slot] = b.targets.len() as u32;
            }
        }
        b
    }

    /// What was measured, one entry per benched function in the order given
    /// to [`new`](Self::new).
    pub fn stats(&self) -> &[CallStats] {
        &self.targets
    }

    /// Forget every measurement, keeping the function set and the switch:
    /// what a warm-up run is followed by.
    pub fn clear(&mut self) {
        for t in &mut self.targets {
            *t = CallStats::new(t.func);
        }
        self.open.clear();
        self.collections = 0;
        self.gc_ns = 0;
    }

    /// A run is starting on an empty frame stack: drop whatever a previous
    /// run that ended in an error left open.
    pub fn begin_run(&mut self) {
        self.unwind(0);
    }

    /// Whether any measured frame is live. The VM's pop path tests this
    /// before calling [`leave`](Self::leave).
    #[inline(always)]
    pub fn any_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// The frames at `depth` and above were discarded without returning (an
    /// error unwound them): close their entries without recording a call.
    pub fn unwind(&mut self, depth: usize) {
        while self.open.last().is_some_and(|e| e.depth >= depth) {
            let e = self.open.pop().unwrap();
            if e.target != 0 {
                let t = &mut self.targets[e.target as usize - 1];
                t.active = t.active.saturating_sub(1);
            }
        }
    }

    /// A frame for `func` was just pushed at index `depth`. Starts measuring
    /// it if it is benched, or if the frame that called it is. `counters` is
    /// only invoked in that case.
    #[inline]
    pub fn enter(
        &mut self,
        func: FunctionId,
        depth: usize,
        counters: impl FnOnce() -> CallCounters,
    ) {
        if !self.enabled {
            return;
        }
        let target = self.target_of.get(func.0 as usize).copied().unwrap_or(0);
        if target == 0 && self.open.is_empty() {
            return;
        }
        self.enter_measured(target, depth, counters());
    }

    #[inline(never)]
    fn enter_measured(&mut self, target: u32, depth: usize, start: CallCounters) {
        // Anything still open at this depth belongs to a frame that is gone.
        self.unwind(depth);
        let parent_benched = self
            .open
            .last()
            .is_some_and(|e| e.depth + 1 == depth && e.target != 0);
        if target == 0 && !parent_benched {
            return;
        }
        let outermost = target != 0 && {
            let t = &mut self.targets[target as usize - 1];
            t.active += 1;
            t.active == 1
        };
        self.open.push(OpenCall {
            depth,
            target,
            outermost,
            start,
            gc_count: self.collections,
            gc_ns: self.gc_ns,
            child_ns: 0,
            child_insts: 0,
            // Read last, so the bookkeeping above is outside the interval.
            t0: Instant::now(),
        });
    }

    /// The frame at index `depth` is about to pop with a value. Records the
    /// call if it was being measured. Call only when
    /// [`any_open`](Self::any_open).
    #[inline(never)]
    pub fn leave(&mut self, depth: usize, counters: impl FnOnce() -> CallCounters) {
        match self.open.last() {
            Some(e) if e.depth == depth => {}
            Some(e) if e.depth > depth => {
                self.unwind(depth + 1);
                if self.open.last().is_none_or(|e| e.depth != depth) {
                    return;
                }
            }
            _ => return,
        }
        let e = self.open.pop().unwrap();
        // Read first, for the same reason `enter` reads last.
        let ns = e.t0.elapsed().as_nanos() as u64;
        let now = counters();
        let insts = now.insts - e.start.insts;
        if let Some(p) = self.open.last_mut()
            && p.depth + 1 == depth
        {
            p.child_ns += ns;
            p.child_insts += insts;
        }
        if e.target == 0 {
            return;
        }
        let t = &mut self.targets[e.target as usize - 1];
        t.active = t.active.saturating_sub(1);
        t.calls += 1;
        t.self_ns += ns.saturating_sub(e.child_ns);
        t.self_insts += insts.saturating_sub(e.child_insts);
        if e.outermost {
            t.outer_calls += 1;
            t.incl_ns += ns;
            t.incl_insts += insts;
            t.allocs += now.allocs - e.start.allocs;
            t.copies += now.copies - e.start.copies;
            t.copy_bytes += now.copy_bytes - e.start.copy_bytes;
            t.collections += self.collections - e.gc_count;
            t.gc_ns += self.gc_ns - e.gc_ns;
            t.min_ns = t.min_ns.min(ns);
            t.max_ns = t.max_ns.max(ns);
            t.min_insts = t.min_insts.min(insts);
            t.max_insts = t.max_insts.max(insts);
            t.push_sample(ns);
        }
    }

    /// A call of `func` was replayed from its memo record instead of run.
    #[inline]
    pub fn note_replay(&mut self, func: FunctionId) {
        if !self.enabled {
            return;
        }
        if let Some(&t) = self.target_of.get(func.0 as usize)
            && t != 0
        {
            self.targets[t as usize - 1].replayed += 1;
        }
    }

    /// One garbage collection finished and took `elapsed`.
    pub fn record_gc(&mut self, elapsed: Duration) {
        if !self.enabled {
            return;
        }
        self.collections += 1;
        self.gc_ns += elapsed.as_nanos() as u64;
    }

    /// What measuring one frame costs: the wall time of the two clock reads
    /// and the bookkeeping between them, in nanoseconds, measured here and now
    /// by timing a run of empty enter/leave pairs. A benched call's inclusive
    /// time carries roughly half of this for itself plus all of it for every
    /// user function it calls directly.
    pub fn timer_overhead_ns() -> f64 {
        const ROUNDS: u32 = 20_000;
        let mut b = CallBench::new(&[FunctionId(0)]);
        let c = CallCounters::default;
        // Each round is a benched call with one direct callee: two timed
        // frames.
        let run = |b: &mut CallBench| {
            for _ in 0..ROUNDS {
                b.enter(FunctionId(0), 1, c);
                b.enter(FunctionId(1), 2, c);
                b.leave(2, c);
                b.leave(1, c);
            }
        };
        run(&mut b); // warm the caches and the branch predictor
        b.clear();
        let t0 = Instant::now();
        run(&mut b);
        t0.elapsed().as_nanos() as f64 / (ROUNDS as f64 * 2.0)
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
pub(crate) fn commas(n: u64) -> String {
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

    fn counters(insts: u64) -> CallCounters {
        CallCounters {
            insts,
            ..Default::default()
        }
    }

    #[test]
    fn call_bench_disabled_records_nothing() {
        let mut b = CallBench::new(&[FunctionId(1)]);
        b.enabled = false;
        b.enter(FunctionId(1), 1, || counters(0));
        assert!(!b.any_open());
        assert_eq!(b.stats()[0].calls, 0);
    }

    #[test]
    fn call_bench_splits_self_from_inclusive() {
        // f (10 insts of its own) calls g (5 insts), which is not benched.
        let mut b = CallBench::new(&[FunctionId(1)]);
        b.enter(FunctionId(1), 1, || counters(100));
        b.enter(FunctionId(2), 2, || counters(104));
        // g's own callee is not timed: only a benched function's direct
        // callees are.
        b.enter(FunctionId(3), 3, || counters(105));
        assert_eq!(b.open.len(), 2);
        b.leave(3, || counters(107));
        b.leave(2, || counters(109));
        b.leave(1, || counters(115));
        let s = &b.stats()[0];
        assert_eq!((s.calls, s.outer_calls), (1, 1));
        assert_eq!(s.incl_insts, 15);
        assert_eq!(s.self_insts, 10);
        assert!(s.self_ns <= s.incl_ns);
        assert_eq!((s.min_insts, s.max_insts), (15, 15));
        assert!(!b.any_open());
    }

    #[test]
    fn call_bench_recursion_counts_inclusive_once() {
        // f calls f calls f: three activations, one outermost.
        let mut b = CallBench::new(&[FunctionId(0)]);
        b.enter(FunctionId(0), 1, || counters(0));
        b.enter(FunctionId(0), 2, || counters(10));
        b.enter(FunctionId(0), 3, || counters(20));
        b.leave(3, || counters(30));
        b.leave(2, || counters(40));
        b.leave(1, || counters(50));
        let s = &b.stats()[0];
        assert_eq!((s.calls, s.outer_calls), (3, 1));
        assert_eq!(s.incl_insts, 50, "the nested calls are inside the outer one");
        assert_eq!(s.self_insts, 50, "self over every activation sums to the same");
        assert_eq!(s.quantile_ns(0.5), Some(s.incl_ns));
    }

    #[test]
    fn call_bench_unwinds_frames_an_error_discarded() {
        let mut b = CallBench::new(&[FunctionId(0)]);
        b.enter(FunctionId(0), 1, || counters(0));
        b.enter(FunctionId(0), 2, || counters(1));
        b.begin_run();
        assert!(!b.any_open());
        // The next call is outermost again, not a recursion into the lost ones.
        b.enter(FunctionId(0), 1, || counters(5));
        b.leave(1, || counters(9));
        let s = &b.stats()[0];
        assert_eq!((s.calls, s.outer_calls, s.incl_insts), (1, 1, 4));
    }

    #[test]
    fn call_bench_percentiles_and_reservoir() {
        let mut s = CallStats::new(FunctionId(0));
        for ns in 1..=100u64 {
            s.outer_calls += 1;
            s.push_sample(ns);
        }
        assert_eq!(s.quantile_ns(0.5), Some(50));
        assert_eq!(s.quantile_ns(0.95), Some(95));
        assert_eq!(s.quantile_ns(0.0), Some(1));
        assert!(!s.sampled());
        for ns in 0..(BENCH_SAMPLE_CAP as u64 * 2) {
            s.outer_calls += 1;
            s.push_sample(ns);
        }
        assert_eq!(s.samples.len(), BENCH_SAMPLE_CAP);
        assert!(s.sampled());
    }

    #[test]
    fn profile_reset_keeps_the_call_bench() {
        let mut p = VmProfile::new();
        p.bench = CallBench::new(&[FunctionId(4)]);
        p.set_enabled(true);
        assert!(p.bench.enabled);
        assert_eq!(p.bench.stats().len(), 1);
    }

    #[test]
    fn commas_groups_digits() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_234_567), "1,234,567");
    }
}

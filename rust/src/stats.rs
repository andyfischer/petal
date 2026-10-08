//! Optional value-duplication statistics.
//!
//! Petal values are immutable: every "mutation" (list append, map set, f64
//! array swap, …) and every speculative fork copies the underlying heap
//! payload instead of editing it in place. These counters measure how much
//! copying that costs — how many duplications happened and how many bytes were
//! copied — so we can watch the numbers fall as escape analysis and structural
//! sharing teach the runtime to reuse live payloads instead of duplicating
//! them.
//!
//! Collection is a **runtime** switch on each counter set (see
//! [`DupStats::set_enabled`]), so a shipped release binary can count without a
//! rebuild: `petal run --dup-stats` and `petal bench` turn it on. It starts at
//! [`DUP_STATS_ENABLED`]: on in debug builds (which includes `cargo test`) and
//! in builds with the `dup-stats` cargo feature, off otherwise. When off,
//! every `record` call is one predictable branch on a field of the heap that
//! is already in cache — the `bytes` closure is never invoked — which does not
//! measurably move the benchmarks (docs/dev/performance.md has the numbers).

use std::fmt;

/// Whether duplication and allocation statistics are collected *by default*.
/// `true` in debug builds, or in any build with the `dup-stats` feature
/// enabled; `false` otherwise. A fresh [`DupStats`] / [`AllocStats`] starts in
/// this state, and `set_enabled` changes it at runtime.
pub const DUP_STATS_ENABLED: bool = cfg!(debug_assertions) || cfg!(feature = "dup-stats");

/// The kind of heap payload that was duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DupKind {
    /// Copy-on-write of a list's backing `Vec<Value>` (append / set / drop_last).
    List,
    /// Copy-on-write of a map's backing entry table (set / remove).
    Map,
    /// Copy-on-write of an f64 array's backing `Vec<f64>` (set / swap).
    F64Array,
    /// Whole-heap clone taken to fork a speculative execution.
    Fork,
}

impl DupKind {
    /// Number of distinct kinds — the width of the [`DupStats`] backing array.
    pub const COUNT: usize = 4;

    /// Every kind, in display order. Indexes line up with [`DupKind::index`].
    pub const ALL: [DupKind; Self::COUNT] = [
        DupKind::List,
        DupKind::Map,
        DupKind::F64Array,
        DupKind::Fork,
    ];

    /// Dense index into the [`DupStats`] backing array.
    const fn index(self) -> usize {
        match self {
            DupKind::List => 0,
            DupKind::Map => 1,
            DupKind::F64Array => 2,
            DupKind::Fork => 3,
        }
    }

    /// Short human-readable label, used by the [`fmt::Display`] impl.
    pub const fn label(self) -> &'static str {
        match self {
            DupKind::List => "list",
            DupKind::Map => "map",
            DupKind::F64Array => "f64array",
            DupKind::Fork => "fork",
        }
    }
}

/// The kind of heap object allocated. Unlike [`DupKind`] this enumerates every
/// heap object type (a `Fork` copies existing objects, it does not allocate
/// new ones, so it has no allocation kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocKind {
    String,
    List,
    F64Array,
    Map,
    Element,
    Cell,
    Vec3,
}

impl AllocKind {
    /// Number of distinct kinds — the width of the [`AllocStats`] backing array.
    pub const COUNT: usize = 7;

    /// Every kind, in display order. Indexes line up with [`AllocKind::index`].
    pub const ALL: [AllocKind; Self::COUNT] = [
        AllocKind::String,
        AllocKind::List,
        AllocKind::F64Array,
        AllocKind::Map,
        AllocKind::Element,
        AllocKind::Cell,
        AllocKind::Vec3,
    ];

    const fn index(self) -> usize {
        match self {
            AllocKind::String => 0,
            AllocKind::List => 1,
            AllocKind::F64Array => 2,
            AllocKind::Map => 3,
            AllocKind::Element => 4,
            AllocKind::Cell => 5,
            AllocKind::Vec3 => 6,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            AllocKind::String => "string",
            AllocKind::List => "list",
            AllocKind::F64Array => "f64array",
            AllocKind::Map => "map",
            AllocKind::Element => "element",
            AllocKind::Cell => "cell",
            AllocKind::Vec3 => "vec3",
        }
    }
}

/// How many new heap objects of each [`AllocKind`] were created during a run.
///
/// Counts *creations*, cumulative over the whole run — it is never decremented
/// when an object is garbage-collected, so it measures total churn (including
/// short-lived temporaries), not live-set size. Every copy-on-write that
/// produces a new id also allocates, so this rises alongside [`DupStats`] and
/// gives visibility into how many intermediate objects a program produces.
///
/// Collected under the same runtime switch as [`DupStats`].
#[derive(Debug, Clone)]
pub struct AllocStats {
    /// Whether [`record`](Self::record) counts. Starts at
    /// [`DUP_STATS_ENABLED`].
    enabled: bool,
    by_kind: [u64; AllocKind::COUNT],
}

impl Default for AllocStats {
    fn default() -> Self {
        AllocStats {
            enabled: DUP_STATS_ENABLED,
            by_kind: [0; AllocKind::COUNT],
        }
    }
}

impl AllocStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether allocations are being counted.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turn counting on or off. What was already counted is kept.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Record one allocation of `kind`: one predictable branch when stats are
    /// off.
    #[inline]
    pub fn record(&mut self, kind: AllocKind) {
        if !self.enabled {
            return;
        }
        self.by_kind[kind.index()] += 1;
    }

    /// Number of `kind` objects allocated.
    pub fn get(&self, kind: AllocKind) -> u64 {
        self.by_kind[kind.index()]
    }

    /// Total objects allocated across all kinds.
    pub fn total(&self) -> u64 {
        self.by_kind.iter().sum()
    }

    /// Iterate `(kind, count)` pairs in [`AllocKind::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (AllocKind, u64)> + '_ {
        AllocKind::ALL.iter().map(move |&k| (k, self.get(k)))
    }

    /// Clear all counters back to zero, leaving the switch alone.
    pub fn reset(&mut self) {
        self.by_kind = [0; AllocKind::COUNT];
    }
}

impl fmt::Display for AllocStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.enabled {
            return write!(f, "heap allocation stats: disabled");
        }
        writeln!(f, "heap allocation stats (objects created):")?;
        for (kind, count) in self.iter() {
            writeln!(f, "  {:<9} count={}", kind.label(), count)?;
        }
        write!(f, "  {:<9} count={}", "total", self.total())
    }
}

/// Count and total bytes for one kind of duplication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DupCounter {
    /// How many times a payload of this kind was duplicated.
    pub count: u64,
    /// Total bytes copied across those duplications.
    pub bytes: u64,
}

/// Aggregated duplication statistics, broken down by [`DupKind`].
///
/// Lives on the [`Heap`](crate::heap::Heap) (the only place copy-on-write and
/// forks actually happen) and is surfaced up through
/// [`ExecutionContext`](crate::execution_context::ExecutionContext) and
/// [`Env`](crate::env::Env). All zero while collection is off, which is how a
/// release build starts (see the module docs).
#[derive(Debug, Clone)]
pub struct DupStats {
    /// Whether [`record`](Self::record) counts. Starts at
    /// [`DUP_STATS_ENABLED`].
    enabled: bool,
    by_kind: [DupCounter; DupKind::COUNT],
}

impl Default for DupStats {
    fn default() -> Self {
        DupStats {
            enabled: DUP_STATS_ENABLED,
            by_kind: [DupCounter::default(); DupKind::COUNT],
        }
    }
}

impl DupStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether duplications are being counted.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turn counting on or off. What was already counted is kept.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Record one duplication of `kind`. `bytes` is computed lazily and is only
    /// invoked when collection is enabled, so callers can pass an expensive
    /// size computation without slowing down builds where stats are off.
    ///
    /// One predictable branch when collection is off.
    #[inline]
    pub fn record(&mut self, kind: DupKind, bytes: impl FnOnce() -> u64) {
        if !self.enabled {
            return;
        }
        let counter = &mut self.by_kind[kind.index()];
        counter.count += 1;
        counter.bytes += bytes();
    }

    /// The counter for a single kind.
    pub fn get(&self, kind: DupKind) -> DupCounter {
        self.by_kind[kind.index()]
    }

    /// Total number of duplications across all kinds.
    pub fn total_count(&self) -> u64 {
        self.by_kind.iter().map(|c| c.count).sum()
    }

    /// Total bytes duplicated across all kinds.
    pub fn total_bytes(&self) -> u64 {
        self.by_kind.iter().map(|c| c.bytes).sum()
    }

    /// `true` if nothing has been recorded (e.g. stats disabled, or no
    /// duplication has happened yet).
    pub fn is_empty(&self) -> bool {
        self.total_count() == 0
    }

    /// Iterate `(kind, counter)` pairs in [`DupKind::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (DupKind, DupCounter)> + '_ {
        DupKind::ALL.iter().map(move |&k| (k, self.get(k)))
    }

    /// Clear all counters back to zero, leaving the switch alone.
    pub fn reset(&mut self) {
        self.by_kind = [DupCounter::default(); DupKind::COUNT];
    }
}

impl fmt::Display for DupStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.enabled {
            return write!(
                f,
                "value duplication stats: disabled (turn them on with \
                 `Env::set_heap_stats(true)` before the run)"
            );
        }
        writeln!(f, "value duplication stats:")?;
        for (kind, c) in self.iter() {
            writeln!(
                f,
                "  {:<9} count={:<8} bytes={}",
                kind.label(),
                c.count,
                c.bytes
            )?;
        }
        write!(
            f,
            "  {:<9} count={:<8} bytes={}",
            "total",
            self.total_count(),
            self.total_bytes()
        )
    }
}

//! What one garbage collection costs with a big table rooted, frozen and not.
//!
//! ```text
//! cargo run --release --example gc_pause -- [records ...]
//! ```
//!
//! Builds a table of N records straight on a `Heap` (three objects a row: the
//! record, its name, its tag list), then times a collection with the table as
//! the only root and **no garbage to free**, so the figure is the cost the
//! table itself adds to every collection: the mark over it plus the sweep's
//! walk over its slots. Then freezes the table (`Heap::freeze`) and times the
//! same collection again: the marker stops at the root and the sweep only
//! walks the slots. Defaults to 10,000, 100,000 and 400,000 records.
//!
//! `tools/bench-gc-pause.ts` is the script-level half: the same table under
//! a churn of short-lived records, where the pause also includes freeing the
//! garbage each collection finds. Figures for both are in
//! docs/dev/performance.md (Frozen data and collection pauses).

#![allow(clippy::disallowed_methods, reason = "examples run natively")]

use std::time::Instant;

use petal::heap::Heap;
use petal::record::RecordMap;
use petal::value::Value;

const ROUNDS: usize = 31;

fn build(heap: &mut Heap, n: usize) -> Value {
    let rows = (0..n as i64)
        .map(|i| {
            // Long enough not to be interned, so every row owns its name.
            let name = heap.alloc_string(format!("row {i}"));
            let tags = heap.alloc_list(vec![Value::Int(i), Value::Int(i + 1)]);
            let mut e = RecordMap::default();
            e.insert("id".to_string(), Value::Int(i));
            e.insert("name".to_string(), Value::String(name));
            e.insert("weight".to_string(), Value::Float(i as f64 * 0.5));
            e.insert("tags".to_string(), Value::List(tags));
            Value::Map(heap.alloc_map(e))
        })
        .collect();
    Value::List(heap.alloc_list(rows))
}

/// Median time of one mark-and-sweep with `root` as the only root, in ms.
fn collection_ms(heap: &mut Heap, root: Value) -> f64 {
    let mut samples: Vec<f64> = (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            heap.mark_value(root);
            heap.sweep();
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[ROUNDS / 2]
}

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.replace(['_', ','], "").parse().expect("a record count"))
        .collect();
    let sizes = if sizes.is_empty() { vec![10_000, 100_000, 400_000] } else { sizes };

    println!("one collection, the table the only root, nothing to free; median of {ROUNDS}\n");
    println!("  records    objects   rooted (ms)   frozen (ms)   ratio");
    println!("  -------    -------   -----------   -----------   -----");
    for n in sizes {
        let mut heap = Heap::new();
        let root = build(&mut heap, n);
        let rooted = collection_ms(&mut heap, root);
        let objects = heap.freeze(root).expect("the table holds only data");
        let frozen = collection_ms(&mut heap, root);
        assert!(heap.is_frozen(root) && heap.is_live(root));
        println!(
            "  {n:>7}  {objects:>9}   {rooted:>11.3}   {frozen:>11.3}   {:>4.0}x",
            rooted / frozen.max(1e-9)
        );
    }
}

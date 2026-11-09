//! Native `text_wrap` against its Petal specification: equivalence, then
//! per-frame cost (results in docs/dev/performance.md, "Text wrap: native vs
//! Petal").
//!
//!   cargo run --release --example bench_text_wrap -- [--frames N] [--runs N] [--sizes 1000,10000,100000]
//!   MEMO_STATS=1 cargo run ...     # also the memo's counters for each case
//!
//! The Petal side is `wrap_rows_reference`
//! (`core-runtime/text-layout/src/wrap_reference.ptl`), the specification of
//! `text.rs`'s `wrap_lines` + `wrap_rows`, which measures with `text_advance`
//! once per word. `tests/text_wrap_reference.rs` runs the equivalence pass
//! below as a test; the corpus is `tests/support/text_wrap_corpus.rs`.
//!
//! 1. **Equivalence.** Both are run over a corpus of edge cases and generated
//!    documents at several widths and styles; every row (`text`, `start`,
//!    `end`) must match. A mismatch is printed and the run fails.
//! 2. **Per-frame time**, under the headless harness with a proportional
//!    advance table bound, the frame gate off (every frame runs) and the
//!    pointer moving one pixel a frame. For each document size:
//!    - `area`: the real `text_area` (native wrap) over the document;
//!    - `wrap/native`, `wrap/petal`: the wrap alone, `text_wrap_rows` or
//!      `wrap_rows_reference`, at the width `text_area` wraps to;
//!
//!    each with the memo on and off, and with the text unchanged (`still`) or
//!    one character different every frame (`typing`, which no memo can save).
//!    A `text_area` on the Petal wrap would cost about
//!    `area − wrap/native + wrap/petal`.
use std::time::Instant;

use petal_ui_headless::harness::Headless;

#[path = "../tests/support/text_wrap_corpus.rs"]
mod corpus;
use corpus::{AREA_W, WRAP_W, document, lit, ratios};

/// The specification, as the timed scripts import it.
const REF: &str = "import text_layout/wrap_reference: wrap_rows_reference";

fn harness(src: &str, memo: bool) -> Headless {
    let mut ui = Headless::with_size(src, 1200, 800).unwrap_or_else(|e| panic!("compile: {e}"));
    petal_ui::text::bind_text_advance_table(&mut ui.env, &ratios());
    let mut policy = ui.policy();
    policy.gate = false;
    policy.memo = memo;
    ui.set_policy(policy);
    ui
}

fn check_equivalence() -> usize {
    let mut ui = harness(&corpus::equivalence_source(), false);
    ui.frame()
        .unwrap_or_else(|e| panic!("equivalence frame: {e}"));
    let checked = ui.state_int("checked").unwrap_or(0);
    let bad = ui.state_int("bad").unwrap_or(-1);
    println!(
        "equivalence: {checked} (doc, style, width) cases over {} docs, {bad} mismatched",
        corpus::corpus().len()
    );
    if bad != 0 {
        println!("  first: {}", ui.state_string("first").unwrap_or_default());
    }
    bad as usize
}

/// The script one timed case runs every frame.
fn script(kind: &str, doc: &str, typing: bool) -> String {
    // `typing` makes the text one character different every frame, as a
    // keystroke does; `still` passes the same string each time.
    let text = if typing {
        "DOC ++ str(int(time() * 60.0) % 10)"
    } else {
        "DOC"
    };
    let body = match kind {
        "area" => format!(
            "state nrows = 0\n\
             let res = text_area(\"doc\", {{x: 10, y: 10, w: {AREA_W}, h: 780}}, {text}, {{size: 14}})\n\
             nrows = len(res.rows)\n"
        ),
        "native" => format!(
            "state nrows = 0\n\
             nrows = len(text_wrap_rows({text}, {{size: 14}}, {WRAP_W}.0))\n"
        ),
        "petal" => format!(
            "state nrows = 0\n\
             nrows = len(wrap_rows_reference({text}, {{size: 14}}, {WRAP_W}.0))\n"
        ),
        _ => unreachable!(),
    };
    format!("import ui\n{REF}\nlet DOC = {}\n{body}", lit(doc))
}

struct Timing {
    p50: f64,
    mean: f64,
    rows: i64,
    /// Memo hits / misses / records / effectful over the timed frames.
    memo: String,
}

fn time_case(src: &str, memo: bool, frames: usize) -> Timing {
    let mut ui = harness(src, memo);
    // Warm-up: the first frame builds state; two more settle the memo.
    for i in 0..3 {
        ui.mouse_move(100 + (i % 2) as i32, 100);
        ui.frame().unwrap_or_else(|e| panic!("frame: {e}"));
    }
    let before = ui.memo_stats();
    let mut times = Vec::with_capacity(frames);
    for i in 0..frames {
        ui.mouse_move(100 + (i % 2) as i32, 100);
        let t = Instant::now();
        ui.frame().unwrap_or_else(|e| panic!("frame: {e}"));
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let after = ui.memo_stats();
    let memo = format!(
        "h{} m{} r{} e{} c{}",
        after.hits - before.hits,
        after.misses - before.misses,
        after.records - before.records,
        after.effectful - before.effectful,
        after.cold - before.cold,
    );
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Timing {
        p50: times[times.len() / 2],
        mean,
        rows: ui.state_int("nrows").unwrap_or(-1),
        memo,
    }
}

/// The run of `runs` (each a fresh harness) with the lowest median: the
/// machine's noise only ever adds time.
fn best_of(src: &str, memo: bool, frames: usize, runs: usize) -> Timing {
    (0..runs.max(1))
        .map(|_| time_case(src, memo, frames))
        .min_by(|a, b| a.p50.partial_cmp(&b.p50).unwrap())
        .unwrap()
}

fn main() {
    let mut frames = 60usize;
    let mut runs = 3usize;
    let mut sizes = vec![1_000usize, 10_000, 100_000];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => {
                frames = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .expect("--frames N")
            }
            "--runs" => runs = args.next().and_then(|s| s.parse().ok()).expect("--runs N"),
            "--sizes" => {
                sizes = args
                    .next()
                    .expect("--sizes a,b,c")
                    .split(',')
                    .map(|s| s.parse().expect("size"))
                    .collect()
            }
            other => panic!("unknown argument {other}"),
        }
    }

    if check_equivalence() != 0 {
        std::process::exit(1);
    }

    println!(
        "\nper-frame ms (p50 / mean over {frames} frames, best of {runs} runs), wrap width {WRAP_W} px, size 14"
    );
    println!(
        "{:>8} {:>6}  {:<15} {:>17} {:>17} {:>17} {:>7}",
        "chars", "rows", "text", "area (native)", "wrap/native", "wrap/petal", "petal/n"
    );
    for &n in &sizes {
        let doc = document(n, 42);
        for typing in [false, true] {
            for memo in [true, false] {
                let area = best_of(&script("area", &doc, typing), memo, frames, runs);
                let native = best_of(&script("native", &doc, typing), memo, frames, runs);
                let petal = best_of(&script("petal", &doc, typing), memo, frames, runs);
                assert_eq!(native.rows, petal.rows, "row counts differ at {n} chars");
                let label = format!(
                    "{} {}",
                    if typing { "typing" } else { "still" },
                    if memo { "memo" } else { "no-memo" }
                );
                println!(
                    "{:>8} {:>6}  {:<15} {:>7.3} / {:>7.3} {:>7.3} / {:>7.3} {:>7.3} / {:>7.3} {:>6.1}x",
                    n,
                    native.rows,
                    label,
                    area.p50,
                    area.mean,
                    native.p50,
                    native.mean,
                    petal.p50,
                    petal.mean,
                    petal.p50 / native.p50.max(1e-6),
                );
                if std::env::var_os("MEMO_STATS").is_some() {
                    println!(
                        "{:>33}memo: area {}  native {}  petal {}",
                        "", area.memo, native.memo, petal.memo
                    );
                }
            }
        }
    }
}

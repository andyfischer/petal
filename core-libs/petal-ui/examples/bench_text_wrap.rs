//! Native `text_wrap` against a Petal wrap, for candidate 1 of
//! docs/tasks/ui-refactor.md ("Moving native code into Petal").
//!
//!   cargo run --release --example bench_text_wrap -- [--frames N] [--runs N] [--sizes 1000,10000,100000]
//!   MEMO_STATS=1 cargo run ...     # also the memo's counters for each case
//!
//! The Petal side is `test/benchmarks/text_wrap/wrap_ref.ptl`, a line-for-line
//! reading of `text.rs`'s `wrap_lines` + `wrap_rows` that measures with
//! `text_advance` once per word (it is not shipped; `core-runtime/text-layout`
//! calls the natives and has no wrap of its own).
//!
//! 1. **Equivalence.** Both are run over a corpus of edge cases and generated
//!    documents at several widths and styles; every row (`text`, `start`,
//!    `end`) must match. A mismatch is printed and the run fails.
//! 2. **Per-frame time**, under the headless harness with a proportional
//!    advance table bound, the frame gate off (every frame runs) and the
//!    pointer moving one pixel a frame. For each document size:
//!    - `area`: the real `text_area` (native wrap) over the document;
//!    - `wrap/native`, `wrap/petal`: the wrap alone, `text_wrap_rows` or
//!      `ref_wrap_rows`, at the width `text_area` wraps to;
//!
//!    each with the memo on and off, and with the text unchanged (`still`) or
//!    one character different every frame (`typing`, which no memo can save).
//!    A `text_area` on the Petal wrap would cost about
//!    `area − wrap/native + wrap/petal`.
use std::time::Instant;

use petal_ui::harness::Headless;

const REF: &str = include_str!("../../../test/benchmarks/text_wrap/wrap_ref.ptl");

/// The width `text_area` wraps to in the timed scripts: a 560 px box less its
/// 6 px inset on each side.
const AREA_W: i32 = 560;
const WRAP_W: i32 = AREA_W - 12;

/// A Petal string literal for `s`.
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Advance ratios for ASCII, roughly a UI sans: narrow punctuation and `il`,
/// wide `mw`, capitals wider than lowercase. Anything else takes the
/// fallback ratio.
fn ratios() -> Vec<f64> {
    (0u32..128)
        .map(|cp| {
            let c = char::from_u32(cp).unwrap();
            match c {
                ' ' => 0.27,
                '\t' => 1.08,
                'i' | 'l' | 'j' | '.' | ',' | ';' | ':' | '!' | '|' | '\'' => 0.24,
                'f' | 't' | 'r' | '(' | ')' | '-' => 0.36,
                'm' | 'w' => 0.83,
                'M' | 'W' => 0.92,
                'a'..='z' => 0.55,
                'A'..='Z' => 0.67,
                '0'..='9' => 0.56,
                _ => 0.5,
            }
        })
        .collect()
}

/// A deterministic document of about `n` characters: paragraphs of words of
/// mixed length, the odd blank line, double space, tab and over-long token
/// (a URL a wrap has to break).
fn document(n: usize, seed: u64) -> String {
    const WORDS: &[&str] = &[
        "the",
        "of",
        "and",
        "a",
        "to",
        "in",
        "is",
        "you",
        "that",
        "it",
        "he",
        "was",
        "for",
        "on",
        "are",
        "as",
        "with",
        "his",
        "they",
        "I",
        "at",
        "be",
        "this",
        "have",
        "from",
        "or",
        "one",
        "had",
        "by",
        "word",
        "but",
        "not",
        "what",
        "all",
        "were",
        "we",
        "when",
        "your",
        "can",
        "said",
        "there",
        "use",
        "an",
        "each",
        "which",
        "she",
        "do",
        "how",
        "their",
        "if",
        "will",
        "up",
        "other",
        "about",
        "out",
        "many",
        "then",
        "them",
        "these",
        "so",
        "some",
        "her",
        "would",
        "make",
        "like",
        "him",
        "into",
        "time",
        "has",
        "look",
        "two",
        "more",
        "write",
        "go",
        "see",
        "number",
        "no",
        "way",
        "could",
        "people",
        "my",
        "than",
        "first",
        "water",
        "been",
        "call",
        "who",
        "oil",
        "its",
        "now",
        "find",
        "long",
        "down",
        "day",
        "did",
        "get",
        "come",
        "made",
        "may",
        "part",
        "Borough",
        "budget",
        "Wednesday,",
        "committee.",
        "reconciliation",
        "infrastructure",
        "(approximately)",
        "well-known",
        "Mr.",
        "e.g.",
    ];
    let mut s = String::with_capacity(n + 64);
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) as usize
    };
    let mut in_para = 0usize;
    while s.chars().count() < n {
        let r = next();
        if in_para > 40 && r % 23 == 0 {
            s.push('\n');
            if r % 5 == 0 {
                s.push('\n');
            }
            in_para = 0;
            continue;
        }
        if in_para > 0 {
            s.push_str(match r % 61 {
                0 => "  ",
                1 => "\t",
                _ => " ",
            });
        }
        if r % 97 == 3 {
            s.push_str("https://example.org/a/really/long/path/that/never/fits/in/one/row/of/the/box/index.html");
        } else {
            s.push_str(WORDS[r % WORDS.len()]);
        }
        in_para += 1;
    }
    s.chars().take(n).collect()
}

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
    let mut corpus: Vec<String> = [
        "",
        " ",
        "   ",
        "a",
        "aaa bbb ccc",
        "  leading blanks then words",
        "trailing blanks   ",
        "double  space and\ttab\t\tinside",
        "a\nb",
        "a\n\nb\n",
        "\n\n",
        "crlf line\r\nnext line\r\n",
        "supercalifragilisticexpialidocious",
        "   supercalifragilisticexpialidocious after leading blanks",
        "x supercalifragilisticexpialidocious y",
        "h\u{e9}llo w\u{f6}rld\n\n  \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{6587}\u{7ae0}",
        "quotes \"and\" {braces} and \\ backslash",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (i, n) in [200usize, 1000, 5000].iter().enumerate() {
        corpus.push(document(*n, 7 + i as u64));
    }
    let docs: Vec<String> = corpus.iter().map(|d| lit(d)).collect();
    let src = format!(
        "import ui\n{REF}\n\
         let docs = [{docs}]\n\
         let widths = [-5, 0, 1, 5, 37, 80.5, 120, 200, 333.3, {WRAP_W}, 2000]\n\
         let styles = [{{size: 14}}, {{size: 11, spacing: 0.5}}, {{size: 20, weight: 700}}, 13]\n\
         state checked = 0\n\
         state bad = 0\n\
         state first = \"\"\n\
         var n = 0\n\
         var nbad = 0\n\
         var msg = \"\"\n\
         for d in docs do\n\
           for ts in styles do\n\
             for w in widths do\n\
               let a = text_wrap_rows(d, ts, float(w))\n\
               let b = ref_wrap_rows(d, ts, w)\n\
               set n = n + 1\n\
               if a != b then\n\
                 set nbad = nbad + 1\n\
                 if msg == \"\" then\n\
                   var k = 0\n\
                   while k < len(a) && k < len(b) && a[k] == b[k] do\n\
                     set k = k + 1\n\
                   end\n\
                   let ra = if k < len(a) then str(a[k]) else \"(none)\" end\n\
                   let rb = if k < len(b) then str(b[k]) else \"(none)\" end\n\
                   set msg = \"style \" ++ str(ts) ++ \" width \" ++ str(w) ++ \" row \" ++ str(k) ++ \" of \" ++ str(len(a)) ++ \"/\" ++ str(len(b)) ++ \" native \" ++ ra ++ \" petal \" ++ rb ++ \" doc \" ++ char_slice(d, 0, 60)\n\
                 end\n\
               end\n\
             end\n\
           end\n\
         end\n\
         checked = n\n\
         bad = nbad\n\
         first = msg\n",
        docs = docs.join(", "),
    );
    let mut ui = harness(&src, false);
    ui.frame()
        .unwrap_or_else(|e| panic!("equivalence frame: {e}"));
    let checked = ui.state_int("checked").unwrap_or(0);
    let bad = ui.state_int("bad").unwrap_or(-1);
    println!(
        "equivalence: {checked} (doc, style, width) cases over {} docs, {bad} mismatched",
        corpus.len()
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
            "state fc = focus_state()\n\
             state nrows = 0\n\
             let res = text_area(fc, \"doc\", {{x: 10, y: 10, w: {AREA_W}, h: 780}}, {text}, {{size: 14}})\n\
             fc = res.focus\n\
             nrows = len(res.rows)\n"
        ),
        "native" => format!(
            "state nrows = 0\n\
             nrows = len(text_wrap_rows({text}, {{size: 14}}, {WRAP_W}.0))\n"
        ),
        "petal" => format!(
            "state nrows = 0\n\
             nrows = len(ref_wrap_rows({text}, {{size: 14}}, {WRAP_W}.0))\n"
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

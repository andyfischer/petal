//! What a declarative drag (`ui/drag`) costs, frame by frame, on a big board.
//!
//!   cargo run --release --example bench_drag -- <kanban app.ptl> [cards] [--runs N]
//!
//! Loads the kanban example (`examples/productivity/kanban/app.ptl`, or any
//! copy of it), replaces its `state cols = [...]` board with `cards` cards
//! (default 50) dealt round-robin over the four lanes, and drives its
//! `scenarios/drag.json` (the first card of BACKLOG dragged into REVIEW). Each
//! run starts from a fresh harness; the report is the minimum over `--runs`
//! (default 15) of each frame's time, for the frames that matter:
//!
//! - **idle**: a frame before the press, with the pointer moving;
//! - **press**: the press frame (`spec_fn()` builds the candidate states);
//! - **drag start**: the first frame past the slop, which speculates the view
//!   once per candidate state;
//! - **drag**: the median of the frames while the card is held after that;
//! - **drop** and **settle**: the release frame and the frames after it.
//!
//! The gate is off (`--no-gate` semantics) so a frame that would be skipped
//! is still timed as a run.
use std::time::Instant;

fn board(cards: usize) -> String {
    let tags = ["render", "input", "vm", "prelude", "docs", "tooling"];
    let who = ["AF", "EK", "JN", "MR"];
    let titles = [
        "Embed the Bold face for panel text runs",
        "Wheel scroll should respect clip regions",
        "Document the panel sleep/wake window",
        "Fast path for immutable list append",
    ];
    let mut cols: Vec<Vec<String>> = vec![Vec::new(); 4];
    for i in 0..cards {
        cols[i % 4].push(format!(
            "Card(id: \"c{}\", title: \"{}\", tag: \"{}\", who: \"{}\", pts: {}, pri: {})",
            i + 1,
            titles[i % titles.len()],
            tags[i % tags.len()],
            who[i % who.len()],
            1 + i % 8,
            i % 3
        ));
    }
    let body: Vec<String> = cols.iter().map(|c| format!("  [\n    {}\n  ]", c.join(",\n    "))).collect();
    format!("state cols = [\n{}\n]\n", body.join(",\n"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut cards = 50usize;
    let mut runs = 15usize;
    while let Some(a) = args.next() {
        if a == "--runs" {
            runs = args.next().and_then(|s| s.parse().ok()).expect("--runs N");
        } else if path.is_none() {
            path = Some(a);
        } else {
            cards = a.parse().expect("cards");
        }
    }
    let path = std::path::PathBuf::from(path.expect("usage: bench_drag <kanban app.ptl> [cards] [--runs N]"));
    let src = std::fs::read_to_string(&path).expect("read app");
    let start = src.find("state cols = [").expect("a `state cols = [` board");
    let end = start + src[start..].find("\n]\n").expect("the board's closing `]`") + 3;
    let src = format!("{}{}{}", &src[..start], board(cards), &src[end..]);
    let scen_path = path.parent().unwrap().join("scenarios/drag.json");
    let scenario = petal_ui::scenario::Scenario::from_json_str(
        &std::fs::read_to_string(&scen_path).expect("read scenarios/drag.json"),
    )
    .expect("parse scenario");
    let frames = 40usize;

    let mut best = vec![f64::INFINITY; frames];
    for _ in 0..runs {
        let mut ui = petal_ui::harness::Headless::with_size(&src, 1280, 850).expect("compile");
        ui.env.set_echo(false);
        let mut policy = ui.policy();
        policy.gate = false;
        ui.set_policy(policy);
        for (i, b) in best.iter_mut().enumerate() {
            scenario.apply(&mut ui, i);
            let t = Instant::now();
            ui.frame().expect("frame");
            *b = b.min(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    // drag.json: press at 3, first move past the slop at 5, release at 18.
    let median = |xs: &[f64]| {
        let mut v = xs.to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    println!("{} cards, {} runs, min per frame (ms):", cards, runs);
    println!("  idle        {:.2}", best[2]);
    println!("  press       {:.2}", best[3]);
    println!("  drag start  {:.2}", best[5]);
    println!("  drag        {:.2}  (median of frames 6-17)", median(&best[6..18]));
    println!("  drop        {:.2}", best[18]);
    println!("  settle      {:.2}  (median of frames 19-30)", median(&best[19..31]));
    println!("  still       {:.2}  (median of frames 35-39)", median(&best[35..40]));
}

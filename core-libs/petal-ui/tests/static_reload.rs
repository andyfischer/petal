//! Following the diff over the panel corpus. See `petal::static_cache`
//! ("Following the diff") and docs/tasks/static-value-optimization.md (S2).
//!
//! A reload keeps the static values the edit cannot have changed. The
//! oracle is the reload that rebuilds every one of them
//! (`Env::set_static_follow(false)`): every corpus app is driven by a monkey
//! scenario through a chain of edits on both, and the two must draw and
//! store the same thing on every frame and hold the same static values
//! after each.

use std::path::{Path, PathBuf};

mod common;
use common::par_corpus;

use petal::env::{ReloadOutcome, StaticSlotState};
use petal::policy::RunPolicy;
use petal_ui::harness::Headless;
use petal_ui::scenario::Scenario;

const SIZE: (i32, i32) = (800, 600);

fn open(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64, follow: bool) -> Headless {
    let mut ui = Headless::panel_from_file_with_paths(app, SIZE.0, SIZE.1, includes)
        .unwrap_or_else(|e| panic!("{}: {e}", app.display()));
    ui.env.set_echo(false);
    ui.env.set_seed(seed);
    ui.set_policy(policy);
    ui.env.set_static_follow(follow);
    // A slow machine must not turn a rebuild into a deferred one here.
    ui.env.set_static_debounce(None);
    ui
}

fn frame(ui: &mut Headless, scenario: &Scenario, at: usize) -> String {
    scenario.apply(ui, at);
    let outcome = ui.frame().map(|_| ());
    serde_json::json!({
        "commands": ui.commands,
        "state": ui.state(),
        "prints": ui.env.take_output(),
        "error": outcome.err(),
    })
    .to_string()
}

/// The table as text. A slot the oracle has not filled says nothing.
fn slots(ui: &Headless) -> Vec<Option<String>> {
    let Some(values) = ui.env.static_values(ui.stack_id()) else { return Vec::new() };
    values
        .slots
        .iter()
        .map(|(info, state)| match state {
            StaticSlotState::Filled(v) => Some(format!(
                "{} = {}",
                info.name.as_deref().unwrap_or("(unnamed)"),
                petal::value::value_to_display_string(v, ui.env.heap())
            )),
            _ => None,
        })
        .collect()
}

/// `source` with a declaration and a statement added at its end.
fn grow(source: &str) -> String {
    format!("{}\nlet __swept = [1, 2, 3]\nlet __swept_n = len(__swept)\n", source.trim_end())
}

/// `source` with every seventh number literal nudged.
fn retune(source: &str) -> String {
    use petal::lexer::{Lexer, Token};
    let mut lexer = Lexer::new(source);
    if lexer.tokenize().is_err() {
        return source.to_string();
    }
    let numbers: Vec<(usize, usize, String)> = lexer
        .tokens_with_spans()
        .filter_map(|(t, s)| {
            let text = match t {
                Token::Int(n) => n.checked_add(1)?.to_string(),
                Token::Float(f) => format!("{:?}", f + 0.5),
                _ => return None,
            };
            Some((s.start.offset as usize, s.end.offset as usize, text))
        })
        .collect();
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for (start, end, text) in numbers.iter().step_by(7) {
        out.extend(&chars[at..*start]);
        out.push_str(text);
        at = *end;
    }
    out.extend(&chars[at..]);
    out
}

/// What one app's chain of reloads did: slots kept and rebuilt, per edit.
#[derive(Default, Clone, Copy)]
struct Tally {
    kept: usize,
    rebuilt: usize,
    reloads: usize,
    recompiled: usize,
}

fn chain(app: &Path, includes: &[PathBuf], policy: RunPolicy, seed: u64) -> Tally {
    let what = format!("{} under {}", app.display(), policy.name().unwrap_or_default());
    let original = std::fs::read_to_string(app).unwrap();
    // Values only, back, a structural edit, values only on top of it, back.
    let edits = [
        retune(&original),
        original.clone(),
        grow(&original),
        grow(&retune(&original)),
        original.clone(),
    ];
    let per = 8;
    let scenario = Scenario::monkey(seed, per * (edits.len() + 1), SIZE);
    let mut a = open(app, includes, policy, seed, true);
    let mut b = open(app, includes, policy, seed, false);
    let mut tally = Tally::default();
    let mut at = 0;
    let mut frames = |a: &mut Headless, b: &mut Headless, at: &mut usize, step: &str| {
        for _ in 0..per {
            let (fa, fb) = (frame(a, &scenario, *at), frame(b, &scenario, *at));
            assert!(fa == fb, "{what}: frame {} ({step}) differs when the reload follows the diff", *at);
            let (sa, sb) = (slots(a), slots(b));
            assert_eq!(sa.len(), sb.len(), "{what}: slot count ({step})");
            for (i, (x, y)) in sa.iter().zip(&sb).enumerate() {
                if y.is_some() {
                    assert!(x == y, "{what}: slot {i} after frame {} ({step}): {x:?} vs {y:?}", *at);
                }
            }
            *at += 1;
        }
    };
    frames(&mut a, &mut b, &mut at, "before any edit");
    for (i, new) in edits.iter().enumerate() {
        let step = format!("edit {i}");
        let (sa, sb) = (a.stack_id(), b.stack_id());
        let ra = a.env.reload_program(sa, new, Some(app));
        let rb = b.env.reload_program(sb, new, Some(app));
        match (ra, rb) {
            (Ok(ra), Ok(rb)) => {
                assert_eq!(ra.outcome, rb.outcome, "{what}: {step}");
                assert!(rb.statics.is_none(), "{what}: the oracle follows nothing");
                if let Some(s) = &ra.statics {
                    tally.kept += s.kept;
                    tally.rebuilt += s.rebuilt;
                    tally.reloads += 1;
                    tally.recompiled += (ra.outcome == ReloadOutcome::Recompiled) as usize;
                }
            }
            // A nudged literal that no longer compiles: both keep running
            // the old program.
            (Err(_), Err(_)) => {}
            (ra, rb) => panic!("{what}: {step}: one side reloaded and the other did not: {:?} / {:?}", ra.is_ok(), rb.is_ok()),
        }
        frames(&mut a, &mut b, &mut at, &step);
    }
    tally
}

#[test]
fn a_followed_reload_equals_a_reload_that_rebuilds_everything_across_the_corpus() {
    let seed = 3u64;
    let results = par_corpus(|app, includes| {
        let mut total = Tally::default();
        for name in ["replay", "fast", "replay-memo"] {
            let t = chain(app, includes, RunPolicy::parse(name).unwrap().with_static_cache(true), seed);
            if name == "replay" {
                total = t;
            }
        }
        total
    });
    let kept: usize = results.iter().map(|(_, t)| t.kept).sum();
    let rebuilt: usize = results.iter().map(|(_, t)| t.rebuilt).sum();
    let reloads: usize = results.iter().map(|(_, t)| t.reloads).sum();
    let recompiled: usize = results.iter().map(|(_, t)| t.recompiled).sum();
    eprintln!(
        "{} apps, {reloads} followed reloads ({recompiled} recompiled): {kept} slots kept, {rebuilt} rebuilt",
        results.len()
    );
    assert!(reloads >= results.len() * 4, "only {reloads} reloads were followed");
    assert!(recompiled >= results.len() * 2, "only {recompiled} recompiled");
    assert!(reloads - recompiled >= results.len(), "only {} were patched in place", reloads - recompiled);
    // The oracle is only worth its time if most values are in fact kept.
    assert!(kept > rebuilt, "{kept} kept against {rebuilt} rebuilt");
}

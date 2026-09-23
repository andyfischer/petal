//! Small-scope exhaustive checking of the core language.
//!
//! Where the Kani proofs (`src/proofs/`) cover pure kernels for every input,
//! these tests cover the whole pipeline — lexer, parser, compiler, type
//! checker, lowering, optimizer, VM — for *every* program in a bounded space.
//! The small-scope hypothesis: most bugs have a small counterexample, so
//! checking all small inputs catches what random sampling misses.
//!
//! Two spaces are enumerated:
//!
//! 1. **Token sequences** up to length `PETAL_SMALL_SCOPE_TOKENS` (default 2)
//!    over a vocabulary that reaches every statement form. Property: nothing
//!    panics — every program either runs or reports an error.
//! 2. **Expressions** built from value atoms and every unary/binary operator,
//!    up to depth `PETAL_SMALL_SCOPE_DEPTH` (default 2). Properties: nothing
//!    panics; the unoptimized VM (`RunPolicy::BASELINE`) and the optimized one
//!    (`RunPolicy::FAST`) print identical output or identical errors; and `==`
//!    is symmetric with `!=` its negation.
//!
//! Raise the bounds for a soak run, e.g.
//! `PETAL_SMALL_SCOPE_TOKENS=4 cargo test --release --test small_scope`.
//! See docs/dev/formal-verification.md.

use std::panic::{AssertUnwindSafe, catch_unwind};

use petal::env::{Env, RunOutcome};
use petal::policy::RunPolicy;

/// Step budget per run: every generated program that terminates does so well
/// inside it; the rest (`while true do end`) are cut off, which is fine.
const MAX_STEPS: u64 = 5_000;

fn bound(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Outcome of one run, rendered for comparison across policies.
#[derive(Debug, PartialEq)]
enum Outcome {
    Done(Vec<String>),
    Error(String),
    OutOfSteps,
}

fn run(src: &str, policy: RunPolicy) -> Outcome {
    let mut env = Env::new();
    env.set_policy(policy);
    let pid = match env.load_program(src) {
        Ok(p) => p,
        Err(e) => return Outcome::Error(e),
    };
    let sid = env.create_stack(pid).expect("stack");
    match env.run_bounded(sid, MAX_STEPS) {
        Ok(RunOutcome::Done(_)) => Outcome::Done(env.take_output()),
        Ok(RunOutcome::Yielded { .. }) => Outcome::OutOfSteps,
        Err(e) => Outcome::Error(e),
    }
}

/// Run `src` under `policy`, turning a panic into a test failure that names
/// the program. (The default panic hook stays installed, so a caught panic
/// still prints its location first.)
fn run_no_panic(src: &str, policy: RunPolicy) -> Outcome {
    match catch_unwind(AssertUnwindSafe(|| run(src, policy))) {
        Ok(o) => o,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panic!("panic on program:\n{src}\n--- panic ---\n{msg}")
        }
    }
}

// ── 1. Token sequences ──────────────────────────────────────────

const TOKENS: &[&str] = &[
    "x", "f", "1", "0", "2.5", "\"s\"", "\"{x}\"", "nil", "true", "(", ")", "[", "]", "{", "}",
    ",", ".", ":", "=", "+", "-", "*", "/", "%", "==", "<", "!", "&&", "??", "++", "|>", "=>", "@",
    "let", "var", "set", "fn", "end", "if", "else", "for", "in", "do", "while", "match", "state",
    "return", "break", "enum", "\n",
];

/// Check `f` on every item, split across the machine's cores.
fn par_for_each<T: Sync>(items: &[T], f: impl Fn(&T) + Sync) {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = items.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for part in items.chunks(chunk) {
            let f = &f;
            s.spawn(move || part.iter().for_each(f));
        }
    });
}

#[test]
fn no_token_sequence_panics() {
    let max_len = bound("PETAL_SMALL_SCOPE_TOKENS", 2);
    // Every sequence of 1..=max_len tokens, space-separated.
    let mut programs: Vec<String> = vec![String::new()];
    let mut all = Vec::new();
    for _ in 0..max_len {
        programs = programs
            .iter()
            .flat_map(|p| TOKENS.iter().map(move |t| format!("{p} {t}")))
            .collect();
        all.extend(programs.iter().cloned());
    }
    par_for_each(&all, |src| {
        run_no_panic(src, RunPolicy::FAST);
    });
    eprintln!("small_scope: {} token sequences, no panics", all.len());
}

// ── 2. Expressions ──────────────────────────────────────────────

/// Value atoms: each numeric edge (zero, negatives, i64 extremes, a fraction,
/// NaN, infinity) and one of every container kind.
const ATOMS: &[&str] = &[
    "0",
    "1",
    "-1",
    "2.5",
    "-0.0",
    "sqrt(-1.0)",
    "1.0 / 1e-320",
    "9223372036854775807",
    "(-9223372036854775807 - 1)",
    "9007199254740993",
    "9007199254740992.0",
    "\"a\"",
    "\"\"",
    "[1, 2]",
    "[]",
    "{a: 1}",
    "nil",
    "true",
];

const BINOPS: &[&str] = &[
    "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "++", "&&", "||", "??",
];

const UNOPS: &[&str] = &["-", "!"];

fn expressions(depth: usize) -> Vec<String> {
    let mut out: Vec<String> = ATOMS.iter().map(|s| s.to_string()).collect();
    for _ in 0..depth.saturating_sub(1) {
        let prev = out.clone();
        let mut next = prev.clone();
        for a in &prev {
            for op in UNOPS {
                next.push(format!("{op}({a})"));
            }
            next.push(format!("({a})[0]"));
            next.push(format!("({a})[-1]"));
            next.push(format!("({a}).a"));
            next.push(format!("abs({a})"));
            next.push(format!("sort([{a}, 1, {a}, 0.5, {a}])"));
        }
        for a in &prev {
            for b in &prev {
                for op in BINOPS {
                    next.push(format!("({a}) {op} ({b})"));
                }
            }
        }
        out = next;
    }
    out
}

#[test]
fn expressions_agree_across_optimizer_and_never_panic() {
    let depth = bound("PETAL_SMALL_SCOPE_DEPTH", 2);
    let exprs = expressions(depth);
    par_for_each(&exprs, |e| {
        let src = format!("print({e})");
        let base = run_no_panic(&src, RunPolicy::BASELINE);
        let fast = run_no_panic(&src, RunPolicy::FAST);
        assert_eq!(base, fast, "optimizer changes the result of:\n{src}");
    });
    eprintln!("small_scope: {} expressions, BASELINE == FAST", exprs.len());
}

/// `a == b` iff `b == a`, and `a != b` is its negation, for every pair of
/// atoms and depth-1 expressions (the value-level laws are checked more deeply
/// by the exhaustive tests in `value.rs`).
#[test]
fn equality_is_symmetric_on_atoms() {
    for a in ATOMS {
        for b in ATOMS {
            let src = format!("let a = {a}\nlet b = {b}\nprint(a == b, b == a, a != b)");
            let Outcome::Done(out) = run_no_panic(&src, RunPolicy::FAST) else {
                panic!("did not run:\n{src}");
            };
            let parts: Vec<&str> = out[0].split(' ').collect();
            assert_eq!(parts[0], parts[1], "== is not symmetric:\n{src}");
            assert_ne!(parts[0], parts[2], "!= is not the negation of ==:\n{src}");
        }
    }
}

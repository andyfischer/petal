//! Direct manipulation v2, milestone M1: the speculative runner
//! (`petal::dm_v2::spec`). The acceptance list is in
//! docs/dev/direct-manipulation-v2-plan.md §5 (M1); each test names the item
//! it covers. Tests 8 and 16 need petal-ui's draw natives and host data and
//! live in `core-libs/petal-ui/tests/dm_v2_spec.rs`.

use std::cell::Cell;
use std::rc::Rc;

use petal::constant_table::ConstantValue;
use petal::dm_v2::spec::{
    CaptureSpec, EmitRecord, NoHost, ObsValue, OverrideStrategy, OverrideValue, Overrides, RngMode,
    RunError, SpecConfig, SpecError, SpecRun, SpecSession, StateMode,
};
use petal::dm_v2::{Observable, PathStep};
use petal::env::Env;
use petal::native_fn::{NativeEffects, NativeFnId};
use petal::program::{ProgramId, TermId, TermOp};
use petal::stack::StackKey;
use petal::value::Value;

// ── helpers ──────────────────────────────────────────────────────────────

/// A fresh env with `source` loaded on a stack that has never run, the
/// random seed pinned.
fn load(source: &str) -> (Env, ProgramId, StackKey) {
    let mut env = Env::new();
    env.set_seed(7);
    let pid = env.load_program(&with_imports(source)).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, pid, sid)
}

/// `source` with the packages these tests use imported, on its first line so
/// line numbers stay the source's own.
fn with_imports(source: &str) -> String {
    format!("import io; import creative; {source}")
}

/// [`with_imports`], declaring the host natives `names` too.
fn with_natives(names: &[&str], source: &str) -> String {
    let decls: String = names.iter().map(|n| format!("native fn {n}(); ")).collect();
    format!("import io; import creative; {decls}{source}")
}

/// The numeric literal terms of the entry file whose source text is `text`,
/// on 1-based line `line`, in source order.
fn lits(env: &Env, pid: ProgramId, line: u32, text: &str) -> Vec<TermId> {
    let program = env.get_program(pid).unwrap();
    let source = &program.source_map.files[0].source;
    let mut found: Vec<(u32, TermId)> = program
        .terms
        .iter()
        .filter(|t| matches!(t.op, TermOp::Constant(_)))
        .filter_map(|t| {
            let span = program.source_map.get(t.id)?;
            if span.file.0 != 0 || span.start.line != line {
                return None;
            }
            let s = source.get(span.start.offset as usize..span.end.offset as usize)?;
            (s == text).then_some((span.start.offset, t.id))
        })
        .collect();
    found.sort_by_key(|(offset, _)| *offset);
    found.into_iter().map(|(_, t)| t).collect()
}

/// The one literal `text` on `line`.
fn lit(env: &Env, pid: ProgramId, line: u32, text: &str) -> TermId {
    let found = lits(env, pid, line, text);
    assert_eq!(found.len(), 1, "literal `{text}` on line {line}: {found:?}");
    found[0]
}

/// Every numeric literal of the entry file that is written in the source as
/// itself (not a constant the compiler made up), with its value.
fn all_numeric_literals(env: &Env, pid: ProgramId) -> Vec<(TermId, ConstantValue)> {
    let program = env.get_program(pid).unwrap();
    let source = &program.source_map.files[0].source;
    program
        .terms
        .iter()
        .filter_map(|t| {
            let TermOp::Constant(k) = t.op else { return None };
            let value = program.constants.get(k).clone();
            let span = program.source_map.get(t.id)?;
            if span.file.0 != 0 {
                return None;
            }
            let text = source.get(span.start.offset as usize..span.end.offset as usize)?;
            let matches = match &value {
                ConstantValue::Int(i) => text.parse::<i64>().ok() == Some(*i),
                ConstantValue::Float(bits) => text.parse::<f64>().ok() == Some(f64::from_bits(*bits)),
                _ => false,
            };
            matches.then_some((t.id, value))
        })
        .collect()
}

fn begin(env: &mut Env, sid: StackKey, params: &[TermId], cfg: SpecConfig) -> SpecSession {
    SpecSession::begin(env, &mut NoHost, sid, params, cfg, None).expect("session opens")
}

fn nums(run: &SpecRun) -> Vec<f64> {
    run.emits.iter().map(|e| e.value.as_f64().expect("a number")).collect()
}

fn live_nums(env: &mut Env, channel: &str) -> Vec<f64> {
    let sym = env.intern_symbol(channel);
    let values = env.take_output_buffer(sym);
    values
        .iter()
        .map(|v| match v {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            other => panic!("not a number: {}", other.type_name()),
        })
        .collect()
}

fn run(session: &mut SpecSession, env: &mut Env, overrides: Overrides) -> SpecRun {
    let r = session.run(env, &mut NoHost, &overrides);
    assert!(r.ok(), "run failed: {:?}", r.error);
    r
}

// ── 1. override_moves_only_that_literal ──────────────────────────────────

#[test]
fn override_moves_only_that_literal() {
    let src = "let s = symbol(\"out\")\nlet a = 10\nlet b = 10\npush_output(s, a)\npush_output(s, b)\n";
    let (mut env, pid, sid) = load(src);
    let a = lit(&env, pid, 2, "10");
    let b = lit(&env, pid, 3, "10");
    // Interned: the two `10`s share one constant before slotting.
    let program = env.get_program(pid).unwrap();
    let constant = |t: TermId| match program.get_term(t).op {
        TermOp::Constant(k) => k,
        _ => unreachable!(),
    };
    assert_eq!(constant(a), constant(b), "one interned constant");

    let mut session = begin(&mut env, sid, &[a], SpecConfig::default());
    assert_eq!(nums(session.baseline()), vec![10.0, 10.0]);
    let r = run(&mut session, &mut env, Overrides::one(a, OverrideValue::Num(12.5)));
    assert_eq!(nums(&r), vec![12.5, 10.0]);
    assert!(r.loaded.contains(0));
    // Slotting changed nothing about the live program.
    env.run(sid).unwrap();
    assert_eq!(live_nums(&mut env, "out"), vec![10.0, 10.0]);
}

// ── 2. live_stack_untouched ──────────────────────────────────────────────

const LIVE_SRC: &str = "let s = symbol(\"out\")
state n = 0
n = n + 1
let k = 3
print(\"frame {n}\")
push_output(s, n * k + random())
";

#[test]
fn live_stack_untouched() {
    // Twin envs: one runs twice; the other runs, opens a session, makes 50
    // speculative runs, and runs again. The second live run must be the same.
    let (mut twin, _, twin_sid) = load(LIVE_SRC);
    twin.run(twin_sid).unwrap();
    twin.take_output();
    let _ = live_nums(&mut twin, "out");
    twin.reset_stack(twin_sid).unwrap();
    twin.run(twin_sid).unwrap();
    let twin_second = live_nums(&mut twin, "out");
    let twin_prints = twin.take_output();

    let (mut env, pid, sid) = load(LIVE_SRC);
    env.set_echo(false);
    env.run(sid).unwrap();
    env.take_output();
    let _ = live_nums(&mut env, "out");
    let state_before = env.get_state_json(pid, sid);

    let k = lit(&env, pid, 4, "3");
    env.reset_stack(sid).unwrap();
    let mut session = begin(&mut env, sid, &[k], SpecConfig::default());
    for i in 0..50 {
        run(&mut session, &mut env, Overrides::one(k, OverrideValue::Num(3.0 + i as f64)));
    }
    session.end(&mut env);
    assert!(env.take_output().is_empty(), "no speculative print reached the live buffer");
    assert!(live_nums(&mut env, "out").is_empty(), "no speculative emit reached the live buffer");
    assert_eq!(env.get_state_json(pid, sid), state_before, "live state unchanged");

    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(live_nums(&mut env, "out"), twin_second);
    assert_eq!(env.take_output(), twin_prints);
}

// ── 3. print_is_contained_and_silent ─────────────────────────────────────

#[test]
fn print_is_contained_and_silent() {
    let src = "let s = symbol(\"out\")\nlet x = 1\nprint(\"x\")\npush_output(s, x)\n";
    let (mut env, pid, sid) = load(src);
    let x = lit(&env, pid, 2, "1");
    let mut session = begin(&mut env, sid, &[x], SpecConfig::default());
    let r = run(&mut session, &mut env, Overrides::one(x, OverrideValue::Int(2)));
    // `print` is contained: called, not suppressed.
    assert!(r.suppressed.is_empty(), "{:?}", r.suppressed);
    assert_eq!(nums(&r), vec![2.0]);
    assert!(env.take_output().is_empty(), "the live output buffer is empty");
}

// ── 4. escaping_effect_suppressed ────────────────────────────────────────

#[test]
fn escaping_effect_suppressed() {
    let escaping = Rc::new(Cell::new(0));
    let contained = Rc::new(Cell::new(0));
    let mut env = Env::new();
    env.set_seed(7);
    let e = escaping.clone();
    env.register_native_boxed(
        "bump_host",
        move |cxt| {
            e.set(e.get() + 1);
            cxt.push_int(1);
            Ok(1)
        },
        NativeEffects::EFFECT,
    );
    let c = contained.clone();
    env.register_native_boxed(
        "bump_inside",
        move |cxt| {
            c.set(c.get() + 1);
            cxt.push_int(2);
            Ok(1)
        },
        NativeEffects::EFFECT.contained(),
    );
    let src = "let s = symbol(\"out\")\nlet x = 5\nlet h = bump_host()\nlet i = bump_inside()\npush_output(s, x)\npush_output(s, h)\npush_output(s, i)\n";
    let pid = env.load_program(&with_natives(&["bump_host", "bump_inside"], src)).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let x = lit(&env, pid, 2, "5");

    let mut session = begin(&mut env, sid, &[x], SpecConfig::default());
    let r = run(&mut session, &mut env, Overrides::one(x, OverrideValue::Int(6)));
    assert_eq!(escaping.get(), 0, "the escaping native was never called");
    assert_eq!(contained.get(), 3, "the contained one was, by both baselines and the run");
    assert_eq!(r.suppressed.len(), 1);
    assert_eq!(r.suppressed[0].native, "bump_host");
    // The suppressed call answered nil.
    assert_eq!(r.emits[1].value, ObsValue::Nil);
    assert_eq!(r.emits[2].value, ObsValue::Int(2));
}

// ── 5. rng_replay_by_callsite ────────────────────────────────────────────

#[test]
fn rng_replay_by_callsite() {
    let src = "let s = symbol(\"r\")
let n = 3
for i in range(0, n) do
  push_output(s, random())
end
push_output(s, random())
";
    let (mut env, pid, sid) = load(src);
    let n = lit(&env, pid, 2, "3");
    let mut session = begin(&mut env, sid, &[n], SpecConfig::default());
    let base = nums(session.baseline());
    assert_eq!(base.len(), 4);

    let r = run(&mut session, &mut env, Overrides::one(n, OverrideValue::Int(4)));
    let got = nums(&r);
    assert_eq!(got.len(), 5);
    assert_eq!(&got[..3], &base[..3], "the first three draws are where they were");
    assert_eq!(got[4].to_bits(), base[3].to_bits(), "the draw after the loop is unchanged");
    assert_eq!(r.new_random_keys, 1, "one draw at a place the baseline never drew at");

    // Under the plain forked stream the draw after the loop shifts.
    let f = session.run_fork_stream(&mut env, &mut NoHost, &Overrides::one(n, OverrideValue::Int(4)));
    let fork = nums(&f);
    assert_eq!(&fork[..3], &base[..3]);
    assert_ne!(fork[4].to_bits(), base[3].to_bits(), "the test bites: ForkStream shifts it");
}

#[test]
fn rand_deterministic_across_reruns() {
    let src = "let s = symbol(\"r\")\nlet k = 2\nfor i in range(0, 5) do push_output(s, random() * k) end\n";
    let (mut env, pid, sid) = load(src);
    let k = lit(&env, pid, 2, "2");
    let mut session = begin(&mut env, sid, &[k], SpecConfig::default());
    let first = run(&mut session, &mut env, Overrides::one(k, OverrideValue::Num(3.0)));
    for _ in 0..5 {
        let again = run(&mut session, &mut env, Overrides::one(k, OverrideValue::Num(3.0)));
        assert_eq!(first.first_difference(&again), None);
    }
    // And the draws are the baseline's, scaled.
    let base = nums(session.baseline());
    for (b, f) in base.iter().zip(nums(&first)) {
        assert!((b / 2.0 * 3.0 - f).abs() < 1e-12);
    }
}

// ── 6. double_baseline_detects_nondeterminism ────────────────────────────

#[test]
fn double_baseline_detects_nondeterminism() {
    let ticks = Rc::new(Cell::new(0i64));
    let mut env = Env::new();
    let t = ticks.clone();
    // Declared PURE on purpose, which is wrong: it reads a Rust counter.
    env.register_native_boxed(
        "tick",
        move |cxt| {
            t.set(t.get() + 1);
            cxt.push_int(t.get());
            Ok(1)
        },
        NativeEffects::PURE,
    );
    let pid = env.load_program(&with_natives(&["tick"], "let s = symbol(\"out\")\npush_output(s, tick())\n")).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let err = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None)
        .err()
        .expect("the session refuses");
    assert!(matches!(err, SpecError::Nondeterministic { .. }), "{err:?}");
}

// ── 7. frozen_state ──────────────────────────────────────────────────────

#[test]
fn frozen_state() {
    let src = "let s = symbol(\"n\")\nstate n = 0\nn = n + 1\nlet d = 1\npush_output(s, n * d)\n";
    let (mut env, pid, sid) = load(src);
    env.run(sid).unwrap();
    let _ = live_nums(&mut env, "n");
    let before = env.get_state_json(pid, sid);
    let d = lit(&env, pid, 4, "1");
    env.reset_stack(sid).unwrap();

    let mut session = begin(&mut env, sid, &[d], SpecConfig::default());
    assert_eq!(nums(session.baseline()), vec![2.0], "runs from the checkpoint's n = 1");
    for _ in 0..5 {
        let r = run(&mut session, &mut env, Overrides::one(d, OverrideValue::Int(1)));
        assert_eq!(nums(&r), vec![2.0], "every run starts from the same state");
    }
    session.end(&mut env);
    assert_eq!(env.get_state_json(pid, sid), before, "the live n is unchanged");

    // Fresh state runs the initializer.
    let cfg = SpecConfig {
        state: StateMode::Fresh,
        ..Default::default()
    };
    let mut fresh = begin(&mut env, sid, &[d], cfg);
    assert_eq!(nums(fresh.baseline()), vec![1.0]);
    let r = run(&mut fresh, &mut env, Overrides::one(d, OverrideValue::Int(5)));
    assert_eq!(nums(&r), vec![5.0]);
}

// ── literal inside a function body ───────────────────────────────────────

#[test]
fn override_inside_function_body() {
    let src = "fn place(i)
  i * 20 + 7
end
let s = symbol(\"x\")
for i in range(0, 3) do
  push_output(s, place(i))
end
";
    let (mut env, pid, sid) = load(src);
    let seven = lit(&env, pid, 2, "7");
    let mut session = begin(&mut env, sid, &[seven], SpecConfig::default());
    assert_eq!(nums(session.baseline()), vec![7.0, 27.0, 47.0]);
    // The baseline saw the calls to `place`.
    assert!(!session.baseline().call_edges.is_empty());
    let r = run(&mut session, &mut env, Overrides::one(seven, OverrideValue::Num(9.5)));
    assert_eq!(nums(&r), vec![9.5, 29.5, 49.5]);
    assert!(r.loaded.contains(0));
}

#[test]
fn loaded_set_names_only_what_ran() {
    let src = "let s = symbol(\"x\")\nlet a = 1\nif false then push_output(s, 2) end\npush_output(s, a)\n";
    let (mut env, pid, sid) = load(src);
    let a = lit(&env, pid, 2, "1");
    let two = lit(&env, pid, 3, "2");
    let session = begin(&mut env, sid, &[a, two], SpecConfig::default());
    let b = session.baseline();
    assert!(b.loaded.contains(0));
    assert!(!b.loaded.contains(1), "a literal on a branch not taken is never loaded");
}

// ── 9. emit_key_stable_in_loops ──────────────────────────────────────────

#[test]
fn emit_key_stable_in_loops() {
    let src = "let s = symbol(\"c\")
let r = 5
for i in range(0, 5) do
  push_output(s, i * r)
end
forEach([0, 1, 2, 3, 4], fn(j) push_output(s, j * r) end)
";
    let (mut env, pid, sid) = load(src);
    let r = lit(&env, pid, 2, "5");
    let mut session = begin(&mut env, sid, &[r], SpecConfig::default());
    let base = session.baseline().emits.clone();
    assert_eq!(base.len(), 10);
    // The loop's five emits share a chain and differ by their loop path.
    let looped = &base[..5];
    assert!(looped.iter().all(|e| e.key.chain_hash == looped[0].key.chain_hash));
    let mut paths: Vec<u64> = looped.iter().map(|e| e.key.path_hash).collect();
    paths.dedup();
    assert_eq!(paths.len(), 5, "one loop path per iteration");
    // A callback has no loop path: its emits are told apart by occurrence.
    let cb = &base[5..];
    let occ: Vec<u32> = cb.iter().map(|e| e.key.occurrence).collect();
    assert_eq!(occ, vec![0, 1, 2, 3, 4]);

    let p = run(&mut session, &mut env, Overrides::one(r, OverrideValue::Int(6)));
    let keys = |es: &[EmitRecord]| es.iter().map(|e| e.key).collect::<Vec<_>>();
    assert_eq!(keys(&p.emits), keys(&base), "same trip count, same keys");
    assert_ne!(nums(&p), nums(session.baseline()));
}

// ── 10. fingerprint_flips_on_branch ──────────────────────────────────────

#[test]
fn fingerprint_flips_on_branch() {
    let src = "let s = symbol(\"b\")\nlet x = 5.0\nif x > 5 then push_output(s, 1) else push_output(s, 2) end\n";
    let (mut env, pid, sid) = load(src);
    let x = lit(&env, pid, 2, "5.0");
    let mut session = begin(&mut env, sid, &[x], SpecConfig::default());
    let base = session.baseline().fingerprint;
    let up = run(&mut session, &mut env, Overrides::one(x, OverrideValue::Num(5.1)));
    let down = run(&mut session, &mut env, Overrides::one(x, OverrideValue::Num(4.9)));
    assert_ne!(up.fingerprint, base, "5.1 takes the other branch");
    assert_eq!(down.fingerprint, base, "4.9 does not");
    assert_eq!(nums(&up), vec![1.0]);
    assert_eq!(nums(&down), vec![2.0]);
}

// ── 11. recompile_oracle_agrees ──────────────────────────────────────────

/// Slot overrides and a recompile of the edited source give the same
/// observables, for every literal of every seed program.
#[test]
fn recompile_oracle_agrees() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dm_v2");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("tests/dm_v2 exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ptl"))
        .collect();
    files.sort();
    assert!(files.len() >= 5, "the seed set: {files:?}");
    let mut checked = 0;
    for file in files {
        let src = std::fs::read_to_string(&file).unwrap();
        let (mut env, pid, sid) = load(&src);
        let literals = all_numeric_literals(&env, pid);
        assert!(!literals.is_empty(), "{}: no literals", file.display());
        let terms: Vec<TermId> = literals.iter().map(|(t, _)| *t).collect();
        let slots_cfg = SpecConfig {
            rng: RngMode::ForkStream,
            ..Default::default()
        };
        let mut slots = begin(&mut env, sid, &terms, slots_cfg.clone());
        let mut recompile = begin(
            &mut env,
            sid,
            &terms,
            SpecConfig {
                strategy: OverrideStrategy::Recompile,
                ..slots_cfg
            },
        );
        for (term, value) in &literals {
            let over = match value {
                ConstantValue::Int(i) => OverrideValue::Int(i + 1),
                ConstantValue::Float(bits) => OverrideValue::Num(f64::from_bits(*bits) + 1e-3),
                _ => unreachable!(),
            };
            let o = Overrides::one(*term, over);
            let a = slots.run(&mut env, &mut NoHost, &o);
            let b = recompile.run(&mut env, &mut NoHost, &o);
            match (&a.error, &b.error) {
                // Both fail (a perturbed range bound, say): fine, as long
                // as both do. The messages name different source text.
                (Some(_), Some(_)) => {}
                _ => {
                    if let Err(e) = a.same_emits_by_position(&b) {
                        panic!("{}: literal {:?} -> {:?}: {e}", file.display(), value, over);
                    }
                    // The override moved something (or the literal is dead
                    // code): either way the oracle compared real runs.
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 20, "checked {checked} literals");
}

// ── 12. budget_overrun_is_a_failed_run ───────────────────────────────────

#[test]
fn budget_overrun_is_a_failed_run() {
    let src = "let s = symbol(\"w\")\nlet n = 10\nvar i = 0\nwhile i < n do\n  set i = i + 1\nend\npush_output(s, i)\n";
    let (mut env, pid, sid) = load(src);
    let n = lit(&env, pid, 2, "10");
    let mut session = begin(&mut env, sid, &[n], SpecConfig::default());
    let r = session.run(&mut env, &mut NoHost, &Overrides::one(n, OverrideValue::Int(1_000_000_000_000)));
    assert_eq!(r.error, Some(RunError::StepBudget));
    // The session is still usable.
    let ok = run(&mut session, &mut env, Overrides::one(n, OverrideValue::Int(12)));
    assert_eq!(nums(&ok), vec![12.0]);
}

#[test]
fn runtime_error_is_a_failed_run() {
    let src = "let s = symbol(\"w\")\nlet n = 2\nlet xs = [1, 2, 3]\npush_output(s, xs[n])\n";
    let (mut env, pid, sid) = load(src);
    let n = lit(&env, pid, 2, "2");
    let mut session = begin(&mut env, sid, &[n], SpecConfig::default());
    let r = session.run(&mut env, &mut NoHost, &Overrides::one(n, OverrideValue::Num(0.5)));
    // Whatever indexing by 0.5 does, an error is data and the session lives.
    if let Some(e) = &r.error {
        assert!(matches!(e, RunError::Runtime(_)), "{e:?}");
    }
    let ok = run(&mut session, &mut env, Overrides::one(n, OverrideValue::Int(0)));
    assert_eq!(nums(&ok), vec![1.0]);
}

// ── 14. env_trace_untouched ──────────────────────────────────────────────

#[test]
fn env_trace_untouched() {
    let src = "let s = symbol(\"t\")
let k = 3
let xs = [k, k + 1, k + 2]
var junk = 0
for i in range(0, 20000) do
  let tmp = [i, i + 1]
  set junk = junk + len(tmp)
end
push_output(s, xs[0] + junk)
";
    let (mut env, pid, sid) = load(src);
    env.trace_mut().enable();
    env.observations_mut().enabled = true;
    env.run(sid).unwrap();
    let _ = live_nums(&mut env, "t");
    let len = env.trace().len();
    let last = env.trace().events().last().map(|e| e.sequence);
    let k = lit(&env, pid, 2, "3");
    // Watch the list: recorded as a value, so a collection inside the run
    // cannot take it away.
    let xs_term = env
        .get_program(pid)
        .unwrap()
        .terms
        .iter()
        .find(|t| t.name.as_deref() == Some("xs"))
        .map(|t| t.id)
        .expect("xs");
    let cfg = SpecConfig {
        capture: CaptureSpec {
            watch: vec![xs_term],
            ..Default::default()
        },
        ..Default::default()
    };
    env.reset_stack(sid).unwrap();
    let mut session = begin(&mut env, sid, &[k], cfg);
    let mut last_run = None;
    for i in 0..20 {
        last_run = Some(run(&mut session, &mut env, Overrides::one(k, OverrideValue::Int(4 + i))));
    }
    assert_eq!(env.trace().len(), len, "no speculative event in the Env trace");
    assert_eq!(env.trace().events().last().map(|e| e.sequence), last);
    let r = last_run.unwrap();
    let watched = r.watched.iter().find(|w| w.term == xs_term).expect("xs was watched");
    assert_eq!(
        watched.value,
        ObsValue::List(vec![ObsValue::Int(23), ObsValue::Int(24), ObsValue::Int(25)])
    );
    // Observable::Watch reads it before the emit.
    let at = r.emits[0].key;
    let o = Observable::Watch {
        term: xs_term,
        at,
        path: vec![PathStep::Index(1)],
    };
    assert_eq!(r.read(&o), Some(24.0));
}

// ── 15. core_effect_rows_contained ───────────────────────────────────────

#[test]
fn core_effect_rows_contained() {
    let env = Env::new();
    let names = env.native_fn_names();
    let mut effects = 0;
    for (i, name) in names.iter().enumerate() {
        let row = env.native_effects(NativeFnId(i as u32));
        if row.effect {
            effects += 1;
            assert!(
                row.contained,
                "core native `{name}` has an effect that is not contained: a speculative run would \
                 suppress it. Mark it `.contained()` if its effect stays in the execution context"
            );
        }
    }
    assert!(effects >= 10, "found {effects} effect rows");
}

#[test]
fn methods_and_enums_run_identically_in_spec_mode() {
    let src = "class Point
  x: float,
  y: float
end
fn Point.len(p: Point) -> float
  sqrt(p.x * p.x + p.y * p.y)
end
enum Shade
  light(v: 0.25),
  dark(v: 0.75)
end
let s = symbol(\"m\")
let p = Point(x: 3.0, y: 4.0)
push_output(s, p.len())
push_output(s, Shade.dark.v)
";
    let (mut env, pid, sid) = load(src);
    let three = lit(&env, pid, 13, "3.0");
    let session = begin(&mut env, sid, &[three], SpecConfig::default());
    let spec = nums(session.baseline());
    env.run(sid).unwrap();
    let live = live_nums(&mut env, "m");
    assert_eq!(spec, live);
    assert_eq!(spec, vec![5.0, 0.75]);
}

// ── 17. state_key_miss_changes_fingerprint ───────────────────────────────

#[test]
fn state_key_miss_changes_fingerprint() {
    let src = "let s = symbol(\"k\")
let base = 0
for i in range(0, 3) key i + base do
  state c = 0
  c = c + 1
  push_output(s, c)
end
";
    let (mut env, pid, sid) = load(src);
    env.run(sid).unwrap();
    let _ = live_nums(&mut env, "k");
    env.reset_stack(sid).unwrap();
    let base = lit(&env, pid, 2, "0");
    let mut session = begin(&mut env, sid, &[base], SpecConfig::default());
    assert_eq!(nums(session.baseline()), vec![2.0, 2.0, 2.0]);
    // Same trip count, but key 3 was never seen: its state initializes.
    let shifted = run(&mut session, &mut env, Overrides::one(base, OverrideValue::Int(1)));
    assert_eq!(nums(&shifted), vec![2.0, 2.0, 1.0]);
    assert_ne!(shifted.fingerprint, session.baseline().fingerprint);
    let same = run(&mut session, &mut env, Overrides::one(base, OverrideValue::Int(0)));
    assert_eq!(same.fingerprint, session.baseline().fingerprint);
}

// ── 18. rng_uniform_replay_maps_args ─────────────────────────────────────

#[test]
fn rng_uniform_replay_maps_args() {
    let src = |w: i64| format!("let s = symbol(\"r\")\nlet w = {w}\npush_output(s, random_int(0, w))\npush_output(s, random(w))\n");
    let (mut env, pid, sid) = load(&src(10));
    let w = lit(&env, pid, 2, "10");
    let mut session = begin(&mut env, sid, &[w], SpecConfig::default());
    let r = run(&mut session, &mut env, Overrides::one(w, OverrideValue::Int(1000)));

    let (mut real, _, real_sid) = load(&src(1000));
    real.run(real_sid).unwrap();
    assert_eq!(nums(&r), live_nums(&mut real, "r"), "the same draws, mapped with the new argument");
}

// ── 19. channel_order_independent ────────────────────────────────────────

#[test]
fn channel_order_independent() {
    let src = "let a = symbol(\"a\")\nlet b = symbol(\"b\")\nlet k = 1\nfor i in range(0, 4) do\n  push_output(a, i * k)\n  push_output(b, i + k)\nend\n";
    let (mut env, pid, sid) = load(src);
    let k = lit(&env, pid, 3, "1");
    let first = begin(&mut env, sid, &[k], SpecConfig::default()).baseline().clone();
    for _ in 0..10 {
        let mut s = begin(&mut env, sid, &[k], SpecConfig::default());
        assert_eq!(s.baseline().first_difference(&first), None);
        let r = run(&mut s, &mut env, Overrides::one(k, OverrideValue::Int(1)));
        assert_eq!(r.first_difference(&first), None);
    }
}

// ── emitted values and raw args as observables ───────────────────────────

#[test]
fn observables_read_emits() {
    let src = "let s = symbol(\"o\")\nlet x = 1.5\npush_output(s, {x: x * 2, y: [x, 7]})\n";
    let (mut env, pid, sid) = load(src);
    let x = lit(&env, pid, 2, "1.5");
    let mut session = begin(&mut env, sid, &[x], SpecConfig::default());
    let key = session.baseline().emits[0].key;
    let field = Observable::EmitValue {
        key,
        path: vec![PathStep::Field("x".into())],
    };
    let nested = Observable::EmitValue {
        key,
        path: vec![PathStep::Field("y".into()), PathStep::Index(0)],
    };
    // `push_output(sym, value)`: raw arg 1 is the value itself.
    let raw = Observable::EmitArg {
        key,
        arg: 1,
        path: vec![PathStep::Field("x".into())],
    };
    assert_eq!(session.baseline().read(&field), Some(3.0));
    let r = run(&mut session, &mut env, Overrides::one(x, OverrideValue::Num(2.0)));
    assert_eq!(r.read(&field), Some(4.0));
    assert_eq!(r.read(&nested), Some(2.0));
    assert_eq!(r.read(&raw), Some(4.0));
}

#[test]
fn live_observation_mismatch_names_suppressed() {
    let mut env = Env::new();
    env.register_native_boxed(
        "host_value",
        |cxt| {
            cxt.push_int(42);
            Ok(1)
        },
        NativeEffects::EFFECT,
    );
    let pid = env.load_program(&with_natives(&["host_value"], "let s = symbol(\"o\")\npush_output(s, host_value())\n")).unwrap();
    let sid = env.create_stack(pid).unwrap();
    // Find the key from a session with no live comparison.
    let key = begin(&mut env, sid, &[], SpecConfig::default()).baseline().emits[0].key;
    let live = petal::dm_v2::LiveObservation {
        goals: vec![(Observable::EmitValue { key, path: vec![] }, 42.0)],
    };
    let err = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), Some(&live))
        .err()
        .expect("mismatch");
    match err {
        SpecError::BaselineMismatch { suppressed, .. } => assert_eq!(suppressed, vec!["host_value"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_override_term_is_a_failed_run() {
    let (mut env, pid, sid) = load("let s = symbol(\"o\")\nlet a = 1\nlet b = 2\npush_output(s, a + b)\n");
    let a = lit(&env, pid, 2, "1");
    let b = lit(&env, pid, 3, "2");
    let mut session = begin(&mut env, sid, &[a], SpecConfig::default());
    let r = session.run(&mut env, &mut NoHost, &Overrides::one(b, OverrideValue::Int(5)));
    assert!(matches!(r.error, Some(RunError::Runtime(_))));
    session.add_params(&mut env, &mut NoHost, &[b]).unwrap();
    let r = run(&mut session, &mut env, Overrides::one(b, OverrideValue::Int(5)).with(a, OverrideValue::Int(0)));
    assert_eq!(nums(&r), vec![5.0]);
}

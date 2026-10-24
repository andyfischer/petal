//! Direct manipulation v2: regressions found by the adversarial review of
//! the v2 change set (docs/dev/direct-manipulation-v2-plan.md §9, "Review").
//! Each test pins one bug that was fixed.

use petal::dm_v2::spec::{NoHost, SpecConfig, SpecSession};
use petal::dm_v2::{DmGoal, DmRequest, DmResponse, EmitKey, Observable, apply_edits_multi, program_files, propose_edits_v2};
use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::value::Value;

fn load(src: &str) -> (Env, ProgramId, StackKey) {
    let mut env = Env::new();
    env.set_seed(7);
    env.set_echo(false);
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, pid, sid)
}

fn keys(env: &mut Env, sid: StackKey) -> Vec<EmitKey> {
    let s = SpecSession::begin(env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
    let keys = s.baseline().emits.iter().map(|e| e.key).collect();
    s.end(env);
    keys
}

fn nums(env: &mut Env, channel: &str) -> Vec<f64> {
    let sym = env.intern_symbol(channel);
    env.take_output_buffer(sym)
        .iter()
        .map(|v| match v {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            other => panic!("not a number: {}", other.type_name()),
        })
        .collect()
}

/// Run `source` from scratch and return what it pushed to `out`.
fn fresh_out(source: &str) -> Vec<f64> {
    let (mut env, _, sid) = load(source);
    env.run(sid).expect("the edited program runs");
    nums(&mut env, "out")
}

fn propose(env: &mut Env, sid: StackKey, goals: Vec<DmGoal>) -> DmResponse {
    propose_edits_v2(env, &mut NoHost, sid, &DmRequest::new(goals)).expect("proposes")
}

fn applied(env: &Env, pid: ProgramId, alt: &petal::dm_v2::Alternative) -> String {
    let mut files = program_files(env.get_program(pid).unwrap());
    apply_edits_multi(&mut files, &alt.edits).expect("edits apply");
    files[0].1.clone()
}

/// A literal that receives a method call (`5.map_range(...)`) and goes
/// negative must be written `(-2).map_range(...)`: `-2.map_range(...)`
/// parses as `-(2.map_range(...))`, so the "verified" edit computed
/// something else once written.
#[test]
fn negative_method_receiver_is_parenthesized() {
    let src = "import io\nimport creative\nlet s = symbol(\"out\")\n\
               push_output(s, 5.map_range(rigid(0), rigid(10), rigid(100), rigid(200)))\n";
    let (mut env, pid, sid) = load(src);
    let k = keys(&mut env, sid);
    let r = propose(&mut env, sid, vec![DmGoal::to(Observable::EmitValue { key: k[0], path: vec![] }, 80.0)]);
    let alt = &r.alternatives[0];
    assert!(alt.verified, "{alt:?}");
    let text = applied(&env, pid, alt);
    assert_eq!(fresh_out(&text), vec![80.0], "the written edit reaches the goal:\n{text}");
}

/// A `--value` goal on `push_output(s, v)` is the same number as the
/// native's argument 1; that row is the goal, not collateral.
#[test]
fn value_goal_is_not_its_own_collateral() {
    let src = "import io\nlet s = symbol(\"out\")\nfn place(x)\n  x * 2 + 10\nend\npush_output(s, place(20))\n";
    let (mut env, _, sid) = load(src);
    let k = keys(&mut env, sid);
    let r = propose(&mut env, sid, vec![DmGoal::to(Observable::EmitValue { key: k[0], path: vec![] }, 80.0)]);
    for alt in &r.alternatives {
        assert!(alt.collateral.items.is_empty(), "{:?}", alt.collateral);
        assert_eq!(alt.collateral.shapes, 0.0, "{}", alt.description);
    }
}

/// Another emit moved by the same literal is still collateral.
#[test]
fn value_goal_keeps_other_collateral() {
    let src = "import io\nlet s = symbol(\"out\")\nlet c = 20\npush_output(s, c * 2)\npush_output(s, c + 1)\n";
    let (mut env, _, sid) = load(src);
    let k = keys(&mut env, sid);
    let r = propose(&mut env, sid, vec![DmGoal::to(Observable::EmitValue { key: k[0], path: vec![] }, 60.0)]);
    let alt = r
        .alternatives
        .iter()
        .find(|a| a.changes.iter().any(|c| c.old_text == "20"))
        .expect("an alternative moving `c`");
    assert_eq!(alt.collateral.items.len(), 1, "{:?}", alt.collateral);
}

/// Proposing slots literals into the live program; a hot reload of the
/// written edit into the same Env, on the same stack, must then run exactly
/// as the edited program does from scratch (statics, a shared interned
/// literal, a helper).
#[test]
fn reload_after_propose_matches_fresh_run() {
    let src = "import io\nlet s = symbol(\"out\")\nlet table = [10, 20, 30]\nlet gap = 10\n\
               fn at(i)\n  table[i] + gap * i\nend\n\
               for i in [0, 1, 2] do\n  push_output(s, at(i))\nend\npush_output(s, 10)\n";
    let (mut env, pid, sid) = load(src);
    env.run(sid).unwrap();
    let before = nums(&mut env, "out");
    assert_eq!(before, vec![10.0, 30.0, 50.0, 10.0]);
    env.reset_stack(sid).unwrap();
    let k = keys(&mut env, sid);
    let r = propose(&mut env, sid, vec![DmGoal::to(Observable::EmitValue { key: k[2], path: vec![] }, 70.0)]);
    let alt = r.alternatives.iter().find(|a| a.verified).expect("a verified alternative");
    let text = applied(&env, pid, alt);
    let want = fresh_out(&text);
    assert_eq!(want[2], 70.0);
    // Live, unchanged, before the reload.
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(nums(&mut env, "out"), before);
    env.reload_program(sid, &text, None).expect("reloads");
    env.reset_stack(sid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(nums(&mut env, "out"), want, "reloaded:\n{text}");
}

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

/// A drag preview writes the same text the finished edit would: a negative
/// value after `-` or before a method call is parenthesized there too.
#[test]
fn preview_parenthesizes_negative_values() {
    let src = "import io\nimport creative\nlet s = symbol(\"out\")\n\
               push_output(s, 5.map_range(rigid(0), rigid(10), rigid(100), rigid(200)))\n\
               push_output(s, rigid(50) - 3)\n";
    let (mut env, _, sid) = load(src);
    let k = keys(&mut env, sid);
    for (emit, target, old, want) in [(0, 80.0, "5", "(-2)"), (1, 60.0, "3", "(-10)")] {
        let goal = DmGoal::to(Observable::EmitValue { key: k[emit], path: vec![] }, target);
        let session = petal::dm_v2::DmSession::open(&mut env, &mut NoHost, sid, &DmRequest::new(vec![goal]))
            .expect("opens");
        let preview = session.preview(&[target]);
        let change = preview.iter().find(|c| c.old_text == old).expect("the literal moves");
        assert_eq!(change.new_text, want);
        session.cancel(&mut env);
    }
}

/// The default tolerance is relative to the target: a goal whose target is
/// known only at `finish` (a drag, `--by`) gets the same tolerance as one
/// given up front, not the tolerance of a target of 1.
#[test]
fn default_tolerance_follows_the_finish_target() {
    let src = "import io\nlet s = symbol(\"out\")\npush_output(s, rigid(102) * 1.5)\n";
    let (mut env, _, sid) = load(src);
    let k = keys(&mut env, sid);
    let obs = Observable::EmitValue { key: k[0], path: vec![] };
    let up_front = propose(&mut env, sid, vec![DmGoal::to(obs.clone(), 183.0)]);
    let goal = DmGoal::to(obs, f64::NAN);
    let session =
        petal::dm_v2::DmSession::open(&mut env, &mut NoHost, sid, &DmRequest::new(vec![goal])).expect("opens");
    let late = session.finish(&mut env, &mut NoHost, &[183.0]).expect("finishes");
    assert_eq!(late.diagnostics.tolerances, up_front.diagnostics.tolerances);
    assert_eq!(late.alternatives[0].changes[0].new_text, up_front.alternatives[0].changes[0].new_text);
    assert_eq!(late.alternatives[0].changes[0].new_text, "1.794118");
}

/// `propose-edit --v2 --arg k` where argument `k` is a name or a parameter
/// (`push_output(s, x)`): the argument has no instruction of its own, so
/// the watched value was never recorded and every such goal failed with
/// "the baseline run has no number".
#[test]
fn arg_goal_on_a_bare_name() {
    let src = "import io\nlet s = symbol(\"out\")\nlet x = 20\npush_output(s, x)\n\
               fn emit(v)\n  push_output(s, v)\nend\nfn go(a)\n  let y = a\n  emit(y)\nend\n\
               go(x + 1)\nfor i in [1, 2] do\n  emit(i * 3)\nend\n";
    let dir = std::env::temp_dir().join(format!("petal-dm-v2-review-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bare.ptl");
    std::fs::write(&path, src).unwrap();
    for (emit, to, want) in [("0", "30", "30"), ("1", "30", "10"), ("3", "12", "4")] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_petal"))
            .args(["propose-edit", "--v2", "--json", "--channel", "out", "--emit", emit, "--arg", "1", "--to", to])
            .arg(&path)
            .env_remove("PETAL_SEED")
            .output()
            .expect("runs petal");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "emit {emit}: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
        let top = &v["alternatives"][0];
        assert_eq!(top["verified"], true, "emit {emit}: {top}");
        let new: Vec<&str> =
            top["changes"].as_array().unwrap().iter().map(|c| c["new_text"].as_str().unwrap()).collect();
        assert!(new.contains(&want), "emit {emit}: {new:?}");
    }
}

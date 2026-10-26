//! Direct manipulation v2 for embedders: the JSON request
//! (`cli::dm_v2::opts_from_json`), `propose_json` and `propose_with_natives`,
//! which hosts with natives of their own (the C bridge's `pb_vm_propose_edit`,
//! petal-ui's `petal-ui-propose-edit`) drive instead of the `petal` CLI.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petal::cli::dm_v2::{GoalAddr, GoalValue, opts_from_json, propose_json, propose_with_natives};
use petal::dm_v2::NoHost;
use petal::dm_v2::PathStep;
use petal::env::Env;
use petal::native_fn::{NativeEffects, PetalCxt};
use petal::value::Value;
use serde_json::json;

const SCRIPT: &str = "\
import io
native fn scale(x)
native fn beep()
config let x = 10
beep()
push_output(symbol(\"shapes\"), scale(x))
";

fn native_scale(cxt: &mut PetalCxt) -> Result<u32, String> {
    let x = cxt.get_float(1)?;
    cxt.push_value(Value::Float(x * 2.0));
    Ok(1)
}

/// The host's natives: a pure `scale`, and an escaping `beep` that counts
/// its calls (a speculative run must never make one).
fn register(beeps: Arc<AtomicUsize>) -> impl FnMut(&mut Env) {
    move |env: &mut Env| {
        env.register_native("scale", native_scale, NativeEffects::PURE);
        let beeps = beeps.clone();
        env.register_native_boxed(
            "beep",
            move |cxt| {
                beeps.fetch_add(1, Ordering::SeqCst);
                cxt.push_value(Value::Nil);
                Ok(1)
            },
            NativeEffects::EFFECT,
        );
    }
}

#[test]
fn a_request_reads_like_the_flags() {
    let o = opts_from_json(&json!({
        "channel": "shapes", "emit": 2, "native_arg": 1, "field": ["center", 0], "by": -25,
        "allow": "config", "pins": ["x", "y"], "tol": 0.5, "pick": 2, "apply": true, "explain": true, "seed": 3
    }))
    .unwrap();
    assert_eq!(o.goals.len(), 1);
    let g = &o.goals[0];
    assert_eq!((g.channel.as_str(), g.emit), ("shapes", 2));
    assert_eq!(g.addr, GoalAddr::NativeArg(1));
    assert_eq!(g.path, vec![PathStep::Field("center".into()), PathStep::Index(0)]);
    assert_eq!(g.value, GoalValue::By(-25.0));
    assert_eq!(o.allow, vec!["config"]);
    assert_eq!(o.deny, vec!["x", "y"]);
    assert_eq!((o.tol, o.pick, o.apply, o.explain, o.seed), (Some(0.5), Some(2), true, true, Some(3)));
    assert!(o.json);

    // Several goals inherit the top-level address.
    let o = opts_from_json(&json!({
        "channel": "draw", "emit": 0,
        "goals": [{"native_arg": 1, "to": 40}, {"native_arg": 2, "to": "-3"}, {"emit": 1, "value": true, "to": 7.5}]
    }))
    .unwrap();
    assert_eq!(o.goals.len(), 3);
    assert_eq!(o.goals[1].value, GoalValue::To("-3".into()));
    assert_eq!((o.goals[2].emit, &o.goals[2].addr), (1, &GoalAddr::Value));

    // Range goals (C6): `at_most`, `at_least`, `within: [lo, hi]`.
    let o = opts_from_json(&json!({
        "channel": "coins", "emit": 0,
        "goals": [{"value": true, "field": "y", "at_most": 3}, {"native_arg": 1, "at_least": -2.5},
                  {"native_arg": 0, "within": [0, 1280]}, {"native_arg": 2, "within": "1..2"}]
    }))
    .unwrap();
    let values: Vec<&GoalValue> = o.goals.iter().map(|g| &g.value).collect();
    assert_eq!(
        values,
        [&GoalValue::AtMost(3.0), &GoalValue::AtLeast(-2.5), &GoalValue::Within(0.0, 1280.0), &GoalValue::Within(1.0, 2.0)]
    );

    // Channel weights: an object, or the flag's spelling.
    let o = opts_from_json(&json!({"channel": "coin", "emit": 0, "value": true, "by": 1, "collateral": {"solid": 10}}))
        .unwrap();
    assert_eq!(o.collateral, vec![("solid".to_string(), 10.0)]);
    let o = opts_from_json(&json!({"channel": "coin", "emit": 0, "value": true, "by": 1, "collateral": "solid=10,coin=0.5"}))
        .unwrap();
    assert_eq!(o.collateral, vec![("solid".to_string(), 10.0), ("coin".to_string(), 0.5)]);

    for bad in [
        json!({"channel": "a", "emit": 0, "native_arg": 0, "at_most": 1, "to": 2}),
        json!({"channel": "a", "emit": 0, "native_arg": 0, "at_most": "1"}),
        json!({"channel": "a", "emit": 0, "native_arg": 0, "within": [3, 1]}),
        json!({"channel": "a", "emit": 0, "native_arg": 0, "within": [1]}),
        json!({"channel": "a", "emit": 0, "value": true, "to": 1, "collateral": {"solid": "x"}}),
        json!({"channel": "a", "emit": 0, "value": true, "to": 1, "collateral": {"solid": -1}}),
        json!({"channel": "a", "emit": 0, "value": true, "to": 1, "collateral": [1]}),
        json!({"channel": "a", "emit": 0, "native_arg": 0, "to": 1, "alow": ["x"]}),
        json!({"channel": "a", "emit": 0, "to": 1}),
        json!({"channel": "a", "emit": 0, "arg": 0, "value": true, "to": 1}),
        json!({"channel": "a", "emit": 0, "arg": 0, "to": 1, "by": 2}),
        json!({"allow": ["x"]}),
        json!([1]),
    ] {
        assert!(opts_from_json(&bad).is_err(), "{bad} should be refused");
    }
}

#[test]
fn a_range_goal_runs_through_propose_json() {
    let beeps = Arc::new(AtomicUsize::new(0));
    // `scale(x)` is 20 at the baseline: at most 15 means x ≤ 7.5.
    let opts = opts_from_json(&json!({"channel": "shapes", "emit": 0, "value": true, "at_most": 15})).unwrap();
    let report =
        propose_with_natives(SCRIPT, None, &[], &opts, &mut NoHost, &mut register(beeps.clone())).expect("proposes");
    let best = &report["alternatives"][0];
    assert_eq!(best["verified"], json!(true), "{report:#}");
    assert!(best["achieved"][0].as_f64().unwrap() <= 15.0, "{report:#}");
    assert_eq!(report["goals"][0]["at_most"], json!(15.0));
}

#[test]
fn host_natives_run_and_escaping_ones_stay_silent() {
    let beeps = Arc::new(AtomicUsize::new(0));
    let opts = opts_from_json(&json!({"channel": "shapes", "emit": 0, "value": true, "to": 50})).unwrap();
    let report =
        propose_with_natives(SCRIPT, None, &[], &opts, &mut NoHost, &mut register(beeps.clone())).expect("proposes");
    let best = &report["alternatives"][0];
    assert_eq!(best["verified"], json!(true), "{report:#}");
    assert_eq!(best["changes"][0]["new"].as_f64(), Some(25.0), "{report:#}");
    assert_eq!(beeps.load(Ordering::SeqCst), 0, "an escaping native fired in a speculative run");
}

#[test]
fn propose_json_answers_errors_in_the_cli_shape() {
    let beeps = Arc::new(AtomicUsize::new(0));
    let mut env = Env::new();
    register(beeps.clone())(&mut env);
    let pid = env.load_program(SCRIPT).unwrap();
    let sid = env.create_stack(pid).unwrap();
    // The host's own frame fires `beep`; the drag that follows must not.
    env.run(sid).unwrap();
    assert_eq!(beeps.load(Ordering::SeqCst), 1);
    env.reset_stack(sid).unwrap();

    let ok = propose_json(&mut env, &mut NoHost, pid, sid, &json!({"channel": "shapes", "emit": 0, "value": true, "by": 10}), None);
    assert_eq!(ok["alternatives"][0]["changes"][0]["new"].as_f64(), Some(15.0), "{ok:#}");
    assert_eq!(beeps.load(Ordering::SeqCst), 1);

    let bad = propose_json(&mut env, &mut NoHost, pid, sid, &json!({"channel": "shapes"}), None);
    assert_eq!((bad["error"].clone(), bad["phase"].clone()), (json!(true), json!("request")), "{bad:#}");
    let missing = propose_json(&mut env, &mut NoHost, pid, sid, &json!({"channel": "nope", "emit": 0, "value": true, "to": 1}), None);
    assert_eq!(missing["phase"], json!("goal"), "{missing:#}");
    assert!(missing["message"].as_str().unwrap().contains("shapes"), "{missing:#}");
}

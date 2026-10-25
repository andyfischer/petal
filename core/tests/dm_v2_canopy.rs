//! Direct manipulation v2: regressions from the Canopy side-scroller study
//! (~/cheesecake/games/canopy/DM-DEBRIEF.md, attempts cited as #N). Each test
//! pins one solver failure the study hit.

use petal::dm_v2::spec::{NoHost, SpecConfig, SpecSession};
use petal::dm_v2::{DmGoal, DmRequest, DmResponse, EmitKey, Observable, PathStep, propose_edits_v2};
use petal::env::Env;
use petal::stack::StackKey;

fn load(src: &str) -> (Env, StackKey) {
    let mut env = Env::new();
    env.set_seed(7);
    env.set_echo(false);
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, sid)
}

fn keys(env: &mut Env, sid: StackKey) -> Vec<EmitKey> {
    let s = SpecSession::begin(env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
    let keys = s.baseline().emits.iter().map(|e| e.key).collect();
    s.end(env);
    keys
}

fn propose(env: &mut Env, sid: StackKey, req: &DmRequest) -> DmResponse {
    propose_edits_v2(env, &mut NoHost, sid, req).expect("proposes")
}

fn field(name: &str) -> PathStep {
    PathStep::Field(name.to_string())
}

/// S1 (#26, #42): a shared margin also places a list of records (the lives
/// icons) and a record inside a record. Moving it moves those, and the
/// collateral says so, naming the nested number; the alternative that moves
/// only the timer ranks first.
#[test]
fn collateral_reads_lists_of_records_and_nested_records() {
    let src = "import io\nlet MARGIN = 24\n\
               push_output(symbol(\"timer\"), {x: 600, y: MARGIN + 8})\n\
               push_output(symbol(\"lives\"), [{x: MARGIN, y: 10}, {x: MARGIN + 40, y: 10}, {x: MARGIN + 80, y: 10}])\n\
               push_output(symbol(\"coins\"), {panel: {x: MARGIN, y: MARGIN}})\n";
    let (mut env, sid) = load(src);
    let k = keys(&mut env, sid);
    let goal = DmGoal::to(Observable::EmitValue { key: k[0], path: vec![field("y")] }, 40.0);
    let r = propose(&mut env, sid, &DmRequest::new(vec![goal]));

    let margin_only = r
        .alternatives
        .iter()
        .find(|a| a.changes.len() == 1 && a.changes[0].binding.as_deref() == Some("MARGIN"))
        .expect("MARGIN alone is an alternative");
    assert!(margin_only.verified, "{margin_only:?}");
    let items = &margin_only.collateral.items;
    let lives = items.iter().find(|i| i.channel == "lives").unwrap_or_else(|| panic!("lives moved: {items:?}"));
    assert_eq!(lives.moved, 3, "every lives icon x: {lives:?}");
    assert_eq!(lives.path.last(), Some(&field("x")));
    assert!(lives.label().starts_with("lives[0]."), "{}", lives.label());
    let coins = items.iter().find(|i| i.channel == "coins").unwrap_or_else(|| panic!("coins moved: {items:?}"));
    assert_eq!(coins.path.first(), Some(&field("panel")), "{coins:?}");
    assert!(margin_only.description.contains("lives[0]."), "{}", margin_only.description);

    let top = &r.alternatives[0];
    assert!(top.verified);
    assert!(top.collateral.items.is_empty(), "the timer's own literal moves nothing else: {top:?}");
    assert_eq!(top.changes[0].old_text, "8", "{top:?}");
}

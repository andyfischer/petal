//! Direct manipulation v2 aimed at elements: `Observable::Element` names the
//! element an element literal built as its n-th (the occurrence element
//! tracing records), so a goal can name an element's prop however the
//! program hands the element over (here: as its result, with no emit at
//! all). Also `ObsValue::Element` (a path steps into an emitted element's
//! props) and the `EntryFile` selector.

use petal::dm::spec::{NoHost, SpecConfig, SpecSession};
use petal::dm::{
    DmError, DmGoal, DmRequest, ObsValue, Observable, ParamFilter, ParamSelector, PathStep, Policy,
    apply_edits_multi, program_files, propose_edits_v2,
};
use petal::env::Env;
use petal::program::{ProgramId, TermId};
use petal::source_map::FileId;
use petal::stack::StackKey;
use petal::value::Value;

fn load_at(src: &str, path: &std::path::Path) -> (Env, ProgramId, StackKey) {
    let mut env = Env::new();
    env.set_echo(false);
    env.enable_element_trace(true);
    let pid = env.load_program_at(src, path).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, pid, sid)
}

fn load(src: &str) -> (Env, ProgramId, StackKey) {
    load_at(src, std::path::Path::new("doc.ptl"))
}

/// Run `sid` and return, for each element of the list it evaluates to, its
/// origin's term and occurrence, and its `x` prop.
fn elements(env: &mut Env, sid: StackKey) -> Vec<(TermId, u32, f64)> {
    let v = env.run(sid).expect("runs");
    let Value::List(id) = v else { panic!("expected a list, got {v:?}") };
    let items = env.heap().get_list(id).to_vec();
    items
        .into_iter()
        .map(|v| {
            let Value::Element(e) = v else { panic!("not an element") };
            let o = env.element_origin(e).cloned().expect("traced");
            let props = env.heap().get_element_props(e);
            let x = match env.heap().get_map(props).iter().find(|(k, _)| k.as_str() == "x").map(|(_, v)| *v) {
                Some(Value::Int(n)) => n as f64,
                Some(Value::Float(f)) => f,
                other => panic!("x is {other:?}"),
            };
            (o.term, o.occurrence, x)
        })
        .collect()
}

fn x_of(term: TermId, occurrence: u32) -> Observable {
    Observable::Element { term, occurrence, path: vec![PathStep::Field("x".into())] }
}

#[test]
fn element_origins_number_each_sites_elements_in_run_order() {
    let src = "fn dot(i)\n  <dot x={i} />\nend\nlet a = for i in range(3) do\n  dot(i)\nend\n[a[0], a[1], a[2], <dot x={9} />, dot(7)]\n";
    let (mut env, _, sid) = load(src);
    let els = elements(&mut env, sid);
    let occ: Vec<u32> = els.iter().map(|e| e.1).collect();
    assert_eq!(occ, vec![0, 1, 2, 0, 3], "per site, in the order the run built them");
    assert_eq!(els[0].0, els[4].0);
    assert_ne!(els[0].0, els[3].0);
    // A stack reset starts the count again.
    env.reset_stack(sid).unwrap();
    let again = elements(&mut env, sid);
    assert_eq!(again.iter().map(|e| e.1).collect::<Vec<_>>(), occ);
}

#[test]
fn a_goal_on_an_element_a_loop_built_solves_with_no_emit() {
    let src = "let items = for i in range(3) do\n  <dot x={100 + i * 200} y={7} />\nend\nitems\n";
    let (mut env, pid, sid) = load(src);
    let els = elements(&mut env, sid);
    assert_eq!(els[1].2, 300.0);
    let (term, occ) = (els[1].0, els[1].1);
    env.reset_stack(sid).unwrap();
    // The baseline records the element and reads its prop.
    let cfg = SpecConfig {
        capture: petal::dm::CaptureSpec { elements: vec![term], ..Default::default() },
        ..Default::default()
    };
    let s = SpecSession::begin(&mut env, &mut NoHost, sid, &[], cfg, None).expect("session");
    assert_eq!(s.baseline().elements.len(), 3);
    assert_eq!(s.baseline().read(&x_of(term, occ)), Some(300.0));
    let rec = s.baseline().element(term, occ).unwrap();
    assert!(matches!(&rec.value, ObsValue::Element { props, .. } if props.len() == 2));
    s.end(&mut env);

    let req = DmRequest { policy: Policy::BestEffort, ..DmRequest::new(vec![DmGoal::to(x_of(term, occ), 450.0)]) };
    let resp = propose_edits_v2(&mut env, &mut NoHost, sid, &req).expect("proposes");
    let top = &resp.alternatives[0];
    assert!(top.verified, "{top:?}");
    let mut files = program_files(env.get_program(pid).unwrap());
    if files.is_empty() {
        files.push((FileId(0), src.to_string()));
    }
    apply_edits_multi(&mut files, &top.edits).unwrap();
    let (mut env2, _, sid2) = load(&files[0].1);
    let after = elements(&mut env2, sid2);
    assert!((after[1].2 - 450.0).abs() < 1e-6, "{}", files[0].1);
}

#[test]
fn an_element_in_an_emit_is_observed_through_its_props() {
    let src = "import io\npush_output(symbol(\"doc\"), [<a x={1} />, <b x={2.5} w={3} />])\n";
    let (mut env, _, sid) = load(src);
    let s = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
    let e = &s.baseline().emits[0];
    let at = |p: Vec<PathStep>| e.value.at(&p).and_then(ObsValue::as_f64);
    assert_eq!(at(vec![PathStep::Index(1), PathStep::Field("x".into())]), Some(2.5));
    assert_eq!(at(vec![PathStep::Index(0), PathStep::Field("x".into())]), Some(1.0));
    assert_eq!(at(vec![PathStep::Index(1), PathStep::Field("nope".into())]), None);
    let key = e.key;
    s.end(&mut env);
    // An emitted element's prop can be a goal.
    let goal = Observable::EmitValue { key, path: vec![PathStep::Index(1), PathStep::Field("w".into())] };
    let resp = propose_edits_v2(&mut env, &mut NoHost, sid, &DmRequest::new(vec![DmGoal::to(goal, 8.0)])).unwrap();
    assert!(resp.alternatives[0].verified);
    assert_eq!(resp.alternatives[0].changes[0].new_text, "8");
}

#[test]
fn the_entry_file_selector_keeps_edits_out_of_imported_modules() {
    assert_eq!(petal::propose_edit::parse_selector("entry-file"), Ok(ParamSelector::EntryFile));
    let dir = std::env::temp_dir().join(format!("petal-dm-elements-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    std::fs::write(dir.join("layout.ptl"), "pub fn spot()\n  20 + 2 * 30\nend\n").unwrap();
    let entry = dir.join("main.ptl");
    // Only the module's literals move `x`.
    let main = "import ./layout: spot\n[<dot x={spot()} />]\n";
    std::fs::write(&entry, main).unwrap();
    let (mut env, _, sid) = load_at(main, &entry);
    let els = elements(&mut env, sid);
    env.reset_stack(sid).unwrap();
    let goal = DmGoal::to(x_of(els[0].0, els[0].1), 95.0);
    let open = DmRequest { policy: Policy::BestEffort, ..DmRequest::new(vec![goal.clone()]) };
    let resp = propose_edits_v2(&mut env, &mut NoHost, sid, &open).expect("a module literal moves it");
    assert!(resp.alternatives[0].edits.iter().all(|e| e.file != FileId(0)));
    let entry_only = DmRequest {
        filter: ParamFilter { allow: Some(vec![ParamSelector::EntryFile]), deny: Vec::new() },
        ..open.clone()
    };
    match propose_edits_v2(&mut env, &mut NoHost, sid, &entry_only) {
        Err(DmError::NothingToMove { .. } | DmError::Unreachable { .. }) => {}
        other => panic!("expected no edit, got {other:?}"),
    }
    // With a literal of its own, the entry file is edited.
    let main = "import ./layout: spot\n[<dot x={spot() + 10} />]\n";
    std::fs::write(&entry, main).unwrap();
    let (mut env, _, sid) = load_at(main, &entry);
    let els = elements(&mut env, sid);
    env.reset_stack(sid).unwrap();
    let req = DmRequest {
        filter: ParamFilter { allow: Some(vec![ParamSelector::EntryFile]), deny: Vec::new() },
        policy: Policy::BestEffort,
        ..DmRequest::new(vec![DmGoal::to(x_of(els[0].0, els[0].1), 95.0)])
    };
    let resp = propose_edits_v2(&mut env, &mut NoHost, sid, &req).expect("the entry literal moves");
    assert!(resp.alternatives[0].edits.iter().all(|e| e.file == FileId(0)));
    assert_eq!(resp.alternatives[0].changes[0].new_text, "15");
    let _ = std::fs::remove_dir_all(&dir);
}

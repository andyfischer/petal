//! Direct manipulation v2, M2 acceptance item 11 (petal-ui half,
//! docs/dev/direct-manipulation-v2-plan.md §5): a goal reached through the
//! bundled `ui` prelude's `draw_circle` overloads and the `std` prelude never
//! offers a literal of either as an edit, even when the client asks to screen
//! library code for the report and allows all user code.

use petal::dm_v2::jacobian::{ColumnQuality, FdOptions, jacobian};
use petal::dm_v2::observe::hash_str;
use petal::dm_v2::params::{
    CodeOrigin, EligibilityContext, ParamFilter, ParamSelector, RunBudget, anchors_for, discover_params,
    eligibility,
};
use petal::dm_v2::spec::{ObsValue, Overrides, SpecConfig, SpecSession};
use petal::dm_v2::{Observable, read_hints};
use petal::program::TermId;
use petal_ui::harness::Headless;

#[test]
fn library_literals_never_proposed() {
    // `draw_circle(center, radius, color)` is the overload the prelude
    // unpacks into the positional native; `clamp01` is the `std` prelude's.
    let src = "let cx = 120.5\nlet k = 0.25\ndraw_circle({x: cx + clamp01(k) * 40.0, y: 60}, 10, {r: 255, g: 0, b: 0})";
    let mut ui = Headless::with_ui(src).unwrap();
    ui.frame().unwrap();
    let (pid, sid) = (ui.program_id(), ui.stack_id());
    ui.env.reset_stack(sid).unwrap();
    let (env, mut host) = ui.spec_parts();
    let mut session = SpecSession::begin(env, &mut host, sid, &[], SpecConfig::default(), None).unwrap();
    let circle = hash_str("circle");
    let key = session
        .baseline()
        .emits
        .iter()
        .find(|e| matches!(&e.value, ObsValue::Variant { tag, .. } if *tag == circle))
        .expect("a circle")
        .key;
    let goal = Observable::EmitArg { key, arg: 0, path: vec![] };
    assert_eq!(session.baseline().read(&goal), Some(130.5));

    let params = {
        let program = env.get_program(pid).unwrap();
        let hints = read_hints(program, env.native_table());
        let anchors = anchors_for(program, session.baseline(), std::slice::from_ref(&goal));
        discover_params(program, env.native_table(), &hints, &anchors, &session.baseline().call_edges).params
    };
    let library: Vec<_> = params.iter().filter(|p| p.origin == CodeOrigin::Library).collect();
    assert!(!library.is_empty(), "the goal goes through library code with literals in it");
    let filter = ParamFilter {
        allow: Some(vec![ParamSelector::ReportLibrary, ParamSelector::AllUserCode]),
        deny: Vec::new(),
    };
    let program = env.get_program(pid).unwrap();
    for p in &params {
        let e = eligibility(program, p, &filter, EligibilityContext::default());
        if p.origin == CodeOrigin::Library {
            assert!(e.edit.is_err(), "a library literal is never an edit: {p:?}");
        }
    }
    let user: Vec<usize> = (0..params.len())
        .filter(|&i| eligibility(program, &params[i], &filter, EligibilityContext::default()).edit.is_ok())
        .collect();
    let values: Vec<f64> = user.iter().map(|&i| params[i].value).collect();
    for v in [120.5, 0.25, 40.0] {
        assert!(values.contains(&v), "{v} is an editable user literal: {values:?}");
    }
    assert!(user.iter().all(|&i| params[i].origin == CodeOrigin::Entry));

    // And the columns of the user literals are what the arithmetic says.
    let terms: Vec<TermId> = params.iter().map(|p| p.term).collect();
    session.add_params(env, &mut host, &terms).unwrap();
    let keep: Vec<usize> = user
        .iter()
        .copied()
        .filter(|&i| [120.5, 0.25, 40.0].contains(&params[i].value))
        .collect();
    let j = jacobian(
        &mut session,
        env,
        &mut host,
        &params,
        &keep,
        vec![goal],
        1,
        &Overrides::none(),
        &FdOptions::default(),
        &mut RunBudget::runs(100),
    );
    for (c, &i) in keep.iter().enumerate() {
        let want = match params[i].value {
            v if v == 120.5 => 1.0,
            v if v == 0.25 => 40.0,
            _ => 0.25,
        };
        assert!((j.get(0, c) - want).abs() < 1e-6, "{}: {} vs {want}", params[i].value, j.get(0, c));
        assert_eq!(j.quality[c], ColumnQuality::Smooth);
    }
}

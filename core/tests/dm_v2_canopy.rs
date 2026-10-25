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

/// A relative layout like Canopy's level (each piece placed by the gap after
/// the one before), with knobs that only decorations read, in the same
/// records the pieces are.
fn relative_layout() -> String {
    let mut src = String::from(
        "import io\n\
         config let DECO = {a: 3.25, b: 4.5, c: 5.75, d: 6.125, e: 7.375, f: 8.625, g: 9.875, h: 1.0625}\n\
         fn piece(gap, w, x0, x1)\n  {gap: gap, w: w, x0: x0, x1: x1, deco: DECO}\nend\n\
         let pieces = [\n",
    );
    for i in 0..24 {
        let f = i as f64;
        let w = 8.0 + f * 0.25;
        src.push_str(&format!("  piece({}, {}, {}, {}),\n", 1.0 + f * 0.125, w, 1.0 + f * 0.0625, w - 1.5 - f * 0.03125));
    }
    src.push_str(
        "]\n\
         fn world()\n\
         \x20 let x = 0.0\n\
         \x20 let walkers = []\n\
         \x20 let decos = []\n\
         \x20 for p in pieces do\n\
         \x20   x = x + p.gap\n\
         \x20   walkers = append(walkers, {x0: x + p.x0, x1: x + p.x1, right: x + p.w})\n\
         \x20   decos = append(decos, {x: x + p.deco.a * p.deco.b, y: p.deco.c + p.deco.d + p.deco.e + p.deco.f + p.deco.g + p.deco.h})\n\
         \x20   x = x + p.w\n\
         \x20 end\n\
         \x20 {walkers: walkers, decos: decos}\n\
         end\n\
         let w = world()\n\
         for b in w.walkers do\n  push_output(symbol(\"walker\"), {x0: b.x0, to_edge: b.right - b.x1})\nend\n\
         for d in w.decos do\n  push_output(symbol(\"deco\"), d)\nend\n",
    );
    src
}

/// S2 (#7, #12, #36): "walker 20 turns one tile earlier, its start where it
/// is". Every earlier gap and width moves the start (the held goal), and the
/// knobs reach the goal's record statically; before, screening ran out of
/// budget, the knobs were measured first (all zero) and then source order,
/// and the walker's own `x1` was never measured: no alternative. Screening
/// now resolves what moves the goal that asks for a change first, and the
/// knobs, which the goal's fields do not read, wait.
#[test]
fn screening_finds_the_goals_own_literal_in_a_relative_layout() {
    let src = relative_layout();
    for holds in [vec![false, true], Vec::new()] {
        let (mut env, sid) = load(&src);
        let k = keys(&mut env, sid);
        let to_edge = Observable::EmitValue { key: k[20], path: vec![field("to_edge")] };
        let x0 = Observable::EmitValue { key: k[20], path: vec![field("x0")] };
        let x1 = 8.0 + 20.0 * 0.25 - 1.5 - 20.0 * 0.03125;
        let x0_base = {
            let s = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None).unwrap();
            let v = s.baseline().read(&x0).unwrap();
            s.end(&mut env);
            v
        };
        // Without `holds`, a target that is the baseline value holds.
        let mut req = DmRequest::new(vec![DmGoal::to(to_edge, 2.125 + 1.0), DmGoal::to(x0, x0_base)]);
        req.holds = holds.clone();
        let r = propose(&mut env, sid, &req);
        let top = r.alternatives.first().unwrap_or_else(|| panic!("an alternative ({holds:?}): {:?}", r.diagnostics));
        assert!(top.verified, "{top:?}");
        assert_eq!(top.changes.len(), 1, "{}", top.description);
        assert_eq!(top.changes[0].old, x1, "the walker's own x1: {}", top.description);
        assert_eq!(top.changes[0].new, x1 - 1.0);
        // The knobs are read only by the decorations.
        let deco = r.candidates.iter().filter(|c| c.param.is_knob()).collect::<Vec<_>>();
        assert_eq!(deco.len(), 8);
        assert!(deco.iter().all(|c| c.param.focus.is_none()), "{:?}", deco.iter().map(|c| c.param.focus).collect::<Vec<_>>());
    }
}

/// The focused reach follows the field a goal reads into a record, and an
/// element into a list, and leaves the rest of the value out (S2).
#[test]
fn focused_reach_follows_fields_and_elements() {
    let src = "import io\nlet rec = {a: 1.5, b: 2.5}\n\
               let xs = [{x: 1.25, y: 9.5}, {x: 2.25, y: 8.5}]\n\
               push_output(symbol(\"one\"), {x: rec.a * 2.0, y: rec.b})\n\
               for p in xs do\n  push_output(symbol(\"row\"), {x: p.x + 0.5, y: p.y + 7.5})\nend\n";
    let (mut env, sid) = load(src);
    let k = keys(&mut env, sid);
    let focus_of = |env: &mut Env, goal: Observable| -> Vec<(f64, bool)> {
        let session =
            petal::dm_v2::DmSession::open(env, &mut NoHost, sid, &DmRequest::new(vec![DmGoal::to(goal, 100.0)])).unwrap();
        let mut out: Vec<(f64, bool)> = session.candidates().iter().map(|c| (c.param.value, c.param.focus.is_some())).collect();
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        session.finish(env, &mut NoHost, &[100.0]).unwrap();
        out
    };
    let one = focus_of(&mut env, Observable::EmitValue { key: k[0], path: vec![field("x")] });
    assert_eq!(one, vec![(1.5, true), (2.0, true), (2.5, false)]);
    let row = focus_of(&mut env, Observable::EmitValue { key: k[2], path: vec![field("x")] });
    assert_eq!(row, vec![(0.5, true), (1.25, true), (2.25, true), (7.5, false), (8.5, false), (9.5, false)]);
}

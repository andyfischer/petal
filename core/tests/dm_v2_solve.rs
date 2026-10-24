//! Direct manipulation v2, milestone M3: the solver, ranking, client
//! disambiguation and the public API (`petal::dm_v2::{solve, rank, api}`).
//! The acceptance list is in docs/dev/direct-manipulation-v2-plan.md §5
//! (M3); each test names the item it covers. The tests after the numbered
//! ones cover the rest of the M3 task: nested calls, trig, the RNG path, the
//! hints, unreachable goals, v1 parity and the drag session.
//!
//! As in the M2 tests, core has no draw natives, so `draw(x, y, r)` is
//! registered here: it emits `[x, y, r]` truncated to ints (as petal-ui's
//! draw natives do), while the raw arguments a goal reads are not. Every
//! accepted alternative is also checked end to end: its edits are applied to
//! the source text, the edited program is compiled and run from scratch, and
//! the goal must hold there.

use petal::direct_manipulation::{self as v1, ManipulationGoal, VarPolicy};
use petal::dm_v2::params::{ParamFilter, ParamSelector};
use petal::dm_v2::spec::{NoHost, SpecConfig, SpecSession};
use petal::dm_v2::{
    Alternative, DmError, DmGoal, DmRequest, DmResponse, DmSession, EmitKey, Engine, Observable, Prefer,
    apply_edits_multi, program_files, propose_edits_v2,
};
use petal::env::Env;
use petal::native_fn::{InputClasses, NativeEffects, PetalCxt};
use petal::program::ProgramId;
use petal::source_map::FileId;
use petal::stack::StackKey;
use petal::static_value::StaticValue;
use petal::value::Value;

// ── harness ──────────────────────────────────────────────────────────────

fn native_draw(cxt: &mut PetalCxt) -> Result<u32, String> {
    let x = cxt.get_int(1)?;
    let y = cxt.get_int(2)?;
    let r = cxt.get_int(3)?;
    let sym = cxt.intern_symbol("draw");
    let list = cxt.heap_mut().alloc_list(vec![Value::Int(x), Value::Int(y), Value::Int(r)]);
    cxt.push_output(sym, Value::List(list));
    cxt.push_value(Value::Nil);
    Ok(1)
}

/// `label(text)`: emits its argument on channel `text`.
fn native_label(cxt: &mut PetalCxt) -> Result<u32, String> {
    let v = cxt.get_value(1)?;
    let sym = cxt.intern_symbol("text");
    cxt.push_output(sym, v);
    cxt.push_value(Value::Nil);
    Ok(1)
}

/// `time()`: the `time` binding, as petal-ui's clock natives read it.
fn native_time(cxt: &mut PetalCxt) -> Result<u32, String> {
    let v = cxt.binding_named("time");
    cxt.push_value(v);
    Ok(1)
}

/// On the first line, so the program's own lines keep their numbers.
const PREFIX: &str =
    "import io; import creative; import noise; native fn draw(x, y, r); native fn label(t); native fn time(); ";

fn env() -> Env {
    let mut env = Env::new();
    env.set_seed(7);
    env.register_native("draw", native_draw, NativeEffects::EMITS);
    env.register_native("label", native_label, NativeEffects::EMITS);
    env.register_native("time", native_time, NativeEffects::probe(InputClasses::CLOCK));
    env
}

struct T {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
    keys: Vec<EmitKey>,
}

impl T {
    fn new(src: &str) -> T {
        let mut env = env();
        let pid = env.load_program(&format!("{PREFIX}{src}")).expect("compiles");
        let sid = env.create_stack(pid).expect("stack");
        T::with(env, pid, sid)
    }

    fn with(mut env: Env, pid: ProgramId, sid: StackKey) -> T {
        let s = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
        let keys = s.baseline().emits.iter().map(|e| e.key).collect();
        s.end(&mut env);
        T { env, pid, sid, keys }
    }

    /// Raw argument `arg` of emit `emit`.
    fn arg(&self, emit: usize, arg: usize) -> Observable {
        Observable::EmitArg { key: self.keys[emit], arg, path: Vec::new() }
    }

    fn propose(&mut self, req: &DmRequest) -> DmResponse {
        propose_edits_v2(&mut self.env, &mut NoHost, self.sid, req).expect("proposes")
    }

    fn try_propose(&mut self, req: &DmRequest) -> Result<DmResponse, DmError> {
        propose_edits_v2(&mut self.env, &mut NoHost, self.sid, req)
    }

    /// The program's files with `alt` applied.
    fn applied(&self, alt: &Alternative) -> Vec<(FileId, String)> {
        let mut files = program_files(self.env.get_program(self.pid).unwrap());
        apply_edits_multi(&mut files, &alt.edits).expect("edits apply");
        files
    }

    /// Compile the entry file with `alt` applied, run it from scratch, and
    /// return every emit's raw numeric arguments.
    fn rerun(&self, alt: &Alternative) -> Vec<Vec<f64>> {
        let files = self.applied(alt);
        rerun_source(&files[0].1)
    }
}

fn rerun_source(source: &str) -> Vec<Vec<f64>> {
    let mut env = env();
    let pid = env.load_program(source).expect("the edited program compiles");
    let sid = env.create_stack(pid).unwrap();
    raw_args(&mut env, sid)
}

fn raw_args(env: &mut Env, sid: StackKey) -> Vec<Vec<f64>> {
    let s = SpecSession::begin(env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
    let out = s
        .baseline()
        .emits
        .iter()
        .map(|e| e.native_args.iter().filter_map(|a| a.as_f64()).collect())
        .collect();
    s.end(env);
    out
}

/// The edit texts of an alternative, as `old→new`.
fn edits(alt: &Alternative) -> Vec<String> {
    alt.changes.iter().map(|c| format!("{}→{}", c.old_text, c.new_text)).collect()
}

fn all_edits(r: &DmResponse) -> Vec<Vec<String>> {
    r.alternatives.iter().map(edits).collect()
}

fn req(goals: Vec<DmGoal>) -> DmRequest {
    DmRequest::new(goals)
}

fn allow(sel: Vec<ParamSelector>) -> ParamFilter {
    ParamFilter { allow: Some(sel), deny: Vec::new() }
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

// ── 1. through_helper_single_goal ────────────────────────────────────────

const HELPER: &str = "fn place(i)\n  20 + i * 30\nend\ndraw(place(1.0), 100, 10)\ndraw(place(2.0), 100, 10)\n";

#[test]
fn through_helper_single_goal() {
    let mut t = T::new(HELPER);
    let mut r = req(vec![DmGoal::to(t.arg(1, 0), 95.0)]);
    r.allow_respell = true;
    let resp = t.propose(&r);
    let alts = &resp.alternatives;
    assert_eq!(
        all_edits(&resp),
        vec![vec!["2.0→2.5"], vec!["30→37.5"], vec!["20→35"]],
        "{:#?}",
        alts.iter().map(|a| (&a.description, a.score)).collect::<Vec<_>>()
    );
    assert!(alts.iter().all(|a| a.verified));
    assert!(alts[0].collateral.items.is_empty(), "the call argument moves nothing else");
    for a in &alts[1..] {
        assert_eq!(a.collateral.items.len(), 1, "the helper body also moves the first circle");
        assert_eq!(a.collateral.items[0].line, Some(4));
        assert_eq!(a.collateral.items[0].callee.as_deref(), Some("draw"));
    }
    assert!(alts[1].changes[0].respelled, "30 → 37.5 is an int written as a float");
    assert_eq!(alts[1].changes[0].function.as_deref(), Some("place"));
    // The ranking, pinned: a weight change shows up here.
    let scores: Vec<f64> = alts.iter().map(|a| (a.score * 1000.0).round() / 1000.0).collect();
    // §2.5.4 as revised in §9 M5: each does the whole drag alone (share 1),
    // so the weights (0.75 in the goal's frame, 1.5 in a function body) and
    // the measured collateral decide.
    assert_eq!(scores, vec![0.769, 2.338, 2.612]);
    for a in alts {
        assert!(near(t.rerun(a)[1][0], 95.0, 0.5), "{}", a.description);
    }

    // With the first circle gone, nothing else moves.
    // Nothing else moves now, so the weights and the respelling decide
    // (§2.5.4 as revised in §9 M5): `20 → 35` keeps its spelling, `30 →
    // 37.5` pays 0.3 for it, and the minimum-norm spread over the helper
    // (`2.0` and `30` together) pays 0.5 for its second edit and 0.3 for
    // the respelling.
    let mut t = T::new("fn place(i)\n  20 + i * 30\nend\ndraw(place(2.0), 100, 10)\n");
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 95.0)]);
    r.allow_respell = true;
    r.max_alternatives = 4;
    let resp = t.propose(&r);
    assert_eq!(
        all_edits(&resp),
        vec![vec!["2.0→2.5"], vec!["20→35"], vec!["2.0→2.4", "30→31.4"], vec!["30→37.5"]]
    );
    assert!(resp.alternatives.iter().all(|a| a.verified && a.collateral.items.is_empty()));
    let scores: Vec<f64> = resp.alternatives.iter().map(|a| (a.score * 1000.0).round() / 1000.0).collect();
    assert_eq!(scores, vec![0.769, 1.612, 1.702, 1.838]);
}

// ── 2. config_preferred_softly ───────────────────────────────────────────

#[test]
fn config_preferred_softly() {
    let mut t = T::new("config let offset = 10\ndraw(offset + 50, 100, 10)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 75.0)]));
    let e = all_edits(&resp);
    assert_eq!(e[0], vec!["10→25"], "{e:?}");
    assert!(resp.alternatives[0].config_only);
    assert!(e.contains(&vec!["50→65".to_string()]), "the call-site literal is still listed: {e:?}");
    // The weight agrees with the tier: the config literal (0.5) costs less
    // than the call-site literal (0.75) for the same share of the drag
    // (§9 M5; under M3's relative edit cost the score alone put `50` first
    // and only the tier kept the config literal on top).
    let lit = resp.alternatives.iter().find(|a| edits(a) == vec!["50→65"]).unwrap();
    assert!(lit.score > resp.alternatives[0].score);

    // v1's rule on request: config only.
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 75.0)]);
    r.prefer = Prefer::ConfigStrict;
    let resp = t.propose(&r);
    assert_eq!(all_edits(&resp), vec![vec!["10→25"]]);
}

/// A literal inside a `config let` record or list is config data: it gets
/// the config weight, matches the `config` selector, and ConfigStrict keeps
/// it (docs/proposals/config-let-vs-tunable.md, bug 1).
#[test]
fn config_record_and_list_literals_are_config() {
    let mut t = T::new("config let BOX = {x: 140, y: 90}\nconfig let XS = [20, 30]\ndraw(BOX.x + 10, XS[1], 10)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 160.0)]));
    let e = all_edits(&resp);
    assert_eq!(e[0], vec!["140→150"], "{e:?}");
    assert!(resp.alternatives[0].config_only);
    assert!(e.contains(&vec!["10→20".to_string()]), "the call-site literal is still listed: {e:?}");
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 160.0)]);
    r.filter = allow(vec![ParamSelector::ConfigOnly]);
    assert_eq!(all_edits(&t.propose(&r)), vec![vec!["140→150"]]);
    let mut r = req(vec![DmGoal::to(t.arg(0, 1), 45.0)]);
    r.filter = allow(vec![ParamSelector::ConfigOnly]);
    assert_eq!(all_edits(&t.propose(&r)), vec![vec!["30→45"]]);
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 160.0)]);
    r.prefer = Prefer::ConfigStrict;
    assert_eq!(all_edits(&t.propose(&r)), vec![vec!["140→150"]]);
}

/// ConfigStrict in a program whose only knobs are `tunable`s: the other
/// named bindings are pinned, as they are under a `config let` (bug 2).
#[test]
fn config_strict_counts_tunable_knobs() {
    let mut t = T::new("let a = tunable(10)\nlet b = 50\ndraw(a + b, 100, 10)\n");
    let soft = all_edits(&t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 75.0)])));
    assert_eq!(soft[0], vec!["10→25"], "{soft:?}");
    assert!(soft.contains(&vec!["50→65".to_string()]), "{soft:?}");
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 75.0)]);
    r.prefer = Prefer::ConfigStrict;
    assert_eq!(all_edits(&t.propose(&r)), vec![vec!["10→25"]]);
}

// ── 3. two_goals_one_gesture ─────────────────────────────────────────────

#[test]
fn two_goals_one_gesture() {
    let src = "let cx = 100.0\nlet cy = 80.0\nlet angle = 0.5\nlet r = 40.0\n\
               fn polar(cx, cy, angle, r)\n  {x: cx + cos(angle) * r, y: cy + sin(angle) * r}\nend\n\
               let p = polar(cx, cy, angle, r)\ndraw(p.x, p.y, 5)\n";
    let mut t = T::new(src);
    let (x0, y0) = (100.0 + 0.5f64.cos() * 40.0, 80.0 + 0.5f64.sin() * 40.0);
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), x0 + 10.0), DmGoal::to(t.arg(0, 1), y0 - 8.0)]);
    r.max_alternatives = 8;
    let resp = t.propose(&r);
    let top = &resp.alternatives[0];
    assert!(top.verified);
    let mut names: Vec<&str> = top.changes.iter().map(|c| c.binding.as_deref().unwrap()).collect();
    names.sort();
    assert_eq!(names, vec!["cx", "cy"], "{:?}", all_edits(&resp));
    assert_eq!(top.collateral.norm, 0.0);
    let polar = resp
        .alternatives
        .iter()
        .find(|a| {
            let mut n: Vec<&str> = a.changes.iter().map(|c| c.binding.as_deref().unwrap()).collect();
            n.sort();
            n == vec!["angle", "r"]
        })
        .unwrap_or_else(|| panic!("the angle-and-radius alternative is listed: {:#?}", resp.alternatives.iter().map(|a| (&a.description, a.score, a.verified)).collect::<Vec<_>>()));
    assert!(polar.verified && polar.score > top.score);
    let after = t.rerun(polar);
    assert!(near(after[0][0], x0 + 10.0, 0.5) && near(after[0][1], y0 - 8.0, 0.5));
}

// ── 4. shared_literal_penalized_by_measured_collateral ───────────────────

const SHARED: &str = "let gap = 10\ndraw(gap + 0, 50, 5)\ndraw(gap + 30, 50, 5)\ndraw(gap + 60, 50, 5)\n\
                      draw(gap + 90, 50, 5)\ndraw(gap + 120, 50, 5)\n";

#[test]
fn shared_literal_penalized_by_measured_collateral() {
    let mut t = T::new(SHARED);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(2, 0), 76.0)]));
    assert_eq!(edits(&resp.alternatives[0]), vec!["60→66"]);
    let gap = resp.alternatives.iter().find(|a| edits(a) == vec!["10→16"]).expect("gap is listed");
    assert_eq!(gap.collateral.items.len(), 4, "the other four shapes move");
    assert!(gap.description.contains("4 other emits"), "{}", gap.description);
    let lines: Vec<u32> = gap.collateral.items.iter().filter_map(|i| i.line).collect();
    assert_eq!(lines, vec![2, 3, 5, 6]);
}

// ── 5. allowed_set_exact ─────────────────────────────────────────────────

#[test]
fn allowed_set_exact() {
    let mut t = T::new(SHARED);
    let mut r = req(vec![DmGoal::to(t.arg(2, 0), 76.0)]);
    r.filter = allow(vec![ParamSelector::Variable("gap".into())]);
    let resp = t.propose(&r);
    assert!(!resp.alternatives.is_empty());
    for a in &resp.alternatives {
        assert!(a.changes.iter().all(|c| c.binding.as_deref() == Some("gap")), "{}", a.description);
        assert!(a.verified);
    }
    let reported: Vec<_> = resp.candidates.iter().filter(|c| c.eligible.is_ok()).collect();
    assert_eq!(reported.len(), 1, "the report says only `gap` may move");
}

// ── 6. range_clamp ───────────────────────────────────────────────────────

#[test]
fn range_clamp() {
    let mut t = T::new("let r = tunable(40, 10, 80)\ndraw(100, 100, r)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 2), 120.0)]));
    let a = &resp.alternatives[0];
    assert!(!a.verified);
    assert_eq!(edits(a), vec!["40→80"]);
    assert_eq!(a.achieved, vec![80.0]);
    assert_eq!(a.max_error, 80.0, "40 short with τ = 0.5");
    assert!(a.description.contains("misses the goal by 40"), "{}", a.description);
    // The bounds themselves are never offered.
    for c in &resp.candidates {
        if c.param.value == 10.0 || c.param.value == 80.0 {
            assert!(c.eligible.is_err());
        }
    }
}

// ── 7. rounding ──────────────────────────────────────────────────────────

#[test]
fn rounding() {
    let mut t = T::new("draw(40 + 7, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 60.0)]));
    let e = all_edits(&resp);
    assert!(e.contains(&vec!["40→53".into()]) && e.contains(&vec!["7→20".into()]), "{e:?}");
    assert!(resp.alternatives.iter().all(|a| a.changes.iter().all(|c| !c.respelled)));

    let mut t = T::new("draw(3.50 * 10.0, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 42.0)]));
    let e = all_edits(&resp);
    assert!(e.contains(&vec!["3.50→4.20".into()]), "keeps two decimals: {e:?}");
    assert!(e.contains(&vec!["10.0→12.0".into()]), "{e:?}");
    // A tolerance that does not round: the fewest decimals that still fit.
    let mut t = T::new("let k = 1.0\npush_output(symbol(\"out\"), k * 3.0)\n");
    let o = Observable::EmitValue { key: t.keys[0], path: Vec::new() };
    let resp = t.propose(&req(vec![DmGoal::to(o, 4.0).within(2e-3)]));
    let k = resp.alternatives.iter().find(|a| a.changes[0].binding.as_deref() == Some("k")).unwrap();
    assert_eq!(edits(k), vec!["1.0→1.333"]);
    assert!(k.verified);
}

// ── 8. loop_shape_any_iteration ──────────────────────────────────────────

#[test]
fn loop_shape_any_iteration() {
    let mut t = T::new("for i in range(5) do\n  draw(20 + i * 40, 50, 5)\nend\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(2, 0), 110.0)]));
    let e = all_edits(&resp);
    assert!(e.contains(&vec!["20→30".into()]) && e.contains(&vec!["40→45".into()]), "{e:?}");
    for a in &resp.alternatives {
        assert!(a.verified);
        assert!(near(t.rerun(a)[2][0], 110.0, 0.5));
    }
    assert!(resp.candidates.iter().any(|c| c.param.value == 5.0 && c.eligible.is_err()), "the trip count is structural");
}

// ── 9. nonlinear_converges ───────────────────────────────────────────────

const TREE: &str = "let spread = 0.4\nlet shrink = 0.7\n\
fn branch(x, y, angle, len, depth)\n\
  if depth > 0 then\n\
    let nx = x + cos(angle) * len\n\
    let ny = y - sin(angle) * len\n\
    draw(nx, ny, 2)\n\
    branch(nx, ny, angle + spread, len * shrink, depth - 1)\n\
  end\n\
end\n\
branch(200.0, 300.0, 1.2, 80.0, 4)\n";

#[test]
fn nonlinear_converges() {
    let mut t = T::new(TREE);
    let base = raw_args(&mut t.env, t.sid);
    assert_eq!(base.len(), 4);
    let tip = base[3][0];
    for var in ["spread", "shrink"] {
        let mut r = req(vec![DmGoal::to(t.arg(3, 0), tip - 25.0)]);
        r.filter = allow(vec![ParamSelector::Variable(var.into())]);
        let resp = t.propose(&r);
        let a = &resp.alternatives[0];
        assert!(a.verified, "{var}: {}", a.description);
        assert!(a.iterations <= 6, "{var}: {} iterations", a.iterations);
        assert!(near(t.rerun(a)[3][0], tip - 25.0, 0.5), "{var}");
    }
}

// ── 10. clock_coupling_penalized ─────────────────────────────────────────

#[test]
fn clock_coupling_penalized() {
    let src = "let mark_x = 40.0\nlet orb = time() * 1.1\nlet rad = 11.0\n\
               draw(mark_x + cos(orb) * rad, 38.0 + sin(orb) * rad, 3)\n";
    let mut env = env();
    let pid = env.load_program(&format!("{PREFIX}{src}")).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let time = env.intern_symbol("time");
    env.set_binding_for(sid, time, Value::Float(1.0));
    let mut t = T::with(env, pid, sid);
    let x0 = 40.0 + 1.1f64.cos() * 11.0;
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), x0 + 5.0)]);
    r.max_alternatives = 6;
    r.explain = true;
    let resp = t.propose(&r);
    let pos = |text: &str| {
        resp.alternatives
            .iter()
            .position(|a| a.changes.len() == 1 && a.changes[0].old_text == text)
            .unwrap_or_else(|| panic!("`{text}` is listed: {:?}", all_edits(&resp)))
    };
    let rate = resp.candidates.iter().find(|c| c.param.value == 1.1).unwrap();
    assert!(rate.clock_coupled, "{:#?}", resp.explain);
    let radius = resp.candidates.iter().find(|c| c.param.value == 11.0).unwrap();
    assert!(!radius.clock_coupled);
    assert!(!resp.candidates.iter().find(|c| c.param.value == 40.0).unwrap().clock_coupled);
    assert_eq!(pos("40.0"), 0);
    assert!(pos("1.1") > pos("11.0"), "{:?}", all_edits(&resp));
}

// ── 11. string_goal_falls_back_to_v1 ─────────────────────────────────────

#[test]
fn string_goal_falls_back_to_v1() {
    let mut t = T::new("label(\"hello\")\n");
    let resp = t.propose(&req(vec![DmGoal::value(t.arg(0, 0), StaticValue::Str("world".into()))]));
    assert_eq!(resp.diagnostics.engine, Some(Engine::V1));
    let a = &resp.alternatives[0];
    assert_eq!(a.engine, Engine::V1);
    assert_eq!(a.edits[0].edit.new_text, "\"world\"");
    let files = t.applied(a);
    assert!(files[0].1.ends_with("label(\"world\")\n"));
}

// ── 12. budget ───────────────────────────────────────────────────────────

#[test]
fn budget() {
    let src = "let a1 = 1.0\nlet a2 = 2.0\nlet a3 = 3.0\nlet a4 = 4.0\nlet a5 = 5.0\nlet a6 = 6.0\n\
               let b1 = 1.5\nlet b2 = 2.5\nlet b3 = 3.5\nlet b4 = 4.5\nlet b5 = 5.5\nlet b6 = 6.5\n\
               draw(100 + a1 + a2 * 2.0 + a3 * 3.0 + a4 * 4.0 + a5 * 5.0 + a6 * 6.0, \
               50 + b1 + b2 * 2.0 + b3 * 3.0 + b4 * 4.0 + b5 * 5.0 + b6 * 6.0, 5)\n";
    let mut t = T::new(src);
    let base = raw_args(&mut t.env, t.sid);
    let goals = vec![DmGoal::to(t.arg(0, 0), base[0][0] + 12.0), DmGoal::to(t.arg(0, 1), base[0][1] - 7.0)];
    let mut r = req(goals.clone());
    r.max_runs = 30;
    let resp = t.propose(&r);
    assert!(resp.diagnostics.runs <= 30, "{} runs", resp.diagnostics.runs);
    assert!(resp.diagnostics.budget_exhausted);

    let mut r = req(goals);
    r.filter.deny = vec![ParamSelector::Line(13)];
    let resp = t.propose(&r);
    assert_eq!(resp.diagnostics.measured, 12, "12 relevant columns");
    assert!(resp.diagnostics.subsets_predicted > 30, "{}", resp.diagnostics.subsets_predicted);
    // `max_alternatives + 2` free subsets, plus up to two single literals
    // with several goals (§9 M5, revision 3).
    assert!(resp.diagnostics.subsets_verified <= r.max_alternatives + 4);
    assert!(resp.alternatives[0].verified);
}

// ── 13. multi_file_edit ──────────────────────────────────────────────────

#[test]
fn multi_file_edit() {
    let dir = std::env::temp_dir().join(format!("petal-dm-v2-solve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = std::fs::canonicalize(&dir).unwrap();
    let layout = "pub fn place()\n  20 + 2 * 30\nend\n";
    std::fs::write(dir.join("layout.ptl"), layout).unwrap();
    let main = format!("import ./layout: place; {PREFIX}\ndraw(place(), 50, 5)\n");
    let entry = dir.join("main.ptl");
    std::fs::write(&entry, &main).unwrap();
    let mut env = env();
    let pid = env.load_program_at(&main, &entry).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut t = T::with(env, pid, sid);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 95.0)]));
    let top = &resp.alternatives[0];
    assert!(top.verified);
    let layout_file = top.edits[0].file;
    assert_ne!(layout_file, FileId(0));
    assert!(top.edits.iter().all(|e| e.file == layout_file));
    assert_eq!(top.edits[0].path.as_deref(), Some(dir.join("layout.ptl").as_path()));
    let files = t.applied(top);
    assert_eq!(files[0].1, main, "the entry file is left alone");
    let edited = &files[layout_file.0 as usize].1;
    assert_ne!(edited, layout);
    // Run the edited project from disk.
    std::fs::write(dir.join("layout.ptl"), edited).unwrap();
    let mut env2 = self::env();
    let pid2 = env2.load_program_at(&main, &entry).unwrap();
    let sid2 = env2.create_stack(pid2).unwrap();
    assert!(near(raw_args(&mut env2, sid2)[0][0], 95.0, 0.5), "{edited}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 14. rng_reshuffle_flagged ────────────────────────────────────────────

#[test]
fn rng_reshuffle_flagged() {
    let src = "let w = 100.0\nif w > 110.0 then\n  print(random())\nend\ndraw(w, 50, 5)\ndraw(random() * 100.0, 80, 5)\n";
    let mut t = T::new(src);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 120.0)]));
    let a = resp.alternatives.iter().find(|a| edits(a) == vec!["100.0→120.0"]).expect("w moves");
    assert!(a.verified, "verified under replay");
    assert!(a.regime_changed, "the branch flips");
    assert!(a.rng_order_changed, "drawing in order reshuffles the second shape");
    assert!(a.rng_difference.is_some());
    assert!(a.score_parts.rng > 0.0);
}

// ── 15. state_init_on_fresh_stack ────────────────────────────────────────

#[test]
fn state_init_on_fresh_stack() {
    let src = "state x = 100\ndraw(x, 50, 5)\n";
    let mut t = T::new(src);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 130.0)]));
    assert_eq!(edits(&resp.alternatives[0]), vec!["100→130"]);
    assert!(resp.alternatives[0].verified && resp.alternatives[0].stateful);

    // A live checkpoint already holds `x`: its initializer does nothing.
    let mut env = env();
    let pid = env.load_program(&format!("{PREFIX}{src}")).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.reset_stack(sid).unwrap();
    let mut t = T::with(env, pid, sid);
    let err = t.try_propose(&req(vec![DmGoal::to(t.arg(0, 0), 130.0)])).err().expect("nothing to move");
    match err {
        DmError::NothingToMove { why } => assert!(why.contains("state"), "{why}"),
        e => panic!("{e:?}"),
    }
}

// ── the rest of the M3 task ──────────────────────────────────────────────

#[test]
fn nested_calls() {
    let src = "fn inner(v)\n  v * 2.0\nend\nfn outer(v)\n  inner(v + 5.0) + 1.0\nend\ndraw(outer(10.0), 50, 5)\n";
    let mut t = T::new(src);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 41.0)]));
    let e = all_edits(&resp);
    assert_eq!(e[0], vec!["10.0→15.0"], "the call argument, then the bodies: {e:?}");
    for a in &resp.alternatives {
        assert!(a.verified);
        assert!(near(t.rerun(a)[0][0], 41.0, 0.5), "{}", a.description);
    }
    // Every literal on the path can do it.
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 41.0)]);
    r.max_alternatives = 6;
    let e = all_edits(&t.propose(&r));
    for want in ["2.0→2.67", "5.0→10.0", "1.0→11.0"] {
        assert!(e.contains(&vec![want.to_string()]), "{want}: {e:?}");
    }
}

#[test]
fn trig_through_a_helper() {
    let src = "fn orbit(cx, a, r)\n  cx + cos(a) * r\nend\nlet a = 0.8\ndraw(orbit(100.0, a, 50.0), 50, 5)\n";
    let mut t = T::new(src);
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 140.0)]);
    r.filter = allow(vec![ParamSelector::Variable("a".into())]);
    let resp = t.propose(&r);
    let top = &resp.alternatives[0];
    assert!(top.verified, "{}", top.description);
    let want = (40.0f64 / 50.0).acos();
    assert!(near(top.changes[0].new, want, 0.02), "{} vs {want}", top.changes[0].new);
    assert!(near(t.rerun(top)[0][0], 140.0, 0.5));
}

#[test]
fn rand_path_excluded() {
    let src = "let hi = 40\ndraw(random_int(0, hi) + 100, 50, 5)\n";
    let mut t = T::new(src);
    let base = raw_args(&mut t.env, t.sid);
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), base[0][0] + 10.0)]));
    for a in &resp.alternatives {
        assert!(a.changes.iter().all(|c| c.binding.as_deref() != Some("hi")), "{}", a.description);
    }
    assert_eq!(edits(&resp.alternatives[0]), vec!["100→110"]);
    let hi = resp.candidates.iter().find(|c| c.param.binding.as_deref() == Some("hi")).unwrap();
    assert_eq!(hi.eligible, Err("reaches the goal only through randomness, noise or `rigid`"));
}

#[test]
fn hints_resolve_ambiguity() {
    let mut t = T::new("let a = 30.0\nlet b = tunable(20.0)\ndraw(a + b, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 60.0)]));
    assert_eq!(edits(&resp.alternatives[0]), vec!["20.0→30.0"], "the tunable literal first");
    assert!(all_edits(&resp).contains(&vec!["30.0→40.0".to_string()]));

    let mut t = T::new("let a = rigid(30.0)\nlet b = 20.0\ndraw(a + b, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 60.0)]));
    assert_eq!(all_edits(&resp), vec![vec!["20.0→30.0"]], "a `rigid` literal never moves");
    let a = resp.candidates.iter().find(|c| c.param.value == 30.0).unwrap();
    assert_eq!(a.eligible, Err("declared `rigid`"));
}

#[test]
fn deny_and_function_selectors() {
    let mut t = T::new(HELPER);
    let mut r = req(vec![DmGoal::to(t.arg(1, 0), 95.0)]);
    r.allow_respell = true;
    r.filter = allow(vec![ParamSelector::Function("place".into())]);
    let e = all_edits(&t.propose(&r));
    assert!(e.iter().all(|a| a.iter().all(|x| x.starts_with("20→") || x.starts_with("30→"))), "{e:?}");
    let mut r = req(vec![DmGoal::to(t.arg(1, 0), 95.0)]);
    r.allow_respell = true;
    r.filter.deny = vec![ParamSelector::Line(5)];
    let e = all_edits(&t.propose(&r));
    assert!(!e.iter().flatten().any(|x| x.starts_with("2.0")), "{e:?}");
}

#[test]
fn unreachable_goal_is_best_effort() {
    let mut t = T::new("let a = 0.5\ndraw(100 + sin(a) * 50.0, 50, 5)\n");
    let mut r = req(vec![DmGoal::to(t.arg(0, 0), 200.0)]);
    r.filter = allow(vec![ParamSelector::Variable("a".into())]);
    let resp = t.propose(&r);
    let a = &resp.alternatives[0];
    assert!(!a.verified);
    assert!(a.achieved[0] > 135.0 && a.achieved[0] <= 150.0 + 1e-9, "as close as sin allows: {:?}", a.achieved);
    assert!(a.max_error > 1.0);
    assert!(a.description.contains("misses the goal"));
}

#[test]
fn nothing_editable_is_an_error() {
    let mut t = T::new("let a = rigid(30.0)\ndraw(a, 50, 5)\n");
    match t.try_propose(&req(vec![DmGoal::to(t.arg(0, 0), 60.0)])) {
        Err(DmError::NothingToMove { why }) => assert!(why.contains("rigid"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(t.try_propose(&req(Vec::new())), Err(DmError::NoGoals)));
}

#[test]
fn already_at_goal_proposes_nothing() {
    let mut t = T::new("draw(40, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 40.2)]));
    assert!(resp.alternatives.is_empty());
    assert!(!resp.diagnostics.warnings.is_empty());
}

#[test]
fn negative_values_keep_the_text_valid() {
    let mut t = T::new("let k = 5\ndraw(100 - k, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 103.0)]));
    let k = resp.alternatives.iter().find(|a| a.changes[0].binding.as_deref() == Some("k")).unwrap();
    assert_eq!(edits(k), vec!["5→-3"]);
    assert!(near(t.rerun(k)[0][0], 103.0, 0.5));

    let mut t = T::new("draw(100 - 5, 50, 5)\n");
    let resp = t.propose(&req(vec![DmGoal::to(t.arg(0, 0), 103.0)]));
    let k = resp.alternatives.iter().find(|a| a.changes[0].old_text == "5").unwrap();
    assert_eq!(edits(k), vec!["5→(-3)"], "`100 - -3` reads badly");
    assert!(near(t.rerun(k)[0][0], 103.0, 0.5));
}

// ── v1 parity ────────────────────────────────────────────────────────────

/// v1's proposals for argument `arg` of the call that made emit `emit`.
fn v1_edits(t: &T, emit: usize, arg: usize, target: f64) -> Vec<String> {
    let mut env = self::env();
    let program_src = t.env.get_program(t.pid).unwrap().source_map.files[0].source.clone();
    let pid = env.load_program(&program_src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let s = SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None).unwrap();
    let call = s.baseline().emits[emit].chain.chain[0];
    s.end(&mut env);
    let program = env.get_program(pid).unwrap();
    let value = if target.fract() == 0.0 { StaticValue::Int(target as i64) } else { StaticValue::Float(target) };
    let goal = ManipulationGoal { term: call, arg_index: arg, new_value: value };
    let props = v1::propose_edits(program, &goal, None, &std::collections::HashMap::<String, VarPolicy>::new())
        .expect("v1 proposes");
    let source: Vec<char> = program.source_map.files[0].source.chars().collect();
    props
        .iter()
        .map(|p| {
            let old: String = source[p.edit.span.start.offset as usize..p.edit.span.end.offset as usize].iter().collect();
            format!("{old}→{}", p.edit.new_text)
        })
        .collect()
}

#[test]
fn v1_parity_on_simple_cases() {
    for (src, emit, arg, target) in [
        ("draw(120, 50, 5)\n", 0, 0, 140.0),
        ("let r = 30\ndraw(100, 50, r)\n", 0, 2, 45.0),
        ("config let offset = 10\ndraw(offset, 50, 5)\n", 0, 0, 40.0),
        ("let w = 4.5\ndraw(10 + w * 2, 50, 5)\n", 0, 0, 40.0),
        ("let offset = 10\ndraw(offset + 2 * 15, 50, 5)\n", 0, 0, 55.0),
    ] {
        let mut t = T::new(src);
        let want = v1_edits(&t, emit, arg, target);
        assert!(!want.is_empty(), "{src}");
        let mut r = req(vec![DmGoal::to(t.arg(emit, arg), target)]);
        r.max_alternatives = 8;
        r.allow_respell = true;
        let resp = t.propose(&r);
        let got: Vec<String> =
            resp.alternatives.iter().filter(|a| a.changes.len() == 1).map(|a| edits(a)[0].clone()).collect();
        // The same literal, to the same value; v2 rounds an unwieldy one
        // (`6.666666666666667`) to the fewest decimals within tolerance.
        let same = |a: &str, b: &str| {
            let (ao, an) = a.split_once('→').unwrap();
            let (bo, bn) = b.split_once('→').unwrap();
            let (x, y): (f64, f64) = (an.parse().unwrap(), bn.parse().unwrap());
            ao == bo && (an == bn || (an.len() > 8 && (x - y).abs() <= 0.05))
        };
        for w in &want {
            assert!(got.iter().any(|g| same(w, g)), "{src}: v1 proposes {w}, v2 has {got:?}");
        }
        if want.len() == 1 {
            assert!(same(&want[0], &got[0]), "{src}: v2's top {} is v1's {want:?}", got[0]);
        }
        // ConfigStrict keeps v1's pinning (and pins nothing in a program
        // with no `config let`).
        let mut r = req(vec![DmGoal::to(t.arg(emit, arg), target)]);
        r.prefer = Prefer::ConfigStrict;
        r.allow_respell = true;
        r.max_alternatives = 8;
        let strict: Vec<String> = t.propose(&r).alternatives.iter().filter(|a| a.changes.len() == 1).map(|a| edits(a)[0].clone()).collect();
        if src.contains("config") {
            assert_eq!(strict.len(), want.len(), "{src}: ConfigStrict {strict:?} vs v1 {want:?}");
        }
        for w in &want {
            assert!(strict.iter().any(|g| same(w, g)), "{src}: ConfigStrict {strict:?} vs v1 {want:?}");
        }
    }
}

// ── the drag session ─────────────────────────────────────────────────────

#[test]
fn drag_session_previews_without_runs() {
    let mut t = T::new(SHARED);
    let r = req(vec![DmGoal::to(t.arg(2, 0), 76.0)]);
    let session = DmSession::open(&mut t.env, &mut NoHost, t.sid, &r).unwrap();
    let before = session.runs_used();
    let mut last = Vec::new();
    for x in [71.0, 73.5, 76.0] {
        last = session.preview(&[x]);
        assert_eq!(session.runs_used(), before, "a preview makes no runs");
    }
    assert_eq!(last.len(), 1);
    assert_eq!((last[0].old_text.as_str(), last[0].new), ("60", 66.0));
    let resp = session.finish(&mut t.env, &mut NoHost, &[76.0]).unwrap();
    assert_eq!(edits(&resp.alternatives[0]), vec!["60→66"]);
}

#[test]
fn explain_reports_each_stage() {
    let mut t = T::new(HELPER);
    let mut r = req(vec![DmGoal::to(t.arg(1, 0), 95.0)]);
    r.explain = true;
    let resp = t.propose(&r);
    let log = resp.explain.expect("explain").lines.join("\n");
    for want in ["discovered", "screened", "column", "subset"] {
        assert!(log.contains(want), "{want} in:\n{log}");
    }
    assert!(resp.candidates.iter().any(|c| c.column.is_some()));
}

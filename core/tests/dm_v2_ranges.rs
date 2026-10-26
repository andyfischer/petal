//! Direct manipulation v2: range goals (C6): `at most`, `at least` and
//! `between`, beside the point goals (`to`, and `--by 0` pins). A range holds
//! anywhere inside its bounds: one that holds at the baseline asks for no
//! edit, one that does not is solved to one tolerance inside the bound it is
//! past, and a range that holds but that a step would push out joins the
//! step as an active constraint. Every verified alternative is checked by
//! applying its edits and running the edited program from scratch.

use petal::dm_v2::spec::{NoHost, SpecConfig, SpecSession};
use petal::dm_v2::{
    Alternative, DmGoal, DmRequest, DmResponse, EmitKey, Observable, PathStep, apply_edits_multi, program_files,
    propose_edits_v2,
};
use petal::env::Env;
use petal::program::ProgramId;
use petal::stack::StackKey;

struct T {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
    keys: Vec<EmitKey>,
}

fn load(src: &str) -> (Env, ProgramId, StackKey) {
    let mut env = Env::new();
    env.set_seed(7);
    env.set_echo(false);
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, pid, sid)
}

/// The emitted values of a fresh run of `src`, read at `path` per emit.
fn values(env: &mut Env, sid: StackKey) -> (Vec<EmitKey>, Vec<petal::dm_v2::ObsValue>) {
    let s = SpecSession::begin(env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
    let out = s.baseline().emits.iter().map(|e| (e.key, e.value.clone())).unzip();
    s.end(env);
    out
}

impl T {
    fn new(src: &str) -> T {
        let (mut env, pid, sid) = load(src);
        let (keys, _) = values(&mut env, sid);
        T { env, pid, sid, keys }
    }

    /// Field `f` of emit `emit`'s value.
    fn field(&self, emit: usize, f: &str) -> Observable {
        Observable::EmitValue { key: self.keys[emit], path: vec![PathStep::Field(f.into())] }
    }

    /// Item `i` of emit `emit`'s value.
    fn item(&self, emit: usize, i: usize) -> Observable {
        Observable::EmitValue { key: self.keys[emit], path: vec![PathStep::Index(i)] }
    }

    fn propose(&mut self, req: &DmRequest) -> DmResponse {
        propose_edits_v2(&mut self.env, &mut NoHost, self.sid, req).expect("proposes")
    }

    /// Run the program with `alt` applied, from scratch; `read` gets each
    /// emit's value.
    fn rerun(&self, alt: &Alternative) -> Vec<petal::dm_v2::ObsValue> {
        let mut files = program_files(self.env.get_program(self.pid).unwrap());
        apply_edits_multi(&mut files, &alt.edits).expect("edits apply");
        let (mut env, _, sid) = load(&files[0].1);
        values(&mut env, sid).1
    }
}

fn num(v: &petal::dm_v2::ObsValue, path: &[PathStep]) -> f64 {
    v.at(path).and_then(|x| x.as_f64()).unwrap_or_else(|| panic!("no number at {path:?} in {v:?}"))
}

fn y() -> Vec<PathStep> {
    vec![PathStep::Field("y".into())]
}

const LIFT: &str = "import io\nconfig let lift = 3.4\npush_output(symbol(\"coins\"), {x: 2.0, y: lift * 1.0})\n";

#[test]
fn at_most_lands_inside_its_bound() {
    let mut t = T::new(LIFT);
    let r = t.propose(&DmRequest::new(vec![DmGoal::at_most(t.field(0, "y"), 3.0)]));
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    assert!(a.achieved[0] <= 3.0, "{a:#?}");
    assert_eq!(a.max_error, 0.0, "inside a range is no error");
    // The nicest literal that keeps it inside: `3.0` (the bound is inclusive).
    assert_eq!(a.changes[0].new_text, "3.0");
    assert!(a.description.contains("coins[0].y ≤ 3 (was 3.4, now 3)"), "{}", a.description);
    assert!(num(&t.rerun(a)[0], &y()) <= 3.0);
    assert_eq!(r.diagnostics.holds_at_baseline, vec![false]);
}

#[test]
fn at_least_and_between() {
    let mut t = T::new(LIFT);
    let r = t.propose(&DmRequest::new(vec![DmGoal::at_least(t.field(0, "y"), 5.25)]));
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    assert!(a.achieved[0] >= 5.25, "{a:#?}");
    assert!(num(&t.rerun(a)[0], &y()) >= 5.25);
    assert!(a.description.contains("coins[0].y ≥ 5.25 (was 3.4"), "{}", a.description);

    let r = t.propose(&DmRequest::new(vec![DmGoal::between(t.field(0, "y"), 10.0, 12.0)]));
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    let v = num(&t.rerun(a)[0], &y());
    assert!((10.0..=12.0).contains(&v), "{v}: {a:#?}");
    assert!(a.description.contains("in 10..12"), "{}", a.description);

    // Below the band: solved up to it.
    let r = t.propose(&DmRequest::new(vec![DmGoal::between(t.field(0, "y"), -2.0, -1.0)]));
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    assert!((-2.0..=-1.0).contains(&num(&t.rerun(a)[0], &y())), "{a:#?}");
}

#[test]
fn a_range_that_already_holds_proposes_nothing() {
    let mut t = T::new(LIFT);
    for goal in [
        DmGoal::at_most(t.field(0, "y"), 3.4),
        DmGoal::at_least(t.field(0, "y"), 0.0),
        DmGoal::between(t.field(0, "y"), 3.0, 4.0),
    ] {
        let r = t.propose(&DmRequest::new(vec![goal.clone()]));
        assert!(r.alternatives.is_empty(), "{goal:?}: {r:#?}");
        assert_eq!(r.diagnostics.holds_at_baseline, vec![true]);
        let w = r.diagnostics.warnings.join("\n");
        assert!(w.contains("the goal already holds at the baseline (coins[0].y = 3.4 "), "{w}");
    }
}

#[test]
fn an_unreachable_range_is_reported_not_verified() {
    // `y = a * a` is never negative.
    let src = "import io\nlet a = 2.0\npush_output(symbol(\"p\"), {y: a * a})\n";
    let mut t = T::new(src);
    let r = t.propose(&DmRequest::new(vec![DmGoal::at_most(t.field(0, "y"), -1.0)]));
    assert!(r.best_verified().is_none(), "{r:#?}");
    for a in &r.alternatives {
        assert!(a.max_error > 1.0, "{a:#?}");
        assert!(a.description.contains("misses the goal by"), "{}", a.description);
    }
    // Bad ranges are refused up front.
    for bad in [DmGoal::between(t.field(0, "y"), 2.0, 1.0), DmGoal::at_most(t.field(0, "y"), f64::NAN)] {
        let e = propose_edits_v2(&mut t.env, &mut NoHost, t.sid, &DmRequest::new(vec![bad])).err();
        assert!(e.is_some(), "a bad range is refused");
    }
}

/// An equality, a `--by 0` pin and a range that holds at the baseline, on
/// one emit. Moving `x` without moving `z` means moving `b`, which pushes
/// `y` past its bound: the range joins the step and `c` comes down to keep
/// it inside.
#[test]
fn a_range_joins_the_step_when_the_step_would_break_it() {
    let src = "import io\nlet a = 10.0\nlet b = 2.0\nlet c = 1.0\n\
               push_output(symbol(\"p\"), {x: a + b + b, y: b + c, z: a})\n";
    let mut t = T::new(src);
    let mut req = DmRequest::new(vec![
        DmGoal::to(t.field(0, "x"), 16.0),
        DmGoal::at_most(t.field(0, "y"), 3.5),
        DmGoal::to(t.field(0, "z"), 10.0),
    ]);
    req.holds = vec![false, false, true];
    let r = t.propose(&req);
    assert_eq!(r.diagnostics.holds_at_baseline, vec![false, true, true]);
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    let out = t.rerun(a);
    let read = |f: &str| num(&out[0], &[PathStep::Field(f.into())]);
    assert!((read("x") - 16.0).abs() <= 1e-5, "{a:#?}");
    assert!(read("y") <= 3.5, "the range still holds: {a:#?}");
    assert_eq!(read("z"), 10.0, "the pin holds");
    let mut moved: Vec<&str> = a.changes.iter().filter_map(|c| c.binding.as_deref()).collect();
    moved.sort();
    assert_eq!(moved, ["b", "c"], "{a:#?}");
    assert!(a.description.contains("keeps p[0].y ≤ 3.5 (was 3"), "{}", a.description);
}

/// Canopy #33/#34/#35 (cheesecake `games/canopy/dm-log.json`): the bot jumps
/// a beetle and misses two of a row of three coins. "The bot passes within
/// 0.6 of each coin" is an inequality on a min-over-frames distance (a
/// kinked measure). Stated as an exact 0.2 for two coins (#34) the coin row
/// cannot meet it, so the solver paired the row with a beetle nudge and
/// still missed; #35 faked a band with `--to 0.3 --tol 0.25`. As three
/// `at_most 0.6` goals, the middle coin's already holding (so it asks for
/// nothing and the tuples have two literals, not three), the coin row's own
/// knobs verify first.
#[test]
fn canopy_coins_on_the_bots_path() {
    let src = "\
import io
let coin_x = tunable(3.0)
let lift = tunable(1.0)
let beetle_x = 5.0
fn bot_y(x)
  let u = (x - beetle_x) / 1.5
  if abs(u) < 1.0 then 1.0 + 2.0 * (1.0 - u * u) else 1.0 end
end
let path = for f in range(0, 200) do
  let x = float(f) * 0.05
  {x: x, y: bot_y(x)}
end
fn near(cx, cy)
  reduce(path, 1000.0, fn(m, p) min(m, sqrt((p.x - cx) * (p.x - cx) + (p.y - cy) * (p.y - cy))) end)
end
push_output(symbol(\"near\"), for i in range(0, 3) do near(coin_x + float(i), 1.0 + lift) end)
";
    let mut t = T::new(src);
    let (_, base) = values(&mut t.env, t.sid);
    let before: Vec<f64> = (0..3).map(|i| num(&base[0], &[PathStep::Index(i)])).collect();
    assert!(before[0] > 0.6 && before[1] <= 0.6 && before[2] > 0.6, "two coins missed: {before:?}");

    let goals: Vec<DmGoal> = (0..3).map(|i| DmGoal::at_most(t.item(0, i), 0.6)).collect();
    let r = t.propose(&DmRequest::new(goals));
    assert_eq!(r.diagnostics.holds_at_baseline, vec![false, true, false]);
    let a = r.best_verified().unwrap_or_else(|| panic!("verified: {r:#?}"));
    assert!(a.config_only, "{a:#?}");
    for c in &a.changes {
        assert!(matches!(c.binding.as_deref(), Some("coin_x" | "lift")), "only the coin row moves: {a:#?}");
    }
    let out = t.rerun(a);
    for i in 0..3 {
        let d = num(&out[0], &[PathStep::Index(i)]);
        assert!(d <= 0.6, "coin {i} at {d}: {a:#?}");
    }
}

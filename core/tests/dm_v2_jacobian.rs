//! Direct manipulation v2, milestone M2: candidate discovery, the
//! finite-difference Jacobian and the `tunable` / `rigid` hints
//! (`petal::dm_v2::{params, jacobian, hints}`). The acceptance list is in
//! docs/dev/direct-manipulation-v2-plan.md §5 (M2); each test names the item
//! it covers. Item 11's petal-ui half (the `draw_circle` overloads of the
//! bundled `ui` prelude) is in `core-libs/petal-ui/tests/dm_v2_params.rs`.
//!
//! Core has no draw natives, so these tests register `draw(x, y, r)`: it
//! emits, and reads its arguments with `get_int` exactly as petal-ui's draw
//! natives do, so the emitted command is truncated while the raw arguments a
//! goal reads are not.

use petal::dm_v2::hints::{self, Hint};
use petal::dm_v2::jacobian::{ColumnQuality, FdOptions, Jacobian, Side, collateral_rows, jacobian};
use petal::dm_v2::params::{
    CodeOrigin, EligibilityContext, Param, ParamFilter, ParamSelector, RunBudget, anchors_for,
    discover_params, eligibility, screen_params,
};
use petal::dm_v2::spec::{NoHost, Overrides, SpecConfig, SpecSession};
use petal::dm_v2::{Observable, read_hints};
use petal::env::Env;
use petal::native_fn::{NativeEffects, PetalCxt};
use petal::program::{ProgramId, TermId, TermOp};
use petal::stack::StackKey;
use petal::value::Value;

// ── helpers ──────────────────────────────────────────────────────────────

/// `draw(x, y, r)`: one command `[x, y, r]` on channel `draw`, the
/// arguments truncated to ints.
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

/// The prefix every test program gets, on its first line so the source's
/// own line numbers are kept.
const PREFIX: &str = "import io; import creative; import noise; native fn draw(x, y, r); ";

fn env() -> Env {
    let mut env = Env::new();
    env.set_seed(7);
    env.register_native("draw", native_draw, NativeEffects::EMITS);
    env
}

fn load_in(mut env: Env, source: &str) -> (Env, ProgramId, StackKey) {
    let pid = env.load_program(&format!("{PREFIX}{source}")).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    (env, pid, sid)
}

fn load(source: &str) -> (Env, ProgramId, StackKey) {
    load_in(env(), source)
}

/// The numeric literal terms of the entry file written `text` on 1-based
/// `line`, in source order.
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
    found.sort_by_key(|(o, _)| *o);
    found.into_iter().map(|(_, t)| t).collect()
}

fn lit(env: &Env, pid: ProgramId, line: u32, text: &str) -> TermId {
    let found = lits(env, pid, line, text);
    assert_eq!(found.len(), 1, "literal `{text}` on line {line}: {found:?}");
    found[0]
}

/// Raw argument `arg` of emit `index` of the session's baseline.
fn goal(session: &SpecSession, index: usize, arg: usize) -> Observable {
    Observable::EmitArg { key: session.baseline().emits[index].key, arg, path: Vec::new() }
}

/// A session, its goals' candidates (static discovery), and the session
/// with every candidate slotted.
struct Setup {
    env: Env,
    pid: ProgramId,
    session: SpecSession,
    params: Vec<Param>,
    goals: Vec<Observable>,
}

impl Setup {
    fn new(source: &str, goals: &[(usize, usize)]) -> Setup {
        let (env, pid, sid) = load(source);
        Setup::from(env, pid, sid, goals)
    }

    fn from(mut env: Env, pid: ProgramId, sid: StackKey, goals: &[(usize, usize)]) -> Setup {
        let mut session =
            SpecSession::begin(&mut env, &mut NoHost, sid, &[], SpecConfig::default(), None).expect("session");
        let goals: Vec<Observable> = goals.iter().map(|&(i, a)| goal(&session, i, a)).collect();
        let params = {
            let program = env.get_program(pid).unwrap();
            let hints = read_hints(program, env.native_table());
            let anchors = anchors_for(program, session.baseline(), &goals);
            assert!(!anchors.is_empty(), "the goals have anchors");
            discover_params(program, env.native_table(), &hints, &anchors, &session.baseline().call_edges).params
        };
        let terms: Vec<TermId> = params.iter().map(|p| p.term).collect();
        session.add_params(&mut env, &mut NoHost, &terms).expect("slots");
        Setup { env, pid, session, params, goals }
    }

    fn index(&self, term: TermId) -> Option<usize> {
        self.params.iter().position(|p| p.term == term)
    }

    fn param(&self, term: TermId) -> &Param {
        &self.params[self.index(term).unwrap_or_else(|| panic!("term {term:?} was not discovered"))]
    }

    fn lit(&self, line: u32, text: &str) -> TermId {
        lit(&self.env, self.pid, line, text)
    }

    fn has(&self, line: u32, text: &str) -> bool {
        self.index(self.lit(line, text)).is_some()
    }

    /// The Jacobian of the goals for `terms`.
    fn jacobian(&mut self, terms: &[TermId], opts: FdOptions) -> Jacobian {
        let keep: Vec<usize> = terms.iter().map(|&t| self.index(t).expect("discovered")).collect();
        let rows = self.goals.clone();
        let n = rows.len();
        jacobian(
            &mut self.session,
            &mut self.env,
            &mut NoHost,
            &self.params,
            &keep,
            rows,
            n,
            &Overrides::none(),
            &opts,
            &mut RunBudget::runs(1000),
        )
    }

    /// The single goal's column for `term`, and its quality.
    fn column(&mut self, term: TermId) -> (f64, ColumnQuality) {
        let j = self.jacobian(&[term], FdOptions::default());
        (j.get(0, 0), j.quality[0].clone())
    }

    fn eligible(&self, term: TermId, filter: &ParamFilter) -> Result<(), &'static str> {
        let program = self.env.get_program(self.pid).unwrap();
        eligibility(program, self.param(term), filter, EligibilityContext::default()).edit
    }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-12)
}

// ── 1. discovers_literal_inside_helper ───────────────────────────────────

#[test]
fn discovers_literal_inside_helper() {
    let src = "fn place(i)\n  20 + i * 30\nend\ndraw(place(2), 50, 5)\n";
    let mut s = Setup::new(src, &[(0, 0)]);
    let (l20, l30, l2) = (s.lit(2, "20"), s.lit(2, "30"), s.lit(4, "2"));
    assert!(s.has(2, "20") && s.has(2, "30") && s.has(4, "2"));
    assert!(!s.has(4, "50"), "the y argument does not reach x");
    assert!(!s.has(4, "5"));
    assert!(s.param(l2).in_goal_frame, "`2` is written in the goal's own call");
    assert!(!s.param(l20).in_goal_frame);
    assert!(s.param(l20).distance > s.param(l2).distance, "the helper body is one call away");
    let j = s.jacobian(&[l20, l30, l2], FdOptions::default());
    assert_eq!((j.get(0, 0), j.get(0, 1), j.get(0, 2)), (1.0, 2.0, 30.0));
    assert!(j.quality.iter().all(|q| *q == ColumnQuality::Discrete), "int literals: {:?}", j.quality);
}

// ── 2. discovers_through_map_and_records ─────────────────────────────────

#[test]
fn discovers_through_map_and_records() {
    let src = "let gap = 8.5\nlet items = [{w: 40.0}, {w: 25.5}, {w: 30.0}]\n\
               let lefts = items.map(fn(it) it.w * 2.0 + gap end)\n\
               draw(lefts[1], 0, 7)\n";
    let mut s = Setup::new(src, &[(0, 0)]);
    let (gap, w0, w1, two) = (s.lit(1, "8.5"), s.lit(2, "40.0"), s.lit(2, "25.5"), s.lit(3, "2.0"));
    for t in [gap, w0, w1, two] {
        assert!(s.index(t).is_some(), "{t:?} discovered");
    }
    assert!(s.param(s.lit(4, "1")).structural, "the index is structural");
    assert_eq!(s.param(gap).binding.as_deref(), Some("gap"));
    let j = s.jacobian(&[gap, w0, w1, two], FdOptions::default());
    let want = [1.0, 0.0, 2.0, 25.5];
    for (c, w) in want.iter().enumerate() {
        if *w == 0.0 {
            assert_eq!(j.quality[c], ColumnQuality::Zero, "w0 does not move lefts[1]");
        } else {
            assert!(close(j.get(0, c), *w, 1e-6), "column {c}: {} vs {w}", j.get(0, c));
            assert_eq!(j.quality[c], ColumnQuality::Smooth);
        }
    }
}

// ── 3. trig_column ───────────────────────────────────────────────────────

#[test]
fn trig_column() {
    let src = "let cx = 100.0\nlet a = 0.7\nlet r = 30.0\ndraw(cx + cos(a) * r, 0, 1)\n";
    let mut s = Setup::new(src, &[(0, 0)]);
    let (cx, a, r) = (s.lit(1, "100.0"), s.lit(2, "0.7"), s.lit(3, "30.0"));
    let j = s.jacobian(&[cx, a, r], FdOptions::default());
    assert!(close(j.get(0, 0), 1.0, 1e-6));
    assert!(close(j.get(0, 1), -(0.7f64).sin() * 30.0, 1e-6), "d/da = {}", j.get(0, 1));
    assert!(close(j.get(0, 2), (0.7f64).cos(), 1e-6));
    assert!(j.quality.iter().all(|q| *q == ColumnQuality::Smooth));
}

// ── 4. rng_cuts_path ─────────────────────────────────────────────────────

#[test]
fn rng_cuts_path() {
    let src = "let lo = 10\nlet hi = 20\nlet f = 0.25\n\
               draw(random_int(lo, hi) + noise(f) * 0 + 5, 0, 1)\n";
    let mut s = Setup::new(src, &[(0, 0)]);
    let (lo, hi, f, five) = (s.lit(1, "10"), s.lit(2, "20"), s.lit(3, "0.25"), s.lit(4, "5"));
    for t in [lo, hi, f] {
        assert!(s.param(t).through_rng, "{t:?} reaches x only through a no-feedback native");
        assert_eq!(
            s.eligible(t, &ParamFilter::default()),
            Err("reaches the goal only through randomness, noise or `rigid`"),
            "pruned before screening"
        );
    }
    assert!(!s.param(five).through_rng);
    assert_eq!(s.eligible(five, &ParamFilter::default()), Ok(()));
    // Screening never sees them: only `5` and the `0` are eligible.
    let program = s.env.get_program(s.pid).unwrap();
    let eligible: Vec<usize> = (0..s.params.len())
        .filter(|&i| eligibility(program, &s.params[i], &ParamFilter::default(), EligibilityContext::default()).screen)
        .collect();
    assert!(eligible.iter().all(|&i| ![lo, hi, f].contains(&s.params[i].term)));
    // Measured anyway (the client opted in), the column is honest: the draw
    // is replayed as a uniform and mapped through the new bounds (§2.1.4).
    let opt_in = ParamFilter { allow: Some(vec![ParamSelector::IncludeThroughRng]), deny: Vec::new() };
    assert_eq!(s.eligible(lo, &opt_in), Ok(()));
    let (col, q) = s.column(five);
    assert_eq!((col, q), (1.0, ColumnQuality::Discrete));
}

// ── 5. rigid_is_a_barrier ────────────────────────────────────────────────

#[test]
fn rigid_is_a_barrier() {
    let src = "let w = rigid(40)\nlet a = 3.0\nlet b = 4.0\ndraw(w + rigid(a + b) + a * 2.0, 0, 1)\n";
    let mut s = Setup::new(src, &[(0, 0)]);
    let (w, a, b) = (s.lit(1, "40"), s.lit(2, "3.0"), s.lit(3, "4.0"));
    assert_eq!(s.param(w).hint, Some(Hint::Rigid));
    assert_eq!(s.eligible(w, &ParamFilter::default()), Err("declared `rigid`"));
    assert!(s.param(b).through_rng, "`b` reaches x only through `rigid`");
    assert!(!s.param(a).through_rng, "`a` also reaches x through `a * 2.0`");
    let j = s.jacobian(&[w, a, b], FdOptions::default());
    assert_eq!(j.quality[0], ColumnQuality::Zero, "the rigid call replays its baseline answer");
    assert!(close(j.get(0, 1), 2.0, 1e-9), "only the path outside `rigid`: {}", j.get(0, 1));
    assert_eq!(j.quality[2], ColumnQuality::Zero);
}

// ── 6. tunable_range_recorded ────────────────────────────────────────────

#[test]
fn tunable_range_recorded() {
    let src = "let r = 30\ntunable(r, 10, 80)\nlet s = tunable(2.5, min: 0, max: 4, step: 0.5)\ndraw(r + s, 0, 1)\n";
    let s = Setup::new(src, &[(0, 0)]);
    let r = s.lit(1, "30");
    assert_eq!(s.param(r).hint, Some(Hint::Tunable { min: Some(10.0), max: Some(80.0), step: None }));
    let st = s.lit(3, "2.5");
    assert_eq!(s.param(st).hint, Some(Hint::Tunable { min: Some(0.0), max: Some(4.0), step: Some(0.5) }));
    assert_eq!(s.param(st).binding.as_deref(), Some("s"), "the binding of the hint call");

    // A computed value marks nothing, and says so twice: where the hints are
    // read, and in `petal lint`.
    let bad = "let a = 1.0\nlet b = 2.0\nprint(tunable(a + b), tunable(a, b, 3), tunable(1, 9, 2))\nstate n = tunable(0)\n";
    let (env, pid, _) = load(bad);
    let h = read_hints(env.get_program(pid).unwrap(), env.native_table());
    let rules: Vec<&str> = h.warnings.iter().map(|w| w.rule).collect();
    assert!(rules.contains(&hints::NOT_LITERAL), "{rules:?}");
    assert!(rules.contains(&hints::BOUNDS), "{rules:?}");
    let out = petal::lint::lint_source(bad, &petal::lint::LintOptions::default()).expect("lints");
    assert_eq!(out.count("dm-hint-not-literal"), 1);
    assert_eq!(out.count("dm-hint-bounds"), 2, "a computed bound and an empty range");
    assert_eq!(out.count("dm-hint-state-init"), 1);
    // Identity at run time.
    let (mut env, _, sid) = load("let s = symbol(\"o\")\npush_output(s, tunable(4, 0, 9) + rigid(5))\n");
    env.run(sid).expect("runs");
    let sym = env.intern_symbol("o");
    assert!(matches!(env.take_output_buffer(sym).as_slice(), [Value::Int(9)]));
}

// ── 7. screening_cost ────────────────────────────────────────────────────

#[test]
fn screening_cost() {
    // 200 literals the goal depends on statically and not at run time (a
    // `min` that never picks them), and three that matter.
    let junk: Vec<String> = (0..200).map(|i| format!("{}.5", i + 1000)).collect();
    let src = format!(
        "let junk = [{}]\nlet a = 3.0\nlet b = 4.0\nlet c = 5.0\ndraw(min(a + b * c, junk[0] + junk[199]), 0, 1)\n",
        junk.join(", ")
    );
    let mut s = Setup::new(&src, &[(0, 0)]);
    let program = s.env.get_program(s.pid).unwrap();
    let eligible: Vec<usize> = (0..s.params.len())
        .filter(|&i| eligibility(program, &s.params[i], &ParamFilter::default(), EligibilityContext::default()).screen)
        .collect();
    assert_eq!(eligible.len(), 203, "the junk, a, b, c (the indices are structural)");
    let mut budget = RunBudget::runs(1000);
    let goals = s.goals.clone();
    let res = screen_params(&mut s.session, &mut s.env, &mut NoHost, &s.params, &eligible, 256, &goals, &mut budget);
    let kept: Vec<TermId> = res.kept.iter().map(|&i| s.params[i].term).collect();
    let want = vec![s.lit(2, "3.0"), s.lit(3, "4.0"), s.lit(4, "5.0")];
    assert_eq!(kept, want);
    eprintln!("screening: {} runs for {} eligible", res.runs, res.eligible);
    assert!(res.runs <= 30, "screened in {} runs", res.runs);
    assert_eq!((res.screened, res.eligible), (203, 203));
}

#[test]
fn dynamic_call_follows_observed_edges() {
    // Twenty functions of a hundred literals each, called through a list:
    // statically unresolved. Only the one the baseline called is followed.
    let mut src = String::new();
    for f in 0..20 {
        let body: Vec<String> = (0..100).map(|i| format!("{}.25", f * 1000 + i)).collect();
        src.push_str(&format!("fn f{f}(x)\n  let t = [{}]\n  x + t[0]\nend\n", body.join(", ")));
    }
    let names: Vec<String> = (0..20).map(|f| format!("f{f}")).collect();
    src.push_str(&format!("let fs = [{}]\nlet k = 3\nlet g = fs[k]\ndraw(g(1.5), 0, 1)\n", names.join(", ")));
    let s = Setup::new(&src, &[(0, 0)]);
    let program = s.env.get_program(s.pid).unwrap();
    let f3 = program.functions.iter().find(|f| f.name.as_deref() == Some("f3")).unwrap().id;
    let in_fns: Vec<_> = s.params.iter().filter(|p| p.function.is_some()).collect();
    assert_eq!(in_fns.len(), 101, "one function's literals (and its `t[0]`), not 2,000");
    assert!(in_fns.iter().all(|p| p.function == Some(f3)));
    assert!(s.params.iter().any(|p| p.value == 1.5), "the call's own argument");
}

// ── 8. branch_flip_and_step_quality ──────────────────────────────────────

#[test]
fn branch_flip_and_step_quality() {
    let mut s = Setup::new("let x = 0.0\ndraw(abs(x) * 10.0, 0, 1)\n", &[(0, 0)]);
    let (_, q) = s.column(s.lit(1, "0.0"));
    assert_eq!(q, ColumnQuality::Kink { left: -10.0, right: 10.0 });

    let mut s = Setup::new("let y = 20.3\ndraw(round(y) * 10, 0, 1)\n", &[(0, 0)]);
    let (col, q) = s.column(s.lit(1, "20.3"));
    assert!(matches!(q, ColumnQuality::Step { .. }), "{q:?}");
    assert!(close(col, 10.0, 0.2), "a secant of about 10: {col}");

    let mut s =
        Setup::new("let z = 5.0\nlet v = if z > 5 then z * 2.0 else z * 3.0 end\ndraw(v, 0, 1)\n", &[(0, 0)]);
    let (col, q) = s.column(s.lit(1, "5.0"));
    assert_eq!(q, ColumnQuality::BranchFlip { side: Side::Left });
    assert!(close(col, 3.0, 1e-9), "the slope of the side that kept its branch: {col}");

    let mut s = Setup::new("let xs = [10, 20, 30, 40]\nlet i = 1\ndraw(xs[i], 0, 1)\n", &[(0, 0)]);
    let i = s.lit(2, "1");
    let (col, q) = s.column(i);
    assert_eq!((col, q), (10.0, ColumnQuality::Discrete));
    assert!(s.param(i).structural);
}

// ── 9. int_literal_type_preserving ───────────────────────────────────────

#[test]
fn int_literal_type_preserving() {
    let mut s = Setup::new("let n = 3\ndraw(len(range(n)) * 10, 0, 1)\n", &[(0, 0)]);
    let (col, q) = s.column(s.lit(1, "3"));
    assert_eq!((col, q), (10.0, ColumnQuality::Discrete));

    // Int division: `11 / 4 - 9 / 4` is 0, so the step grows to ±2:
    // `(12 / 4 - 8 / 4) / 4` = 1/4. Never the float jump 0.5 / h.
    let mut s = Setup::new("let a = 10\ndraw(a / 4, 0, 1)\n", &[(0, 0)]);
    let j = s.jacobian(&[s.lit(1, "10")], FdOptions::default());
    assert_eq!((j.get(0, 0), j.step[0], j.quality[0].clone()), (0.25, 2.0, ColumnQuality::Discrete));
}

// ── 11. library_literals_never_proposed (core half) ──────────────────────

#[test]
fn library_literals_never_proposed() {
    let s = Setup::new("let v = 0.5\ndraw(clamp01(v) * 100.0, 0, 1)\n", &[(0, 0)]);
    let program = s.env.get_program(s.pid).unwrap();
    let library: Vec<&Param> = s.params.iter().filter(|p| p.origin == CodeOrigin::Library).collect();
    assert!(!library.is_empty(), "the prelude's `clamp01` has literals: {:?}", s.params);
    let filter = ParamFilter {
        allow: Some(vec![ParamSelector::ReportLibrary, ParamSelector::AllUserCode]),
        deny: Vec::new(),
    };
    for p in library {
        let e = eligibility(program, p, &filter, EligibilityContext::default());
        assert!(e.screen, "ReportLibrary screens them for the report");
        assert!(e.edit.is_err(), "and never offers them as edits");
    }
    let v = s.lit(1, "0.5");
    assert_eq!(s.param(v).origin, CodeOrigin::Entry);
    assert_eq!(s.eligible(v, &filter), Ok(()));
}

// ── 12. structural_and_hint_bounds_excluded ──────────────────────────────

#[test]
fn structural_and_hint_bounds_excluded() {
    let src = "let x = 7.0\nlet xs = [1.0, 2.0, 3.0]\nlet r = tunable(30.0, 10, 80)\n\
               let y = if x > 5 then x else 0.0 end\ndraw(y + xs[2] + r, 0, 1)\n";
    let s = Setup::new(src, &[(0, 0)]);
    let five = s.lit(4, "5");
    let index = s.lit(5, "2");
    let (lo, hi) = (s.lit(3, "10"), s.lit(3, "80"));
    let structural = Err("structural: a comparison, index, loop bound or key");
    assert_eq!(s.eligible(five, &ParamFilter::default()), structural);
    assert_eq!(s.eligible(index, &ParamFilter::default()), structural);
    for t in [lo, hi] {
        assert!(s.param(t).hint_bound);
        assert_eq!(s.eligible(t, &ParamFilter::default()), Err("a `tunable` bound is never edited"));
    }
    // Opting in lifts the structural exclusion, never the bound one.
    let opt_in = ParamFilter { allow: Some(vec![ParamSelector::IncludeStructural]), deny: Vec::new() };
    assert_eq!(s.eligible(five, &opt_in), Ok(()));
    let named = ParamFilter { allow: Some(vec![ParamSelector::Term(lo)]), deny: Vec::new() };
    assert!(s.eligible(lo, &named).is_err());
    // The ordinary ones are fine.
    for (l, t) in [(1, "7.0"), (3, "30.0"), (4, "0.0")] {
        assert_eq!(s.eligible(s.lit(l, t), &ParamFilter::default()), Ok(()), "{t}");
    }
    assert_eq!(s.param(s.lit(3, "30.0")).hint, Some(Hint::Tunable { min: Some(10.0), max: Some(80.0), step: None }));
}

// ── 13. embedder_rigid_is_not_a_hint ─────────────────────────────────────

fn native_rigid_body(cxt: &mut PetalCxt) -> Result<u32, String> {
    let v = cxt.get_value(1)?;
    cxt.push_value(v);
    Ok(1)
}

#[test]
fn embedder_rigid_is_not_a_hint() {
    let mut env = env();
    env.register_native("rigid", native_rigid_body, NativeEffects::PURE);
    let (env, pid, sid) = load_in(env, "let w = rigid(40)\ndraw(w + tunable(2), 0, 1)\n");
    let h = read_hints(env.get_program(pid).unwrap(), env.native_table());
    assert_eq!(h.disabled, vec!["rigid"]);
    assert!(h.warnings.iter().any(|w| w.rule == hints::DISABLED && w.message.contains("`rigid`")));
    let s = Setup::from(env, pid, sid, &[(0, 0)]);
    let w = s.lit(1, "40");
    assert_eq!(s.param(w).hint, None, "the host's `rigid` is not the hint");
    assert!(!s.param(w).through_rng, "nor a barrier");
    assert_eq!(s.param(s.lit(2, "2")).hint, Some(Hint::Tunable { min: None, max: None, step: None }));
}

// ── plumbing: collateral rows, budget, eligibility selectors ─────────────

#[test]
fn collateral_rows_are_the_other_raw_args() {
    let s = Setup::new("draw(1, 2, 3)\ndraw(4, 5, 6)\n", &[(1, 0)]);
    let rows = collateral_rows(s.session.baseline(), &s.goals, 20_000);
    assert_eq!(rows.len(), 5, "six raw args, minus the goal");
    assert!(!rows.contains(&s.goals[0]));
    let capped = collateral_rows(s.session.baseline(), &s.goals, 2);
    assert_eq!(capped.len(), 2);
}

#[test]
fn budget_stops_the_jacobian() {
    let mut s = Setup::new("let a = 1.5\nlet b = 2.5\ndraw(a * b, 0, 1)\n", &[(0, 0)]);
    let (a, b) = (s.lit(1, "1.5"), s.lit(2, "2.5"));
    let keep = vec![s.index(a).unwrap(), s.index(b).unwrap()];
    let mut budget = RunBudget::runs(2);
    let goals = s.goals.clone();
    let j = jacobian(
        &mut s.session, &mut s.env, &mut NoHost, &s.params, &keep, goals, 1, &Overrides::none(),
        &FdOptions::default(), &mut budget,
    );
    assert!(j.budget_exhausted);
    assert_eq!(j.runs, 2);
    assert!(close(j.get(0, 0), 2.5, 1e-9));
    assert_eq!(j.quality[1], ColumnQuality::Errored("run budget exhausted".into()));
}

#[test]
fn selectors_name_parameters() {
    let src = "config let gap = 12\nfn layout(i)\n  i * 40 + gap\nend\nlet extra = 3\ndraw(layout(2) + extra, 0, 1)\n";
    let s = Setup::new(src, &[(0, 0)]);
    let (gap, forty, extra) = (s.lit(1, "12"), s.lit(3, "40"), s.lit(5, "3"));
    assert!(s.param(gap).config);
    let only = |sel: ParamSelector| ParamFilter { allow: Some(vec![sel]), deny: Vec::new() };
    assert_eq!(s.eligible(gap, &only(ParamSelector::ConfigOnly)), Ok(()));
    assert!(s.eligible(extra, &only(ParamSelector::ConfigOnly)).is_err());
    assert_eq!(s.eligible(forty, &only(ParamSelector::Function("layout".into()))), Ok(()));
    assert!(s.eligible(gap, &only(ParamSelector::Function("layout".into()))).is_err());
    assert_eq!(s.eligible(extra, &only(ParamSelector::Variable("extra".into()))), Ok(()));
    assert_eq!(s.eligible(extra, &only(ParamSelector::Line(5))), Ok(()));
    let deny = ParamFilter { allow: None, deny: vec![ParamSelector::Variable("gap".into())] };
    assert_eq!(s.eligible(gap, &deny), Err("denied by the request"));
}

#[test]
fn state_and_var_paths_are_marked() {
    let src = "state var n = 4\nvar acc = 1.5\nset acc = acc * 2.0\nlet k = 7.0\ndraw(n + get acc + k, 0, 1)\n";
    let s = Setup::new(src, &[(0, 0)]);
    let (four, one5, two, seven) = (s.lit(1, "4"), s.lit(2, "1.5"), s.lit(3, "2.0"), s.lit(4, "7.0"));
    assert!(s.param(four).state_init && s.param(four).through_state);
    assert!(s.param(one5).through_var && !s.param(one5).through_state);
    assert!(s.param(two).through_var);
    assert!(!s.param(seven).through_var && !s.param(seven).state_init);
    // On a fresh stack an initializer literal is ordinary; from a live
    // checkpoint that holds the state it is not.
    let program = s.env.get_program(s.pid).unwrap();
    let live = EligibilityContext { checkpoint_has_state: true };
    assert!(eligibility(program, s.param(four), &ParamFilter::default(), live).edit.is_err());
    assert_eq!(s.eligible(four, &ParamFilter::default()), Ok(()));
}

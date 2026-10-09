// Differential tests for intelligent hot reload (docs/hot-reload.md).
//
// The contract of `Env::reload_program` is that whichever path it takes, the
// result is what a full recompile of the new source plus `transfer_state`
// leaves. So every case here is run twice from the same starting point: once
// through `reload_program`, once through the full recompile, and the two are
// then compared on everything a host could tell apart:
//
//   - the frames each runs afterwards: printed output, the run's value or
//     error (error text carries a line and column, so this checks the moved
//     source positions too), and the state after each frame;
//   - the program itself: IR equivalence, source text, every term's span,
//     the compiler's warnings, and the bytecode the VM will execute.
//
// Each case also states which path it expects, so a change that silently
// stops being incremental (or starts being) fails here.

use petal::env::{Env, ReloadOutcome};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::source_diff::SourceChange;
use petal::stack::StackKey;
use petal::static_value::StaticValue;

/// Frames run before the edit and after it.
const BEFORE: usize = 2;
const AFTER: usize = 3;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

/// One frame the way a host drives it: reset, run, collect.
fn frame(w: &mut World) -> String {
    w.env.reset_stack(w.sid).unwrap();
    run_frame(w)
}

/// A run with no reset before it: what a host that reloads and runs straight
/// away does. Every reload leaves the stack ready for this.
fn run_frame(w: &mut World) -> String {
    let result = match w.env.run(w.sid) {
        Ok(v) => format!("ok {}", petal::value::value_to_display_string(&v, w.env.heap())),
        Err(e) => format!("error {e}"),
    };
    let out = w.env.take_output().join("\n");
    let state = serde_json::Value::Object(w.env.get_state_json(w.pid, w.sid));
    format!("{out}\n=> {result}\nstate {state}")
}

fn world(entry: &str, modules: &[(&str, &str)], policy: RunPolicy) -> World {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    for (name, source) in modules {
        env.register_module(name, source);
    }
    let pid = env.load_program(entry).unwrap_or_else(|e| panic!("old source: {e}"));
    let sid = env.create_stack(pid).unwrap();
    let mut w = World { env, pid, sid };
    for _ in 0..BEFORE {
        frame(&mut w);
    }
    w
}

/// Everything about the loaded program a host or tool could read.
fn assert_same_program(a: &mut World, b: &mut World, what: &str) {
    let text_a = a.env.bytecode_text(a.pid).unwrap();
    let text_b = b.env.bytecode_text(b.pid).unwrap();
    assert_eq!(text_a, text_b, "[{what}] bytecode");
    let pa = a.env.get_program(a.pid).unwrap();
    let pb = b.env.get_program(b.pid).unwrap();
    if let Err(diff) = petal::ir_equiv::ir_equivalent(pa, pb) {
        panic!("[{what}] the programs are not IR-equivalent: {diff}");
    }
    assert_eq!(pa.source, pb.source, "[{what}] entry source");
    assert_eq!(
        pa.source_map.files.len(),
        pb.source_map.files.len(),
        "[{what}] file count"
    );
    for (fa, fb) in pa.source_map.files.iter().zip(&pb.source_map.files) {
        assert_eq!(fa.name, fb.name, "[{what}] file name");
        assert_eq!(fa.source, fb.source, "[{what}] source of {}", fa.name);
    }
    assert_eq!(pa.terms.len(), pb.terms.len(), "[{what}] term count");
    for t in &pb.terms {
        assert_eq!(
            pa.source_map.get(t.id),
            pb.source_map.get(t.id),
            "[{what}] span of term {} ({:?})",
            t.id.0,
            t.op
        );
    }
    assert_eq!(pa.warnings, pb.warnings, "[{what}] warnings");
}

struct Case<'a> {
    name: &'a str,
    old: &'a str,
    new: &'a str,
    old_modules: &'a [(&'a str, &'a str)],
    new_modules: &'a [(&'a str, &'a str)],
    expect: ReloadOutcome,
}

impl<'a> Case<'a> {
    fn new(name: &'a str, old: &'a str, new: &'a str, expect: ReloadOutcome) -> Case<'a> {
        Case {
            name,
            old,
            new,
            old_modules: &[],
            new_modules: &[],
            expect,
        }
    }

    fn modules(mut self, old: &'a [(&'a str, &'a str)], new: &'a [(&'a str, &'a str)]) -> Self {
        self.old_modules = old;
        self.new_modules = new;
        self
    }

    fn check(&self) {
        for policy in [RunPolicy::FAST, RunPolicy::REPLAY, RunPolicy::BASELINE] {
            let what = format!("{} / {}", self.name, policy.name().unwrap_or_default());

            // Incremental side.
            let mut a = world(self.old, self.old_modules, policy);
            for (name, source) in self.new_modules {
                a.env.register_module(name, source);
            }
            let report = a
                .env
                .reload_program(a.sid, self.new, None)
                .unwrap_or_else(|e| panic!("[{what}] reload failed: {e}"));
            assert_eq!(
                report.outcome, self.expect,
                "[{what}] took the wrong path ({:?}, fallback {:?})",
                report.change, report.fallback
            );

            // Full side.
            let mut b = world(self.old, self.old_modules, policy);
            for (name, source) in self.new_modules {
                b.env.register_module(name, source);
            }
            let program = b
                .env
                .compile_program(b.pid, self.new)
                .unwrap_or_else(|e| panic!("[{what}] new source: {e}"));
            let transfer = b.env.transfer_state(b.sid, program).unwrap();
            if report.outcome != ReloadOutcome::Relocated && report.outcome != ReloadOutcome::Unchanged {
                assert_eq!(
                    (report.state_preserved, report.state_dropped),
                    (transfer.state_preserved, transfer.state_dropped),
                    "[{what}] state counts"
                );
            }

            assert_same_program(&mut a, &mut b, &what);
            // The first run after a reload needs no reset, whichever path the
            // reload took.
            let (fa, fb) = (run_frame(&mut a), run_frame(&mut b));
            assert_eq!(fa, fb, "[{what}] the run straight after the reload");
            for i in 0..AFTER {
                let (fa, fb) = (frame(&mut a), frame(&mut b));
                assert_eq!(fa, fb, "[{what}] frame {i} after the reload");
            }
            assert_same_program(&mut a, &mut b, &format!("{what}, after running"));
        }
    }
}

use ReloadOutcome::{Patched, Recompiled, Relocated};

// ── Value edits ──────────────────────────────────────────────────────────

#[test]
fn config_scalar_change() {
    Case::new(
        "config scalar",
        "config let SPEED = 4\nstate x = 0\nx += SPEED\nprint(x)\n",
        "config let SPEED = 7\nstate x = 0\nx += SPEED\nprint(x)\n",
        Patched,
    )
    .check();
}

#[test]
fn nested_config_field_and_element() {
    Case::new(
        "nested config",
        "config let POST = {bloom: 0.2, effects: [{name: \"crt\", amount: 0.3}, {name: \"blur\", amount: 1.5}]}\n\
         print(POST.bloom, POST.effects[1].amount, POST.effects[0].name)\n",
        "config let POST = {bloom: 0.25, effects: [{name: \"scan\", amount: 0.3}, {name: \"blur\", amount: 2.5}]}\n\
         print(POST.bloom, POST.effects[1].amount, POST.effects[0].name)\n",
        Patched,
    )
    .check();
}

#[test]
fn config_sign_flip_and_call_arguments() {
    Case::new(
        "sign flip",
        "fn v(x, y)\n  {x: x, y: y}\nend\nconfig let DIR = v(-1.5, 2)\nconfig let N = -3\nprint(DIR.x, DIR.y, N, N * 2)\n",
        "fn v(x, y)\n  {x: x, y: y}\nend\nconfig let DIR = v(1.5, -2)\nconfig let N = 3\nprint(DIR.x, DIR.y, N, N * 2)\n",
        Patched,
    )
    .check();
}

#[test]
fn config_color_and_string_and_bool() {
    Case::new(
        "color string bool",
        "config let TINT = #ff2e88\nconfig let TITLE = \"neon\"\nconfig let ON = true\nprint(TINT.r, TINT.g, TINT.b, TITLE, ON)\n",
        "config let TINT = #ff2e80\nconfig let TITLE = \"night\"\nconfig let ON = false\nprint(TINT.r, TINT.g, TINT.b, TITLE, ON)\n",
        Patched,
    )
    .check();
}

#[test]
fn config_used_by_a_closure_and_a_lambda_in_state() {
    Case::new(
        "closures",
        "config let K = 10\n\
         fn scale(x)\n  x * K\nend\n\
         let twice = fn(x) -> scale(x) + K\n\
         state held = fn(x) -> x + K\n\
         state n = 0\n\
         n += 1\n\
         print(scale(n), twice(n), map([1, 2], fn(x) -> x + K))\n",
        "config let K = 12\n\
         fn scale(x)\n  x * K\nend\n\
         let twice = fn(x) -> scale(x) + K\n\
         state held = fn(x) -> x + K\n\
         state n = 0\n\
         n += 1\n\
         print(scale(n), twice(n), map([1, 2], fn(x) -> x + K))\n",
        Patched,
    )
    .check();
}

#[test]
fn config_used_by_a_state_initializer() {
    // The state was initialized from the old value; both reloads keep it.
    Case::new(
        "state initializer",
        "config let START = 100\nstate hp = START\nstate var seen = START * 2\nhp -= 1\nprint(hp, get seen, START)\n",
        "config let START = 50\nstate hp = START\nstate var seen = START * 2\nhp -= 1\nprint(hp, get seen, START)\n",
        Patched,
    )
    .check();
}

#[test]
fn config_used_by_a_default_parameter() {
    Case::new(
        "default parameter",
        "config let PAD = 4\nfn box(w, pad = PAD, gap = 2)\n  w + pad * 2 + gap\nend\nprint(box(10), box(10, 1), box(10, 1, 0))\n",
        "config let PAD = 6\nfn box(w, pad = PAD, gap = 3)\n  w + pad * 2 + gap\nend\nprint(box(10), box(10, 1), box(10, 1, 0))\n",
        Patched,
    )
    .check();
}

#[test]
fn config_used_by_other_config_lets() {
    Case::new(
        "derived config",
        "config let SEED = 7\nconfig let GEN = {seed: SEED, half: 800.0}\nconfig let AREA = GEN.half * 2.0\nprint(GEN.seed, AREA)\n",
        "config let SEED = 9\nconfig let GEN = {seed: SEED, half: 650.0}\nconfig let AREA = GEN.half * 2.0\nprint(GEN.seed, AREA)\n",
        Patched,
    )
    .check();
}

#[test]
fn config_in_an_imported_module() {
    let old = [
        ("cfg", "pub config let SPEED = 3\npub config let LOOK = {fog: 0.5}\n"),
        ("mover", "import cfg\npub fn step(x)\n  x + cfg.SPEED\nend\n"),
    ];
    let new = [
        ("cfg", "pub config let SPEED = 5\npub config let LOOK = {fog: 0.75}\n"),
        ("mover", "import cfg\npub fn step(x)\n  x + cfg.SPEED\nend\n"),
    ];
    let entry = "import cfg\nimport mover\nimport cfg: LOOK\nstate x = 0\nx = mover.step(x)\nprint(x, cfg.SPEED, LOOK.fog)\n";
    Case::new("module config", entry, entry, Patched)
        .modules(&old, &new)
        .check();
}

#[test]
fn a_literal_in_a_function_body() {
    Case::new(
        "body literal",
        "fn area(w)\n  let pad = 2\n  (w + pad) * 10\nend\nfor i in range(0, 3) do\n  print(area(i), \"px\")\nend\n",
        "fn area(w)\n  let pad = 3\n  (w + pad) * 11\nend\nfor i in range(0, 3) do\n  print(area(i), \"pt\")\nend\n",
        Patched,
    )
    .check();
}

#[test]
fn a_literal_a_memoized_function_reads() {
    // `cost` is called with the same argument every frame, so under the memo
    // its result is replayed. The literal it loads is an input no record
    // lists: the patch has to drop the records.
    Case::new(
        "memoized body",
        "fn cost(n)\n  let total = 0\n  for i in range(0, n) do\n    total = total + i * 3\n  end\n  total\nend\nprint(cost(20), cost(20))\n",
        "fn cost(n)\n  let total = 0\n  for i in range(0, n) do\n    total = total + i * 5\n  end\n  total\nend\nprint(cost(20), cost(20))\n",
        Patched,
    )
    .check();
}

#[test]
fn a_value_edit_together_with_a_layout_edit() {
    Case::new(
        "value and layout",
        "config let A = {x: 1, y: 22}\nfn f(v)\n  v.x + v.y\nend\nprint(f(A))\n",
        "// tuned\n\nconfig let A = {\n  x: 1000,   // wide\n  y: 22,\n}\n\nfn f(v)\n    v.x + v.y\nend\nprint( f(A) )\n",
        Patched,
    )
    .check();
}

#[test]
fn value_edits_in_two_files_at_once() {
    let old = [("cfg", "pub config let A = 1\n")];
    let new = [("cfg", "// new\npub config let A = 2\n")];
    Case::new(
        "two files",
        "import cfg\nconfig let B = 10\nprint(cfg.A + B)\n",
        "import cfg\n\nconfig let B = 20\nprint(cfg.A + B)\n",
        Patched,
    )
    .modules(&old, &new)
    .check();
}

// ── Layout edits ─────────────────────────────────────────────────────────

#[test]
fn whitespace_only() {
    Case::new(
        "whitespace",
        "config let K = 2\nstate n = 0\nn += K\nfn show(v)\n  print(v)\nend\nshow(n)\n",
        "config let K   =   2\n\n\nstate n = 0\nn += K\nfn show( v )\n        print( v )\nend\n\nshow(n)",
        Relocated,
    )
    .check();
}

#[test]
fn comment_only() {
    Case::new(
        "comments",
        "config let K = 2\nstate n = 0\nn += K\nprint(n)\n",
        "// the step\nconfig let K = 2 // per frame\nstate n = 0\n// accumulate\nn += K\nprint(n)\n// done\n",
        Relocated,
    )
    .check();
}

#[test]
fn a_runtime_error_reports_its_moved_position() {
    // The frame's error text names a line and column: after a layout edit it
    // must name the new ones, exactly as a recompile would.
    Case::new(
        "error position",
        "let xs = [1, 2]\nfn pick(i)\n  xs[i]\nend\nprint(pick(1))\nprint(pick(5))\n",
        "let xs = [1, 2]\n\n// lookup\nfn pick(i)\n      xs[i]\nend\nprint(pick(1))\n\n\n  print(pick(5))\n",
        Relocated,
    )
    .check();
}

#[test]
fn a_layout_edit_moves_warnings_too() {
    Case::new(
        "warnings",
        "fn f(a: int) -> int\n  a\nend\nlet s: int = \"no\"\nprint(f(\"x\"))\n",
        "\n\nfn f(a: int) -> int\n  a\nend\n// wrong on purpose\nlet s: int =    \"no\"\nprint(  f(\"x\"))\n",
        Relocated,
    )
    .check();
}

/// A warning that names another line (`... written on line 5 ...`) is
/// re-rendered with the line that code moved to.
#[test]
fn a_layout_edit_rewrites_a_warning_that_cites_a_line() {
    let old = "state grid = 1\nfn show()\n  print(grid)\nend\nshow()\ngrid = grid + 1\n";
    let new = "state grid = 1\nfn show()\n  print(grid)\nend\nshow()\n\n// bump\n\ngrid = grid + 1\n";
    Case::new("cited line", old, new, Relocated).check();
    let mut w = world(old, &[], RunPolicy::FAST);
    let cites = |w: &World, line: &str| {
        let warnings = &w.env.get_program(w.pid).unwrap().warnings;
        warnings.iter().any(|d| d.message.contains(line))
    };
    assert!(cites(&w, "written on line 6"), "the fixture no longer warns");
    w.env.reload_program(w.sid, new, None).unwrap();
    assert!(cites(&w, "written on line 9"));
}

#[test]
fn a_layout_edit_inside_strings_interpolation_and_elements() {
    Case::new(
        "interpolation",
        "let n = 3\nlet s = \"n is {n + 1} and {n}\"\nlet t = \"\"\"raw\n  text\"\"\"\nprint(s, t)\n",
        "let n = 3\n\nlet s =   \"n is {n + 1} and {n}\"\n// raw\nlet t =     \"\"\"raw\n  text\"\"\"\nprint(s, t)\n",
        Relocated,
    )
    .check();
}

// ── Structural edits ─────────────────────────────────────────────────────

#[test]
fn fn_body_change() {
    Case::new(
        "fn body",
        "state n = 0\nn += 1\nfn show(v)\n  print(v)\nend\nshow(n)\n",
        "state n = 0\nn += 1\nfn show(v)\n  print(\"n\", v * 2)\nend\nshow(n)\n",
        Recompiled,
    )
    .check();
}

#[test]
fn added_and_removed_bindings() {
    Case::new(
        "added binding",
        "state a = 1\na += 1\nprint(a)\n",
        "state a = 1\nstate b = 10\na += 1\nb += a\nprint(a, b)\n",
        Recompiled,
    )
    .check();
    Case::new(
        "removed binding",
        "state a = 1\nstate b = 10\na += 1\nb += a\nprint(a, b)\n",
        "state a = 1\na += 1\nprint(a)\n",
        Recompiled,
    )
    .check();
}

#[test]
fn type_changing_config_edit() {
    Case::new(
        "int to float",
        "config let N = 10\nprint(N / 4)\n",
        "config let N = 10.5\nprint(N / 4)\n",
        Recompiled,
    )
    .check();
    Case::new(
        "scalar to record",
        "config let N = 10\nprint(N)\n",
        "config let N = {v: 10}\nprint(N)\n",
        Recompiled,
    )
    .check();
    Case::new(
        "list grew",
        "config let XS = [1, 2]\nprint(len(XS))\n",
        "config let XS = [1, 2, 3]\nprint(len(XS))\n",
        Recompiled,
    )
    .check();
}

#[test]
fn a_divisor_that_reaches_zero_is_recompiled() {
    // `let H = 10 / 2` is hoisted above the first statement; `10 / 0` is not.
    Case::new(
        "divisor",
        "print(\"start\")\nlet H = 10 / 2\nprint(H)\n",
        "print(\"start\")\nlet H = 10 / 0\nprint(H)\n",
        Recompiled,
    )
    .check();
}

#[test]
fn an_edit_the_syntax_tree_does_not_show_is_recompiled() {
    // `export` is the deprecated spelling of `pub`: same tree, one warning
    // fewer.
    let old = [("m", "export let A = 1\n")];
    let new = [("m", "pub let A = 1\n")];
    Case::new("export to pub", "import m\nprint(m.A)\n", "import m\nprint(m.A)\n", Recompiled)
        .modules(&old, &new)
        .check();
}

#[test]
fn a_module_body_change_is_recompiled() {
    let old = [("m", "pub fn f(x)\n  x + 1\nend\n")];
    let new = [("m", "pub fn f(x)\n  x - 1\nend\n")];
    let entry = "import m\nstate n = 0\nn = m.f(n)\nprint(n)\n";
    Case::new("module body", entry, entry, Recompiled)
        .modules(&old, &new)
        .check();
}

// ── The paths themselves ─────────────────────────────────────────────────

#[test]
fn an_unchanged_source_is_a_no_op() {
    let src = "state n = 0\nn += 1\nprint(n)\n";
    let mut w = world(src, &[], RunPolicy::FAST);
    let before = w.env.work_counters();
    let report = w.env.reload_program(w.sid, src, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Unchanged);
    assert_eq!(report.change, SourceChange::None);
    assert_eq!(w.env.work_counters(), before);
    // Like any reload it leaves the stack ready to run: no reset needed.
    assert!(run_frame(&mut w).starts_with("3\n"));
    assert!(frame(&mut w).starts_with("4\n"));
}

/// The config-only path compiles nothing and lowers nothing, and neither
/// does the frame that follows it.
#[test]
fn the_value_path_does_no_recompile() {
    let old = "config let K = {speed: 2, tint: #102030}\nfn step(x)\n  x + K.speed\nend\nstate x = 0\nx = step(x)\nprint(x, K.tint.g)\n";
    let new = "config let K = {speed: 5, tint: #104030}\nfn step(x)\n  x + K.speed\nend\nstate x = 0\nx = step(x)\nprint(x, K.tint.g)\n";
    let mut w = world(old, &[], RunPolicy::FAST);
    let before = w.env.work_counters();

    let report = w.env.reload_program(w.sid, new, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(frame(&mut w).starts_with("9 64\n"), "4 from two frames, plus 5");
    // A second edit of the same values writes the slots the first one made.
    let report = w.env.reload_program(w.sid, old, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(frame(&mut w).starts_with("11 32\n"));

    assert_eq!(
        w.env.work_counters(),
        before,
        "a value edit must not compile or lower"
    );

    // The layout path is the same, and keeps even the memo and the closures.
    let report = w
        .env
        .reload_program(w.sid, &format!("// note\n{old}"), None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Relocated);
    assert_eq!(w.env.work_counters(), before);

    // A structural edit does both.
    let report = w
        .env
        .reload_program(w.sid, &format!("{old}print(1)\n"), None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    frame(&mut w);
    let after = w.env.work_counters();
    assert_eq!(after.compiles, before.compiles + 1);
    assert_eq!(after.lowerings, before.lowerings + 1);
}

/// A layout edit leaves the run exactly as it was: the frame gate still says
/// the next frame would reproduce the last one, and memo records survive.
#[test]
fn a_layout_edit_keeps_the_gate_and_the_memo() {
    let old = "fn cost(n)\n  let t = 0\n  for i in range(0, n) do\n    t = t + i\n  end\n  t\nend\nprint(cost(50))\n";
    let mut w = world(old, &[], RunPolicy::FAST);
    assert!(!w.env.run_needed(w.sid));
    let slots = w.env.memo_slots(w.sid);
    let report = w.env.reload_program(w.sid, &format!("// c\n{old}"), None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Relocated);
    assert!(!w.env.run_needed(w.sid), "a layout edit forces no frame");
    assert_eq!(w.env.memo_slots(w.sid), slots);
    // A value edit does force one.
    let report = w
        .env
        .reload_program(w.sid, &old.replace("50", "60"), None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(w.env.run_needed(w.sid));
}

#[test]
fn a_broken_edit_leaves_the_old_program_running() {
    let old = "config let K = 3\nstate n = 0\nn += K\nprint(n)\n";
    let mut w = world(old, &[], RunPolicy::FAST);
    assert!(w.env.reload_program(w.sid, "config let K = (\n", None).is_err());
    assert!(frame(&mut w).starts_with("9\n"));
    // And the next good edit is diffed against the source that is running.
    let report = w
        .env
        .reload_program(w.sid, &old.replace("K = 3", "K = 4"), None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(frame(&mut w).starts_with("13\n"));
}

#[test]
fn the_report_names_what_changed() {
    let old = "config let POST = {bloom: 0.2}\nfn f()\n  1\nend\nprint(POST.bloom + f())\n";
    let mut w = world(old, &[], RunPolicy::FAST);
    let report = w
        .env
        .reload_program(w.sid, &old.replace("0.2", "0.4"), None)
        .unwrap();
    match &report.change {
        SourceChange::Values(v) => {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].path.as_deref(), Some("POST.bloom"));
            assert!(v[0].config);
        }
        other => panic!("{other:?}"),
    }
    let report = w
        .env
        .reload_program(w.sid, &old.replace("0.2", "0.4").replace("  1\n", "  2\n  3\n"), None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    match &report.change {
        SourceChange::Constructs(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].name.as_deref(), Some("f"));
        }
        other => panic!("{other:?}"),
    }
}

// ── Files on disk ────────────────────────────────────────────────────────

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("petal-hot-reload-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_config_file_on_disk_is_patched_and_a_missing_one_falls_back() {
    let dir = scratch("disk");
    let main = dir.join("main.ptl");
    let config = dir.join("config.ptl");
    let entry = "import config\nstate x = 0\nx += config.SPEED\nprint(x)\n";
    std::fs::write(&main, entry).unwrap();
    std::fs::write(&config, "pub config let SPEED = 2\n").unwrap();

    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program_at(entry, &main).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut w = World { env, pid, sid };
    frame(&mut w);
    let before = w.env.work_counters();

    std::fs::write(&config, "// faster\npub config let SPEED = 10\n").unwrap();
    let report = w.env.reload_program(w.sid, entry, Some(&main)).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(report.changed_files, ["config.ptl"]);
    assert_eq!(w.env.work_counters(), before);
    assert!(frame(&mut w).starts_with("12\n"));
    assert_eq!(
        w.env.program_source(w.pid, Some(&config)),
        Some("// faster\npub config let SPEED = 10\n")
    );

    // A source file that vanished cannot be diffed: the reload is the full
    // one, and reports the compile error a full one reports.
    std::fs::remove_file(&config).unwrap();
    assert!(w.env.reload_program(w.sid, entry, Some(&main)).is_err());
    assert!(frame(&mut w).starts_with("22\n"), "the old program still runs");
    std::fs::remove_dir_all(&dir).ok();
}

// ── Setting a value directly ─────────────────────────────────────────────

/// `set_config_value` is a reload of the text the same edit produces.
#[test]
fn setting_a_value_directly_equals_reloading_the_edited_file() {
    let old = "// knobs\nconfig let POST = {\n  bloom: 0.2,   // glow\n  effects: [{amount: 0.3}, {amount: -1.5}],\n}\nconfig let NAME = \"a\"\nstate t = 0\nt += 1\nprint(POST.bloom, POST.effects[1].amount, NAME, t)\n";
    let edits: [(&str, StaticValue); 3] = [
        ("POST.bloom", StaticValue::float(0.75)),
        ("POST.effects[1].amount", StaticValue::float(2.25)),
        ("NAME", StaticValue::str("b")),
    ];
    for policy in [RunPolicy::FAST, RunPolicy::BASELINE] {
        let mut a = world(old, &[], policy);
        let mut b = world(old, &[], policy);
        let before = a.env.work_counters();
        let mut text = old.to_string();
        for (path, value) in &edits {
            let report = a.env.set_config_value(a.sid, None, path, value).unwrap();
            assert_eq!(report.outcome, ReloadOutcome::Patched, "{path}");
            text = petal::literal_edit::set_path(&text, path, value).unwrap();
            let program = b.env.compile_program(b.pid, &text).unwrap();
            b.env.transfer_state(b.sid, program).unwrap();
            assert_same_program(&mut a, &mut b, path);
            assert_eq!(frame(&mut a), frame(&mut b), "{path}");
        }
        assert_eq!(a.env.work_counters(), before);
        // The comments and layout of the running text are the file's own.
        assert_eq!(a.env.program_source(a.pid, None), Some(text.as_str()));
        assert!(text.contains("bloom: 0.75,   // glow"));
        // The host now writes that same text to disk; the reload has nothing
        // to do.
        let report = a.env.reload_program(a.sid, &text, None).unwrap();
        assert_eq!(report.outcome, ReloadOutcome::Unchanged);
        // Abandoning the drag instead: reloading the file as it still is on
        // disk puts the old values back.
        let report = a.env.reload_program(a.sid, old, None).unwrap();
        assert_eq!(report.outcome, ReloadOutcome::Patched);
        assert!(frame(&mut a).contains("0.2 -1.5 a"));
    }
}

#[test]
fn setting_a_value_that_needs_a_recompile_is_refused_and_changes_nothing() {
    let old = "config let N = 10\nconfig let XS = [1, 2]\nlet plain = 5\nprint(N, XS, plain)\n";
    let mut w = world(old, &[], RunPolicy::FAST);
    let before = w.env.work_counters();
    // A different type.
    assert!(w.env.set_config_value(w.sid, None, "N", &StaticValue::float(10.5)).is_err());
    // A different shape.
    assert!(
        w.env
            .set_config_value(w.sid, None, "XS", &StaticValue::list([1, 2, 3].map(StaticValue::int)))
            .is_err()
    );
    // No such binding.
    assert!(w.env.set_config_value(w.sid, None, "MISSING", &StaticValue::int(1)).is_err());
    assert_eq!(w.env.program_source(w.pid, None), Some(old));
    assert_eq!(w.env.work_counters(), before);
    assert!(frame(&mut w).starts_with("10 [1, 2] 5\n"));
    // Setting the value it already has is a no-op.
    let report = w.env.set_config_value(w.sid, None, "N", &StaticValue::int(10)).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Unchanged);
    // A plain `let` of data is settable too, and so is a whole list of the
    // same shape.
    let report = w.env.set_config_value(w.sid, None, "plain", &StaticValue::int(6)).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    let report = w
        .env
        .set_config_value(w.sid, None, "XS", &StaticValue::list([7, 8].map(StaticValue::int)))
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(frame(&mut w).starts_with("10 [7, 8] 6\n"));
}

#[test]
fn setting_a_value_in_a_module_finds_its_file() {
    let dir = scratch("set-module");
    let main = dir.join("main.ptl");
    let config = dir.join("config.ptl");
    let entry = "import config\nprint(config.LOOK.fog)\n";
    std::fs::write(&main, entry).unwrap();
    std::fs::write(&config, "pub config let LOOK = {fog: 0.5}\n").unwrap();
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program_at(entry, &main).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let mut w = World { env, pid, sid };
    frame(&mut w);

    // By search, and by naming the file.
    w.env.set_config_value(w.sid, None, "LOOK.fog", &StaticValue::float(0.25)).unwrap();
    assert!(frame(&mut w).starts_with("0.25\n"));
    w.env
        .set_config_value(w.sid, Some(&config), "LOOK.fog", &StaticValue::float(0.125))
        .unwrap();
    assert!(frame(&mut w).starts_with("0.125\n"));
    assert!(
        w.env
            .set_config_value(w.sid, Some(&main), "LOOK.fog", &StaticValue::float(1.0))
            .is_err()
    );
    // The file on disk was never touched; reloading it restores its value.
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "pub config let LOOK = {fog: 0.5}\n");
    let report = w.env.reload_program(w.sid, entry, Some(&main)).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(frame(&mut w).starts_with("0.5\n"));
    std::fs::remove_dir_all(&dir).ok();
}

// ── A corpus of real programs ────────────────────────────────────────────

/// A layout-only rewrite of `source`: a leading comment, trailing spaces on
/// every line, doubled blank lines. (Inside a multi-line string this changes
/// the string, which makes it a value edit; the sweep accepts either.)
fn relayout(source: &str) -> String {
    let mut out = String::from("// swept\n\n");
    for line in source.lines() {
        out.push_str(line);
        out.push_str("  \n");
        if line.trim().is_empty() {
            out.push('\n');
        }
    }
    out
}

/// `source` with a few of its number literals changed.
fn retune(source: &str) -> Option<String> {
    use petal::lexer::{Lexer, Token};
    let mut lexer = Lexer::new(source);
    lexer.tokenize().ok()?;
    let numbers: Vec<(usize, usize, String)> = lexer
        .tokens_with_spans()
        .filter_map(|(t, s)| {
            let text = match t {
                Token::Int(n) => n.checked_add(1)?.to_string(),
                Token::Float(f) => format!("{:?}", f + 0.5),
                _ => return None,
            };
            Some((s.start.offset as usize, s.end.offset as usize, text))
        })
        .collect();
    if numbers.is_empty() {
        return None;
    }
    // Every third number, so neighbours of a changed literal stay put.
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    for (start, end, text) in numbers.iter().step_by(3) {
        out.extend(&chars[at..*start]);
        out.push_str(text);
        at = *end;
    }
    out.extend(&chars[at..]);
    Some(out)
}

/// Reload `path`'s program from `old` to `new` both ways and compare the
/// programs. Returns the path the incremental side took, or `None` when the
/// new text does not compile (both sides must then refuse it).
fn sweep_one(path: &std::path::Path, old: &str, new: &str) -> Option<ReloadOutcome> {
    let load = || {
        let mut env = Env::new();
        env.set_echo(false);
        let pid = env.load_program_at(old, path).ok()?;
        let sid = env.create_stack(pid).ok()?;
        Some(World { env, pid, sid })
    };
    let mut a = load()?;
    let mut b = load()?;
    // Lower both before the edit, so the incremental side patches a live
    // lowering instead of lowering afresh afterwards.
    a.env.bytecode_text(a.pid).ok()?;
    let what = path.display().to_string();
    let full = b.env.compile_program_at(b.pid, new, path);
    let report = a.env.reload_program(a.sid, new, Some(path));
    let (Ok(program), Ok(report)) = (full, &report) else {
        assert!(report.is_err(), "[{what}] reload accepted a source a compile rejects");
        return None;
    };
    b.env.transfer_state(b.sid, program).unwrap();
    assert_same_program(&mut a, &mut b, &what);
    Some(report.outcome)
}

fn sweep(files: &[std::path::PathBuf]) -> [usize; 3] {
    let mut seen = [0usize; 3];
    let mut count = |outcome: Option<ReloadOutcome>| match outcome {
        Some(Relocated) => seen[0] += 1,
        Some(Patched) => seen[1] += 1,
        Some(Recompiled) => seen[2] += 1,
        _ => {}
    };
    for path in files {
        let Ok(old) = std::fs::read_to_string(path) else {
            continue;
        };
        count(sweep_one(path, &old, &relayout(&old)));
        if let Some(new) = retune(&old) {
            count(sweep_one(path, &old, &new));
        }
    }
    seen
}

fn ptl_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skip = path.file_name().is_some_and(|n| {
                n == "node_modules" || n == "target" || n.to_string_lossy().starts_with('.')
            });
            if !skip {
                ptl_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "ptl") {
            out.push(path);
        }
    }
    out.sort();
}

/// Every console example, relaid and retuned: whichever path the reload
/// takes, the program it leaves is the one a recompile builds.
#[test]
fn example_programs_reload_to_what_a_recompile_builds() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    ptl_files(&root.join("examples/console"), &mut files);
    assert!(files.len() > 20, "the console examples moved");
    let [relocated, patched, recompiled] = sweep(&files);
    // The sweep is only worth its time if the fast paths are what it tests.
    assert!(relocated >= files.len() / 2, "only {relocated} layout edits were relocated");
    assert!(patched >= files.len() / 2, "only {patched} value edits were patched");
    let _ = recompiled;
}

/// The same over any tree of `.ptl` files: `PETAL_RELOAD_CORPUS=<dir>[:<dir>…]
/// cargo test --release --test hot_reload -- --ignored whole_corpus`. Slow in a
/// debug build, which is why it is not part of the default run.
#[test]
#[ignore]
fn whole_corpus_reloads_to_what_a_recompile_builds() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let dirs = std::env::var("PETAL_RELOAD_CORPUS").unwrap_or_else(|_| root.display().to_string());
    let mut files = Vec::new();
    for dir in dirs.split(':').filter(|d| !d.is_empty()) {
        ptl_files(std::path::Path::new(dir), &mut files);
    }
    let [relocated, patched, recompiled] = sweep(&files);
    eprintln!(
        "{} files: {relocated} relocated, {patched} patched, {recompiled} recompiled",
        files.len()
    );
}

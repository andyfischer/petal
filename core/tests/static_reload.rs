//! Following the diff (`petal::static_cache`, "Following the diff";
//! docs/tasks/static-value-optimization.md, S2): a reload keeps the static
//! values the edit cannot have changed and rebuilds the rest.
//!
//! The oracle is the reload that rebuilds everything
//! (`Env::set_static_follow(false)`, which is what every reload did before).
//! Every case here drives the same frames through both and compares what a
//! host could tell apart (prints, the run's value or error, state) and then
//! the tables themselves, slot by slot. A value kept that should have been
//! rebuilt shows up as a slot that differs.

use petal::env::{Env, ReloadOutcome, ReloadReport, StaticSlotState};
use petal::policy::RunPolicy;
use petal::program::ProgramId;
use petal::stack::StackKey;
use petal::static_cache::StaticDebounce;
use petal::static_value::StaticValue;
use petal::value::Value;

struct World {
    env: Env,
    pid: ProgramId,
    sid: StackKey,
}

type Modules<'a> = &'a [(&'a str, &'a str)];

fn world(entry: &str, modules: Modules, policy: RunPolicy, follow: bool) -> World {
    let mut env = Env::new();
    env.set_policy(policy);
    env.set_echo(false);
    env.set_static_follow(follow);
    for (name, source) in modules {
        env.register_module(name, source);
    }
    let pid = env.load_program(entry).unwrap_or_else(|e| panic!("old source: {e}"));
    let sid = env.create_stack(pid).unwrap();
    World { env, pid, sid }
}

fn frame(w: &mut World) -> String {
    w.env.reset_stack(w.sid).unwrap();
    run_frame(w)
}

fn run_frame(w: &mut World) -> String {
    let result = match w.env.run(w.sid) {
        Ok(v) => format!("ok {}", petal::value::value_to_display_string(&v, w.env.heap())),
        Err(e) => format!("error {e}"),
    };
    let out = w.env.take_output().join("\n");
    let state = serde_json::Value::Object(w.env.get_state_json(w.pid, w.sid));
    format!("{out}\n=> {result}\nstate {state}{}", observed(w))
}

/// What a host that observes reads after a frame: the map by qualified
/// name, and the buffer term by term. Nothing when observation is off.
fn observed(w: &World) -> String {
    if !w.env.observations().enabled {
        return String::new();
    }
    let by_name = serde_json::Value::Object(w.env.get_observations_json(w.pid, w.sid));
    let heap = w.env.heap_for(w.sid).unwrap();
    let mut by_term: Vec<(u32, String)> = w
        .env
        .observations()
        .iter()
        .map(|(t, v)| (t.0, petal::value::value_to_json(&v, heap).to_string()))
        .collect();
    by_term.sort();
    format!("\nobserved {by_name}\nterms {by_term:?}")
}

/// A stack's table as text: one line per slot, with the value it holds.
fn slots(w: &World) -> Vec<String> {
    let Some(values) = w.env.static_values(w.sid) else { return Vec::new() };
    values
        .slots
        .iter()
        .enumerate()
        .map(|(i, (info, state))| {
            let name = info.name.clone().unwrap_or_else(|| "(unnamed)".to_string());
            let held = match state {
                StaticSlotState::Filled(v) => {
                    // A fork has a heap of its own.
                    petal::value::value_to_display_string(v, w.env.heap_for(w.sid).unwrap())
                }
                other => format!("{other:?}"),
            };
            format!("s{i} {name} = {held}")
        })
        .collect()
}

/// The table of `a`, which followed the edit, against the table of `b`,
/// which rebuilt everything. A slot `b` has not filled (its run raised, or
/// returned early, before reaching the store) says nothing about what the
/// value should be; every other slot must hold the same thing.
fn assert_same_tables(a: &World, b: &World, what: &str) {
    let (sa, sb) = (slots(a), slots(b));
    assert_eq!(sa.len(), sb.len(), "{what}");
    for (x, y) in sa.iter().zip(&sb) {
        if !y.ends_with("= Empty") {
            assert_eq!(x, y, "{what}\n{sa:#?}\n{sb:#?}");
        }
    }
}

/// The heap value a named slot holds.
fn held(w: &World, name: &str) -> Option<Value> {
    let values = w.env.static_values(w.sid)?;
    values.slots.iter().find_map(|(info, state)| match state {
        StaticSlotState::Filled(v) if info.name.as_deref() == Some(name) => Some(*v),
        _ => None,
    })
}

fn policies() -> Vec<RunPolicy> {
    ["replay", "replay-memo", "fast", "baseline+static"]
        .iter()
        .map(|n| RunPolicy::parse(n).unwrap().with_static_cache(true))
        .collect()
}

/// Reload `old` to `new` with the diff followed and with everything
/// rebuilt, and require the two to be indistinguishable: the frames after,
/// and the tables after each of them. Returns the report of the following
/// reload under the default policy.
fn follow_modules(old: &str, new: &str, old_modules: Modules, new_modules: Modules) -> ReloadReport {
    let mut first = None;
    // Every policy twice: as it is, and with observations on, where a
    // third stack runs without the cache at all. What a host observes after
    // each frame is then part of what is compared, and following the diff
    // must agree both with rebuilding everything and with keeping nothing.
    for (observe, policy) in [false, true].into_iter().flat_map(|o| policies().into_iter().map(move |p| (o, p))) {
        let what = format!("{}{}", policy.name().unwrap_or_default(), if observe { ", observed" } else { "" });
        let mut a = world(old, old_modules, policy, true);
        let mut b = world(old, old_modules, policy, false);
        let mut off = observe.then(|| world(old, old_modules, policy.with_static_cache(false), true));
        if observe {
            for w in [&mut a, &mut b].into_iter().chain(off.as_mut()) {
                w.env.observations_mut().enable();
            }
        }
        // `a` against the reload that rebuilds everything, and against no
        // cache at all.
        let same = |a: &mut World, b: &mut World, off: &mut Option<World>, reset: bool, step: &str| {
            let run = |w: &mut World| if reset { frame(w) } else { run_frame(w) };
            let fa = run(a);
            assert_eq!(fa, run(b), "[{what}] {step}\n{new}");
            if let Some(off) = off.as_mut() {
                assert_eq!(fa, run(off), "[{what}] {step}, against the cache off\n{new}");
            }
        };
        for _ in 0..2 {
            same(&mut a, &mut b, &mut off, true, "before the edit");
        }
        for w in [&mut a, &mut b].into_iter().chain(off.as_mut()) {
            for (name, source) in new_modules {
                w.env.register_module(name, source);
            }
        }
        let report = a
            .env
            .reload_program(a.sid, new, None)
            .unwrap_or_else(|e| panic!("[{what}] reload: {e}"));
        let plain = b.env.reload_program(b.sid, new, None).unwrap();
        if let Some(off) = off.as_mut() {
            let none = off.env.reload_program(off.sid, new, None).unwrap();
            assert_eq!(report.outcome, none.outcome, "[{what}] outcome, against the cache off");
            assert!(a.env.static_cache_enabled() && !off.env.static_cache_enabled(), "[{what}]");
        }
        assert_eq!(report.outcome, plain.outcome, "[{what}] outcome");
        assert_report_counts(&a, &report, &format!("[{what}]\n{new}"));
        assert!(plain.statics.is_none(), "[{what}] the oracle follows nothing");
        same(&mut a, &mut b, &mut off, false, "the run after the reload");
        assert_same_tables(&a, &b, &format!("[{what}] the tables after the reload\n{new}"));
        for i in 0..2 {
            same(&mut a, &mut b, &mut off, true, &format!("frame {i} after the reload"));
            assert_same_tables(&a, &b, &format!("[{what}] the tables, frame {i}"));
        }
        first.get_or_insert(report);
    }
    first.unwrap()
}

/// What the report says against the table the reload left, before
/// anything runs: `kept` slots hold a value, `rebuilt` ones hold none, and
/// every name it gives is a slot that is waiting to be built.
fn assert_report_counts(w: &World, report: &ReloadReport, what: &str) {
    let Some(s) = &report.statics else { return };
    let values = w.env.static_values(w.sid).expect("a table");
    let filled = values.slots.iter().filter(|(_, st)| matches!(st, StaticSlotState::Filled(_))).count();
    assert_eq!(s.kept + s.deferred.len(), filled, "{what}\n{s:?}");
    assert_eq!(s.kept + s.deferred.len() + s.rebuilt, values.slots.len(), "{what}\n{s:?}");
    for name in &s.rebuilt_names {
        let waiting = values.slots.iter().any(|(info, st)| {
            let short = name.rsplit("::").next().unwrap();
            (info.name.as_deref() == Some(name) || info.name.as_deref() == Some(short))
                && !matches!(st, StaticSlotState::Filled(_))
        });
        assert!(waiting, "{name} is named as rebuilt and holds a value: {what}\n{s:?}");
    }
    if s.cleared.is_some() {
        assert_eq!(s.kept, 0, "{what}\n{s:?}");
    }
}

fn follow(old: &str, new: &str) -> ReloadReport {
    follow_modules(old, new, &[], &[])
}

fn rebuilt(report: &ReloadReport) -> Vec<&str> {
    let s = report.statics.as_ref().expect("the reload followed the edit");
    s.rebuilt_names.iter().map(String::as_str).collect()
}

fn kept(report: &ReloadReport) -> usize {
    report.statics.as_ref().expect("the reload followed the edit").kept
}

/// Three tables, one built from another, and something dynamic reading all
/// of them so each has a slot.
const TABLES: &str = "\
let TILES = [{id: 1, w: 2}, {id: 2, w: 3}, {id: 3, w: 5}]
let NAMES = [\"a\", \"b\", \"c\"]
fn widths(ts) map(ts, fn(t) t.w * 2 end) end
let WIDTHS = widths(TILES)
state n = 0
n = n + 1
print(n, TILES[n % 3], NAMES[n % 3], WIDTHS)
";

#[test]
fn an_edit_to_dynamic_code_keeps_every_constant() {
    let new = TABLES.replace("n = n + 1", "n = n + 1 + n % 2");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(rebuilt(&report), Vec::<&str>::new(), "{:?}", report.statics);
    assert!(kept(&report) >= 3, "{:?}", report.statics);
    assert_eq!(report.static_lines(), Vec::<String>::new(), "nothing to say");
}

#[test]
fn an_edit_to_a_table_rebuilds_it_and_what_is_built_from_it() {
    let new = TABLES.replace("{id: 3, w: 5}]", "{id: 3, w: 5}, {id: 4, w: 7}]");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(rebuilt(&report), ["TILES", "WIDTHS"]);
    assert!(kept(&report) >= 1, "NAMES is kept: {:?}", report.statics);
    let lines = report.static_lines();
    assert!(lines[0].starts_with("constants: 2 rebuilt (TILES, WIDTHS), "), "{lines:?}");
}

#[test]
fn a_patched_literal_rebuilds_what_depends_on_its_declaration() {
    let new = TABLES.replace("{id: 2, w: 3}", "{id: 2, w: 4}");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(rebuilt(&report), ["TILES", "WIDTHS"]);
    assert!(kept(&report) >= 1, "NAMES is kept: {:?}", report.statics);

    let new = TABLES.replace("\"b\"", "\"B\"");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(rebuilt(&report), ["NAMES"]);
}

/// The case the proposal names: the literal is not in any constant's own
/// text, but in the body of a function a constant calls.
#[test]
fn a_literal_inside_a_function_a_constant_calls_rebuilds_the_constant() {
    let new = TABLES.replace("t.w * 2", "t.w * 3");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(rebuilt(&report), ["WIDTHS"]);
    assert!(kept(&report) >= 2, "TILES and NAMES are kept: {:?}", report.statics);

    // The same through a recompile: the body changes shape.
    let new = TABLES.replace("t.w * 2", "t.w + t.id");
    let report = follow(TABLES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(rebuilt(&report), ["WIDTHS"]);

    // And two calls deep, through a function declared below its caller.
    let deep = "\
fn outer(x) inner(x) + 1 end
fn inner(x) x * 10 end
let DEEP = [outer(1), outer(2)]
let OTHER = [1, 2, 3]
state n = 0
n = n + 1
print(n, DEEP, OTHER)
";
    let report = follow(deep, &deep.replace("x * 10", "x * 11"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert!(!rebuilt(&report).contains(&"OTHER"), "{:?}", report.statics);
}

#[test]
fn a_changed_import_rebuilds_what_read_the_old_module() {
    let entry = "\
import a: BASE
let SCALED = map(BASE, fn(x) x * 2 end)
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, SCALED, OWN, BASE)
";
    let modules = [("a", "pub let BASE = [1, 2, 3]\n"), ("b", "pub let BASE = [7, 8, 9]\n")];
    // Another module that exports the same name.
    let report = follow_modules(entry, &entry.replace("import a: BASE", "import b: BASE"), &modules, &modules);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(kept(&report), 0, "an import that goes away rebuilds everything: {:?}", report.statics);

    // The module's own text changes under an unchanged entry file.
    let edited = [("a", "pub let BASE = [1, 2, 3, 4]\n"), ("b", "pub let BASE = [7, 8, 9]\n")];
    let report = follow_modules(entry, entry, &modules, &edited);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let names = rebuilt(&report);
    assert!(names.contains(&"SCALED") && names.contains(&"a::BASE"), "{names:?}");
    assert!(!names.contains(&"OWN"), "{names:?}");

    // A literal of the module, patched in place.
    let edited = [("a", "pub let BASE = [1, 2, 30]\n"), ("b", "pub let BASE = [7, 8, 9]\n")];
    let report = follow_modules(entry, entry, &modules, &edited);
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    let names = rebuilt(&report);
    assert!(names.contains(&"SCALED") && !names.contains(&"OWN"), "{names:?}");

    // A second import of the same module, of a name nothing static reads.
    let more = [("a", "pub let BASE = [1, 2, 3]\npub let EXTRA = [5]\n"), ("b", "")];
    let report = follow_modules(
        entry,
        &entry.replace("import a: BASE\n", "import a: BASE\nimport a: EXTRA\n").replace("print(n, ", "print(EXTRA, n, "),
        &more,
        &more,
    );
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
}

#[test]
fn a_shadowing_declaration_rebuilds_what_read_the_shadowed_name() {
    // `abs` is the builtin until the file declares its own.
    let old = "\
let OWN = [1, 2, 3]
let DIST = [abs(-3), abs(4)]
state n = 0
n = n + 1
print(n, DIST, OWN)
";
    let new = old.replace("let DIST", "fn abs(x) x * 100 end\nlet DIST");
    let report = follow(old, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    assert_eq!(rebuilt(&report), ["DIST"], "{:?}", report.statics);
    // And back: the declaration goes away and the builtin is read again.
    let report = follow(&new, old);
    assert!(rebuilt(&report).contains(&"DIST"), "{:?}", report.statics);

    // A module's export shadowed by a declaration of the entry file.
    let lib = [("lib", "pub let BASE = [1, 2, 3]\n")];
    let old = "\
import lib
let OWN = [1, 2, 3]
let SUM = reduce(lib.BASE, 0, fn(a, b) a + b end)
state n = 0
n = n + 1
print(n, SUM, OWN)
";
    let new = old.replace("let SUM = reduce(lib.BASE", "let BASE = [10, 20]\nlet SUM = reduce(BASE");
    follow_modules(old, &new, &lib, &lib);
}

#[test]
fn an_added_overload_rebuilds_what_calls_the_name() {
    let old = "\
fn scale(x) x * 2 end
let SCALED = [scale(1), scale(2)]
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, SCALED, OWN)
";
    let new = old.replace("let SCALED", "fn scale(x, k) x * k end\nlet SCALED");
    let report = follow(old, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let names = rebuilt(&report);
    assert!(names.contains(&"SCALED") && !names.contains(&"OWN"), "{names:?}");
    // The overload removed again, and one arity edited while the other is
    // the one called.
    let report = follow(&new, old);
    assert!(rebuilt(&report).contains(&"SCALED"));
    let report = follow(&new, &new.replace("x * k", "x * k + 1"));
    assert!(rebuilt(&report).contains(&"SCALED"), "one changed arity changes the name");
}

const SHAPES: &str = "\
class Circle
  r,
end
class Square
  s,
end
fn Circle.area(c) c.r * c.r * 3 end
let SHAPE_LIST = [Circle(1), Circle(2)]
let AREAS = map(SHAPE_LIST, fn(s) s.area() end)
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, AREAS, OWN, len(SHAPE_LIST))
";

#[test]
fn an_added_method_rebuilds_what_calls_a_method_of_that_name() {
    let new = SHAPES.replace("let SHAPE_LIST", "fn Square.area(q) q.s * q.s end\nlet SHAPE_LIST");
    let report = follow(SHAPES, &new);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let names = rebuilt(&report);
    assert!(names.contains(&"AREAS") && !names.contains(&"OWN"), "{names:?}");

    // A method's body edited, by value and by shape.
    let report = follow(SHAPES, &SHAPES.replace("c.r * c.r * 3", "c.r * c.r * 4"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    let names = rebuilt(&report);
    assert!(names.contains(&"AREAS") && !names.contains(&"OWN"), "{names:?}");
    let report = follow(SHAPES, &SHAPES.replace("c.r * c.r * 3", "c.r + 1"));
    assert!(rebuilt(&report).contains(&"AREAS"));

    // A class that gains a field: its constructor changed.
    let report = follow(&new, &new.replace("  r,\n", "  r,\n  label,\n"));
    assert!(rebuilt(&report).contains(&"AREAS"), "{:?}", report.statics);
}

const KINDS: &str = "\
enum Kind
  park(shape: \"area\", cost: 3),
  road(shape: \"route\", cost: 9),
end
let ROW = [Kind.park, Kind.road, Kind.park]
let COSTS = map(ROW, fn(k) k.cost end)
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, ROW, COSTS, OWN, Kind.all)
";

#[test]
fn an_edit_to_an_enum_rebuilds_what_holds_or_reads_its_variants() {
    // An edit that does not reach the enum keeps its data too: the reload
    // empties the stack's enum table and publishes the kept records again,
    // so `k.cost` reads on the frame after it without the run that built
    // them.
    let report = follow(KINDS, &KINDS.replace("n = n + 1", "n = n + 1 + n % 2"));
    let s = report.statics.as_ref().unwrap();
    assert_eq!(s.rebuilt, 0, "{s:?}");
    assert!(s.kept >= 5, "two records and three tables: {s:?}");
    let report = follow(KINDS, &KINDS.replace("[1, 2, 3]", "[1, 2, 4]"));
    assert_eq!(rebuilt(&report), ["OWN"]);
    assert_eq!(report.statics.as_ref().unwrap().rebuilt, 1);
    // Its data, patched in place.
    let report = follow(KINDS, &KINDS.replace("cost: 9", "cost: 12"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    let names = rebuilt(&report);
    assert!(names.contains(&"COSTS") && !names.contains(&"OWN"), "{names:?}");
    // A variant added, removed, and its data reshaped.
    let report = follow(KINDS, &KINDS.replace("end\nlet ROW", "  lake(shape: \"area\", cost: 1),\nend\nlet ROW"));
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let names = rebuilt(&report);
    assert!(names.contains(&"ROW") && names.contains(&"COSTS") && !names.contains(&"OWN"), "{names:?}");
    follow(KINDS, &KINDS.replace("cost: 9)", "cost: 9, lanes: 2)").replace("cost: 3)", "cost: 3, lanes: 1)"));
    // A second enum that declares the same data key.
    let report = follow(KINDS, &KINDS.replace("let ROW", "enum Fee\n  toll(cost: 50),\nend\nlet ROW"));
    assert!(rebuilt(&report).contains(&"COSTS"), "{:?}", report.statics);
}

#[test]
fn a_value_with_no_declaration_follows_its_statement() {
    let old = "\
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, [10, 20, 30][n % 3], len(OWN))
print(n, {a: 1, b: [2, 3]}, OWN)
";
    // Another statement changes: the unnamed tables are kept.
    let report = follow(old, &old.replace("n = n + 1", "n = n + 1 + n % 2"));
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let s = report.statics.as_ref().unwrap();
    assert_eq!(s.rebuilt, 0, "{s:?}");
    assert!(s.kept >= 2, "{s:?}");
    // Their own statement changes, by shape and by value.
    let report = follow(old, &old.replace("[10, 20, 30]", "[10, 20, 30, 40]"));
    assert!(report.statics.as_ref().unwrap().rebuilt >= 1);
    let report = follow(old, &old.replace("[10, 20, 30]", "[10, 21, 30]"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    let s = report.statics.as_ref().unwrap();
    assert!(s.rebuilt >= 1 && s.kept >= 1, "{s:?}");
    // What one of them reads changes.
    follow(old, &old.replace("let OWN = [1, 2, 3]", "let OWN = [1, 2, 3, 4]"));
    // The same statement written twice, and one copy edited.
    let twice = format!("{old}print(n, {{a: 1, b: [2, 3]}}, OWN)\n");
    follow(&twice, &twice.replacen("{a: 1, b: [2, 3]}", "{a: 1, b: [2, 4]}", 1));
    follow(&twice, &twice.replacen("print(n, {a: 1, b: [2, 3]}, OWN)\n", "", 1));
}

#[test]
fn what_the_diff_cannot_follow_rebuilds_everything() {
    // Statements both versions have, in another order, around an addition.
    let new = "\
let NAMES = [\"a\", \"b\", \"c\"]
let TILES = [{id: 1, w: 2}, {id: 2, w: 3}, {id: 3, w: 5}]
let ADDED = [0]
fn widths(ts) map(ts, fn(t) t.w * 2 end) end
let WIDTHS = widths(TILES)
state n = 0
n = n + 1
print(n, TILES[n % 3], NAMES[n % 3], WIDTHS)
";
    let report = follow(TABLES, new);
    let s = report.statics.as_ref().unwrap();
    assert_eq!(s.kept, 0, "{s:?}");
    assert!(s.cleared.as_deref().is_some_and(|why| why.contains("reordered")), "{s:?}");
    // An edit the syntax tree does not show.
    let report = follow(TABLES, &TABLES.replace("t.w * 2", "(t.w * 2)"));
    assert_eq!(report.statics.as_ref().unwrap().kept, 0);
    // A new source that does not compile leaves the table alone.
    let mut w = world(TABLES, &[], policies()[0], true);
    frame(&mut w);
    let before = slots(&w);
    assert!(w.env.reload_program(w.sid, "let TILES = [", None).is_err());
    assert_eq!(slots(&w), before);
}

/// A value rebuilt to what it was keeps the heap value it had, so whatever
/// holds the old one holds the current one.
#[test]
fn a_rebuilt_value_identical_to_the_old_one_keeps_the_old_heap_value() {
    let mut w = world(TABLES, &[], policies()[0], true);
    frame(&mut w);
    let (tiles, names, widths) = (held(&w, "TILES"), held(&w, "NAMES"), held(&w, "WIDTHS"));
    assert!(tiles.is_some() && names.is_some() && widths.is_some());

    // The same table written another way: rebuilt, and identical.
    let same = TABLES.replace("w: 5}", "w: 2 + 3}");
    let report = w.env.reload_program(w.sid, &same, None).unwrap();
    assert_eq!(rebuilt(&report), ["TILES", "WIDTHS"]);
    frame(&mut w);
    assert_eq!(held(&w, "TILES"), tiles, "the old heap value");
    assert_eq!(held(&w, "WIDTHS"), widths, "and of what was built from it");
    assert_eq!(held(&w, "NAMES"), names, "kept outright");
    assert_eq!(w.env.static_values(w.sid).unwrap().stats.reused, 2);

    // A table that really changed is a new value; what was built from it
    // and came out the same is not.
    let wider = same.replace("id: 2, w: 3", "id: 20, w: 3");
    w.env.reload_program(w.sid, &wider, None).unwrap();
    frame(&mut w);
    assert_ne!(held(&w, "TILES"), tiles);
    assert_eq!(held(&w, "WIDTHS"), widths, "the widths did not change");
    assert_eq!(held(&w, "NAMES"), names);

    // `1` is not `1.0`, and a key order is part of a record.
    let mut w = world("let T = [1, {a: 1, b: 2}]\nstate n = 0\nn = n + 1\nprint(n, T)\n", &[], policies()[0], true);
    frame(&mut w);
    let t = held(&w, "T");
    w.env.reload_program(w.sid, "let T = [1.0, {a: 1, b: 2}]\nstate n = 0\nn = n + 1\nprint(n, T)\n", None).unwrap();
    frame(&mut w);
    let floated = held(&w, "T");
    assert_ne!(floated, t);
    w.env.reload_program(w.sid, "let T = [1.0, {b: 2, a: 1}]\nstate n = 0\nn = n + 1\nprint(n, T)\n", None).unwrap();
    frame(&mut w);
    assert_ne!(held(&w, "T"), floated);
}

#[test]
fn the_report_names_state_that_kept_a_value_its_initializer_no_longer_gives() {
    let old = "\
let TILES = [{id: 1, w: 2}, {id: 2, w: 3}]
let OTHER = [1, 2, 3]
fn total(ts) reduce(ts, 0, fn(a, t) a + t.w end) end
state grid = map(TILES, fn(t) t.w end)
state sum = total(TILES)
state plain = len(OTHER)
state n = 0
n = n + 1
state width = 0
width = total(TILES)
print(n, grid, sum, plain, width, TILES, OTHER)
";
    // (`width` is assigned from the table on every run and seeded from
    // nothing: it is never stale, and never named.)
    let seeds = |report: &ReloadReport| -> Vec<(String, Option<String>)> {
        let s = report.statics.as_ref().expect("followed");
        s.stale_seeds.iter().map(|x| (x.name.clone(), x.read.clone())).collect()
    };
    let tiles = Some("TILES".to_string());
    // A table two cells were seeded from, by shape and by value.
    let report = follow(old, &old.replace("{id: 2, w: 3}]", "{id: 2, w: 3}, {id: 3, w: 9}]"));
    assert_eq!(seeds(&report), [("grid".to_string(), tiles.clone()), ("sum".to_string(), tiles.clone())]);
    let lines = report.static_lines();
    assert!(
        lines.contains(&"stale seed: state grid was initialized from TILES, which changed; it keeps its value".to_string()),
        "{lines:?}"
    );
    let report = follow(old, &old.replace("w: 3", "w: 4"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(seeds(&report), [("grid".to_string(), tiles.clone()), ("sum".to_string(), tiles)]);
    // A function one initializer calls.
    let report = follow(old, &old.replace("a + t.w", "a + t.w * 2"));
    assert_eq!(seeds(&report), [("sum".to_string(), Some("total".to_string()))]);
    // A cell's own declaration.
    let report = follow(old, &old.replace("state plain = len(OTHER)", "state plain = len(OTHER) + 1"));
    assert_eq!(seeds(&report), [("plain".to_string(), None)]);
    assert!(report.static_lines().iter().any(|l| l.contains("the declaration of state plain changed")));
    // Nothing a cell read: no line. A cell declared by the edit has no
    // value to be stale.
    let report = follow(old, &old.replace("n = n + 1", "n = n + 1 + n % 2"));
    assert_eq!(seeds(&report), []);
    let report = follow(old, &old.replace("state n = 0", "state fresh = len(TILES)\nstate n = 0"));
    assert_eq!(seeds(&report), []);
}

/// A cache: a `state var` a function fills behind a test of the cell itself,
/// or of a key remembered beside it. The stale-seed report could not name
/// it (its initializer reads nothing), and three of them in one game kept
/// what they had built from a constant the reload rebuilt.
#[test]
fn the_report_names_a_cache_filled_from_a_constant_that_changed() {
    let old = "\
const GENERIC = {a: 1, b: 2}
const RAMPS = [10, 20, 30]
const OTHER = [1, 2, 3]
fn build() {...GENERIC, c: 3} end
state var DEFS = nil
state var KEY = \"\"
state var BAKED = nil
state var G = nil
state var count = 0
state var last = 0
fn defs()
  if get DEFS == nil then set DEFS = build() end
  get DEFS
end
fn bake(k)
  if k != get KEY then
    set KEY = k
    set BAKED = [RAMPS[0], k]
  end
  get BAKED
end
fn tick()
  set count = get count + 1
  set last = len(OTHER) + get count
  nil
end
if get G == nil then set G = map(RAMPS, fn(r) r * 2 end) end
tick()
print(defs(), bake(\"x\"), get G, get count, get last, OTHER)
";
    let caches = |report: &ReloadReport| -> Vec<(String, String)> {
        let s = report.statics.as_ref().expect("followed");
        s.stale_caches.iter().map(|x| (x.name.clone(), x.read.clone().unwrap_or_default())).collect()
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    // The constant a cached value is built from, through a function.
    let report = follow(old, &old.replace("{a: 1, b: 2}", "{a: 1, b: 2, d: 4}"));
    assert_eq!(caches(&report), [pair("DEFS", "GENERIC")]);
    assert_eq!(
        report.static_lines().last().map(String::as_str),
        Some("stale cache: state DEFS is filled from GENERIC, which changed, behind a test; it keeps what it built")
    );
    assert!(report.statics.as_ref().unwrap().stale_seeds.is_empty());
    // By value, without a recompile: the report is the same.
    let report = follow(old, &old.replace("{a: 1, b: 2}", "{a: 1, b: 5}"));
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    assert_eq!(caches(&report), [pair("DEFS", "GENERIC")]);
    // A constant two caches read: one behind a remembered key in a
    // function, one behind a nil test at the top level. The key itself is
    // written from the argument and is not named.
    let report = follow(old, &old.replace("[10, 20, 30]", "[10, 20, 30, 40]"));
    assert_eq!(caches(&report), [pair("BAKED", "RAMPS"), pair("G", "RAMPS")]);
    // The function that builds the cached value.
    let report = follow(old, &old.replace("c: 3}", "c: 4}"));
    assert_eq!(caches(&report), [pair("DEFS", "build")]);
    // A cell written on every run from a constant is assigned again on the
    // next one: never named.
    let report = follow(old, &old.replace("const OTHER = [1, 2, 3]", "const OTHER = [1, 2, 3, 4]"));
    assert_eq!(caches(&report), []);
    // An edit to the function that holds the guard, which the cached value
    // does not depend on, and to code nothing cached reads.
    let report = follow(old, &old.replace("  get DEFS\nend", "  let d = get DEFS\n  d\nend"));
    assert_eq!(caches(&report), []);
    let json = report.statics.as_ref().unwrap().to_json();
    assert_eq!(json["stale_caches"], serde_json::json!([]));
    // A cache nothing has filled yet holds nothing stale.
    let unfilled = old.replace("print(defs(), ", "print(");
    let report = follow(&unfilled, &unfilled.replace("{a: 1, b: 2}", "{a: 1, b: 2, d: 4}"));
    assert_eq!(caches(&report), []);
}

const DRAG: &str = "\
config let K = 2
let BIG = map([1, 2, 3, 4], fn(i) i * K end)
let OTHER = [1, 2, 3]
state n = 0
n = n + 1
print(n, K, BIG, OTHER)
";

fn drag(w: &mut World, k: i64) -> ReloadReport {
    w.env.set_config_value(w.sid, None, "K", &StaticValue::Int(k)).unwrap()
}

/// While a value is dragged, a constant whose build is slow keeps the value
/// of the tick before and is rebuilt when the drag pauses.
#[test]
fn a_slow_rebuild_waits_for_the_drag_to_pause() {
    let every = StaticDebounce { threshold: std::time::Duration::ZERO, quiet: std::time::Duration::from_secs(60) };
    let mut w = world(DRAG, &[], policies()[0], true);
    w.env.set_static_debounce(Some(every));
    assert!(frame(&mut w).starts_with("1 2 [2, 4, 6, 8] [1, 2, 3]"));

    // The first tick is not yet a drag.
    let report = drag(&mut w, 3);
    let s = report.statics.as_ref().unwrap();
    assert!(s.rebuilt_names.contains(&"BIG".to_string()) && s.deferred.is_empty(), "{s:?}");
    assert!(frame(&mut w).starts_with("2 3 [3, 6, 9, 12] [1, 2, 3]"));

    // The second is: the literal moves, the table waits.
    let report = drag(&mut w, 4);
    let s = report.statics.as_ref().unwrap();
    assert_eq!(s.deferred, ["BIG"], "{s:?}");
    assert!(report.static_lines()[0].contains("1 waiting for the drag to pause (BIG)"), "{:?}", report.static_lines());
    assert!(frame(&mut w).starts_with("3 4 [3, 6, 9, 12] [1, 2, 3]"));
    let report = drag(&mut w, 5);
    assert_eq!(report.statics.as_ref().unwrap().deferred, ["BIG"]);
    assert!(frame(&mut w).starts_with("4 5 [3, 6, 9, 12] [1, 2, 3]"));

    // Asked for, the rebuild happens on the next frame.
    assert_eq!(w.env.flush_static_rebuilds(w.sid), 1);
    assert!(w.env.run_needed(w.sid));
    assert!(frame(&mut w).starts_with("5 5 [5, 10, 15, 20] [1, 2, 3]"));
    assert_eq!(w.env.flush_static_rebuilds(w.sid), 0);

    // A reload does not leave a deferred value standing either.
    drag(&mut w, 6);
    let report = drag(&mut w, 7);
    assert_eq!(report.statics.as_ref().unwrap().deferred, ["BIG"]);
    let text = w.env.program_source(w.pid, None).unwrap().replace("n = n + 1", "n = n + 10");
    let report = w.env.reload_program(w.sid, &text, None).unwrap();
    assert!(rebuilt(&report).contains(&"BIG"), "{:?}", report.statics);
    assert!(frame(&mut w).starts_with("15 7 [7, 14, 21, 28] [1, 2, 3]"));
}

#[test]
fn a_paused_drag_rebuilds_by_itself_and_a_fast_build_never_waits() {
    let quick = StaticDebounce { threshold: std::time::Duration::ZERO, quiet: std::time::Duration::from_millis(40) };
    let mut w = world(DRAG, &[], policies()[0], true);
    w.env.set_static_debounce(Some(quick));
    frame(&mut w);
    drag(&mut w, 3);
    frame(&mut w);
    let report = drag(&mut w, 4);
    assert_eq!(report.statics.as_ref().unwrap().deferred, ["BIG"]);
    assert!(frame(&mut w).starts_with("3 4 [3, 6, 9, 12]"));
    std::thread::sleep(std::time::Duration::from_millis(60));
    assert!(w.env.run_needed(w.sid), "the gate asks for the frame that rebuilds");
    assert!(frame(&mut w).starts_with("4 4 [4, 8, 12, 16]"));
    // After the pause the next tick starts a new drag.
    let report = drag(&mut w, 5);
    assert!(report.statics.as_ref().unwrap().deferred.is_empty());

    // The default threshold is milliseconds; this build is microseconds.
    let mut w = world(DRAG, &[], policies()[0], true);
    frame(&mut w);
    for k in 3..8 {
        let report = drag(&mut w, k);
        assert!(report.statics.as_ref().unwrap().deferred.is_empty(), "tick {k}");
        assert!(frame(&mut w).contains(&format!("[{}, {}, {}, {}]", k, 2 * k, 3 * k, 4 * k)));
    }
    // Off: every tick rebuilds, whatever the build cost.
    let mut w = world(DRAG, &[], policies()[0], true);
    w.env.set_static_debounce(None);
    frame(&mut w);
    for k in 3..6 {
        assert!(drag(&mut w, k).statics.unwrap().deferred.is_empty());
        frame(&mut w);
    }
}

/// A drag is the differential's hardest case for the table: many patched
/// reloads in a row on one lowering.
#[test]
fn a_drag_with_no_debounce_equals_rebuilding_everything_each_tick() {
    for policy in policies() {
        let mut a = world(DRAG, &[], policy, true);
        let mut b = world(DRAG, &[], policy, false);
        a.env.set_static_debounce(None);
        for k in 0..12 {
            assert_eq!(frame(&mut a), frame(&mut b));
            drag(&mut a, k % 5);
            drag(&mut b, k % 5);
            assert_eq!(run_frame(&mut a), run_frame(&mut b));
            assert_same_tables(&a, &b, &format!("tick {k}"));
        }
        let stats = a.env.static_values(a.sid).unwrap().stats;
        assert!(stats.carried > 0, "something was kept: {stats:?}");
    }
}

#[test]
fn a_reload_before_the_first_run_or_without_the_cache_reports_nothing() {
    let mut w = world(TABLES, &[], policies()[0], true);
    let report = w.env.reload_program(w.sid, &TABLES.replace("n + 1", "n + n"), None).unwrap();
    assert!(report.statics.is_none());
    let mut w = world(TABLES, &[], RunPolicy::BASELINE, true);
    frame(&mut w);
    let report = w.env.reload_program(w.sid, &TABLES.replace("n + 1", "n + n"), None).unwrap();
    assert!(report.statics.is_none());
    assert_eq!(report.static_lines(), Vec::<String>::new());
}

/// Edit after edit on one stack, each followed, against a stack that
/// rebuilds everything each time: the kept values of one reload are the
/// input of the next.
#[test]
fn a_chain_of_edits_never_leaves_a_stale_value() {
    let steps = [
        TABLES.to_string(),
        TABLES.replace("w: 5}", "w: 6}"),
        TABLES.replace("w: 5}", "w: 6}").replace("t.w * 2", "t.w * 4"),
        TABLES.replace("w: 5}", "w: 6}").replace("t.w * 2", "t.w * 4").replace("\"c\"", "\"c\", \"d\""),
        TABLES.replace("\"c\"", "\"c\", \"d\"").replace("n = n + 1", "n = n + 3"),
        TABLES.replace("let WIDTHS = widths(TILES)", "let WIDTHS = widths(TILES)\nlet LAST = WIDTHS[2]")
            .replace("print(n, ", "print(LAST, n, "),
        TABLES.to_string(),
        TABLES.replace("fn widths(ts) map(ts, fn(t) t.w * 2 end) end", "fn widths(ts) [len(ts)] end"),
        TABLES.replace("let TILES", "let SPARE = [0]\nlet TILES"),
        TABLES.to_string(),
    ];
    for policy in policies() {
        let mut a = world(&steps[0], &[], policy, true);
        let mut b = world(&steps[0], &[], policy, false);
        for (i, step) in steps.iter().enumerate().skip(1) {
            assert_eq!(frame(&mut a), frame(&mut b), "before step {i}");
            a.env.reload_program(a.sid, step, None).unwrap();
            b.env.reload_program(b.sid, step, None).unwrap();
            assert_eq!(run_frame(&mut a), run_frame(&mut b), "after step {i}");
            assert_same_tables(&a, &b, &format!("the tables after step {i}"));
        }
        assert_eq!(frame(&mut a), frame(&mut b));
        assert_same_tables(&a, &b, "the last tables");
    }
}

/// A frame that raises part of the way down fills only the slots above the
/// error; the reload that follows keeps exactly those.
#[test]
fn a_reload_after_a_frame_that_raised_keeps_only_what_was_built() {
    let old = "\
let HEAD = [1, 2, 3]
state n = 0
n = n + 1
if n % 2 == 0 then
  return HEAD[0]
end
let TAIL = [HEAD[0] + 10, HEAD[1] + 20]
print(n, HEAD, TAIL, [7, 8, 9][n % 3])
";
    follow(old, &old.replace("HEAD[1] + 20", "HEAD[1] + 21"));
    follow(old, &old.replace("let HEAD = [1, 2, 3]", "let HEAD = [1, 2, 3, 4]"));
    follow(old, &old.replace("n % 2 == 0", "n % 3 == 0"));
    let raising = "\
let HEAD = [1, 2, 3]
state n = 0
n = n + 1
let BAD = HEAD[n + 5] + 1
let TAIL = [HEAD[0] + 10]
print(n, HEAD, TAIL)
";
    follow(raising, &raising.replace("n + 5", "n - 1"));
}


/// A cheap value that reads a slow one is rebuilt while the slow one waits,
/// from the slow one's old value. When the drag pauses both are rebuilt:
/// nothing is left that was built from a value that has since been
/// replaced.
#[test]
fn what_was_built_from_a_waiting_value_is_rebuilt_with_it() {
    let src = "\
config let K = 2
let BIG = map(range(0, 20000), fn(i) i * K + K end)
state n = 0
n = n + 1
let SMALL = [[K, BIG[1]], [BIG[2]]]
print(n, K, SMALL, BIG[n % 5])
";
    let slow = StaticDebounce {
        threshold: std::time::Duration::from_micros(300),
        quiet: std::time::Duration::from_secs(60),
    };
    let mut a = world(src, &[], policies()[0], true);
    let mut b = world(src, &[], policies()[0], false);
    a.env.set_static_debounce(Some(slow));
    let mut last = None;
    for k in [3, 4, 5] {
        frame(&mut a);
        frame(&mut b);
        last = drag(&mut a, k).statics;
        drag(&mut b, k);
    }
    // The table of twenty thousand waits. (The small one does not, unless
    // the machine was busy enough to make its build look slow.)
    let last = last.unwrap();
    assert!(last.deferred.contains(&"BIG".to_string()), "{last:?}");
    let during = frame(&mut a);
    frame(&mut b);
    if !last.deferred.contains(&"SMALL".to_string()) {
        assert!(during.contains("5 [[5, 6], [9]]"), "the new K beside the old table: {during}");
    }
    assert!(a.env.flush_static_rebuilds(a.sid) >= 1);
    assert_eq!(frame(&mut a), frame(&mut b), "after the drag paused");
    assert!(frame(&mut a).contains("5 [[5, 10], [15]]"));
    frame(&mut b);
    assert_same_tables(&a, &b, "after the drag paused");
}

/// One literal, and a kept table for every route the classifier resolves a
/// dependency through. Editing the literal, by value and by shape, must
/// reach every one of them; so must editing each function on the way.
const ROUTES: &str = "\
config let K = 2
let BASE = [1, 2, 3]
fn direct(x) x * K end
fn defaulted(x, k = K * 10) x + k end
fn outer(x) inner(x) + 1 end
fn inner(x) x * K end
fn pick(x) x end
fn pick(x, y) x + y * K end
fn maker() fn(x) x - K end end
fn count(n) if n <= 0 then K else count(n - 1) + 1 end end
fn label(x) \"{x}:{K}\" end
fn choose(x)
  match x
    when 1 -> K
    when v if v > K -> v * 100
    when _ -> 0
  end
end
class Box
  w,
end
fn Box.area(b) b.w * K end
enum Kind
  small(cost: 3),
  large(cost: 30),
end
let A_DIRECT = map(BASE, direct)
let B_DEFAULT = [defaulted(1), defaulted(2, 3)]
let C_NESTED = [outer(1), outer(2)]
let D_OVERLOAD = [pick(1), pick(1, 2)]
let E_RETURNED = map(BASE, maker())
let F_RECURSIVE = [count(0), count(3)]
let G_STRING = map(BASE, label)
let H_MATCH = map(BASE, choose)
let I_METHOD = map([Box(1), Box(2)], fn(b) b.area() end)
let J_LAMBDA = map(BASE, fn(x) map(BASE, fn(y) x * y * K end) end)
let L_CHAIN = [A_DIRECT[0] + 1, C_NESTED[1] + 1]
let M_CHAIN = [L_CHAIN[0] * 2, L_CHAIN[1] * 2]
let N_LOOP = for i in range(0, 3) do i * K end
let O_IF = if K > 1 then [K, K] else [0] end
let P_SORTED = sort_by(BASE, fn(x) 0 - x * K end)
let Q_REDUCED = [reduce(BASE, 0, fn(a, x) a + x * K end), 0]
let R_ENUM = map([Kind.small, Kind.large], fn(k) k.cost * K end)
let OWN = [[1, 2], [3]]
state n = 0
n = n + 1
print(n, A_DIRECT, B_DEFAULT, C_NESTED, D_OVERLOAD, E_RETURNED, F_RECURSIVE)
print(G_STRING, H_MATCH, I_METHOD, J_LAMBDA, L_CHAIN, M_CHAIN)
print(N_LOOP, O_IF, P_SORTED, Q_REDUCED, R_ENUM, OWN)
";

#[test]
fn every_route_from_a_literal_to_a_kept_value_is_followed() {
    // The program is what it should be: every table has a slot.
    let mut w = world(ROUTES, &[], policies()[0], true);
    let first = frame(&mut w);
    assert!(first.contains("[2, 4, 6]"), "{first}");
    let tables: Vec<String> = "A_DIRECT B_DEFAULT C_NESTED D_OVERLOAD E_RETURNED F_RECURSIVE G_STRING H_MATCH \
                               I_METHOD J_LAMBDA L_CHAIN M_CHAIN N_LOOP O_IF P_SORTED Q_REDUCED R_ENUM"
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let kept: Vec<String> = tables.iter().filter(|t| held(&w, t).is_some()).cloned().collect();
    assert!(kept.len() >= 12, "only these are kept: {kept:?}\n{:#?}", slots(&w));

    // The literal, by value and by shape: every kept table is reached, and
    // the one that does not read it is not.
    for new in [ROUTES.replace("config let K = 2", "config let K = 3"), ROUTES.replace("config let K = 2", "let K = 1 + 2")] {
        let report = follow(ROUTES, &new);
        let names = rebuilt(&report);
        for t in &kept {
            assert!(names.contains(&t.as_str()), "{t} was not rebuilt: {names:?}");
        }
        assert!(!names.contains(&"OWN"), "{names:?}");
    }
    // Each thing on the way, by value where it has a literal and by shape.
    for (old, by_value, by_shape) in [
        ("fn direct(x) x * K end", None, "fn direct(x) x * K + 1 end"),
        ("k = K * 10", Some("k = K * 11"), "k = K + 10"),
        ("fn inner(x) x * K end", None, "fn inner(x) x * K * K end"),
        ("fn pick(x, y) x + y * K end", None, "fn pick(x, y) x - y * K end"),
        ("fn maker() fn(x) x - K end end", None, "fn maker() fn(x) K - x end end"),
        ("count(n - 1) + 1", Some("count(n - 1) + 2"), "count(n - 1) * 2"),
        ("\"{x}:{K}\"", Some("\"{x}/{K}\""), "\"{K}:{x}\""),
        ("v * 100", Some("v * 101"), "v + 100"),
        ("when 1 -> K", None, "when 1 -> K + K"),
        ("fn Box.area(b) b.w * K end", None, "fn Box.area(b) b.w + K end"),
        ("small(cost: 3)", Some("small(cost: 4)"), "small(cost: 1 + 2)"),
        ("let BASE = [1, 2, 3]", Some("let BASE = [1, 2, 4]"), "let BASE = [1, 2, 3, 4]"),
        ("x * y * K", None, "x + y * K"),
        ("0 - x * K", Some("1 - x * K"), "x * K"),
    ] {
        for new in by_value.into_iter().chain([by_shape]) {
            assert!(ROUTES.contains(old), "{old}");
            let report = follow(ROUTES, &ROUTES.replace(old, new));
            assert!(!rebuilt(&report).contains(&"OWN"), "{old} -> {new}: {:?}", report.statics);
        }
    }
    // Things added that change what a name means.
    follow(ROUTES, &ROUTES.replace("class Box", "class Tin\n  w,\nend\nfn Tin.area(t) 7 end\nclass Box"));
    follow(ROUTES, &ROUTES.replace("fn maker()", "fn pick(x, y, z) 0 end\nfn maker()"));
    follow(ROUTES, &ROUTES.replace("let A_DIRECT", "fn range(a, b) [a, b] end\nlet A_DIRECT"));
    follow(ROUTES, &ROUTES.replace("let A_DIRECT", "enum Fee\n  toll(cost: 5),\nend\nlet A_DIRECT"));
}

/// The value a rebuilt slot is compared with is a root until the
/// comparison: a frame that collects many times before it reaches the
/// declaration still finds it.
#[test]
fn a_remembered_value_survives_collections_until_it_is_compared() {
    let src = "\
state n = 0
n = n + 1
let junk = for i in range(0, 30000 + n) do [i, \"x{i}\", {k: i}] end
let WORDS = [\"alpha\", \"beta\", \"gamma\"]
let T = map(WORDS, fn(w) {word: w, parts: [w, w ++ \"!\"]} end)
print(n, len(junk), T[n % 3].parts, WORDS)
";
    let mut w = world(src, &[], policies()[0], true);
    frame(&mut w);
    let t = held(&w, "T");
    assert!(t.is_some(), "{:#?}", slots(&w));
    let before = w.env.heap().collections();
    // The same tables written another way: rebuilt, identical, and behind
    // thirty thousand allocations.
    let same = src.replace("\"gamma\"]", "\"gam\" ++ \"ma\"]");
    let report = w.env.reload_program(w.sid, &same, None).unwrap();
    assert!(rebuilt(&report).contains(&"T"), "{:?}", report.statics);
    let out = run_frame(&mut w);
    assert!(out.contains("gamma"), "{out}");
    assert!(w.env.heap().collections() > before, "the frame collected");
    assert_eq!(held(&w, "T"), t, "the old heap value, found again after the collections");
    // `WORDS` (one list, kept on its own) and `T`.
    assert_eq!(w.env.static_values(w.sid).unwrap().stats.reused, 2);
    for _ in 0..3 {
        frame(&mut w);
    }
    assert_eq!(held(&w, "T"), t);
    follow(src, &same);
    follow(src, &src.replace("\"beta\"", "\"BETA\""));
}

/// A literal inside a module's function that the entry file's constant
/// calls, and a module constant another module's constant is built from.
#[test]
fn an_edit_in_a_module_reaches_the_constants_of_every_file_that_reads_it() {
    let entry = "\
import geo
import pal
let SIDES = [geo.scale(1), geo.scale(2)]
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, SIDES, pal.RAMP, OWN, geo.UNIT)
";
    let old = [
        ("geo", "pub let UNIT = [4, 4]\npub fn scale(x) x * 10 end\n"),
        ("pal", "import geo\npub let RAMP = map(geo.UNIT, fn(u) u + 1 end)\n"),
    ];
    let edit = |from: &str, to: &str| -> Vec<(&'static str, String)> {
        old.iter().map(|(n, s)| (*n, s.replace(from, to))).collect()
    };
    for (from, to, expect, outcome) in [
        ("x * 10", "x * 11", vec!["SIDES"], ReloadOutcome::Patched),
        ("x * 10", "x + 10", vec!["SIDES"], ReloadOutcome::Recompiled),
        ("[4, 4]", "[4, 5]", vec!["pal::RAMP"], ReloadOutcome::Patched),
        ("[4, 4]", "[4, 4, 4]", vec!["pal::RAMP"], ReloadOutcome::Recompiled),
        ("u + 1", "u + 2", vec!["pal::RAMP"], ReloadOutcome::Patched),
    ] {
        let new = edit(from, to);
        let new: Vec<(&str, &str)> = new.iter().map(|(n, s)| (*n, s.as_str())).collect();
        let report = follow_modules(entry, entry, &old, &new);
        assert_eq!(report.outcome, outcome, "{from} -> {to}");
        let names = rebuilt(&report);
        for e in &expect {
            assert!(names.contains(e), "{from} -> {to}: {e} not in {names:?}");
        }
        assert!(!names.contains(&"OWN"), "{from} -> {to}: {names:?}");
    }
}

// ---------------------------------------------------------------------
// Review of S2: attacks on the dirtying.
// ---------------------------------------------------------------------

/// A kept value that holds functions, instances and variants must still
/// mean the same thing in a program whose functions, classes and enums
/// were renumbered by a declaration added in front of them.
#[test]
fn review_a_kept_value_survives_renumbering() {
    let old = "\
let OPS = [fn(x) x * 2 end, fn(x) x + 1 end]
fn maker(k) fn(x) x - k end end
let SUB = [maker(5), maker(7)]
class Box
  w,
end
fn Box.area(b) b.w * 3 end
enum Kind
  small(cost: 3),
  large(cost: 30),
end
let BOXES = [Box(1), Box(2)]
let KINDS = [Kind.small, Kind.large, Kind.small]
fn named(x) x * 1000 end
let FNS = [named, named]
state n = 0
n = n + 1
let b = BOXES[n % 2]
let k = KINDS[n % 3]
let word = match k
  when Kind.small -> \"small\"
  when Kind.large -> \"large\"
end
print(n, OPS[0](n), OPS[1](n), SUB[0](n), SUB[1](n), b.area(), k.cost, word, k == Kind.small, FNS[0](n))
print(BOXES, KINDS)
";
    // In front of everything.
    let pre = "fn extra(y) y - 1000 end\nlet PRE = [fn(q) q + 100 end]\nclass Tin\n  t,\nend\nfn Tin.area(t) 7 end\nenum Fee\n  toll(cost: 50),\n  free(cost: 0),\nend\n";
    let report = follow(old, &format!("{pre}{old}"));
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    follow(&format!("{pre}{old}"), old);
    // In the dynamic part, where a lambda appears.
    follow(old, &old.replace("let b = BOXES", "let tmp = map([1, 2], fn(z) z * n end)\nlet b = BOXES"));
    // Each kind alone.
    follow(old, &format!("fn extra(y) y - 1000 end\n{old}"));
    follow(old, &format!("class Tin\n  t,\nend\n{old}"));
    follow(old, &format!("enum Fee\n  toll(cost: 50),\nend\n{old}"));
    follow(old, &old.replace("fn maker", "let MID = [fn(q) q + 100 end, fn(q) q + 200 end]\nfn maker"));
    follow(old, &old.replace("fn named", "fn unnamed(x) x * 5 end\nfn named"));
}

/// Positions are compared between the diff and the program's source map:
/// they have to agree after an edit that moved everything (a comment
/// added, a literal that got longer) and in a text with characters wider
/// than a byte.
#[test]
fn review_positions_agree_after_text_moved() {
    let chain = |steps: &[String]| {
        for policy in policies() {
            let mut a = world(&steps[0], &[], policy, true);
            let mut b = world(&steps[0], &[], policy, false);
            for (i, step) in steps.iter().enumerate().skip(1) {
                assert_eq!(frame(&mut a), frame(&mut b), "before step {i}");
                let ra = a.env.reload_program(a.sid, step, None);
                let rb = b.env.reload_program(b.sid, step, None);
                assert_eq!(ra.is_ok(), rb.is_ok(), "step {i}");
                assert_eq!(run_frame(&mut a), run_frame(&mut b), "after step {i}\n{step}");
                assert_same_tables(&a, &b, &format!("the tables after step {i}\n{step}"));
                assert_eq!(frame(&mut a), frame(&mut b), "frame after step {i}\n{step}");
                assert_same_tables(&a, &b, &format!("the tables, a frame after step {i}\n{step}"));
            }
        }
    };
    // A comment in front (relocated), then a literal in a function.
    let c = format!("// a comment that moves every line\n// and another\n{TABLES}");
    chain(&[TABLES.to_string(), c.clone(), c.replace("t.w * 2", "t.w * 3"), c.replace("t.w * 2", "t.w * 3").replace("w: 5", "w: 6")]);
    // A literal that gets longer, then one behind it.
    let l = TABLES.replace("{id: 1, w: 2}", "{id: 100000, w: 2222}");
    chain(&[TABLES.to_string(), l.clone(), l.replace("\"b\"", "\"bbbbbbbbb\""), l.replace("\"b\"", "\"bbbbbbbbb\"").replace("t.w * 2", "t.w * 7")]);
    // A string literal patched to a longer one, then the table behind it by shape and value.
    let s = TABLES.replace("\"a\"", "\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"");
    chain(&[TABLES.to_string(), s.clone(), s.replace("t.w * 2", "t.w * 9"), s.replace("t.w * 2", "t.w + 9"), s.replace("t.w * 2", "t.w + 8")]);
    // Wide characters in front of the edit.
    let u = TABLES.replace("\"a\"", "\"héllo wörld ✓✓✓ 日本語\"");
    let u = format!("// ünïcödé ✓ 日本語のコメント\n{u}");
    chain(&[u.clone(), u.replace("t.w * 2", "t.w * 3"), u.replace("t.w * 2", "t.w + 3"), u.replace("w: 5", "w: 6"), u.replace("w: 5", "w: 6").replace("\"c\"", "\"ç\"")]);
    // Blank lines removed and added between edits.
    let b = TABLES.replace("let NAMES", "\n\n\nlet NAMES");
    chain(&[b.clone(), TABLES.replace("t.w * 2", "t.w * 5"), b.replace("w: 3", "w: 33"), TABLES.to_string()]);
}

/// A reload that does not compile leaves the program alone; the next one
/// is an edit of the program that is running, not of the text that failed.
#[test]
fn review_a_failed_reload_then_one_that_compiles() {
    for policy in policies() {
        let mut a = world(TABLES, &[], policy, true);
        let mut b = world(TABLES, &[], policy, false);
        assert_eq!(frame(&mut a), frame(&mut b));
        // The broken text already has the table's edit in it.
        let broken = TABLES.replace("w: 5}", "w: 500}, {id: 9, w: 9}").replace("print(n, ", "print(n, (");
        let fixed = TABLES.replace("w: 5}", "w: 500}, {id: 9, w: 9}");
        for w in [&mut a, &mut b] {
            assert!(w.env.reload_program(w.sid, &broken, None).is_err());
        }
        assert_eq!(frame(&mut a), frame(&mut b), "after the failed reload");
        assert_same_tables(&a, &b, "after the failed reload");
        let report = a.env.reload_program(a.sid, &fixed, None).unwrap();
        b.env.reload_program(b.sid, &fixed, None).unwrap();
        assert!(rebuilt(&report).contains(&"TILES"), "{:?}", report.statics);
        assert_eq!(run_frame(&mut a), run_frame(&mut b));
        assert_same_tables(&a, &b, "after the reload that compiled");
        // The same with a literal-only difference, and a runtime-only error.
        let broken = TABLES.replace("w: 5}", "w: 6}").replace("print(n, ", "print(n, (");
        let fixed = TABLES.replace("w: 5}", "w: 6}");
        for w in [&mut a, &mut b] {
            assert!(w.env.reload_program(w.sid, &broken, None).is_err());
            w.env.reload_program(w.sid, &fixed, None).unwrap();
        }
        assert_eq!(run_frame(&mut a), run_frame(&mut b));
        assert_same_tables(&a, &b, "after the second fix");
        assert_eq!(frame(&mut a), frame(&mut b));
    }
}

/// A value set by `set_config_value` is not what the text says. A reload
/// after it has to end where rebuilding everything ends.
#[test]
fn review_a_reload_after_a_config_value_was_set() {
    let edits = [
        DRAG.replace("n = n + 1", "n = n + 2"),
        DRAG.replace("[1, 2, 3]\n", "[1, 2, 3, 4]\n"),
        DRAG.replace("config let K = 2", "config let K = 9"),
        DRAG.replace("i * K", "i + K"),
        DRAG.replace("[1, 2, 3]\n", "[1, 2, 4]\n"),
        DRAG.to_string(),
    ];
    for policy in policies() {
        for edit in &edits {
            let mut a = world(DRAG, &[], policy, true);
            let mut b = world(DRAG, &[], policy, false);
            a.env.set_static_debounce(None);
            b.env.set_static_debounce(None);
            assert_eq!(frame(&mut a), frame(&mut b));
            for k in [5, 6, 7] {
                drag(&mut a, k);
                drag(&mut b, k);
                assert_eq!(frame(&mut a), frame(&mut b), "tick {k}");
                assert_same_tables(&a, &b, "tick");
            }
            a.env.reload_program(a.sid, edit, None).unwrap();
            b.env.reload_program(b.sid, edit, None).unwrap();
            assert_eq!(run_frame(&mut a), run_frame(&mut b), "{edit}");
            assert_same_tables(&a, &b, edit);
            drag(&mut a, 11);
            drag(&mut b, 11);
            assert_eq!(frame(&mut a), frame(&mut b), "a tick after\n{edit}");
            assert_same_tables(&a, &b, edit);
        }
    }
}

/// Names that come to mean something else with no change to the statement
/// that reads them.
#[test]
fn review_names_that_change_meaning() {
    // Two modules both imported already; the named import switches.
    let entry = "\
import a
import b
import a: BASE
let SCALED = map(BASE, fn(x) x * 2 end)
let BOTH = [a.BASE[0], b.BASE[0]]
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, SCALED, OWN, BASE, BOTH)
";
    let modules = [("a", "pub let BASE = [1, 2, 3]\n"), ("b", "pub let BASE = [7, 8, 9]\n")];
    let report = follow_modules(entry, &entry.replace("import a: BASE", "import b: BASE"), &modules, &modules);
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    // A rename, with the reader following it.
    let old = "\
let A = [1, 2, 3]
let B = [4, 5, 6]
let C = [A[0], B[0]]
let D = map(A, fn(x) x + B[1] end)
state n = 0
n = n + 1
print(n, A, B, C, D)
";
    follow(old, &old.replace("let A = [1, 2, 3]\nlet B = [4, 5, 6]", "let B = [1, 2, 3]\nlet A = [4, 5, 6]"));
    follow(old, &old.replace("let A = [1, 2, 3]", "let A = [1, 2, 3]\nlet B = [40, 50, 60]").replace("let B = [4, 5, 6]\n", ""));
    follow(old, &old.replace("let C", "let A = [9, 9, 9]\nlet C"));
    follow(old, &old.replace("let D", "let B = [9, 9, 9]\nlet D"));
    // A function declared twice; the later one is edited, removed, added.
    let twice = "\
fn f(x) x * 2 end
let T = [f(1), f(2)]
fn g(x) f(x) + 1 end
let U = [g(1), g(2)]
state n = 0
n = n + 1
print(n, T, U, f(n))
";
    follow(twice, &twice.replace("state n = 0", "fn f(x) x * 50 end\nstate n = 0"));
    follow(twice, &twice.replace("fn g(x) f(x) + 1 end", "fn g(x) f(x) + 1 end\nfn f(x) x * 50 end"));
    follow(twice, &twice.replace("fn f(x) x * 2 end\n", "").replace("let T", "fn f(x) x * 2 end\nlet T"));
    // A function reached only through a name held in a table.
    let held = "\
fn double(x) x * 2 end
fn triple(x) x * 3 end
let OPS = {d: double, t: triple}
let R = [OPS.d(1), OPS.t(1)]
let LIST = [double, triple]
let S = map(LIST, fn(f) f(10) end)
let OWN = [1]
state n = 0
n = n + 1
print(n, R, S, OWN, OPS.d(n), LIST[1](n))
";
    let report = follow(held, &held.replace("x * 3", "x * 4"));
    assert!(!rebuilt(&report).contains(&"OWN"), "{:?}", report.statics);
    follow(held, &held.replace("x * 3", "x + 4"));
    follow(held, &held.replace("fn triple(x) x * 3 end", "fn triple(x) x * 3 end\nfn triple(x, y) 0 end"));
    follow(held, &held.replace("fn double(x) x * 2 end\nfn triple(x) x * 3 end", "fn triple(x) x * 2 end\nfn double(x) x * 3 end"));
}

/// Methods of one name on several classes, and a class whose declaration
/// the edit reaches without touching the value that holds its instances.
#[test]
fn review_methods_and_classes() {
    let old = "\
class Circle
  r,
end
class Square
  s,
end
fn Circle.area(c) c.r * c.r * 3 end
fn Square.area(q) q.s * q.s end
fn Square.side(q) q.s end
fn total(shapes) reduce(shapes, 0, fn(a, s) a + s.area() end) end
let SHAPE_LIST = [Circle(1), Square(2), Circle(3)]
let TOTAL = [total(SHAPE_LIST)]
let SIDES = map([Square(4)], fn(q) q.side() end)
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, TOTAL, SIDES, OWN, SHAPE_LIST[n % 3].area(), SHAPE_LIST)
";
    for (from, to) in [
        ("q.s * q.s end", "q.s * q.s * 2 end"),
        ("q.s * q.s end", "q.s * q.s + 1 end"),
        ("c.r * c.r * 3", "c.r * c.r * 4"),
        ("fn Square.side(q) q.s end", "fn Square.side(q) q.s + 1 end"),
        ("fn Square.area(q) q.s * q.s end\n", ""),
        ("  s,\nend", "  s,\n  t,\nend"),
        ("fn Square.side(q) q.s end", "fn Square.side(q) q.s end\nfn Circle.side(c) c.r end"),
    ] {
        assert!(old.contains(from), "{from}");
        follow(old, &old.replace(from, to));
    }
}

/// Enum edits that do not touch the statement holding the variants.
#[test]
fn review_enums() {
    let old = "\
enum Kind
  park(shape: \"area\", cost: 3),
  road(shape: \"route\", cost: 9),
end
enum Mode
  on,
  off,
end
let ROW = [Kind.park, Kind.road, Kind.park]
let ALL = Kind.all
let COUNT = [len(Kind.all), len(Mode.all)]
let NAMES = map(Kind.all, fn(k) \"{k}\" end)
let MODES = [Mode.on, Mode.off]
let OWN = [1, 2, 3]
state n = 0
n = n + 1
print(n, ROW, ALL, COUNT, NAMES, MODES, OWN, ROW[n % 3].cost, ROW[n % 3] == Kind.park, MODES[n % 2] == Mode.on)
";
    for (from, to) in [
        ("  road(shape: \"route\", cost: 9),\n", "  road(shape: \"route\", cost: 9),\n  lake(shape: \"area\", cost: 1),\n"),
        ("  park(shape: \"area\", cost: 3),\n", "  lake(shape: \"area\", cost: 1),\n  park(shape: \"area\", cost: 3),\n"),
        ("park(shape: \"area\", cost: 3),\n  road(shape: \"route\", cost: 9),", "road(shape: \"route\", cost: 9),\n  park(shape: \"area\", cost: 3),"),
        ("  on,\n  off,\n", "  off,\n  on,\n"),
        ("  on,\n  off,\n", "  on,\n  off,\n  auto,\n"),
        ("cost: 3", "cost: 4"),
        ("\"area\"", "\"zone\""),
        ("enum Mode", "enum Extra\n  x,\n  y,\nend\nenum Mode"),
        ("enum Kind", "enum Extra\n  x(cost: 77),\n  y(cost: 78),\nend\nenum Kind"),
    ] {
        assert!(old.contains(from), "{from}");
        follow(old, &old.replace(from, to));
        follow(&old.replace(from, to), old);
    }
}

/// Edits in one module of several, twice in a row, with re-registration.
#[test]
fn review_modules_twice_in_a_row() {
    let entry = "\
import geo
import pal
import geo: scale
let SIDES = [scale(1), geo.scale(2)]
let DEEP = map(pal.RAMP, fn(r) r * geo.UNIT[0] end)
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, SIDES, pal.RAMP, OWN, geo.UNIT, DEEP, pal.tint(n))
";
    let geo = "pub let UNIT = [4, 4]\nfn helper(x) x * 10 end\npub fn scale(x) helper(x) + 1 end\n";
    let pal = "import geo\nlet HIDDEN = [1, 2]\npub let RAMP = map(geo.UNIT, fn(u) u + HIDDEN[0] end)\npub fn tint(x) x + HIDDEN[1] end\n";
    let steps: Vec<(String, String, String)> = vec![
        (entry.into(), geo.replace("x * 10", "x * 11"), pal.into()),
        (entry.into(), geo.replace("x * 10", "x * 11"), pal.replace("[1, 2]", "[5, 2]")),
        (entry.into(), geo.replace("x * 10", "x - 11"), pal.replace("[1, 2]", "[5, 2]")),
        (entry.replace("[[1], [2]]", "[[1], [3]]"), geo.replace("x * 10", "x - 11"), pal.replace("[1, 2]", "[5, 6, 7]")),
        (entry.into(), format!("// moved\n\n{geo}"), pal.into()),
        (entry.into(), format!("// moved\n\n{}", geo.replace("[4, 4]", "[4, 5]")), pal.into()),
        (entry.into(), format!("// moved\n\n{}", geo.replace("[4, 4]", "[4, 5]")), pal.replace("x + HIDDEN[1]", "x * HIDDEN[1]")),
        (entry.into(), geo.replace("fn helper(x) x * 10 end", "fn helper(x) x * 10 end\nfn helper(x, y) 0 end"), pal.into()),
        (entry.into(), geo.into(), pal.replace("let HIDDEN = [1, 2]", "let HIDDEN = [1, 2]\nlet UNIT = [0]")),
        (entry.into(), geo.into(), pal.into()),
    ];
    for policy in policies() {
        let old: [(&str, &str); 2] = [("geo", geo), ("pal", pal)];
        let mut a = world(entry, &old, policy, true);
        let mut b = world(entry, &old, policy, false);
        for (i, (e, g, p)) in steps.iter().enumerate() {
            assert_eq!(frame(&mut a), frame(&mut b), "before step {i}");
            for w in [&mut a, &mut b] {
                w.env.register_module("geo", g);
                w.env.register_module("pal", p);
                w.env.reload_program(w.sid, e, None).unwrap_or_else(|err| panic!("step {i}: {err}"));
            }
            assert_eq!(run_frame(&mut a), run_frame(&mut b), "after step {i}");
            assert_same_tables(&a, &b, &format!("the tables after step {i}"));
            assert_eq!(frame(&mut a), frame(&mut b), "a frame after step {i}");
            assert_same_tables(&a, &b, &format!("the tables, a frame after step {i}"));
        }
    }
}

/// Run every step of `steps` (entry text and module texts) through a
/// following world and a rebuilding one.
fn chain_modules(steps: &[(String, Vec<(&str, String)>)]) {
    for policy in policies() {
        let what = policy.name().unwrap_or_default();
        let first: Vec<(&str, &str)> = steps[0].1.iter().map(|(n, s)| (*n, s.as_str())).collect();
        let mut a = world(&steps[0].0, &first, policy, true);
        let mut b = world(&steps[0].0, &first, policy, false);
        for (i, (entry, modules)) in steps.iter().enumerate().skip(1) {
            assert_eq!(frame(&mut a), frame(&mut b), "[{what}] before step {i}");
            let mut ok = (true, true);
            for (w, ok) in [(&mut a, &mut ok.0), (&mut b, &mut ok.1)] {
                for (name, source) in modules {
                    w.env.register_module(name, source);
                }
                *ok = match w.env.reload_program(w.sid, entry, None) {
                    Ok(_) => true,
                    Err(e) => panic!("[{what}] step {i} does not compile: {e}\n{entry}\n{modules:#?}"),
                };
            }
            assert_eq!(ok.0, ok.1, "[{what}] step {i} compiles for both or neither");
            assert_eq!(run_frame(&mut a), run_frame(&mut b), "[{what}] after step {i}\n{entry}\n{modules:#?}");
            assert_same_tables(&a, &b, &format!("[{what}] the tables after step {i}\n{entry}\n{modules:#?}"));
            assert_eq!(frame(&mut a), frame(&mut b), "[{what}] a frame after step {i}");
            assert_same_tables(&a, &b, &format!("[{what}] the tables, a frame after step {i}"));
        }
    }
}

/// The module system's tiers: a name that resolves somewhere else after an
/// edit to a file the reader is not in.
#[test]
fn review_resolution_through_modules() {
    let base = "pub let BASE = [1, 2, 3]\npub fn button(x) x * 2 end\npub fn score(p) p + 1 end\n";
    let kit = "import base\npub let EXTRA = [5]\npub fn knob(x) x * 3 end\n";
    let other = "pub let BASE = [7, 8, 9]\npub let SPARE = [0]\n";
    let facade = "pub import base: BASE\npub import other: SPARE\n";
    let entry = "\
import base
import kit
import other as o
import facade as f
let PRESSED = [button(1), button(2)]
let SCORES = [score(1), score(2)]
let VIA = map(f.BASE, fn(x) x * 2 end)
let CLAMPED = [clamp01(2), clamp01(0 - 1)]
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, PRESSED, SCORES, VIA, CLAMPED, OWN, o.BASE, EXTRA)
";
    let m = |b: &str, k: &str, o: &str, f: &str| -> Vec<(&'static str, String)> {
        vec![("base", b.to_string()), ("kit", k.to_string()), ("other", o.to_string()), ("facade", f.to_string())]
    };
    let start = (entry.to_string(), m(base, kit, other, facade));
    let variants = [
        // The importer's export shadows the export of what it imports.
        (entry.to_string(), m(base, &format!("{kit}pub fn button(x) x * 100 end\n"), other, facade)),
        // A second arity from another module.
        (entry.to_string(), m(base, &format!("{kit}pub fn score(p, q) p * q end\n"), other, facade)),
        // A module export over the prelude's.
        (entry.to_string(), m(base, &format!("{kit}pub fn clamp01(x) x * 1000 end\n"), other, facade)),
        // The facade passes on another module's name.
        (entry.to_string(), m(base, kit, other, "pub import other: BASE\npub import other: SPARE\n")),
        (entry.to_string(), m(base, kit, other, "pub import base\npub import other: SPARE\npub let BASE = [100, 200]\n")),
        (entry.to_string(), m(base, kit, other, "pub import other\n")),
        // A selective import over the plain one.
        (entry.replace("import facade as f\n", "import facade as f\nimport kit: knob\nimport other: SPARE\n"), m(base, kit, other, facade)),
        (entry.replace("let PRESSED", "fn button(x) 0 - x end\nlet PRESSED"), m(base, kit, other, facade)),
        // What the module's function calls, privately.
        (entry.to_string(), m(&base.replace("x * 2", "x * 2 + len(BASE)"), kit, other, facade)),
        (entry.to_string(), m(&base.replace("[1, 2, 3]", "[1, 2, 4]"), kit, other, facade)),
        (entry.to_string(), m(&base.replace("p + 1", "p + 2"), kit, other, facade)),
    ];
    for v in &variants {
        chain_modules(&[start.clone(), v.clone(), start.clone()]);
    }
    // And all of them one after another.
    let mut all = vec![start.clone()];
    all.extend(variants.iter().cloned());
    all.push(start);
    chain_modules(&all);
}

/// A plain `let` or `const` literal that other constants read, however the
/// compiler chose to read it.
#[test]
fn review_a_plain_literal_others_read() {
    for decl in ["let K = 2", "const K = 2", "config let K = 2"] {
        let old = format!("\
{decl}
let NAME = \"ab\"
const HALF = 0.5
let T = [K * 3, K, K + 1]
let U = map([1, 2, 3], fn(x) x * K end)
fn f(x) x + K end
let V = [f(1), f(2)]
let W = [\"{{NAME}}!\", NAME ++ NAME]
let X = [HALF * 2.0, HALF]
let Y = [T[0] + V[0], len(W)]
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, K, T, U, V, W, X, Y, OWN, NAME, HALF)
");
        for (from, to) in [
            (decl, decl.replace("= 2", "= 3")),
            (decl, decl.replace("= 2", "= 1 + 1")),
            (decl, decl.replace("= 2", "= 5 - 1")),
            ("\"ab\"", "\"cd\"".to_string()),
            ("\"ab\"", "\"a\" ++ \"x\"".to_string()),
            ("const HALF = 0.5", "const HALF = 0.25".to_string()),
            ("const HALF = 0.5", "let HALF = 0.75".to_string()),
        ] {
            assert!(old.contains(from));
            let new = old.replace(from, &to);
            let report = follow(&old, &new);
            assert!(!rebuilt(&report).contains(&"OWN"), "{from} -> {to}: {:?}", report.statics);
            follow(&new, &old);
        }
    }
}

/// Values taken apart by a match, and names bound more than once.
#[test]
fn review_patterns_and_rebinding() {
    let old = "\
let PAIR = [[1, 2], [3, 4]]
let LEFT = match PAIR
  when [l, ...rest] -> l
  when [] -> [0]
end
let RIGHT = match {a: PAIR[1], b: [7, 8]}
  when {a: r, b: [x, y]} -> [r[0], x + y]
  when _ -> [0]
end
let SUMS = [LEFT[0] + RIGHT[0], RIGHT[1]]
let T = [1, 2]
let FIRST = [T[0], T[1]]
let T = [10, 20]
let SECOND = [T[0], T[1]]
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, PAIR, LEFT, RIGHT, SUMS, FIRST, SECOND, OWN)
";
    for (from, to) in [
        ("[[1, 2], [3, 4]]", "[[1, 2], [3, 5]]"),
        ("[[1, 2], [3, 4]]", "[[9, 2], [3, 4], [0]]"),
        ("when [l, ...rest] -> l", "when [l, ...rest] -> rest[0]"),
        ("b: [7, 8]}", "b: [7, 9]}"),
        ("when {a: r, b: [x, y]} -> [r[0], x + y]", "when {a: r, b: [x, y]} -> [r[1], x * y]"),
        ("let T = [1, 2]", "let T = [1, 3]"),
        ("let T = [10, 20]", "let T = [10, 30]"),
        ("let T = [10, 20]\n", ""),
        ("let T = [1, 2]", "let T = [1, 2, 3]"),
        ("let SECOND", "let T = [100, 200]\nlet SECOND"),
        // A reader moved across the second binding, and the binding across the reader.
        ("let FIRST = [T[0], T[1]]\nlet T = [10, 20]\n", "let T = [10, 20]\nlet FIRST = [T[0], T[1]]\n"),
        ("let T = [10, 20]\nlet SECOND = [T[0], T[1]]\n", "let SECOND = [T[0], T[1]]\nlet T = [10, 20]\n"),
    ] {
        assert!(old.contains(from), "{from}");
        follow(old, &old.replace(from, to));
        follow(&old.replace(from, to), old);
    }
}

/// A drag that puts rebuilds off, with every other kind of reload arriving
/// in the middle of it. While values wait the two worlds differ, by
/// design; once the waiting ones are flushed nothing may be left that a
/// rebuild of everything would not have.
#[test]
fn review_a_drag_interrupted_by_every_kind_of_reload() {
    let src = "\
config let K = 2
config let J = 10
let BIG = map([1, 2, 3, 4], fn(i) i * K end)
let NEXT = [BIG[0] + J, BIG[1]]
let LAST = [NEXT[0] * 2, J]
let SIDE = [J, J + 1]
let OTHER = [1, 2, 3]
state n = 0
n = n + 1
print(n, K, J, BIG, NEXT, LAST, SIDE, OTHER, [K, J, 5])
";
    let every = StaticDebounce { threshold: std::time::Duration::ZERO, quiet: std::time::Duration::from_secs(600) };
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut waited = 0;
    for policy in policies() {
        let what = policy.name().unwrap_or_default();
        for round in 0..40 {
            let mut a = world(src, &[], policy, true);
            let mut b = world(src, &[], policy, false);
            a.env.set_static_debounce(Some(every));
            b.env.set_static_debounce(None);
            assert_eq!(frame(&mut a), frame(&mut b));
            let mut log = Vec::new();
            for _ in 0..3 {
                for _ in 0..(2 + next(6)) {
                    let op = next(9);
                    let v = next(50) as i64;
                    log.push((op, v));
                    for w in [&mut a, &mut b] {
                        let text = w.env.program_source(w.pid, None).unwrap().to_string();
                        match op {
                            0..=3 => {
                                let name = if op == 3 { "J" } else { "K" };
                                let report = w.env.set_config_value(w.sid, None, name, &StaticValue::Int(v)).unwrap();
                                waited += report.statics.map_or(0, |s| s.deferred.len());
                            }
                            4 => {
                                let new = if text.contains("n + 1 + 0") {
                                    text.replace("n + 1 + 0", "n + 1")
                                } else {
                                    text.replace("n = n + 1", "n = n + 1 + 0")
                                };
                                w.env.reload_program(w.sid, &new, None).unwrap();
                            }
                            5 => {
                                // One number of a table nothing else reads.
                                let at = text.find("let OTHER = [1, ").unwrap() + "let OTHER = [1, ".len();
                                let end = at + text[at..].find(',').unwrap();
                                let new = format!("{}{}{}", &text[..at], 100 + v, &text[end..]);
                                w.env.reload_program(w.sid, &new, None).unwrap();
                            }
                            6 => {
                                assert!(w.env.reload_program(w.sid, &format!("{text}let BROKEN = [\n"), None).is_err());
                            }
                            7 => {
                                let new = match text.strip_prefix("// moved\n") {
                                    Some(rest) => rest.to_string(),
                                    None => format!("// moved\n{text}"),
                                };
                                w.env.reload_program(w.sid, &new, None).unwrap();
                            }
                            _ => {
                                // The table by shape, with whatever K is.
                                let new = if text.contains("i * K end") {
                                    text.replace("i * K end", "i * K + 1 end")
                                } else {
                                    text.replace("i * K + 1 end", "i * K end")
                                };
                                w.env.reload_program(w.sid, &new, None).unwrap();
                            }
                        }
                        // A frame after every event, as a host draws one.
                        frame(w);
                    }
                }
                a.env.flush_static_rebuilds(a.sid);
                assert_eq!(
                    a.env.program_source(a.pid, None),
                    b.env.program_source(b.pid, None),
                    "[{what}] round {round}: {log:?}"
                );
                assert_eq!(frame(&mut a), frame(&mut b), "[{what}] round {round}: {log:?}");
                assert_same_tables(&a, &b, &format!("[{what}] round {round}: {log:?}"));
                assert_eq!(frame(&mut a), frame(&mut b), "[{what}] round {round}, a frame later: {log:?}");
            }
        }
    }
    assert!(waited > 200, "the drags put rebuilds off: {waited}");
}

/// Two stacks on one program, and a fork: the reload is asked of one of
/// them, and every one of them must run the new program.
#[test]
fn review_other_stacks_and_forks_see_the_new_program() {
    let edits = [
        TABLES.replace("w: 5}", "w: 6}"),
        TABLES.replace("w: 5}", "w: 5}, {id: 4, w: 9}"),
        TABLES.replace("t.w * 2", "t.w * 3"),
        TABLES.replace("t.w * 2", "t.w + 3"),
        TABLES.replace("n = n + 1", "n = n + 2"),
    ];
    for policy in policies() {
        for edit in &edits {
            let mut a = world(TABLES, &[], policy, true);
            let mut b = world(TABLES, &[], policy, false);
            let mut out = Vec::new();
            for w in [&mut a, &mut b] {
                let second = w.env.create_stack(w.pid).unwrap();
                let first = w.sid;
                let mut log = Vec::new();
                for sid in [first, second, first, second] {
                    w.sid = sid;
                    log.push(frame(w));
                }
                w.sid = first;
                let fork = w.env.fork_execution(first).unwrap();
                w.env.reload_program(first, edit, None).unwrap();
                for sid in [second, first, fork, second, first] {
                    w.sid = sid;
                    log.push(frame(w));
                    log.push(format!("{:?}", slots(w)));
                }
                w.sid = first;
                out.push(log);
            }
            assert_eq!(out[0], out[1], "{edit}");
        }
    }
}

/// One object two constants share, frozen through one of them. A rebuilt
/// value is only swapped for the old one when `frozen` answers the same for
/// both, so the share is not broken by the swap.
#[test]
fn review_a_shared_element_frozen_through_one_name() {
    let old = "\
let A = [[1], [2]]
let B = A[0]
let C = [B, A[1]]
state n = 0
n = n + 1
if n >= 2 then freeze(B) end
print(n, frozen(B), frozen(A[0]), frozen(A), frozen(C), frozen(C[0]), A, B, C)
";
    follow(old, &old.replace("[[1], [2]]", "[[1], [3]]"));
    follow(old, &old.replace("[[1], [2]]", "[[1], [2], [3]]"));
    follow(old, &old.replace("n = n + 1", "n = n + 2"));
}

/// Method dispatch and enums span the whole program: a method or a variant
/// added in a module reaches a constant of a file that does not import it.
#[test]
fn review_methods_and_enums_declared_in_other_modules() {
    let shapes = "pub class Circle\n  r,\nend\npub class Square\n  s,\nend\npub fn Circle.area(c) c.r * c.r * 3 end\n";
    let extra = "import shapes\npub let NOTHING = [0]\n";
    let kinds = "pub enum Kind\n  park(cost: 3),\n  road(cost: 9),\nend\n";
    let entry = "\
import shapes
import extra
import kinds
let LIST = [Circle(1), Circle(2)]
let AREAS = map(LIST, fn(s) s.area() end)
let ROW = [Kind.park, Kind.road]
let COSTS = map(ROW, fn(k) k.cost end)
let COUNT = [len(Kind.all)]
let OWN = [[1], [2]]
state n = 0
n = n + 1
print(n, AREAS, COSTS, COUNT, OWN, NOTHING, LIST[n % 2].area(), ROW[n % 2].cost)
";
    let m = |s: &str, e: &str, k: &str| -> Vec<(&'static str, String)> {
        vec![("shapes", s.to_string()), ("extra", e.to_string()), ("kinds", k.to_string())]
    };
    let start = (entry.to_string(), m(shapes, extra, kinds));
    for v in [
        m(shapes, &format!("{extra}fn Square.area(q) q.s end\n"), kinds),
        m(shapes, &format!("{extra}fn Circle.area(c, k) 0 - c.r * k end\n"), kinds),
        m(&shapes.replace("c.r * c.r * 3", "c.r * c.r * 4"), extra, kinds),
        m(&shapes.replace("c.r * c.r * 3", "c.r + 4"), extra, kinds),
        m(&shapes.replace("  r,\n", "  r,\n  tag,\n"), extra, kinds),
        m(shapes, extra, &kinds.replace("cost: 9", "cost: 10")),
        m(shapes, extra, &kinds.replace("  road(cost: 9),\n", "  road(cost: 9),\n  lake(cost: 1),\n")),
        m(shapes, extra, &kinds.replace("  park(cost: 3),\n  road(cost: 9),\n", "  road(cost: 9),\n  park(cost: 3),\n")),
        m(shapes, &format!("{extra}enum Fee\n  toll(cost: 50),\nend\n"), kinds),
    ] {
        chain_modules(&[start.clone(), (entry.to_string(), v), start.clone()]);
    }
}

/// A literal patched through one stack leaves the table of every other
/// stack of the program holding values of the old literal, stamped with
/// the old constant revision. A reload asked of such a stack before it has
/// run again has nothing it may keep.
#[test]
fn review_a_table_another_stacks_reload_made_stale_is_not_kept() {
    for policy in policies() {
        let what = policy.name().unwrap_or_default();
        for second_edit in ["\"b\"", "n = n + 1", "t.w * 2"] {
            let mut out = Vec::new();
            for follow in [true, false] {
                let mut w = world(TABLES, &[], policy, follow);
                let first = w.sid;
                let second = w.env.create_stack(w.pid).unwrap();
                let mut log = Vec::new();
                for sid in [first, second] {
                    w.sid = sid;
                    log.push(frame(&mut w));
                }
                // One literal through the first stack, another edit through
                // the second with no frame of it in between.
                let one = TABLES.replace("{id: 2, w: 3}", "{id: 2, w: 40}");
                w.env.reload_program(first, &one, None).unwrap();
                let two = match second_edit {
                    "\"b\"" => one.replace("\"b\"", "\"B\""),
                    "n = n + 1" => one.replace("n = n + 1", "n = n + 2"),
                    _ => one.replace("t.w * 2", "t.w * 3"),
                };
                w.env.reload_program(second, &two, None).unwrap();
                for sid in [second, first, second] {
                    w.sid = sid;
                    log.push(frame(&mut w));
                    log.push(format!("{:?}", slots(&w)));
                }
                // The same through a fork and `set_config_value`.
                let mut w = world(DRAG, &[], policy, follow);
                w.env.set_static_debounce(None);
                log.push(frame(&mut w));
                let fork = w.env.fork_execution(w.sid).unwrap();
                drag(&mut w, 7);
                let text = w.env.program_source(w.pid, None).unwrap().replace("[1, 2, 3]\n", "[1, 2, 4]\n");
                w.env.reload_program(fork, &text, None).unwrap();
                for sid in [fork, w.sid, fork] {
                    let own = std::mem::replace(&mut w.sid, sid);
                    log.push(frame(&mut w));
                    log.push(format!("{:?}", slots(&w)));
                    w.sid = own;
                }
                out.push(log);
            }
            assert_eq!(out[0], out[1], "[{what}] {second_edit}");
        }
    }
}

// --- a store per value ------------------------------------------------------

const COUNT: &str = "state n = 0\nn = n + 1\n";

/// A value rebuilt beside a kept one it holds takes the kept object, not a
/// copy made for the occasion: `freeze` through one name reaches the other,
/// as it does when everything is rebuilt.
#[test]
fn a_rebuilt_value_holds_its_kept_neighbour_itself() {
    let old = format!(
        "let B = [1, 2]\nlet C = [B, 1]\nlet D = [B, [7]]\n{COUNT}\
         if n >= 2 then freeze(B) end\n\
         print(n, frozen(B), frozen(C[0]), frozen(D[0]), frozen(C), B, C, D)\n"
    );
    let report = follow(&old, &old.replace("[B, 1]", "[B, 2]"));
    assert_eq!(rebuilt(&report), ["C"]);
    let report = follow(&old, &old.replace("[B, [7]]", "[B, [8], 9]"));
    assert_eq!(rebuilt(&report), ["D"]);
    // The neighbour itself rebuilt: everything that holds it goes with it.
    let report = follow(&old, &old.replace("[1, 2]", "[1, 3]"));
    assert_eq!(rebuilt(&report), ["B", "C", "D"]);
}

/// A table nothing but other tables read is kept as well, so two values
/// built from it go on sharing it when one of them is rebuilt; with dynamic
/// code between them and without.
#[test]
fn a_table_two_values_are_built_from_stays_shared() {
    for between in ["", "print(\"between\")\n"] {
        let old = format!(
            "let P = [1, 2]\n{between}let A = [P, 1]\n{between}let B = [P, 2]\n{COUNT}\
             if n >= 2 then freeze(A[0]) end\n\
             print(n, frozen(A[0]), frozen(B[0]), frozen(A), A, B)\n"
        );
        let report = follow(&old, &old.replace("[P, 2]", "[P, 3]"));
        assert_eq!(rebuilt(&report), ["B"], "{between:?}");
        follow(&old, &old.replace("[P, 1]", "[P, [4]]"));
        follow(&old, &old.replace("[1, 2]", "[1, 2, 3]"));
        // A third reader appears: the shared table is still the one object.
        follow(
            &old,
            &old.replace("state n", "let E = [P, 5]\nstate n").replace("frozen(A), A", "frozen(E[0]), A"),
        );
    }
    // The table becomes shared by the edit: it was part of `A` alone.
    let old = format!(
        "let P = [1, 2]\nlet A = [P, 1]\n{COUNT}if n >= 2 then freeze(A[0]) end\nprint(n, frozen(A[0]), A)\n"
    );
    let new = old
        .replace("state n", "let B = [P, 2]\nstate n")
        .replace("print(n, frozen(A[0]), A)", "print(n, frozen(A[0]), frozen(B[0]), A, B)");
    follow(&old, &new);
    follow(&new, &old);
}

/// A rebuilt value that equals the old one is not swapped for it when it
/// holds a kept object the old one does not: here `D` is rebuilt to a new
/// object, part of it is frozen before `X` is built, and by then the new
/// `X` and the old one answer `frozen` alike all the way down.
#[test]
fn an_equal_value_holding_another_object_is_not_swapped_for_the_old_one() {
    let old = format!(
        "fn mk(x) [x, 1] end\nfn mk2(x) [x] end\n{COUNT}\
         let D = [mk2(5), 0]\n\
         if n >= 2 then freeze(D[0]) end\n\
         let X = [mk(D), 0]\n\
         print(n, frozen(D), frozen(X[0][0]), frozen(D[0]), D, X)\n\
         if n >= 4 then freeze(D) end\n"
    );
    let new = old.replace("[x, 1]", "[x, 2 - 1]").replace("[x] end", "[x + 0] end");
    let mut out = Vec::new();
    for on in [true, false] {
        let mut w = world(&old, &[], policies()[0], on);
        let mut log = vec![frame(&mut w), frame(&mut w)];
        w.env.reload_program(w.sid, &new, None).unwrap();
        for _ in 0..4 {
            log.push(frame(&mut w));
        }
        out.push(log);
    }
    assert_eq!(out[0], out[1]);
    assert!(out[0][5].starts_with("6 true true true"), "{}", out[0][5]);
}

/// One rebuilt value leaves the others of its stretch alone: the frame
/// after the reload builds that value and nothing else.
#[test]
fn one_rebuilt_value_does_not_rebuild_its_neighbours() {
    let old = format!(
        "let A = [[1], [2]]\nlet B = [[3], [4]]\nlet C = [[5], [6]]\nlet D = [A, [0]]\n{COUNT}print(n, A, B, C, D)\n"
    );
    let mut w = world(&old, &[], policies()[0], true);
    frame(&mut w);
    frame(&mut w);
    let before = w.env.static_values(w.sid).unwrap();
    assert_eq!((before.runs, before.segments), (1, 4), "one check a frame, four values stored alone");
    let report = w.env.reload_program(w.sid, &old.replace("[[3], [4]]", "[[3], [4, 4]]"), None).unwrap();
    assert_eq!(rebuilt(&report), ["B"]);
    let a = held(&w, "A");
    assert!(frame(&mut w).starts_with("3 [[1], [2]] [[3], [4, 4]] [[5], [6]] [[[1], [2]], [0]]"));
    let after = w.env.static_values(w.sid).unwrap();
    assert_eq!(after.stats.stored - before.stats.stored, 1, "one value was built");
    assert_eq!(after.stats.skips - before.stats.skips, 3, "the other three segments were skipped");
    assert_eq!(held(&w, "A"), a);
}

/// A dragged value inside an expression does not wait with a slow table
/// that stands beside it. A table that only exists to build another is
/// timed either way: with the value it is built for when nothing else reads
/// it, and on its own when the reader stands in another stretch.
#[test]
fn a_dragged_expression_does_not_wait_with_its_slow_neighbour() {
    for (between, waits, rebuilt_at_once) in [("", "HEAD", "SCALED"), ("print(\"between\")\n", "BIG", "HEAD")] {
        let src = format!(
            "config let K = 2\nlet SCALED = [K * 3, [K]]\n\
             let BIG = map(range(0, 20000), fn(i) i * K + K end)\n{between}\
             let HEAD = [BIG[1], [BIG[2]]]\n{COUNT}print(n, SCALED, HEAD)\n"
        );
        let slow = StaticDebounce {
            threshold: std::time::Duration::from_micros(300),
            quiet: std::time::Duration::from_secs(60),
        };
        let mut a = world(&src, &[], policies()[0], true);
        let mut b = world(&src, &[], policies()[0], false);
        a.env.set_static_debounce(Some(slow));
        let mut last = None;
        for k in [3, 4, 5] {
            frame(&mut a);
            frame(&mut b);
            last = drag(&mut a, k).statics;
            drag(&mut b, k);
        }
        let last = last.unwrap();
        assert_eq!(last.deferred, [waits], "{last:?}");
        assert!(last.rebuilt_names.contains(&"SCALED".to_string()), "{last:?}");
        assert!(last.rebuilt_names.contains(&rebuilt_at_once.to_string()), "{last:?}");
        let during = frame(&mut a);
        frame(&mut b);
        assert!(during.contains("[15, [5]]"), "the dragged value at once: {during}");
        assert!(a.env.flush_static_rebuilds(a.sid) >= 1);
        assert_eq!(frame(&mut a), frame(&mut b), "after the drag paused");
        assert_same_tables(&a, &b, "after the drag paused");
    }
}

/// Tables that hold one another, frozen through arbitrary names at
/// arbitrary frames after one of them was edited: which objects are shared is
/// the same after the reload that follows the edit as after the one that
/// rebuilds everything, as far as `frozen` can see.
#[test]
fn sharing_is_the_same_as_after_a_rebuild_of_everything() {
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }
    let iters: u64 = std::env::var("PETAL_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    for seed in 0..iters {
        let mut rng = Rng(seed * 7919 + 1);
        let count = 3 + rng.below(4) as usize;
        // One table: its elements are numbers, small lists of its own, and
        // earlier tables.
        let table = |rng: &mut Rng, i: usize| -> String {
            let mut parts = Vec::new();
            for _ in 0..1 + rng.below(3) {
                parts.push(match rng.below(4) {
                    0 => format!("{}", rng.below(5)),
                    1 => format!("[{}]", rng.below(5)),
                    _ if i > 0 => format!("V{}", rng.below(i as u64)),
                    _ => format!("[{}, {}]", rng.below(5), rng.below(5)),
                });
            }
            format!("[{}]", parts.join(", "))
        };
        let mut values: Vec<String> = (0..count).map(|i| table(&mut rng, i)).collect();
        let between: Vec<&str> = (0..count)
            .map(|_| match rng.below(4) {
                0 => "print(\"-\")\n",
                1 => "fn pass(x) x end\n",
                _ => "",
            })
            .collect();
        // Some tables only other tables read: they have a slot when two
        // of those are stored apart, and none when one is, which an edit
        // can change.
        let named: Vec<usize> = (0..count).filter(|_| rng.below(3) > 0).collect();
        let named = if named.is_empty() { vec![count - 1] } else { named };
        let mut tail = String::from(COUNT);
        for _ in 0..2 + rng.below(3) {
            // After the reload, which follows the second frame: a table
            // frozen before it stays frozen when it is kept and is new when
            // it is rebuilt, which is the difference between any two runs
            // and not what this is about.
            let (v, at) = (named[rng.below(named.len() as u64) as usize], 3 + rng.below(3));
            tail.push_str(&match rng.below(2) {
                0 => format!("if n >= {at} then freeze(V{v}) end\n"),
                _ => format!("if n >= {at} then freeze(V{v}[0]) end\n"),
            });
        }
        let shown: Vec<String> = named
            .iter()
            .flat_map(|i| [format!("frozen(V{i})"), format!("frozen(V{i}[0])"), format!("V{i}")])
            .collect();
        tail.push_str(&format!("print(n, {})\n", shown.join(", ")));
        let text = |values: &[String]| -> String {
            let mut out = String::from("fn same(x) x end\n");
            for (i, v) in values.iter().enumerate() {
                out.push_str(&format!("let V{i} = {v}\n{}", between[i].replace("pass", &format!("pass{i}"))));
            }
            out + &tail
        };
        let old = text(&values);
        // The edit: another table in one place, or the same one written
        // another way.
        let i = rng.below(count as u64) as usize;
        values[i] = match rng.below(3) {
            0 => format!("same({})", values[i]),
            1 => format!("[{}, {}]", &values[i][1..values[i].len() - 1], rng.below(5)),
            _ => table(&mut rng, i),
        };
        let new = text(&values);
        follow(&old, &new);
        follow(&new, &old);
    }
}

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
    format!("{out}\n=> {result}\nstate {state}")
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
                    petal::value::value_to_display_string(v, w.env.heap())
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
    for policy in policies() {
        let what = policy.name().unwrap_or_default();
        let mut a = world(old, old_modules, policy, true);
        let mut b = world(old, old_modules, policy, false);
        for _ in 0..2 {
            assert_eq!(frame(&mut a), frame(&mut b), "[{what}] before the edit");
        }
        for w in [&mut a, &mut b] {
            for (name, source) in new_modules {
                w.env.register_module(name, source);
            }
        }
        let report = a
            .env
            .reload_program(a.sid, new, None)
            .unwrap_or_else(|e| panic!("[{what}] reload: {e}"));
        let plain = b.env.reload_program(b.sid, new, None).unwrap();
        assert_eq!(report.outcome, plain.outcome, "[{what}] outcome");
        assert!(plain.statics.is_none(), "[{what}] the oracle follows nothing");
        assert_eq!(run_frame(&mut a), run_frame(&mut b), "[{what}] the run after the reload\n{new}");
        assert_same_tables(&a, &b, &format!("[{what}] the tables after the reload\n{new}"));
        for i in 0..2 {
            assert_eq!(frame(&mut a), frame(&mut b), "[{what}] frame {i} after the reload");
            assert_same_tables(&a, &b, &format!("[{what}] the tables, frame {i}"));
        }
        first.get_or_insert(report);
    }
    first.unwrap()
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
print(n, grid, sum, plain, TILES, OTHER)
";
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

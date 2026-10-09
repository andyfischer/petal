// `petal apply-change convert-to-var` — turning a `let` into a `var` and a
// `state` into a `state var`, everywhere the binding is mentioned
// (src/apply_change/, docs/CLI.md).
//
// The path syntax has its own unit tests in `src/apply_change/target.rs`.
// These cover the operation end to end: which mentions are rewritten and
// which are left alone, the importers it follows a `pub` binding into, what
// it refuses, and that nothing is written unless the result compiles.

use std::path::{Path, PathBuf};
use std::process::Command;

use petal::apply_change::{ChangeOptions, Plan, Request, plan};
use petal::env::Env;
use petal::typecheck::globals::HostProfile;

/// Path to the freshly built `petal` binary for this test run.
const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A scratch directory, emptied on creation.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let root =
            std::env::temp_dir().join(format!("petal-apply-change-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // The scan for importers compares canonical paths; hand them out.
        Dir(std::fs::canonicalize(&root).unwrap())
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, source).unwrap();
        path
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options(from: &[&Path]) -> ChangeOptions {
    ChangeOptions {
        include_dirs: Vec::new(),
        from: from.iter().map(|p| p.to_path_buf()).collect(),
        host: HostProfile::Core,
    }
}

fn convert(file: &Path, target: &str, from: &[&Path]) -> Result<Plan, String> {
    plan(
        file,
        &Request::ConvertToVar {
            target: target.to_string(),
        },
        &options(from),
    )
}

/// Convert `target` in a one-file program and return the new text.
fn converted(tag: &str, source: &str, target: &str) -> String {
    let dir = Dir::new(tag);
    let file = dir.write("main.ptl", source);
    let plan = convert(&file, target, &[]).unwrap_or_else(|e| panic!("refused: {e}"));
    plan.after(&file)
        .expect("the target file changes")
        .to_string()
}

/// The refusal for converting `target` in a one-file program.
fn refusal(tag: &str, source: &str, target: &str) -> String {
    let dir = Dir::new(tag);
    let file = dir.write("main.ptl", source);
    let err = convert(&file, target, &[]).expect_err("should refuse");
    assert_eq!(dir.read("main.ptl"), source, "a refusal must not write");
    err
}

/// Run the program at `path` and return what it printed.
fn run(path: &Path) -> Vec<String> {
    let mut env = Env::new();
    let source = std::fs::read_to_string(path).unwrap();
    let pid = env
        .load_program_at(&source, path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

/// Run `source` as a one-file program.
fn run_source(tag: &str, source: &str) -> Vec<String> {
    let dir = Dir::new(tag);
    run(&dir.write("main.ptl", source))
}

// ---------------------------------------------------------------------------
// The rewrite
// ---------------------------------------------------------------------------

#[test]
fn a_let_becomes_a_var_and_its_assignments_become_set() {
    let src = "\
fn evens(xs)
  let out = []   // collected so far
  for x in xs do
    if x % 2 == 0 then
      out = append(out, x)
    end
  end
  out
end
print(evens([1, 2, 3, 4]))
";
    let got = converted("let", src, "evens/out");
    assert_eq!(
        got,
        src.replace("let out", "var out")
            .replace("      out = ", "      set out = ")
    );
    // No function boundary is crossed, so nothing about the program changed.
    assert_eq!(run_source("let-run", &got), run_source("let-run0", src));
}

#[test]
fn a_state_becomes_a_state_var() {
    let src = "state hits = 0\nhits = hits + 1\nprint(hits)\n";
    assert_eq!(
        converted("state", src, "/hits"),
        "state var hits = 0\nset hits = hits + 1\nprint(hits)\n"
    );
}

#[test]
fn the_var_goes_after_a_state_key_and_before_an_annotation() {
    let src = "for i in [1, 2] do\n  state(str((i))) n: int = 0\n  n += i\n  print(n)\nend\n";
    let got = converted("state-key", src, "n");
    assert_eq!(
        got,
        "for i in [1, 2] do\n  state(str((i))) var n: int = 0\n  set n += i\n  print(n)\nend\n"
    );
    assert_eq!(run_source("state-key-run", &got), ["1", "2"]);
}

#[test]
fn field_index_and_compound_writes_all_take_set() {
    let src = "\
let r = {a: 1, xs: [1, 2]}
r.a = 2
r.xs[0] = 5
r.a += 1
r.xs[len(r.xs) - 1] *= 3
print(r)
";
    let got = converted("targets", src, "/r");
    assert_eq!(
        got,
        "\
var r = {a: 1, xs: [1, 2]}
set r.a = 2
set r.xs[0] = 5
set r.a += 1
set r.xs[len(r.xs) - 1] *= 3
print(r)
"
    );
    assert_eq!(
        run_source("targets-run", &got),
        run_source("targets-run0", src)
    );
}

#[test]
fn reads_inside_nested_functions_take_get() {
    // The original does not compile: `award` assigns a binding from outside
    // itself. Converting the binding is the fix for exactly that error.
    let src = "\
state score = 0
fn award(n)
  score = score + n
  score += 1
  print(\"now {score}\")
end
let peek = fn(bonus = score) bonus + score end
award(2)
print(peek())
";
    let got = converted("nested", src, "/score");
    assert_eq!(
        got,
        "\
state var score = 0
fn award(n)
  set score = get score + n
  set score += 1
  print(\"now {get score}\")
end
let peek = fn(bonus = get score) bonus + get score end
award(2)
print(peek())
"
    );
    assert_eq!(run_source("nested-run", &got), ["now 3", "6"]);
}

#[test]
fn an_index_inside_a_set_target_is_a_read_but_its_root_is_not() {
    let src = "\
fn tally()
  let xs = [1, 2, 3]
  let bump = fn(i)
    xs[i] = xs[i] + 1
    xs[len(xs) - 1] += 10
  end
  bump(0)
  xs
end
print(tally())
";
    let got = converted("index", src, "tally/xs");
    assert!(got.contains("    set xs[i] = get xs[i] + 1\n"), "{got}");
    assert!(got.contains("    set xs[len(get xs) - 1] += 10\n"), "{got}");
    assert_eq!(run_source("index-run", &got), ["[2, 2, 13]"]);
}

#[test]
fn reads_in_the_declaring_scope_stay_bare() {
    let src = "\
fn total(xs)
  let sum = 0
  for x in xs do
    if x > 0 then
      sum = sum + x
    end
  end
  let doubled = sum * 2
  \"{sum} {doubled}\"
end
print(total([1, 2]))
";
    let got = converted("bare", src, "total/sum");
    assert!(!got.contains("get"), "{got}");
    assert!(got.contains("      set sum = sum + x\n"), "{got}");
    assert_eq!(run_source("bare-run", &got), ["3 6"]);
}

#[test]
fn a_conversion_makes_a_captured_read_live() {
    // This is the behaviour change the command exists to make: the function
    // read the value captured where it was written, and now reads the cell.
    let src = "let n = 1\nfn show() n end\nn = 2\nprint(show())\n";
    assert_eq!(run_source("live0", src), ["1"]);
    let got = converted("live", src, "/n");
    assert_eq!(
        got,
        "var n = 1\nfn show() get n end\nset n = 2\nprint(show())\n"
    );
    assert_eq!(run_source("live1", &got), ["2"]);
}

// ---------------------------------------------------------------------------
// Scoping
// ---------------------------------------------------------------------------

#[test]
fn a_shadowing_binding_keeps_its_own_mentions() {
    let src = "\
let x = 1
fn param(x) x + 1 end
fn local()
  let x = 10
  x = x + 1
  x
end
for x in [5] do print(x) end
let m = match 3
  when x -> x
end
let squares = for x in [1, 2] do x * x end
fn late(a = x, b = 0) a + b end
x = x + 1
print(x)
print(param(1))
print(local())
print(late())
";
    let got = converted("shadow", src, "/x");
    assert_eq!(
        got,
        "\
var x = 1
fn param(x) x + 1 end
fn local()
  let x = 10
  x = x + 1
  x
end
for x in [5] do print(x) end
let m = match 3
  when x -> x
end
let squares = for x in [1, 2] do x * x end
fn late(a = get x, b = 0) a + b end
set x = x + 1
print(x)
print(param(1))
print(local())
print(late())
"
    );
    assert_eq!(run_source("shadow-run", &got), ["5", "2", "2", "11", "2"]);
}

#[test]
fn a_shadow_in_a_nested_block_ends_with_the_block() {
    let src = "\
let x = 1
if true then
  x = 5
  let x = 7
  x = x + 1
  print(x)
end
x = x + 1
print(x)
";
    let got = converted("block-shadow", src, "/x[1]");
    assert_eq!(
        got,
        "\
var x = 1
if true then
  set x = 5
  let x = 7
  x = x + 1
  print(x)
end
set x = x + 1
print(x)
"
    );
    assert_eq!(run_source("block-shadow-run", &got), ["8", "6"]);
}

#[test]
fn a_redeclaration_in_the_same_block_ends_the_binding() {
    let src = "let x = 1\nx = 2\nlet x = x + 1\nx = 9\nprint(x)\n";
    assert_eq!(
        converted("redecl-1", src, "/x[1]"),
        "var x = 1\nset x = 2\nlet x = x + 1\nx = 9\nprint(x)\n"
    );
    assert_eq!(
        converted("redecl-2", src, "/x[2]"),
        "let x = 1\nx = 2\nvar x = x + 1\nset x = 9\nprint(x)\n"
    );
}

#[test]
fn a_function_of_the_same_name_ends_the_binding() {
    let src = "let f = 1\nprint(f)\nfn f() f end\nprint(f)\n";
    assert_eq!(
        converted("fn-shadow", src, "/f[1]"),
        "var f = 1\nprint(f)\nfn f() f end\nprint(f)\n"
    );
}

#[test]
fn an_index_picks_one_of_several_same_named_bindings() {
    let src = "\
fn build_view(items)
  let row = fn(item)
    let out = [item]
    out = append(out, 0)
    if item > 1 then
      let out = [9]
      out = append(out, item)
      print(out)
    end
    out
  end
  map(items, row)
end
print(build_view([1, 2]))
";
    let got = converted("repeat", src, "build_view/row/out[2]");
    assert_eq!(
        got,
        src.replace("      let out = [9]", "      var out = [9]")
            .replace(
                "      out = append(out, item)",
                "      set out = append(out, item)"
            )
    );
    assert_eq!(
        run_source("repeat-run", &got),
        run_source("repeat-run0", src)
    );

    // Without the index the path names both, and says so.
    let err = refusal("repeat-ambiguous", src, "build_view/row/out");
    assert!(err.contains("ambiguous"), "{err}");
    assert!(
        err.contains("/build_view/row/out[1]  (line 3, `let`)"),
        "{err}"
    );
    assert!(
        err.contains("/build_view/row/out[2]  (line 6, `let`)"),
        "{err}"
    );
}

#[test]
fn a_leading_slash_means_module_level() {
    let src = "\
let score = 0
fn step()
  let score = 10
  score = score + 1
  score
end
score = score + step()
print(score)
";
    let module = converted("module", src, "/score");
    assert_eq!(
        module,
        src.replace("let score = 0", "var score = 0")
            .replace("score = score + step()", "set score = score + step()")
    );
    let local = converted("local", src, "step/score");
    assert_eq!(
        local,
        src.replace("  let score = 10", "  var score = 10")
            .replace("  score = score + 1", "  set score = score + 1")
    );
    let err = refusal("module-ambiguous", src, "score");
    assert!(err.contains("/score  (line 1, `let`)"), "{err}");
    assert!(err.contains("/step/score  (line 3, `let`)"), "{err}");
}

#[test]
fn a_path_that_matches_nothing_lists_what_it_could_have_meant() {
    let src = "fn step()\n  let vx = 1\n  vx\nend\n";
    let err = refusal("no-match", src, "stop/vx");
    assert!(
        err.contains("no binding matches the target `stop/vx`"),
        "{err}"
    );
    assert!(err.contains("/step/vx  (line 2, `let`)"), "{err}");
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn an_at_rebind_of_the_binding_is_refused() {
    let src = "let xs = [1]\nprint(xs)\nappend(@xs, 2)\nprint(xs)\n";
    let err = refusal("at", src, "/xs");
    assert!(err.contains("`@xs` rebinds the binding"), "{err}");
    assert!(err.contains("line 3"), "{err}");
}

#[test]
fn an_at_rebind_of_a_shadow_is_not_the_bindings_business() {
    let src = "let xs = [1]\nfn f(xs)\n  append(@xs, 2)\n  xs\nend\nprint(f(xs))\n";
    assert_eq!(
        converted("at-shadow", src, "/xs"),
        src.replace("let xs", "var xs")
    );
}

#[test]
fn what_cannot_be_a_var_is_refused() {
    let src =
        "var a = 1\nstate var b = 2\nconfig let c = 3\nfn d() 4 end\nprint(a + b + c + d())\n";
    assert!(refusal("already-var", src, "/a").contains("already a `var`"));
    assert!(refusal("already-state-var", src, "/b").contains("already a `state var`"));
    assert!(refusal("config", src, "/c").contains("`config let`"));
    assert!(refusal("function", src, "/d").contains("is a function"));
}

// ---------------------------------------------------------------------------
// Exported bindings
// ---------------------------------------------------------------------------

const TALLY: &str = "\
pub state hits = 0
pub fn bump()
  hits = hits + 1
end
";

#[test]
fn an_exported_binding_is_followed_into_the_files_that_read_it() {
    let dir = Dir::new("importers");
    let tally = dir.write("tally.ptl", TALLY);
    let main_src = "\
import tally: hits, bump
fn describe()
  \"hits: {hits}\"
end
bump()
bump()
print(hits)
print(describe())
";
    let main = dir.write("main.ptl", main_src);
    // A qualified read is live wherever it is written, so it needs nothing.
    let qualified_src = "import tally\nfn n() tally.hits end\ntally.bump()\nprint(n())\n";
    let qualified = dir.write("qualified.ptl", qualified_src);
    // Not importers: from `sub/` the name `tally` resolves to nothing, and
    // from `other/` it resolves to a different file.
    let stray = dir.write(
        "sub/stray.ptl",
        "import tally: *\nfn n() hits + 1 end\nprint(n())\n",
    );
    dir.write("other/tally.ptl", "pub let hits = 5\n");
    let stranger = dir.write("other/app.ptl", "import tally: hits\nfn n() hits end\n");

    let plan = convert(&tally, "/hits", &[]).unwrap();
    assert_eq!(
        plan.after(&tally).unwrap(),
        "pub state var hits = 0\npub fn bump()\n  set hits = get hits + 1\nend\n"
    );
    assert_eq!(
        plan.after(&main).unwrap(),
        main_src.replace("{hits}", "{get hits}")
    );
    assert_eq!(plan.after(&qualified), None);
    assert_eq!(plan.after(&stray), None);
    assert_eq!(plan.after(&stranger), None);

    // The report says where it looked and what it found.
    let report = plan
        .importers
        .as_ref()
        .expect("an exported binding has a report");
    assert!(report.how.contains("scanned"), "{}", report.how);
    let found: Vec<(String, &str)> = report
        .importers
        .iter()
        .map(|i| {
            (
                i.path.file_name().unwrap().to_string_lossy().into_owned(),
                i.form.as_str(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("main.ptl".to_string(), "import tally: hits, bump"),
            ("qualified.ptl".to_string(), "import tally")
        ]
    );

    plan.write().unwrap();
    assert_eq!(run(&main), ["2", "hits: 2"]);
    assert_eq!(run(&qualified), ["1"]);
}

#[test]
fn a_star_import_binds_the_name_unless_the_file_declares_it() {
    let dir = Dir::new("star");
    let tally = dir.write("tally.ptl", TALLY);
    let star = dir.write(
        "star.ptl",
        "import tally: *\nfn n() hits + 1 end\nprint(n())\n",
    );
    let own_src = "import tally: *\nfn n() 0 end\nlet hits = 7\nfn m() hits end\nprint(m())\n";
    let own = dir.write("own.ptl", own_src);

    let plan = convert(&tally, "hits", &[]).unwrap();
    assert_eq!(
        plan.after(&star).unwrap(),
        "import tally: *\nfn n() get hits + 1 end\nprint(n())\n"
    );
    assert_eq!(plan.after(&own), None, "its own `hits` wins over the star");
}

#[test]
fn an_importer_that_writes_the_binding_is_refused() {
    let dir = Dir::new("importer-write");
    let tally = dir.write("tally.ptl", "pub let name = \"t\"\n");
    let reader = dir.write(
        "reader.ptl",
        "import tally: name\nfn f() name end\nprint(f())\n",
    );
    let writer_src = "import tally: name\nprint(name)\nname = \"z\"\nprint(name)\n";
    dir.write("writer.ptl", writer_src);

    let err = convert(&tally, "/name", &[]).expect_err("an importer writes it");
    assert!(
        err.contains("writer.ptl imports `name` and writes it (line 3)"),
        "{err}"
    );
    assert!(err.contains("never write it"), "{err}");
    // Nothing moved, the well-behaved importer included.
    assert_eq!(dir.read("tally.ptl"), "pub let name = \"t\"\n");
    assert_eq!(dir.read("writer.ptl"), writer_src);
    assert_eq!(
        std::fs::read_to_string(&reader).unwrap(),
        "import tally: name\nfn f() name end\nprint(f())\n"
    );
}

#[test]
fn an_importer_that_rebinds_with_at_is_refused() {
    let dir = Dir::new("importer-at");
    let tally = dir.write("tally.ptl", "pub let names = [\"t\"]\n");
    dir.write(
        "writer.ptl",
        "import tally: names\nappend(@names, \"z\")\nprint(names)\n",
    );
    let err = convert(&tally, "/names", &[]).expect_err("an importer rebinds it");
    assert!(
        err.contains("writer.ptl imports `names` and writes it (line 2)"),
        "{err}"
    );
}

#[test]
fn an_importer_that_writes_through_the_modules_name_is_refused() {
    // `m.x = …` has never worked (a module is not a value), but it is the
    // importer asking to write the binding, and a `var` does not make it so.
    let dir = Dir::new("importer-qualified-write");
    let tally = dir.write("tally.ptl", "pub let hits = [0]\npub let name = \"t\"\n");
    let writer_src = "import tally as t\nfn f()\n  t.hits[0] = 1\nend\nt.name = \"z\"\nset t.hits = []\n";
    dir.write("writer.ptl", writer_src);

    let err = convert(&tally, "/hits", &[]).expect_err("an importer writes it");
    assert!(
        err.contains("writer.ptl imports `hits` and writes it (lines 3, 6)"),
        "{err}"
    );
    assert_eq!(dir.read("tally.ptl"), "pub let hits = [0]\npub let name = \"t\"\n");
    assert_eq!(dir.read("writer.ptl"), writer_src);

    // Reading through the module's name, or writing another of its names,
    // is nobody's business.
    let dir = Dir::new("importer-qualified-read");
    let tally = dir.write("tally.ptl", "pub let hits = [0]\npub let name = \"t\"\n");
    dir.write(
        "reader.ptl",
        "import tally\nfn f() tally.hits[0] end\nlet local = {hits: 1}\nlocal.hits = 2\nprint(f())\n",
    );
    let plan = convert(&tally, "/hits", &[]).unwrap();
    let report = plan.importers.as_ref().unwrap();
    assert_eq!(report.importers.len(), 1, "{:?}", report.importers);
    assert!(
        report.importers[0].detail.contains("reads it qualified"),
        "{:?}",
        report.importers
    );
}

#[test]
fn a_from_entry_that_does_not_exist_is_said_to_be_missing() {
    let dir = Dir::new("from-missing");
    let tally = dir.write("tally.ptl", TALLY);
    let missing = dir.0.join("nowhere.ptl");
    let err = convert(&tally, "/hits", &[&missing]).expect_err("no such entry");
    assert!(err.starts_with("--from "), "{err}");
    assert!(err.contains("nowhere.ptl"), "{err}");
    assert!(!err.contains("would not compile"), "{err}");
    assert_eq!(dir.read("tally.ptl"), TALLY);
}

#[test]
fn from_names_the_programs_whose_imports_are_followed() {
    let dir = Dir::new("from");
    // The module lives in a library directory; the app that uses it does not.
    let tally = dir.write("lib/tally.ptl", TALLY);
    let facade = dir.write("lib/facade.ptl", "pub import tally: hits, bump\n");
    let app_src = "import facade: hits, bump\nfn n() hits end\nbump()\nprint(n())\n";
    let app = dir.write("lib/app.ptl", app_src);
    let unrelated = dir.write("lib/unrelated.ptl", "import tally: hits\nfn n() hits end\n");

    let plan = convert(&tally, "/hits", &[&app]).unwrap();
    let report = plan.importers.as_ref().unwrap();
    assert!(
        report.how.contains("followed the imports of"),
        "{}",
        report.how
    );
    // The facade passes the name on, so the app behind it is an importer too.
    assert_eq!(
        plan.after(&app).unwrap(),
        app_src.replace("fn n() hits end", "fn n() get hits end")
    );
    assert_eq!(plan.after(&facade), None, "a re-export is not a read");
    // Not reachable from the entry that was named, so not considered.
    assert_eq!(plan.after(&unrelated), None);
    assert_eq!(report.importers.len(), 2, "{:?}", report.importers);

    plan.write().unwrap();
    assert_eq!(run(&app), ["1"]);
}

#[test]
fn a_scan_stops_at_the_package_root() {
    let dir = Dir::new("package");
    dir.write(
        "pkg/petal.toml",
        "[package]\nname = \"pkg\"\nmodules = \"src\"\n",
    );
    let tally = dir.write("pkg/src/tally.ptl", TALLY);
    // A sibling under the package root, outside the module directory.
    let demo_src = "import src/tally: hits\nfn n() hits end\nprint(n())\n";
    let demo = dir.write("pkg/demo.ptl", demo_src);

    let plan = convert(&tally, "/hits", &[]).unwrap();
    let report = plan.importers.as_ref().unwrap();
    // From the manifest's directory, not just the module's own.
    assert!(report.how.contains("pkg (2 .ptl files)"), "{}", report.how);
    assert_eq!(
        plan.after(&demo).unwrap(),
        demo_src.replace("fn n() hits end", "fn n() get hits end")
    );
}

#[test]
fn a_binding_that_is_not_exported_has_no_importers() {
    let dir = Dir::new("private");
    let file = dir.write("m.ptl", "let n = 1\nn = 2\npub fn f() 1 end\n");
    dir.write("user.ptl", "import m\nprint(m.f())\n");
    let plan = convert(&file, "/n", &[]).unwrap();
    assert!(plan.importers.is_none());
    assert_eq!(plan.files.len(), 1);
}

#[test]
fn the_deprecated_export_spelling_is_an_exported_binding_too() {
    let dir = Dir::new("export");
    let tally = dir.write("tally.ptl", "export let hits = 0\n");
    let user = dir.write(
        "user.ptl",
        "import tally: hits\nfn n() hits end\nprint(n())\n",
    );
    let plan = convert(&tally, "/hits", &[]).unwrap();
    assert_eq!(plan.after(&tally).unwrap(), "export var hits = 0\n");
    assert!(plan.after(&user).unwrap().contains("fn n() get hits end"));
}

// ---------------------------------------------------------------------------
// The compile gate
// ---------------------------------------------------------------------------

#[test]
fn a_file_may_keep_the_errors_it_had_while_it_is_fixed_one_binding_at_a_time() {
    let dir = Dir::new("gate-steps");
    let src = "\
let a = 0
let b = 0
fn step()
  a = a + 1
  b = b + 1
end
step()
print(a + b)
";
    let file = dir.write("main.ptl", src);
    let first = convert(&file, "/a", &[]).unwrap();
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.contains("still does not compile")
                && n.contains("`b` is bound outside this function")),
        "{:?}",
        first.notes
    );
    first.write().unwrap();
    let second = convert(&file, "/b", &[]).unwrap();
    assert!(second.notes.is_empty(), "{:?}", second.notes);
    second.write().unwrap();
    assert_eq!(run(&file), ["2"]);
}

#[test]
fn nothing_is_written_when_a_from_entry_cannot_be_compiled() {
    let dir = Dir::new("gate-refuse");
    let tally = dir.write("tally.ptl", TALLY);
    let app_src = "import tally: hits\nimport nowhere\nfn n() hits end\nprint(n())\n";
    let app = dir.write("app.ptl", app_src);
    let err = convert(&tally, "/hits", &[&app]).expect_err("the entry does not compile");
    assert!(err.contains("refusing to write"), "{err}");
    assert!(err.contains("cannot find module 'nowhere'"), "{err}");
    assert_eq!(dir.read("tally.ptl"), TALLY);
    assert_eq!(dir.read("app.ptl"), app_src);
}

#[test]
fn a_formatted_file_stays_formatted_and_comments_survive() {
    let src = "\
// The running total.
let total = 0 // starts empty

fn add(n)
  // one more
  total = total + n // accumulate
end

add(2)
print(total)
";
    assert_eq!(petal::fmt::format_source(src).unwrap(), src);
    let got = converted("fmt", src, "/total");
    assert_eq!(petal::fmt::format_source(&got).unwrap(), got);
    assert!(
        got.contains("// The running total.\nvar total = 0 // starts empty\n"),
        "{got}"
    );
    assert!(
        got.contains("  // one more\n  set total = get total + n // accumulate\n"),
        "{got}"
    );
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

fn petal(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(PETAL)
        .args(args)
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

const COUNTER: &str = "state n = 0\nfn bump()\n  n = n + 1\nend\nbump()\nprint(n)\n";

#[test]
fn dry_run_prints_a_diff_and_writes_nothing() {
    let dir = Dir::new("dry-run");
    let file = dir.write("main.ptl", COUNTER);
    let path = file.to_str().unwrap();
    let (stdout, stderr, ok) = petal(&[
        "apply-change",
        "convert-to-var",
        path,
        "--target",
        "/n",
        "--dry-run",
        "--host",
        "core",
    ]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("-state n = 0\n+state var n = 0\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("-  n = n + 1\n+  set n = get n + 1\n"),
        "{stdout}"
    );
    assert!(stderr.contains("/n: `state` -> `state var`"), "{stderr}");
    assert!(stderr.contains("dry run"), "{stderr}");
    assert_eq!(dir.read("main.ptl"), COUNTER);
}

#[test]
fn without_dry_run_the_file_is_rewritten_in_place() {
    let dir = Dir::new("in-place");
    let file = dir.write("main.ptl", COUNTER);
    let path = file.to_str().unwrap();
    let (stdout, stderr, ok) = petal(&[
        "apply-change",
        "convert-to-var",
        path,
        "--target",
        "/n",
        "--host",
        "core",
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(stdout, "", "the diff is only printed for a dry run");
    assert!(stderr.contains("wrote 1 file(s)"), "{stderr}");
    assert_eq!(
        dir.read("main.ptl"),
        "state var n = 0\nfn bump()\n  set n = get n + 1\nend\nbump()\nprint(n)\n"
    );
    assert_eq!(run(&file), ["1"]);
}

#[test]
fn a_refusal_exits_nonzero_and_leaves_the_file_alone() {
    let dir = Dir::new("cli-refusal");
    let src = "let xs = [1]\nappend(@xs, 2)\nprint(xs)\n";
    let file = dir.write("main.ptl", src);
    let path = file.to_str().unwrap();
    let (_, stderr, ok) = petal(&["apply-change", "convert-to-var", path, "--target", "xs"]);
    assert!(!ok);
    assert!(
        stderr.contains("convert-to-var: refusing: `@xs`"),
        "{stderr}"
    );
    assert_eq!(dir.read("main.ptl"), src);

    let (_, stderr, ok) = petal(&["apply-change", "convert-to-var", path]);
    assert!(!ok);
    assert!(stderr.contains("needs --target"), "{stderr}");

    let (_, stderr, ok) = petal(&["apply-change", "rename", path]);
    assert!(!ok);
    assert!(stderr.contains("Unknown operation 'rename'"), "{stderr}");
}

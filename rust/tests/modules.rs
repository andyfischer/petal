// Integration tests for the module / import system (docs/module-system.md).
//
// Most cases use in-memory modules (`Env::register_module`) — which doubles
// as coverage for the wasm-shaped "no filesystem" embedding. Filesystem
// resolution (importer-relative and search-path) gets its own temp-dir cases
// at the bottom.

use petal::compiler::Compiler;
use petal::env::Env;
use petal::program::StateKey;

/// Run `entry` with the given in-memory modules and return its print output.
fn run_with_modules(modules: &[(&str, &str)], entry: &str) -> Vec<String> {
    let mut env = Env::new();
    for (name, source) in modules {
        env.register_module(name, source);
    }
    let pid = env.load_program(entry).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output()
}

/// Assert `entry` produces the expected output.
fn check_output(modules: &[(&str, &str)], entry: &str, expect: &[&str]) {
    let out = run_with_modules(modules, entry);
    assert_eq!(out, expect);
}

/// Load `entry` expecting a compile-time error; return the message.
fn load_error(modules: &[(&str, &str)], entry: &str) -> String {
    let mut env = Env::new();
    for (name, source) in modules {
        env.register_module(name, source);
    }
    env.load_program(entry).unwrap_err()
}

const UI: &str = "\
let palette = { fg: 15, bg: 2 }
fn button(label)
  \"[\" ++ label ++ \"]\"
end
fn _secret()
  42
end
";

// ── Binding forms ────────────────────────────────────────────────

#[test]
fn qualified_access_through_module_alias() {
    check_output(
        &[("ui", UI)],
        "import ui\nprint(ui.button(\"go\"))\nprint(ui.palette.bg)",
        &["[go]", "2"],
    );
}

#[test]
fn selective_import_binds_bare_names() {
    check_output(
        &[("ui", UI)],
        "import ui: button\nprint(button(\"x\"))",
        &["[x]"],
    );
}

#[test]
fn alias_import() {
    check_output(
        &[("ui", UI)],
        "import ui as u\nprint(u.button(\"y\"))",
        &["[y]"],
    );
}

#[test]
fn module_member_used_inside_function_is_captured() {
    check_output(
        &[("ui", UI)],
        "import ui\nimport ui: button\nfn both(l)\n  ui.button(l) ++ button(l)\nend\nprint(both(\"a\"))",
        &["[a][a]"],
    );
}

#[test]
fn local_binding_shadows_module_alias() {
    // Once `ui` is an ordinary value, `ui.fg` is plain field access.
    check_output(
        &[("ui", UI)],
        "import ui\nlet ui = { fg: 9 }\nprint(ui.fg)",
        &["9"],
    );
}

#[test]
fn imports_can_nest() {
    let base = "fn double(x)\n  x * 2\nend";
    let mid = "import base\nfn quad(x)\n  base.double(base.double(x))\nend";
    check_output(
        &[("base", base), ("mid", mid)],
        "import mid\nprint(mid.quad(3))",
        &["12"],
    );
}

// ── Execution semantics ──────────────────────────────────────────

#[test]
fn module_top_level_runs_once_before_importer_diamond() {
    let base = "print(\"base-init\")\nlet shared = 7";
    let left = "import base\nlet l = base.shared + 1";
    let right = "import base\nlet r = base.shared + 2";
    check_output(
        &[("base", base), ("left", left), ("right", right)],
        "import left\nimport right\nprint(left.l + right.r)",
        &["base-init", "17"],
    );
}

#[test]
fn enum_variants_export_and_match_across_modules() {
    let shapes = "enum Shape\n  Circle(r)\n  Dot\nend";
    check_output(
        &[("shapes", shapes)],
        "import shapes: Circle, Dot\n\
         let c = Circle(5)\n\
         print(match c\n  when Circle(r) -> r\n  when Dot -> 0\nend)",
        &["5"],
    );
}

#[test]
fn overloaded_module_fn_exports_as_one_set() {
    let m = "fn f(a)\n  a\nend\nfn f(a, b)\n  a + b\nend";
    check_output(
        &[("m", m)],
        "import m: f\nprint(f(1))\nprint(f(1, 2))",
        &["1", "3"],
    );
}

// ── Errors ───────────────────────────────────────────────────────

#[test]
fn import_cycle_is_a_compile_error() {
    let err = load_error(
        &[("a", "import b\nlet x = 1"), ("b", "import a\nlet y = 1")],
        "import a",
    );
    assert!(err.contains("import cycle: a -> b -> a"), "got: {err}");
}

#[test]
fn missing_module_is_a_compile_error() {
    let err = load_error(&[], "import nope");
    assert!(err.contains("cannot find module 'nope'"), "got: {err}");
}

#[test]
fn unknown_selective_name_is_a_compile_error() {
    let err = load_error(&[("ui", UI)], "import ui: knob");
    assert!(err.contains("no export 'knob'"), "got: {err}");
    assert!(err.contains("button"), "error lists exports: {err}");
}

#[test]
fn selective_import_of_private_name_is_a_compile_error() {
    let err = load_error(&[("ui", UI)], "import ui: _secret");
    assert!(err.contains("module-private"), "got: {err}");
}

#[test]
fn selective_collision_between_modules_is_a_compile_error() {
    let a = "fn draw()\n  1\nend";
    let b = "fn draw()\n  2\nend";
    let err = load_error(&[("a", a), ("b", b)], "import a: draw\nimport b: draw");
    assert!(
        err.contains("'draw' is imported from both 'a' and 'b'"),
        "got: {err}"
    );
}

#[test]
fn selective_collision_with_local_decl_is_a_compile_error() {
    let err = load_error(
        &[("ui", UI)],
        "import ui: button\nfn button(x)\n  x\nend",
    );
    assert!(err.contains("also declared in this file"), "got: {err}");
}

#[test]
fn conflicting_aliases_are_a_compile_error() {
    let err = load_error(
        &[("a", "let x = 1"), ("b", "let y = 1")],
        "import a as m\nimport b as m",
    );
    assert!(err.contains("already an alias"), "got: {err}");
}

#[test]
fn import_after_statement_is_a_parse_error() {
    let err = load_error(&[("ui", UI)], "let x = 1\nimport ui");
    assert!(
        err.contains("import statements must appear before any other statement"),
        "got: {err}"
    );
}

#[test]
fn module_alias_as_value_is_a_deferred_error() {
    // Consistent with undefined variables: the error fires at runtime, only
    // if the expression actually executes.
    let mut env = Env::new();
    env.register_module("ui", UI);
    let pid = env.load_program("import ui\nprint(ui)").unwrap();
    let sid = env.create_stack(pid).unwrap();
    let err = env.run(sid).unwrap_err();
    assert!(err.contains("'ui' is a module"), "got: {err}");
}

#[test]
fn private_member_access_is_a_deferred_error() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    let pid = env.load_program("import ui\nprint(ui._secret())").unwrap();
    let sid = env.create_stack(pid).unwrap();
    let err = env.run(sid).unwrap_err();
    assert!(err.contains("private"), "got: {err}");
}

#[test]
fn runtime_error_in_module_names_the_file() {
    let bad = "fn boom(x)\n  x + nil\nend";
    let mut env = Env::new();
    env.register_module("bad", bad);
    let pid = env.load_program("import bad\nbad.boom(1)").unwrap();
    let sid = env.create_stack(pid).unwrap();
    let err = env.run(sid).unwrap_err();
    // Module positions carry the module's display name; entry-file errors
    // keep the bare [line N, column M] format.
    assert!(err.contains("[bad line 2"), "got: {err}");
}

// ── State keys ───────────────────────────────────────────────────

#[test]
fn same_state_name_in_two_modules_gets_distinct_slots() {
    let m1 = "state scroll = 0\nscroll += 1\nfn get1()\n  scroll\nend";
    let m2 = "state scroll = 0\nscroll += 10\nfn get2()\n  scroll\nend";
    check_output(
        &[("m1", m1), ("m2", m2)],
        "import m1\nimport m2\nprint(m1.get1())\nprint(m2.get2())",
        &["1", "10"],
    );
}

#[test]
fn entry_file_state_keys_stay_bare_named() {
    // The entry file keeps bare-name hashing, so existing host code that
    // computes keys via hash_state_name keeps working (and hot-reload state
    // of pre-module programs survives).
    let mut env = Env::new();
    env.register_module("m", "state n = 0\nn += 5");
    let pid = env.load_program("import m\nstate n = 100\nn += 1").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let entry_key = StateKey(Compiler::hash_state_name("n"));
    let module_key = StateKey(Compiler::hash_state_name("m::n"));
    assert_eq!(format!("{:?}", env.get_state(sid, entry_key).unwrap()), "Int(101)");
    assert_eq!(format!("{:?}", env.get_state(sid, module_key).unwrap()), "Int(5)");
}

#[test]
fn hot_reload_of_module_preserves_its_state() {
    let mut env = Env::new();
    env.register_module("counter", "state n = 0\nn += 1\nfn get()\n  n\nend");
    let pid = env.load_program("import counter\nprint(counter.get())").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["1"]);

    // Edit the module (init unchanged, increment becomes +10) and reload.
    env.register_module("counter", "state n = 0\nn += 10\nfn get()\n  n\nend");
    let new_program = env
        .compile_program(pid, "import counter\nprint(counter.get())")
        .unwrap();
    let result = env.transfer_state(sid, new_program).unwrap();
    assert_eq!(result.state_preserved, 1);
    assert_eq!(result.state_dropped, 0);

    env.run(sid).unwrap();
    // Preserved n=1, then += 10 → 11.
    assert_eq!(env.take_output(), vec!["11"]);
}

#[test]
fn renaming_a_module_drops_its_state() {
    let counter = "state n = 0\nn += 1\nfn get()\n  n\nend";
    let mut env = Env::new();
    env.register_module("counter", counter);
    env.register_module("tally", counter);
    let pid = env.load_program("import counter\nprint(counter.get())").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output();

    // Same state decl, different module name → different key → dropped.
    let new_program = env
        .compile_program(pid, "import tally\nprint(tally.get())")
        .unwrap();
    let result = env.transfer_state(sid, new_program).unwrap();
    assert_eq!(result.state_preserved, 0);
    assert_eq!(result.state_dropped, 1);
}

// ── Implicit imports ─────────────────────────────────────────────

#[test]
fn implicit_imports_bind_exports_bare() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    env.set_implicit_imports(&["ui"]);
    let pid = env.load_program("print(button(\"z\"))").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["[z]"]);
}

#[test]
fn script_bindings_win_over_implicit_imports() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    env.set_implicit_imports(&["ui"]);
    let pid = env
        .load_program("fn button(l)\n  \"<\" ++ l ++ \">\"\nend\nprint(button(\"z\"))")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["<z>"]);
}

#[test]
fn explicit_import_of_implicit_module_is_a_noop() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    env.set_implicit_imports(&["ui"]);
    let pid = env
        .load_program("import ui\nprint(button(\"z\"))\nprint(ui.button(\"q\"))")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["[z]", "[q]"]);
}

// ── Host-facing surfaces ─────────────────────────────────────────

#[test]
fn call_function_reaches_module_fns_by_qualified_name() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    let pid = env.load_program("import ui\nlet _ = 0").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();

    let arg = petal::value::Value::String(env.heap_mut().alloc_string("hi".to_string()));
    let result = env.call_function(sid, "ui::button", &[arg]).unwrap();
    let rendered = petal::value::value_to_json(&result, env.heap());
    assert_eq!(rendered, serde_json::json!("[hi]"));
}

#[test]
fn module_manifest_lists_all_files() {
    let mut env = Env::new();
    env.register_module("ui", UI);
    let pid = env.load_program("import ui\nlet x = 1").unwrap();
    let manifest = env.module_manifest(pid);
    let names: Vec<&str> = manifest.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["<entry>", "ui"]);
    // In-memory modules have no filesystem origin.
    assert!(manifest.iter().all(|e| e.origin.is_none()));
}

#[test]
fn imports_are_not_reexported() {
    let base = "fn helper()\n  1\nend";
    let mid = "import base: helper\nfn use_it()\n  helper()\nend";
    let err = load_error(
        &[("base", base), ("mid", mid)],
        "import mid: helper",
    );
    assert!(err.contains("no export 'helper'"), "got: {err}");
}

// ── Filesystem resolution ────────────────────────────────────────

/// Create a unique temp directory tree for a filesystem test.
struct TempTree {
    root: std::path::PathBuf,
}

impl TempTree {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "petal-modtest-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, rel: &str, content: &str) -> std::path::PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn imports_resolve_relative_to_the_importing_file() {
    let tree = TempTree::new("relative");
    tree.write("lib/palette.ptl", "let bg = 3");
    // panel.ptl imports its sibling, from a different working directory.
    let panel = tree.write("lib/panel.ptl", "import palette\nprint(palette.bg)");
    let source = std::fs::read_to_string(&panel).unwrap();

    let mut env = Env::new();
    let pid = env.load_program_at(&source, &panel).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["3"]);

    // The manifest records where the module came from.
    let manifest = env.module_manifest(pid);
    let module_entry = manifest.iter().find(|e| e.name == "palette.ptl").unwrap();
    assert!(module_entry.origin.as_ref().unwrap().ends_with("lib/palette.ptl"));
}

#[test]
fn registered_module_beats_file_of_same_name() {
    let tree = TempTree::new("priority");
    tree.write("dep.ptl", "let v = \"file\"");
    let entry = tree.write("main.ptl", "import dep\nprint(dep.v)");
    let source = std::fs::read_to_string(&entry).unwrap();

    let mut env = Env::new();
    env.register_module("dep", "let v = \"memory\"");
    let pid = env.load_program_at(&source, &entry).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["memory"]);
}

#[test]
fn module_search_paths_are_consulted_after_importer_dir() {
    let tree = TempTree::new("searchpath");
    tree.write("libs/util.ptl", "let tag = \"from-libs\"");

    let mut env = Env::new();
    env.add_module_path(tree.root.join("libs"));
    // Entry loaded with no origin (inline) — only the search path can hit.
    let pid = env.load_program("import util\nprint(util.tag)").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["from-libs"]);
}

#[test]
fn wasm_shaped_env_compiles_from_memory_only() {
    // No filesystem involvement at all: every module is registered.
    let mut env = Env::new();
    env.register_module("a", "import b\nfn f()\n  b.g() + 1\nend");
    env.register_module("b", "fn g()\n  41\nend");
    let pid = env.load_program("import a\nprint(a.f())").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), vec!["42"]);
}

//! `petal check` and `petal run` agree, and both hold a script to what its
//! imports and `native fn` declarations say. Each test here is a case an
//! independent review of the module system found the two disagreeing on, or
//! the checker missing. They shell out to the built binary: the point is
//! what the two commands say about the same file.
//!
//! - a relative import from an entry file named with no directory;
//! - the arity a `native fn` declares;
//! - method syntax reaching a native only in a file that has it in scope;
//! - a name nothing defines, in a line that would never have run;
//! - a call through a plain import, held to the module the name resolved to;
//! - a qualified call, held to the module its qualifier names and no other;
//! - a `let` that starts as `nil` and is assigned later, passed on;
//! - assigning through a module qualifier;
//! - serialized IR linked like source;
//! - `fix-imports` on a sibling file that shares a bundled package's name.

use std::path::{Path, PathBuf};
use std::process::Command;

const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// A throwaway directory tree, removed on drop.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(tag: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("petal-agree-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree {
            root: std::fs::canonicalize(&root).unwrap(),
        }
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Run `petal <args>` in `dir`; returns (stdout, stderr, exit code).
fn petal_in(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(PETAL)
        .args(args)
        .current_dir(dir)
        .env_remove("PETAL_OPT")
        .env_remove("PETAL_POLICY")
        .env_remove("PETAL_PATH")
        .env("PETAL_OFFLINE", "1")
        .output()
        .expect("failed to run petal");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

const HUD: &str = "pub fn draw()\n  return \"hud.draw\"\nend\n";

#[test]
fn a_parent_import_works_whatever_way_the_entry_file_is_named() {
    let t = Tree::new("parent");
    t.write("hud.ptl", HUD);
    t.write("st/esc.ptl", "import ../hud\nprint(hud.draw())\n");
    let st = t.dir("st");
    let abs = st.join("esc.ptl");
    for (dir, file) in [
        (st.as_path(), "esc.ptl"),
        (st.as_path(), "./esc.ptl"),
        (t.root.as_path(), "st/esc.ptl"),
        (st.as_path(), abs.to_str().unwrap()),
    ] {
        let (out, err, code) = petal_in(dir, &["run", file]);
        assert_eq!((out.as_str(), code), ("hud.draw\n", 0), "run {file}: {err}");
        let (_, err, code) = petal_in(dir, &["check", file]);
        assert_eq!(code, 0, "check {file}: {err}");
    }
}

#[test]
fn a_bare_entry_name_inside_a_package_still_knows_the_package_root() {
    let t = Tree::new("pkgroot");
    t.write("app/petal.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nmodules = \"src\"\n");
    t.write("app/examples/ex.ptl", "pub fn ex()\n  return \"ex\"\nend\n");
    t.write("outside/secret.ptl", "pub fn s()\n  return \"s\"\nend\n");
    t.write("app/src/ok.ptl", "import ../examples/ex\nprint(ex.ex())\n");
    t.write("app/src/esc.ptl", "import ../../outside/secret\nprint(secret.s())\n");
    let src = t.dir("app/src");
    let (out, err, code) = petal_in(&src, &["run", "ok.ptl"]);
    assert_eq!((out.as_str(), code), ("ex\n", 0), "{err}");
    let (_, err, code) = petal_in(&src, &["run", "esc.ptl"]);
    assert_eq!(code, 1);
    assert!(err.contains("leaves the package 'app'"), "{err}");
    assert!(!err.contains("filesystem root"), "{err}");
}

#[test]
fn check_holds_a_native_call_to_the_arity_its_declaration_gives() {
    let t = Tree::new("arity");
    t.write(
        "n.ptl",
        "native fn k1(a: int, b?: string) -> int\n\
         native fn k2(x)\n\
         native fn many(first, ...rest)\n\
         fn f()\n\
         \x20 k1(1, \"s\", 3)\n\
         \x20 k2()\n\
         \x20 many()\n\
         \x20 many(1, 2, 3, 4)\n\
         \x20 k1(1)\n\
         \x20 let u = len(1, 2, 3)\n\
         \x20 let w = clamp(1, 2)\n\
         end\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "n.ptl"]);
    assert_eq!(code, 1, "{err}");
    for expected in [
        "`k1` expects 1-2 arguments, got 3",
        "`k2` expects 1 argument, got 0",
        "`many` expects at least 1 argument, got 0",
        "`len` expects 1 argument, got 3",
        "`clamp` expects 3 arguments, got 2",
        "5 errors found by check",
    ] {
        assert!(err.contains(expected), "missing {expected:?} in:\n{err}");
    }
}

#[test]
fn check_holds_a_host_native_to_its_interface_package() {
    let t = Tree::new("uiarity");
    t.write("u.ptl", "import ui\nfn f()\n  mouse_x(1, 2, 3)\n  text_width()\n  mouse_x()\nend\n");
    let (_, err, code) = petal_in(&t.root, &["check", "u.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("`mouse_x` expects 0 arguments, got 3"), "{err}");
    assert!(err.contains("`text_width` expects"), "{err}");
    assert!(err.contains("2 errors found by check"), "{err}");
}

#[test]
fn method_syntax_reaches_a_packaged_builtin_only_in_a_file_that_imports_it() {
    let t = Tree::new("method");
    t.write(
        "mod.ptl",
        "import creative\npub fn half(x)\n  return x.lerp(10.0, 0.5)\nend\n",
    );
    // The importing module has `lerp` in scope; this file does not.
    t.write("main.ptl", "import ./mod\nlet y = 0.0\nprint(half(y))\nprint(y.lerp(10.0, 0.5))\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no method `lerp` on a `float` here"), "{err}");
    assert!(err.contains("add `import creative`"), "{err}");
    // With the import, in this file, it is fine — and the module's own use
    // was fine all along.
    t.write(
        "ok.ptl",
        "import ./mod\nimport creative\nlet y = 0.0\nprint(half(y))\nprint(y.lerp(10.0, 0.5))\n",
    );
    let (out, err, code) = petal_in(&t.root, &["run", "ok.ptl"]);
    assert_eq!((out.as_str(), code), ("5.0\n5.0\n", 0), "{err}");
    // A receiver the checker cannot type is still refused when it runs: the
    // other module's import does not leak.
    t.write(
        "dyn.ptl",
        "import ./mod\nfn id(v)\n  return v\nend\nprint(half(0.0))\nprint(id(0.0).lerp(10.0, 0.5))\n",
    );
    let (out, err, code) = petal_in(&t.root, &["run", "dyn.ptl"]);
    assert_eq!((out.as_str(), code), ("5.0\n", 1), "{err}");
    assert!(err.contains("No method 'lerp'"), "{err}");
    assert!(err.contains("add `import creative`"), "{err}");
}

#[test]
fn run_refuses_a_name_nothing_defines_before_the_first_line() {
    let t = Tree::new("unknown");
    t.write(
        "un.ptl",
        "fn never()\n  totally_unknown(1)\n  return nowhere\nend\nprint(\"ran\")\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "un.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("unknown function `totally_unknown`"), "{err}");
    assert!(err.contains("undefined variable `nowhere`"), "{err}");

    let (out, err, code) = petal_in(&t.root, &["run", "un.ptl"]);
    assert_eq!((out.as_str(), code), ("", 1), "nothing may run: {err}");
    assert!(err.contains("Unknown function: totally_unknown"), "{err}");
    assert!(err.contains("Undefined variable: nowhere"), "{err}");

    // The same for a host native reached without its interface package.
    t.write("d.ptl", "print(\"before\")\ndraw_rect(1, 2, 3, 4)\n");
    let (out, err, code) = petal_in(&t.root, &["run", "d.ptl"]);
    assert_eq!((out.as_str(), code), ("", 1), "{err}");
    assert!(err.contains("Unknown function: draw_rect"), "{err}");

    let (out, _, code) = petal_in(&t.root, &["run", "--json", "d.ptl"]);
    assert_eq!(code, 1);
    let obj: serde_json::Value = serde_json::from_str(out.trim()).expect("one JSON document");
    assert_eq!(obj["phase"], "compile");
    assert_eq!(obj["line"], 2);
}

#[test]
fn a_call_through_an_import_is_held_to_the_module_it_resolved_to() {
    let t = Tree::new("resolved");
    t.write("hud.ptl", "pub fn score(a)\n  return a\nend\n");
    // `top` is built on `hud` (through `mid`), so its `score` shadows hud's
    // for a file that imports both: the one-argument form is not reachable
    // bare, though the checker's name-keyed table has seen both.
    t.write("mid.ptl", "import ./hud\npub fn mid()\n  return score(0)\nend\n");
    t.write(
        "top.ptl",
        "import ./mid\npub fn score(a, b, c = 0)\n  return a + b + c\nend\n",
    );
    t.write(
        "main.ptl",
        "import ./hud\nimport ./top\nprint(score(1, 2))\nprint(score(1, 2, 3))\nprint(hud.score(1))\nprint(score(1))\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    // Held to top's `score` alone, so check says what run says.
    assert!(err.contains("score() is missing a value for parameter 'b'"), "{err}");
    assert!(err.contains("[line 6, column 7]"), "{err}");
    assert!(err.contains("1 error found by check"), "{err}");
    let (out, run_err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("3\n6\n1\n", 1));
    assert!(run_err.contains("score() is missing a value for parameter 'b'"), "{run_err}");
}

/// A module that calls a native it imported is held to that native's
/// declaration — not to a function some unrelated module, compiled earlier,
/// happens to declare under the same name. (The checker's table is keyed by
/// bare name across the program; found by a game whose `universe.body(id)`
/// made the host package's own `body(id, opts)` call an arity error, but only
/// when the game file imported `./universe` before the host package.)
#[test]
fn a_native_call_in_a_package_is_not_held_to_another_modules_function() {
    let t = Tree::new("nativename");
    t.write("host/petal.toml", "[package]\nname = \"host\"\nversion = \"0.1.0\"\n");
    t.write("host/host.ptl", "pub import ./natives\npub import ./helpers\n");
    // Bound to builtins, so the program also runs under `petal run`.
    t.write(
        "host/natives.ptl",
        "pub native fn body(id, opts) = \"concat\"\npub native fn show(...values) = \"print\"\n",
    );
    t.write(
        "host/helpers.ptl",
        "import ./natives\npub fn build(id)\n  return body([id], [\"built\"])\nend\n",
    );
    t.write(
        "game/petal.toml",
        "[package]\nname = \"game\"\nversion = \"0.1.0\"\n[dependencies]\nhost = { path = \"../host\" }\n",
    );
    t.write("game/universe.ptl", "pub fn body(only)\n  return \"universe\"\nend\n");
    // Either import order: `universe` compiles first in one, `host` in the other.
    for imports in ["import ./universe\nimport host\n", "import host\nimport ./universe\n"] {
        t.write(
            "game/main.ptl",
            &format!("{imports}show(build(1))\nshow(universe.body(2))\n"),
        );
        let (_, err, code) = petal_in(&t.dir("game"), &["check", "main.ptl"]);
        assert_eq!((err.as_str(), code), ("", 0), "{imports}");
        let (out, err, code) = petal_in(&t.dir("game"), &["run", "main.ptl"]);
        assert_eq!((out.as_str(), code), ("[1, \"built\"]\nuniverse\n", 0), "{imports}{err}");
    }
    // The native's own arity is still held where the name is the native: a
    // selective import pins `body` to the host's, with `universe` compiled
    // first and its one-argument `body` in the checker's table.
    t.write(
        "game/main.ptl",
        "import ./universe\nimport host\nimport host: body\nshow(body(1))\n",
    );
    let (_, err, code) = petal_in(&t.dir("game"), &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("expects 2 arguments, got 1"), "{err}");
}

/// The other side of the rule above: a `fn` of the module's own — top-level,
/// nested, or a parameter or local holding a function — takes a name from an
/// imported native, and its calls are held to *it*.
#[test]
fn a_modules_own_function_takes_the_name_of_an_imported_native() {
    let t = Tree::new("ownfn");
    t.write(
        "widgets.ptl",
        "import ui\n\
         pub fn hovered(x, y, w, h)\n  return x + y + w + h\nend\n\
         pub fn inside()\n  return hovered(1, 2, 3, 4)\nend\n",
    );
    t.write(
        "main.ptl",
        "import ./widgets\n\
         import ui\n\
         fn outer()\n  fn mouse_x(a, b)\n    return a + b\n  end\n  return mouse_x(1, 2)\nend\n\
         fn through(mouse_y)\n  return mouse_y(5)\nend\n\
         let scroll_x = fn(a) -> a * 2\n\
         print(inside(), outer(), through(fn(a) -> a), scroll_x(4))\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    // The import's native is still held to its declaration where nothing of
    // the module's own is in the way.
    t.write("bad.ptl", "import ./widgets\nimport ui\nfn f()\n  return mouse_dx(1)\nend\n");
    let (_, err, code) = petal_in(&t.root, &["check", "bad.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("`mouse_dx` expects 0 arguments, got 1"), "{err}");
}

/// A core builtin is held to its own declaration in every module that has
/// not taken the name: a function another module declares under it — private
/// to that module, even — is not what the call reaches, so check must not
/// hold the call to it (run never did).
#[test]
fn another_modules_function_does_not_take_a_builtin_from_this_one() {
    let t = Tree::new("builtinname");
    t.write(
        "models.ptl",
        "fn keys(a, b)\n  return a + b\nend\npub fn sum2()\n  return keys(1, 2)\nend\n",
    );
    t.write("main.ptl", "import ./models\nprint(keys({q: 1}), sum2())\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("[\"q\"] 3\n", 0), "{err}");
    // The builtin's own arity is still held here, and the function's there.
    t.write("main.ptl", "import ./models\nfn f()\n  return keys()\nend\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("`keys` expects 1 argument, got 0"), "{err}");
    t.write(
        "models.ptl",
        "fn keys(a, b)\n  return a + b\nend\npub fn sum2()\n  return keys(1)\nend\n",
    );
    t.write("main.ptl", "import ./models\nprint(keys({q: 1}))\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("expects 2 arguments, got 1"), "{err}");
    assert!(err.contains("models.ptl"), "{err}");
}

/// `m.f(...)` is a call to module `m`'s `f`. Check holds it to that
/// function: not to another module's `f` of the same arity (a private one
/// included), and not to whatever a bare `f` means in the calling file.
#[test]
fn a_qualified_call_is_held_to_the_module_its_qualifier_names() {
    let t = Tree::new("qualified");
    // Another module's private fn of the same name, one more parameter.
    t.write(
        "projects.ptl",
        "pub fn shares(d, extra = nil)\n  return d.n\nend\n\
         pub fn place(pos, yaw)\n  return pos + yaw\nend\n",
    );
    t.write(
        "people.ptl",
        "fn shares(ctx, yf)\n  return ctx.total * yf\nend\n\
         fn place(role, ctx)\n  return ctx.sys.x + role\nend\n\
         pub fn count(c)\n  return shares(c, 2) + place(1, {sys: {x: 1}})\nend\n",
    );
    t.write(
        "main.ptl",
        "import ./projects\nimport ./people\n\
         print(projects.shares({n: 3}), projects.place(1.0, 2.0), people.count({total: 2}))\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), err.as_str(), code), ("3 3.0 6\n", "", 0));

    // A builtin through its package's qualifier, with a sibling's export of
    // the same name plainly imported — and the sibling's through its own.
    t.write("ev.ptl", "pub fn log()\n  return \"ev\"\nend\n");
    t.write("main.ptl", "import ./ev\nimport core\nprint(core.log(1.0), ev.log())\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    let (out, _, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("0.0 ev\n", 0));
    t.write("main.ptl", "import ./ev\nimport core: log\nprint(log(1.0), ev.log())\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    let (out, _, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), code), ("0.0 ev\n", 0));

    // Each is still held to its own declaration.
    t.write("main.ptl", "import ./ev\nimport core\nfn f()\n  return [core.log(), ev.log(1)]\nend\n");
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("`log` expects 1 argument, got 0"), "{err}");
    assert!(err.contains("`log` expects 0 arguments, got 1"), "{err}");
    t.write(
        "main.ptl",
        "import ./projects\nimport ./people\nfn f()\n  return projects.shares()\nend\n",
    );
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("shares() is missing a value for parameter 'd'"), "{err}");
}

/// `let found = nil`, assigned in a branch or a loop and then passed on, is
/// the ordinary way to search for something. Check must not go on calling
/// the name `nil` after the assignment — nor on a line of a loop body above
/// it, which the next iteration reaches with the assigned value. A `let`
/// nothing ever assigns is still what its initializer says.
#[test]
fn a_reassigned_let_is_not_held_to_the_nil_it_started_as() {
    let t = Tree::new("reassigned");
    let id = "fn id_of(x)\n  x.id\nend\n";
    t.write(
        "main.ptl",
        &format!(
            "{id}fn find(xs)\n  let g = nil\n  for b in xs do\n    if b.id == 2 then g = b end\n  end\n  \
             if g == nil then return nil end\n  return id_of(g)\nend\n\
             fn carried(xs)\n  let prev = nil\n  let n = 0\n  for x in xs do\n    \
             if prev != nil then n = n + id_of(prev) end\n    prev = x\n  end\n  return n\nend\n\
             fn counted(xs)\n  let prev = nil\n  let i = 0\n  while i < len(xs) do\n    \
             if prev != nil then id_of(prev) end\n    prev = xs[i]\n    i = i + 1\n  end\n  return i\nend\n\
             let xs = [{{id: 1}}, {{id: 2}}]\nprint(find(xs), carried(xs), counted(xs))\n"
        ),
    );
    let (_, err, code) = petal_in(&t.root, &["check", "main.ptl"]);
    assert_eq!((err.as_str(), code), ("", 0));
    let (out, err, code) = petal_in(&t.root, &["run", "main.ptl"]);
    assert_eq!((out.as_str(), err.as_str(), code), ("2 1 2\n", "", 0));

    t.write("main.ptl", &format!("{id}let g = nil\nprint(id_of(g))\n"));
    let (_, err, _) = petal_in(&t.root, &["check", "main.ptl"]);
    assert!(err.contains("reads field `id` from it, found `nil`"), "{err}");
}

#[test]
fn assigning_through_a_qualifier_is_the_same_error_for_check_and_run() {
    let t = Tree::new("assign");
    t.write("hud.ptl", "pub let shared = 1\n");
    t.write("main.ptl", "import ./hud\nhud.shared = 5\nprint(hud.shared)\n");
    for cmd in ["check", "run"] {
        let (out, err, code) = petal_in(&t.root, &[cmd, "main.ptl"]);
        assert_eq!((out.as_str(), code), ("", 1), "{cmd}: {err}");
        assert!(err.contains("cannot assign through 'hud': it is a module"), "{cmd}: {err}");
    }
    // A qualifier read as a value: the hint spells the import as written.
    t.write("read.ptl", "import ./hud\nprint(hud)\n");
    for cmd in ["check", "run"] {
        let (out, err, code) = petal_in(&t.root, &[cmd, "read.ptl"]);
        assert_eq!((out.as_str(), code), ("", 1), "{cmd}: {err}");
        assert!(err.contains("'hud' is a module, not a value"), "{cmd}: {err}");
        assert!(err.contains("`import ./hud: <name>`"), "{cmd}: {err}");
    }
    // A member the module does not export.
    t.write("member.ptl", "import ./hud\nfn never()\n  return hud.nope\nend\nprint(1)\n");
    for cmd in ["check", "run"] {
        let (out, err, code) = petal_in(&t.root, &[cmd, "member.ptl"]);
        assert_eq!((out.as_str(), code), ("", 1), "{cmd}: {err}");
        assert!(err.contains("module 'hud' has no export 'nope'"), "{cmd}: {err}");
    }
}

#[test]
fn serialized_ir_is_linked_before_any_line_runs() {
    let t = Tree::new("ir");
    t.write("irs.ptl", "import ui\nprint(\"line one runs\")\nprint(mouse_x())\n");
    let (ir, err, code) = petal_in(&t.root, &["show-ir", "--json", "irs.ptl"]);
    assert_eq!(code, 0, "{err}");
    t.write("irs.json", &ir);
    let (out, err, code) = petal_in(&t.root, &["run", "--ir", "irs.json"]);
    assert_eq!((out.as_str(), code), ("", 1), "{err}");
    assert!(err.contains("this host does not register"), "{err}");
    assert!(err.contains("use a petal-ui runner"), "{err}");
    // A check of the same IR needs no host.
    let (_, err, code) = petal_in(&t.root, &["check", "--ir", "irs.json"]);
    assert_eq!(code, 0, "{err}");
    // IR that calls only what every Env registers still runs.
    t.write("ok.ptl", "import creative\nprint(lerp(0.0, 10.0, 0.5))\n");
    let (ir, _, _) = petal_in(&t.root, &["show-ir", "--json", "ok.ptl"]);
    t.write("ok.json", &ir);
    let (out, err, code) = petal_in(&t.root, &["run", "--ir", "ok.json"]);
    assert_eq!((out.as_str(), code), ("5.0\n", 0), "{err}");
}

#[test]
fn fix_imports_keeps_a_sibling_that_shares_a_bundled_packages_name() {
    let t = Tree::new("sibling");
    t.write("color.ptl", "pub fn tint()\n  return \"tint\"\nend\n");
    t.write("util.ptl", "pub fn u()\n  return 1\nend\n");
    // Old style: `color` here is the file beside this one.
    t.write("a.ptl", "import color\nimport util: *\nprint(color.tint())\nprint(u())\n");
    // New style: the bundled package, used as such. Must be left alone.
    t.write("b.ptl", "import color\nprint(color.hsv(0.0, 0.0, 1.0).r)\n");
    // Selective, naming something only the sibling exports.
    t.write("c.ptl", "import color: tint\nprint(tint())\n");
    // Nothing taken by name: no telling, so a note and no change.
    t.write("d.ptl", "import color\nprint(1)\n");

    let (out, err, code) = petal_in(&t.root, &["fix-imports", "."]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("`import color` -> `import ./color`"), "{out}");
    assert!(err.contains("d.ptl: line 1: `import color` is the package `color`"), "{err}");
    let read = |f: &str| std::fs::read_to_string(t.dir(f)).unwrap();
    assert_eq!(read("a.ptl"), "import ./color\nimport ./util\nprint(color.tint())\nprint(u())\n");
    assert_eq!(read("b.ptl"), "import color\nprint(color.hsv(0.0, 0.0, 1.0).r)\n");
    assert_eq!(read("c.ptl"), "import ./color: tint\nprint(tint())\n");
    assert_eq!(read("d.ptl"), "import color\nprint(1)\n");

    let (out, err, code) = petal_in(&t.root, &["run", "a.ptl"]);
    assert_eq!((out.as_str(), code), ("tint\n1\n", 0), "{err}");
    let (out, err, code) = petal_in(&t.root, &["run", "c.ptl"]);
    assert_eq!((out.as_str(), code), ("tint\n", 0), "{err}");
    // Idempotent.
    let (_, err, code) = petal_in(&t.root, &["fix-imports", "--check", "."]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn fix_imports_on_inline_code_sees_the_projects_dependencies() {
    let t = Tree::new("inline");
    t.write("libs/tween/petal.toml", "[package]\nname = \"tween\"\nversion = \"0.1.0\"\n");
    t.write("libs/tween/tween.ptl", "pub fn go()\n  return \"go\"\nend\n");
    t.write(
        "app/petal.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ntween = { path = \"../libs/tween\" }\n",
    );
    let (out, err, code) = petal_in(&t.dir("app"), &["fix-imports", "-e", "print(go())"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("import tween\n"), "{out}");
}

//! `const NAME = expr`: a `let` whose value must be static
//! (docs/language-guide.md, Constants).
//!
//! The contract under test: a `const` parses, formats, exports, hoists and
//! runs exactly as the `let` it otherwise is; the one difference is that a
//! compile ends by asking the static-value classifier about it, and reports
//! an error carrying the reason chain when the answer is "dynamic". Where the
//! answer rests on natives the compiling process does not have, nothing is
//! reported and the host decides.
//!
//! (Every parse in a debug build also compares the parser's own AST with the
//! CST projection's, so each program compiled here checks both pipelines.)

use petal::ast::StmtKind;
use petal::diagnostic::Diagnostic;
use petal::env::{Env, ReloadOutcome};
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::source_diff::{ConstructEdit, ConstructKind, SourceChange, diff_source};
use petal::source_map::ENTRY_FILE;
use petal::static_classify::{ConstCheck, DeclClass, DeclKind};

fn quiet() -> Env {
    let mut env = Env::new();
    env.set_echo(false);
    env
}

/// Run `src` and return what it printed.
fn run(src: &str) -> Vec<String> {
    let mut env = quiet();
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- {e}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{src}\n-- {e}"));
    env.take_output()
}

/// The diagnostics of compiling `src` in `env`.
fn diagnostics_in(env: &mut Env, src: &str) -> Vec<Diagnostic> {
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- {e}"));
    env.get_program(pid).unwrap().warnings.clone()
}

/// The error messages of compiling `src` with no host.
fn errors(src: &str) -> Vec<String> {
    diagnostics_in(&mut quiet(), src)
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| d.message)
        .collect()
}

/// The error messages of compiling `src` the way `petal check` and the
/// language server do: nothing is linked, so a `native fn` has no host.
fn check_errors(src: &str) -> Vec<String> {
    let mut env = quiet();
    env.set_check_only(true);
    diagnostics_in(&mut env, src)
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| d.message)
        .collect()
}

fn parse(src: &str) -> Vec<petal::ast::Stmt> {
    petal::rewrite::parse_ast(src).unwrap_or_else(|e| panic!("{src}\n-- {e}")).1
}

// ---------------------------------------------------------------------------
// Syntax
// ---------------------------------------------------------------------------

#[test]
fn const_is_a_let_with_a_flag() {
    let stmts = parse("const A = 1\npub const B: int = 2\nlet c = 3\n");
    let flags: Vec<(String, bool, bool)> = stmts
        .iter()
        .map(|s| match &s.kind {
            StmtKind::Let { name, is_const, .. } => (name.clone(), *is_const, s.exported),
            other => panic!("not a binding: {other:?}"),
        })
        .collect();
    assert_eq!(
        flags,
        [
            ("A".to_string(), true, false),
            ("B".to_string(), true, true),
            ("c".to_string(), false, false),
        ]
    );
    let StmtKind::Let { ty, .. } = &stmts[1].kind else { unreachable!() };
    assert_eq!(ty.as_ref().map(|t| t.name.as_str()), Some("int"));
}

#[test]
fn const_stays_usable_as_a_field_and_a_record_key() {
    assert_eq!(run("let r = {const: 3}\nprint(r.const)"), ["3"]);
}

#[test]
fn const_is_no_longer_a_name() {
    let err = quiet().load_program("let const = 1").unwrap_err();
    assert!(err.contains("const"), "{err}");
}

#[test]
fn a_const_needs_an_initializer() {
    let err = quiet().load_program("const A").unwrap_err();
    assert!(err.contains("`const A` needs an initializer; write `const A = ...`"), "{err}");
}

#[test]
fn config_does_not_combine_with_const() {
    for src in ["config const A = 1", "pub config const A = 1"] {
        let err = quiet().load_program(src).unwrap_err();
        assert!(err.contains("config"), "{src}: {err}");
    }
    // The two that exist.
    assert_eq!(errors("config let A = 1\nconst B = A + 1\nprint(B)"), Vec::<String>::new());
}

#[test]
fn the_formatter_treats_const_like_let() {
    let fmt = |src: &str| petal::fmt::format_source(src).unwrap();
    assert_eq!(fmt("const   A=[1,2]\n"), "const A = [1, 2]\n");
    assert_eq!(fmt("pub  const A:int=1\n"), "pub const A: int = 1\n");
    assert_eq!(fmt("export const A = 1\n"), "pub const A = 1\n");
    assert_eq!(
        fmt("const T = [\n{a: 1},\n{a: 2},\n]\n"),
        "const T = [\n  {a: 1},\n  {a: 2},\n]\n"
    );
}

// ---------------------------------------------------------------------------
// It runs as the let it is
// ---------------------------------------------------------------------------

#[test]
fn a_const_evaluates_like_a_let() {
    let src = "\
fn codes(xs)
  map(xs, fn(x) -> x.code)
end
const TILES = [{code: 1}, {code: 2}]
const CODES = codes(TILES)
const TOTAL = reduce(CODES, 0, fn(a, b) -> a + b)
print(CODES, TOTAL)
";
    assert_eq!(run(src), run(&src.replace("const ", "let ")));
    assert_eq!(errors(src), Vec::<String>::new());
}

#[test]
fn a_literal_const_is_hoisted_like_a_literal_let() {
    // The call is above the declarations, which a hoisted function reads.
    let src = "print(area())\nconst W = 3\nconst H = W + 1\nfn area() W * H end\n";
    assert_eq!(run(src), ["12"]);
    assert_eq!(run(&src.replace("const ", "let ")), ["12"]);
    assert_eq!(errors(src), Vec::<String>::new());
}

#[test]
fn a_const_is_exported_like_a_let() {
    let lib = "pub const LIMIT = 10\nconst HIDDEN = 1\npub fn twice() LIMIT * 2 end\n";
    let mut env = quiet();
    env.register_module("lib", lib);
    let pid = env.load_program("import lib\nprint(lib.LIMIT, lib.twice())").unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["10 20"]);

    let mut env = quiet();
    env.register_module("lib", lib);
    let private = env.load_program("import lib\nprint(lib.HIDDEN)");
    let reached = match private {
        Err(_) => false,
        Ok(pid) => !env.get_program(pid).unwrap().warnings.iter().any(|d| d.is_error()),
    };
    assert!(!reached, "a const without `pub` must not be importable");
}

#[test]
fn a_const_in_a_module_is_checked_too() {
    let mut env = quiet();
    env.register_module("lib", "state n = 0\npub const BAD = n + 1\n");
    let found = diagnostics_in(&mut env, "import lib\nprint(lib.BAD)");
    assert!(
        found.iter().any(|d| d.is_error() && d.message.contains("`BAD` is declared `const`")),
        "{found:?}"
    );
}

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

#[test]
fn a_dynamic_const_is_an_error_with_the_reason_chain() {
    let src = "\
state events = []
fn build_label_w(evs)
  print(\"measuring\")
  len(evs)
end
const LAB_W = build_label_w(events)
";
    let found = errors(src);
    assert_eq!(found.len(), 1, "{found:?}");
    let message = &found[0];
    let lines: Vec<&str> = message.lines().collect();
    assert_eq!(lines[0], "`LAB_W` is declared `const` but is not constant");
    assert!(lines.len() >= 2, "{message}");
    assert!(lines[1..].iter().all(|l| l.starts_with("  = ")), "{message}");
    // The chain ends at a root cause and cites where each link is written.
    assert!(message.contains("at line 6"), "{message}");
    assert!(
        message.contains("events is state") || message.contains("calls print"),
        "{message}"
    );
}

#[test]
fn the_chain_crosses_functions_down_to_the_native() {
    let src = "\
fn inner(x)
  print(x)
  x
end
fn outer(x)
  inner(x) + 1
end
const A = outer(2)
";
    let found = errors(src);
    assert_eq!(
        found,
        ["`A` is declared `const` but is not constant\n  \
          = A calls outer at line 8\n  \
          = outer calls inner at line 6\n  \
          = inner calls print, which is not declared constant at line 2"]
    );
}

#[test]
fn each_dynamic_input_is_rejected() {
    for (src, expect) in [
        ("state n = 0\nconst A = n", "n is state"),
        ("var v = 0\nconst A = get v", "var"),
        ("const A = random()", "random"),
        ("const A = [1, 2]\nA = [3]", "assigned again"),
        ("const A = [1, 2]\nA[0] = 3", "assigned again"),
        ("let xs = []\nfor i in range(3) do xs = append(xs, i) end\nconst A = xs", "assigned again"),
    ] {
        let found = errors(src);
        assert_eq!(found.len(), 1, "{src}: {found:?}");
        assert!(found[0].contains("is declared `const` but is not constant"), "{src}: {found:?}");
        assert!(found[0].contains(expect), "{src}: expected `{expect}` in {found:?}");
    }
}

#[test]
fn a_compile_error_inside_a_const_is_reported_once() {
    // The unknown name is the finding; "not constant because it has a
    // compile error" on top of it would be the same finding twice.
    let found = errors("const A = nope + 1\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("nope"), "{found:?}");
    assert!(!found[0].contains("declared `const`"), "{found:?}");
}

#[test]
fn a_function_is_not_a_constant_value() {
    let found = errors("const f = fn(x) -> x + 1\nprint(f(1))");
    assert_eq!(
        found,
        ["`f` is declared `const` but is a function, not a constant value; declare it with `fn` or `let`"]
    );
    let found = errors("fn g(x) x end\nconst T = [g]\n");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("holds a function"), "{found:?}");
}

#[test]
fn a_const_must_be_at_the_top_level() {
    for src in [
        "fn f()\n  const A = 1\n  A\nend\n",
        "if true then\n  const A = 1\nend\n",
        "for i in range(2) do\n  const A = i\nend\n",
        "let f = fn()\n  const A = 1\n  A\nend\n",
    ] {
        let found = errors(src);
        assert_eq!(found.len(), 1, "{src}: {found:?}");
        assert!(
            found[0].starts_with("`const A` must be declared at the top level of a file"),
            "{src}: {found:?}"
        );
    }
}

#[test]
fn a_dynamic_const_still_runs_as_a_let_for_a_host_that_runs_anyway() {
    // A host that prints the findings of a reload and keeps the frame alive
    // gets the value the `let` would have had.
    let mut env = quiet();
    let src = "state n = 41\nconst A = n + 1\nprint(A)";
    let pid = env.load_program(src).unwrap();
    assert!(!env.check_program(pid).passed());
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["42"]);
}

#[test]
fn the_error_is_what_check_prints() {
    let mut env = quiet();
    env.set_check_only(true);
    let pid = env.load_program("state n = 0\nconst A = n + 1\n").unwrap();
    let report = env.check_program(pid);
    assert!(!report.passed());
    assert_eq!(
        report.render(env.get_program(pid).unwrap()),
        "error: `A` is declared `const` but is not constant\n  \
         = A reads n at line 2\n  \
         = n is state, which changes between runs at line 1\n \
         --> [line 2, column 11]\n  |\n2 | const A = n + 1\n  |           ^^^^^\n"
    );
}

// ---------------------------------------------------------------------------
// Natives: registered, constant, and missing
// ---------------------------------------------------------------------------

fn seven(cxt: &mut PetalCxt) -> NativeResult {
    cxt.push_int(7);
    Ok(1)
}

const HOST: &str = "native fn tile_count()\nnative fn frame_count()\n";

/// A host with one constant native and one that is not.
fn host() -> Env {
    let mut env = quiet();
    env.register_native("tile_count", seven, NativeEffects::CONST);
    env.register_native("frame_count", seven, NativeEffects::PURE);
    env
}

#[test]
fn a_host_native_is_constant_only_when_registered_const() {
    let ok = format!("{HOST}const N = tile_count() * 2\nprint(N)");
    assert_eq!(
        diagnostics_in(&mut host(), &ok).iter().filter(|d| d.is_error()).count(),
        0
    );

    let bad = format!("{HOST}const N = frame_count() * 2\nprint(N)");
    let found: Vec<String> = diagnostics_in(&mut host(), &bad)
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| d.message)
        .collect();
    assert_eq!(
        found,
        ["`N` is declared `const` but is not constant\n  \
          = N calls frame_count, which is not declared constant at line 3"]
    );
}

#[test]
fn without_the_host_a_const_on_its_natives_is_not_judged() {
    // `petal check` and the language server: neither native is registered,
    // so neither declaration can be verified, and neither is reported —
    // including the one its host rejects (the test above).
    for native in ["tile_count", "frame_count"] {
        let src = format!("{HOST}const N = {native}() * 2\nprint(N)");
        assert_eq!(check_errors(&src), Vec::<String>::new(), "{native}");
        let mut env = quiet();
        env.set_check_only(true);
        let pid = env.load_program(&src).unwrap();
        let report = env.static_report(env.get_program(pid).unwrap());
        let n = report.get("N").unwrap();
        assert_eq!(n.kind, DeclKind::Const);
        assert_eq!(n.class, DeclClass::Dynamic, "never called static without its host");
        assert_eq!(n.const_check, Some(ConstCheck::Unverified));
    }
}

#[test]
fn without_the_host_a_const_that_no_host_could_accept_is_still_an_error() {
    // The missing native is not the only reason: `n` is state whatever the
    // host registers. The chain names that reason, not the native.
    let src = format!("{HOST}state n = 0\nconst A = tile_count() + n\n");
    let found = check_errors(&src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("n is state"), "{found:?}");
    assert!(!found[0].contains("tile_count"), "{found:?}");

    // Through a function, and through a callback handed to the native.
    let src = format!("{HOST}fn noisy()\n  print(1)\n  tile_count()\nend\nconst A = noisy()\n");
    let found = check_errors(&src);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("calls print"), "{found:?}");
}

#[test]
fn check_never_accepts_what_a_host_rejects_as_verified() {
    // The property behind "cannot verify here": under check-only a const is
    // marked `Constant` only if it is static with *no* host natives at all,
    // and then every host agrees.
    let src = format!("{HOST}const A = [1, 2, 3]\nconst B = len(A) + tile_count()\n");
    let mut env = quiet();
    env.set_check_only(true);
    let pid = env.load_program(&src).unwrap();
    let report = env.static_report(env.get_program(pid).unwrap());
    assert_eq!(report.get("A").unwrap().const_check, Some(ConstCheck::Constant));
    assert_eq!(report.get("B").unwrap().const_check, Some(ConstCheck::Unverified));

    let mut env = host();
    let pid = env.load_program(&src).unwrap();
    let report = env.static_report(env.get_program(pid).unwrap());
    assert_eq!(report.get("A").unwrap().const_check, Some(ConstCheck::Constant));
    assert_eq!(report.get("B").unwrap().const_check, Some(ConstCheck::Constant));
}

// ---------------------------------------------------------------------------
// show-static
// ---------------------------------------------------------------------------

#[test]
fn show_static_reports_the_kind_and_the_check() {
    let mut env = quiet();
    env.set_check_only(true);
    let src = format!(
        "{HOST}state n = 0\nconst GOOD = [1, 2]\nconst BAD = n\nconst HOSTED = tile_count()\nprint(GOOD)\n"
    );
    let pid = env.load_program(&src).unwrap();
    let report = env.static_report(env.get_program(pid).unwrap());
    let text = report.render_text(false);
    assert!(text.contains("static    const GOOD"), "{text}");
    assert!(text.contains("dynamic   const BAD"), "{text}");
    assert!(text.contains("! declared const but not constant: a compile error"), "{text}");
    assert!(
        text.contains("! declared const: cannot be verified without its host's natives"),
        "{text}"
    );
    let json = report.to_json(false);
    let decl = |name: &str| {
        json["declarations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(decl("GOOD")["kind"], "const");
    assert_eq!(decl("GOOD")["const_check"], "constant");
    assert_eq!(decl("BAD")["const_check"], "not_constant");
    assert_eq!(decl("HOSTED")["const_check"], "unverified");
    assert!(decl("n").get("const_check").is_none());
}

// ---------------------------------------------------------------------------
// Hot reload
// ---------------------------------------------------------------------------

fn kinds(old: &str, new: &str) -> Vec<(ConstructKind, Option<String>, ConstructEdit)> {
    match diff_source(old, new, ENTRY_FILE).change {
        SourceChange::Constructs(c) => c.into_iter().map(|c| (c.kind, c.name, c.edit)).collect(),
        other => panic!("expected a structural change, got {other:?}"),
    }
}

#[test]
fn the_source_diff_names_a_const_as_one() {
    assert_eq!(
        kinds("const A = [1]\nprint(A)\n", "const A = [1]\nconst B = len(A)\nprint(A)\n"),
        [(ConstructKind::Const, Some("B".to_string()), ConstructEdit::Added)]
    );
    // `let` <-> `const` is a structural edit: only a recompile runs the check.
    assert_eq!(
        kinds("let A = [1]\nprint(A)\n", "const A = [1]\nprint(A)\n"),
        [(ConstructKind::Const, Some("A".to_string()), ConstructEdit::Changed)]
    );
}

#[test]
fn a_value_edit_inside_a_const_is_a_value_edit_like_its_let() {
    for kw in ["const", "let"] {
        let old = format!("{kw} A = [1, 2]\nprint(A)\n");
        let new = format!("{kw} A = [1, 5]\nprint(A)\n");
        assert!(
            matches!(diff_source(&old, &new, ENTRY_FILE).change, SourceChange::Values(_)),
            "{kw}"
        );
    }
}

struct World {
    env: Env,
    pid: petal::program::ProgramId,
    sid: petal::stack::StackKey,
}

fn world(src: &str) -> World {
    let mut env = quiet();
    let pid = env.load_program(src).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    env.take_output();
    World { env, pid, sid }
}

#[test]
fn a_reloaded_const_follows_its_edit_where_a_state_does_not() {
    let mut w = world("const A = [1, 2]\nstate S = [1, 2]\nprint(A, S)\n");
    let report = w
        .env
        .reload_program(w.sid, "const A = [1, 5]\nstate S = [1, 5]\nprint(A, S)\n", None)
        .unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Patched);
    w.env.run(w.sid).unwrap();
    assert_eq!(w.env.take_output(), ["[1, 5] [1, 2]"]);
}

#[test]
fn a_layout_only_reload_moves_the_error_and_the_lines_it_cites() {
    let old = "state n = 0\nconst A = n + 1\nprint(A)\n";
    let new = "// a comment\n\nstate n = 0\n\nconst A = n + 1\nprint(A)\n";
    let mut w = world(old);
    let report = w.env.reload_program(w.sid, new, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Relocated);
    let pid = w.pid;
    let moved: Vec<Diagnostic> = w
        .env
        .get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .filter(|d| d.is_error())
        .cloned()
        .collect();
    // What a fresh compile of the new text reports.
    let fresh: Vec<Diagnostic> = diagnostics_in(&mut quiet(), new)
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].message, fresh[0].message);
    assert_eq!(moved[0].span, fresh[0].span);
    assert!(moved[0].message.contains("A reads n at line 5"), "{}", moved[0].message);
    assert!(moved[0].message.contains("at line 3"), "{}", moved[0].message);
}

#[test]
fn making_a_const_dynamic_by_an_edit_is_reported_by_the_reload() {
    let mut w = world("fn build() [1, 2] end\nconst A = build()\nprint(A)\n");
    let new = "fn build()\n  print(\"building\")\n  [1, 2]\nend\nconst A = build()\nprint(A)\n";
    let report = w.env.reload_program(w.sid, new, None).unwrap();
    assert_eq!(report.outcome, ReloadOutcome::Recompiled);
    let pid = w.pid;
    let text = w.env.diagnostics_text(pid);
    assert!(text.contains("error: `A` is declared `const` but is not constant"), "{text}");
    assert!(text.contains("build calls print"), "{text}");
}

// ---------------------------------------------------------------------------
// The config-value API reaches a const as it reaches a let
// ---------------------------------------------------------------------------

#[test]
fn rewrite_finds_a_const_binding() {
    let src = "const SIZE = 14\nprint(SIZE)\n";
    let stmts = parse(src);
    let span = petal::rewrite::find_binding(&stmts, "SIZE").expect("found");
    assert_eq!(&src[span.start.offset as usize..span.end.offset as usize], "14");
}

// ---------------------------------------------------------------------------
// The static-value cache keeps a const exactly as it keeps its let
// ---------------------------------------------------------------------------

/// The names of the filled slots of `src`'s stack after two frames with the
/// static-value cache on, and what the two frames printed.
fn kept(src: &str) -> (Vec<String>, Vec<String>) {
    let mut env = quiet();
    env.set_static_cache(true);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{src}\n-- {e}"));
    let sid = env.create_stack(pid).unwrap();
    let mut printed = Vec::new();
    for _ in 0..2 {
        env.reset_stack(sid).unwrap();
        env.run(sid).unwrap();
        printed.extend(env.take_output());
    }
    let mut names: Vec<String> = env
        .static_values(sid)
        .expect("a lowered stack")
        .slots
        .into_iter()
        .filter(|(_, state)| matches!(state, petal::env::StaticSlotState::Filled(_)))
        .filter_map(|(info, _)| info.name)
        .collect();
    names.sort();
    (names, printed)
}

#[test]
fn the_static_cache_keeps_a_const_as_it_keeps_its_let() {
    let src = "\
fn row(n)
  {n: n, sq: n * n}
end
const ROWS = [row(1), row(2), row(3)]
const SQUARES = map(ROWS, fn(r) -> r.sq)
state frame = 0
frame = frame + 1
print(SQUARES[frame % 3], len(ROWS))
";
    let as_const = kept(src);
    let as_let = kept(&src.replace("const ", "let "));
    assert_eq!(as_const, as_let);
    assert!(as_const.0.contains(&"SQUARES".to_string()), "{:?}", as_const.0);
    assert_eq!(as_const.1, ["4 3", "9 3"]);
}

// ── A `const` that is constant and still built on every run ────────────────

/// The warnings (not the errors) of compiling `src` with no host.
fn warnings(src: &str) -> Vec<Diagnostic> {
    diagnostics_in(&mut quiet(), src)
        .into_iter()
        .filter(|d| !d.is_error())
        .collect()
}

#[test]
fn a_const_whose_build_is_handed_to_append_is_warned_about() {
    // The list `append` is handed may be updated in place, so it gets no
    // slot: `SKINS` is kept and the comprehension runs on every frame all
    // the same. `petal show-static` could not say so (the list has no name).
    let src = "\
const PAL = {paper: 1}
const IDS = [\"a\", \"b\"]
const SKINS = append(for id in IDS do {...PAL, id: id} end, PAL)
print(len(SKINS))
";
    let w = warnings(src);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].span.start.line, 3);
    assert!(
        w[0].message.starts_with(
            "`SKINS` is declared `const`, but part of what builds it is built again on every run: \
             the value written on line 3 is handed to `append`"
        ),
        "{}",
        w[0].message
    );
    assert!(w[0].message.contains("`concat(xs, [v])`"), "{}", w[0].message);
    // It is a warning: the value is right, and the program runs.
    assert_eq!(errors(src), Vec::<String>::new());
    assert_eq!(run(src), vec!["3"]);
    // The line it cites follows a layout edit (it is a cited line, not text).
    assert!(w[0].cited.is_some());
}

#[test]
fn a_const_that_builds_a_new_list_is_not_warned_about() {
    for src in [
        "const IDS = [1, 2]\nconst ALL = concat(for i in IDS do [i] end, [[9]])\nprint(ALL)\n",
        "const IDS = [1, 2]\nconst ALL = (for i in IDS do i * 2 end) ++ [9]\nprint(ALL)\n",
        "const IDS = [1, 2]\nconst N = len(IDS)\nconst S = sort(IDS)\nprint(N, S)\n",
        // An `append` inside a function works on the function's own value.
        "fn grown(xs) append(xs, 9) end\nconst IDS = [1, 2]\nprint(grown(IDS))\n",
    ] {
        let w = warnings(src);
        assert!(
            !w.iter().any(|d| d.message.contains("built again on every run")),
            "{src}: {w:?}"
        );
    }
}

#[test]
fn a_named_const_handed_to_append_is_the_one_warned_about() {
    // `T` is what gets no slot; `U`, built from it, is kept and says nothing.
    let src = "\
const IDS = [1, 2]
const T = for i in IDS do [i] end
const U = append(T, [9])
print(len(U))
";
    let w = warnings(src);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].span.start.line, 2);
    assert!(
        w[0].message.starts_with(
            "`T` is declared `const` but is built again on every run: T is passed to append, \
             which may update it in place on line 3"
        ),
        "{}",
        w[0].message
    );
}

#[test]
fn a_const_built_from_a_let_that_is_not_kept_names_the_let() {
    // A `let` makes no promise, so it gets no warning of its own; the
    // `const` built from it does.
    let src = "\
const IDS = [1, 2]
let L = for i in IDS do [i] end
const V = append(L, [9])
print(len(V))
";
    let w = warnings(src);
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].span.start.line, 3);
    assert!(
        w[0].message.starts_with(
            "`V` is declared `const`, but `L`, which it is built from, is built again on every run"
        ),
        "{}",
        w[0].message
    );
}

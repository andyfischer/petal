//! `native fn` declarations: the syntax, what a declared name binds to,
//! overload sets mixing natives with Petal `fn`s, the signatures the checker
//! reads, and linking — a program loaded to run needs every native it refers
//! to registered. See docs/ffi.md (Native interfaces).

use petal::ast::{Stmt, StmtKind};
use petal::env::Env;
use petal::error::Phase;
use petal::native_fn::{NativeEffects, NativeResult, PetalCxt};
use petal::typecheck::globals::{self, HostProfile};

// ── Helpers ──────────────────────────────────────────────────────────────

fn parse(src: &str) -> Vec<Stmt> {
    petal::cst::parse_source(src, petal::source_map::ENTRY_FILE)
        .unwrap_or_else(|e| panic!("{src:?} should parse: {e}"))
        .1
}

fn parse_err(src: &str) -> String {
    petal::cst::parse_source(src, petal::source_map::ENTRY_FILE)
        .err()
        .unwrap_or_else(|| panic!("{src:?} should not parse"))
}

/// The one `native fn` of `src`.
fn decl(src: &str) -> petal::ast::NativeFnDecl {
    match parse(src).into_iter().next().map(|s| s.kind) {
        Some(StmtKind::NativeFn(decl)) => decl,
        other => panic!("{src:?}: expected a native fn, got {other:?}"),
    }
}

/// Run `src` in a fresh `Env` and return what it printed.
fn run(src: &str) -> Vec<String> {
    run_in(&mut Env::new(), src)
}

fn run_in(env: &mut Env, src: &str) -> Vec<String> {
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("{src:?} should load: {e}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{src:?} should run: {e}"));
    env.take_output()
}

fn load_err(src: &str) -> String {
    Env::new()
        .load_program(src)
        .err()
        .unwrap_or_else(|| panic!("{src:?} should not load"))
}

/// The checker's warnings for `src`, compiled without linking.
fn warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_check_only(true);
    let pid = env
        .load_program(src)
        .unwrap_or_else(|e| panic!("{src:?} should compile: {e}"));
    env.get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// What `petal check` adds on top: names nothing defines, and named arguments
/// a native cannot take.
fn unresolved(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_check_only(true);
    let pid = env.load_program(src).expect("compiles");
    let host = globals::host_names(HostProfile::Core, &[]);
    globals::unresolved_globals(
        env.get_program(pid).unwrap(),
        |n| env.native_signatures(n),
        &host,
    )
    .into_iter()
    .map(|d| d.message)
    .collect()
}

fn host_add(state: &mut PetalCxt) -> NativeResult {
    let a = state.get_int(1)?;
    let b = if state.arg_count() >= 2 { state.get_int(2)? } else { 100 };
    state.push_int(a + b);
    Ok(1)
}

fn host_count(state: &mut PetalCxt) -> NativeResult {
    state.push_int(state.arg_count() as i64);
    Ok(1)
}

// ── Syntax ───────────────────────────────────────────────────────────────

#[test]
fn a_declaration_has_a_name_parameters_and_a_result_type() {
    let d = decl("native fn clamp01(value: num, lo: int, hi) -> float");
    assert_eq!(d.name, "clamp01");
    assert_eq!(d.host_name(), "clamp01");
    let params: Vec<(&str, Option<&str>, bool)> = d
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.ty.as_ref().map(|t| t.name.as_str()), p.optional))
        .collect();
    assert_eq!(
        params,
        [
            ("value", Some("num"), false),
            ("lo", Some("int"), false),
            ("hi", None, false)
        ]
    );
    assert_eq!(d.ret.unwrap().name, "float");
    assert!(d.rest.is_none() && d.host.is_none() && d.class.is_none());
}

#[test]
fn optional_rest_and_host_name_forms() {
    let d = decl("native fn slice(collection, start, end?: num)");
    assert_eq!(d.required(), 2);
    assert!(d.params[2].optional);
    // A parameter name is an argument label, so a keyword works.
    assert_eq!(d.params[2].name, "end");
    assert_eq!(decl("native fn range(start, end, step?)").params[1].name, "end");

    let d = decl("native fn format(template: string, ...args) -> string");
    assert_eq!(d.rest.as_deref(), Some("args"));
    assert_eq!(d.params.len(), 1);
    assert_eq!(decl("native fn print(...values)").params.len(), 0);

    let d = decl("native fn _native_rect(x, y, a?) = \"draw_rect\"");
    assert_eq!(d.name, "_native_rect");
    assert_eq!(d.host_name(), "draw_rect");

    let d = decl("native fn Rect.inset(r, n)");
    assert_eq!(d.name, "Rect.inset");
    assert_eq!(d.class.as_deref(), Some("Rect"));
}

#[test]
fn pub_marks_a_declaration_exported() {
    let stmts = parse("pub native fn mouse_x() -> int\nnative fn hidden()");
    assert!(stmts[0].exported);
    assert!(!stmts[1].exported);
}

#[test]
fn a_declaration_has_no_body_and_no_end() {
    // The next line is the next statement.
    let stmts = parse("native fn one() -> int\nnative fn two()\nlet x = 1");
    assert_eq!(stmts.len(), 3);
    assert!(matches!(stmts[2].kind, StmtKind::Let { .. }));
    let err = parse_err("native fn one() 1 end");
    assert!(err.contains("A native fn declaration has no body"), "{err}");
}

#[test]
fn native_is_only_a_keyword_before_fn() {
    assert_eq!(
        run("let native = 2\nfn twice(native) native * 2 end\nprint(native, twice(native), {native: 1}.native)"),
        ["2 4 1"]
    );
}

#[test]
fn malformed_declarations_are_refused_with_a_reason() {
    for (src, want) in [
        (
            "native fn f(a = 1)",
            "A native fn parameter cannot have a default value — write 'a?'",
        ),
        (
            "native fn f(a?, b)",
            "Parameter 'b' is required but follows 'a?', which is optional",
        ),
        (
            "native fn f(...rest, a)",
            "The rest parameter '...rest' must be the last parameter",
        ),
        ("native fn f(a, a)", "Parameter 'a' is declared twice"),
        (
            "native fn f(a) = other",
            "Expected the host native's name as a string after '='",
        ),
        ("native fn (a)", "Expected an identifier"),
        ("pub native 1", "`pub` must be followed by"),
    ] {
        let err = parse_err(src);
        assert!(err.contains(want), "{src:?}: {err}");
    }
}

#[test]
fn the_cst_round_trips_and_keeps_comments() {
    let src = "// the interface\npub native fn mouse_x() -> int  // pixels\n\
               native fn slice(collection, start, end?: num)\n\
               native fn print(...values)\n\
               native fn _r(x, y) = \"draw_rect\"\n\
               print(1)\n";
    let green = petal::cst::parse_cst(src).expect("parses");
    assert_eq!(green.text(), src);
}

#[test]
fn the_formatter_does_not_indent_after_a_declaration() {
    let src = "native fn a(x)\n  native fn b(x: num,y?)->int\nfn f()\nprint(1)\nend\n";
    let out = petal::fmt::format_source(src).expect("formats");
    assert_eq!(
        out,
        "native fn a(x)\nnative fn b(x: num, y?) -> int\nfn f()\n  print(1)\nend\n"
    );
    // Formatting again changes nothing.
    assert_eq!(petal::fmt::format_source(&out).unwrap(), out);
}

#[test]
fn show_ast_prints_a_declaration_as_written() {
    let stmts = parse("pub native fn f(a: num, b?, ...rest) -> int = \"g\"");
    let text = petal::ast_display::display_stmts(&stmts);
    assert!(
        text.contains("NativeFn pub f (a: num, b?, ...rest) -> int = \"g\""),
        "{text}"
    );
}

// ── Binding ──────────────────────────────────────────────────────────────

#[test]
fn a_declared_name_calls_the_host_native() {
    // A second name for a registered builtin: same native, same result.
    assert_eq!(
        run("native fn root(x: num) -> float = \"sqrt\"\nprint(root(16), sqrt(16))"),
        ["4.0 4.0"]
    );
}

#[test]
fn a_declaration_binds_file_wide_and_inside_functions() {
    // Used above the declaration, and from a function body.
    let src = "fn hyp(a, b) root(a * a + b * b) end\n\
               print(root(9), hyp(3, 4))\n\
               native fn root(x: num) -> float = \"sqrt\"";
    assert_eq!(run(src), ["3.0 5.0"]);
}

#[test]
fn a_declared_native_is_a_first_class_value() {
    let src = "native fn root(x: num) -> float = \"sqrt\"\n\
               let f = root\n\
               let table = {op: root}\n\
               print(f(25), table.op(4), type(f))";
    assert_eq!(run(src), ["5.0 2.0 function"]);
}

#[test]
fn a_host_native_is_reached_through_its_declaration() {
    let mut env = Env::new();
    env.register_native("host_add", host_add, NativeEffects::PURE);
    let src = "native fn add(a: num, b?: num) -> int = \"host_add\"\nprint(add(1, 2), add(1))";
    assert_eq!(run_in(&mut env, src), ["3 101"]);
}

#[test]
fn a_call_through_a_declaration_is_a_static_builtin_call() {
    let mut env = Env::new();
    let pid = env
        .load_program("native fn root(x: num) = \"sqrt\"\nprint(root(4))")
        .unwrap();
    let ir = petal::ir_display::display_program(env.get_program(pid).unwrap());
    // The call names the host native, not the script's name for it.
    assert!(ir.contains("BuiltinCall(\"sqrt\")"), "{ir}");
    assert!(!ir.contains("BuiltinCall(\"root\")"), "{ir}");
}

#[test]
fn named_arguments_bind_against_the_declaration() {
    let mut env = Env::new();
    env.register_native("host_add", host_add, NativeEffects::PURE);
    let src = "native fn add(first: num, second?: num) -> int = \"host_add\"\n\
               print(add(second: 2, first: 40), add(first: 1))";
    assert_eq!(run_in(&mut env, src), ["42 101"]);
}

#[test]
fn a_rest_native_takes_any_count_and_no_names() {
    let mut env = Env::new();
    env.register_native("host_count", host_count, NativeEffects::PURE);
    let src = "native fn count(...values) -> int = \"host_count\"\nprint(count(), count(1, 2, 3))";
    assert_eq!(run_in(&mut env, src), ["0 3"]);
    let pid = env
        .load_program("native fn count(...values) = \"host_count\"\ncount(values: 1)")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    let err = env.run(sid).unwrap_err();
    assert!(err.contains("does not accept named arguments"), "{err}");
}

#[test]
fn several_declarations_are_call_forms_of_one_native() {
    let mut env = Env::new();
    env.register_native("host_add", host_add, NativeEffects::PURE);
    // The forms may overlap in count: they are one native, told apart by the
    // names a call uses.
    let src = "native fn add(a: num) = \"host_add\"\n\
               native fn add(left: num, right: num) = \"host_add\"\n\
               print(add(1), add(a: 2), add(right: 1, left: 5))";
    assert_eq!(run_in(&mut env, src), ["101 102 6"]);
}

#[test]
fn one_name_cannot_bind_two_host_natives() {
    let err = load_err("native fn f(x) = \"sqrt\"\nnative fn f(x, y) = \"pow\"");
    assert!(
        err.contains("native fn 'f' is declared for two host natives, 'sqrt' and 'pow'"),
        "{err}"
    );
}

#[test]
fn a_declaration_belongs_at_the_top_level() {
    let err = load_err("fn f()\n  native fn inner(x) = \"sqrt\"\n  1\nend");
    assert!(
        err.contains("native fn 'inner' must be declared at the top level of a file"),
        "{err}"
    );
}

#[test]
fn a_local_binding_shadows_a_declared_native() {
    let src = "native fn root(x: num) = \"sqrt\"\n\
               fn f(root) root(2) end\n\
               print(f(fn(x) -> x * 10))";
    assert_eq!(run(src), ["20"]);
}

// ── Overload sets mixing natives and Petal fns ───────────────────────────

#[test]
fn a_native_shares_an_overload_set_with_petal_fns() {
    let src = "native fn area(x: num) -> float = \"sqrt\"\n\
               fn area(w, h) w * h end\n\
               print(area(16), area(3, 4))";
    assert_eq!(run(src), ["4.0 12"]);
    // Whichever order they are declared in, and called above both.
    let src = "print(area(16), area(3, 4))\n\
               fn area(w, h) w * h end\n\
               native fn area(x: num) -> float = \"sqrt\"";
    assert_eq!(run(src), ["4.0 12"]);
}

#[test]
fn an_optional_native_parameter_fills_each_count_it_takes() {
    let mut env = Env::new();
    env.register_native("host_add", host_add, NativeEffects::PURE);
    let src = "native fn add(a: num, b?: num) -> int = \"host_add\"\n\
               fn add(a, b, c) a + b + c end\n\
               print(add(1), add(1, 2), add(1, 2, 3))";
    assert_eq!(run_in(&mut env, src), ["101 3 6"]);
}

#[test]
fn a_mixed_overload_set_takes_named_arguments_and_is_a_value() {
    let src = "native fn area(x: num) -> float = \"sqrt\"\n\
               fn area(w, h) w * h end\n\
               let f = area\n\
               print(area(x: 9), area(h: 2, w: 5), f(4), f(2, 3))";
    assert_eq!(run(src), ["3.0 10 2.0 6"]);
}

#[test]
fn the_same_count_twice_in_a_mixed_set_is_an_error() {
    let err = load_err("native fn f(x) = \"sqrt\"\nfn f(y) y end");
    assert!(
        err.contains("native fn 'f' takes 1 argument, and so does a fn of the same name"),
        "{err}"
    );
    let err = load_err("native fn f(...xs) = \"print\"\nfn f(y) y end");
    assert!(
        err.contains("takes a rest parameter and so cannot share its name with a Petal fn"),
        "{err}"
    );
}

#[test]
fn mixed_pub_markers_on_one_name_are_refused() {
    let mut env = Env::new();
    env.register_module(
        "m",
        "pub native fn f(x: num) = \"sqrt\"\nfn f(a, b) a + b end",
    );
    let err = env.load_program("import m\nprint(1)").unwrap_err();
    assert!(err.contains("mixed `pub` markers"), "{err}");
}

// ── Modules ──────────────────────────────────────────────────────────────

const IFACE: &str = "pub native fn root(x: num) -> float = \"sqrt\"\n\
                     native fn _secret(x: num) = \"abs\"\n\
                     pub native fn area(x: num) -> float = \"sqrt\"\n\
                     pub fn area(w, h) w * h end\n\
                     pub fn twice_root(x) root(x) * 2 end\n";

#[test]
fn an_exported_declaration_is_reached_through_every_import_form() {
    let mut env = Env::new();
    env.register_module("iface", IFACE);
    assert_eq!(
        run_in(&mut env, "import iface: root, twice_root\nprint(root(4), twice_root(4))"),
        ["2.0 4.0"]
    );
    assert_eq!(
        run_in(&mut env, "import iface\nprint(iface.root(9), iface.area(4), iface.area(2, 3))"),
        ["3.0 2.0 6"]
    );
    assert_eq!(
        run_in(&mut env, "import iface as i\nlet f = i.root\nprint(f(16))"),
        ["4.0"]
    );
    env.set_implicit_imports(&["iface"]);
    assert_eq!(run_in(&mut env, "print(root(25), area(1, 2))"), ["5.0 2"]);
}

#[test]
fn an_unexported_declaration_is_private_to_its_module() {
    let mut env = Env::new();
    env.register_module("iface", IFACE);
    let err = env
        .load_program("import iface: _secret\nprint(1)")
        .unwrap_err();
    assert!(err.contains("module 'iface' has no export '_secret'"), "{err}");
}

// ── What the checker reads ───────────────────────────────────────────────

#[test]
fn argument_types_and_counts_come_from_the_declaration() {
    let iface = "native fn key_down(key: string) -> bool\n\
                 native fn blit(x: num, y: num, on?: bool)\n";
    assert_eq!(
        warnings(&format!("{iface}print(key_down(1))")),
        ["argument 1 to `key_down`: expected a string, found `int`"]
    );
    assert_eq!(
        warnings(&format!("{iface}blit(1, \"two\", 3)")),
        [
            "argument 2 to `blit`: expected a number, found `string`",
            "argument 3 to `blit`: expected `bool`, found `int`"
        ]
    );
    assert!(warnings(&format!("{iface}blit(1, 2.5)\nblit(1, 2, true)\nprint(key_down(\"a\"))")).is_empty());
}

#[test]
fn the_result_type_comes_from_the_declaration() {
    let iface = "native fn width(text: string) -> int\nnative fn anything(x)\n";
    let w = warnings(&format!("{iface}let s: string = width(\"a\")\nprint(s)"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("`string`") && w[0].contains("`int`"), "{w:?}");
    // No declared result is `any`: nothing to hold it to.
    assert!(warnings(&format!("{iface}let s: string = anything(1)\nprint(s)")).is_empty());
}

#[test]
fn builtins_are_checked_from_the_core_package() {
    assert_eq!(
        warnings("print(sqrt(\"x\"))"),
        ["argument 1 to `sqrt`: expected a number, found `string`"]
    );
    assert_eq!(
        warnings("print(join(separator: 3, list: [1]))"),
        ["argument 1 to `join`: expected a string, found `int`"]
    );
    let w = warnings("let n: string = len([1])\nprint(n)");
    assert_eq!(w.len(), 1, "{w:?}");
    // The argument-dependent results stay intrinsic: an all-int clamp is int.
    assert!(warnings("let i: int = clamp(1, 0, 3)\nprint(i)").is_empty());
}

#[test]
fn a_mixed_overload_set_is_checked_like_any_fn() {
    let src = "native fn area(x: num) -> float = \"sqrt\"\n\
               fn area(w, h) w * h end\n";
    let w = warnings(&format!("{src}print(area(\"x\"))"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("argument 1 to `area`"), "{w:?}");
    let w = warnings(&format!("{src}print(area(1, 2, 3))"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("expects"), "{w:?}");
}

#[test]
fn check_knows_a_declared_native_it_does_not_register() {
    let iface = "native fn mouse_down(button: num) -> bool\n";
    assert!(unresolved(&format!("{iface}print(mouse_down(0), mouse_down(button: 1))")).is_empty());
    assert_eq!(
        unresolved(&format!("{iface}print(mouse_down(btn: 0))")),
        ["mouse_down() has no parameter named 'btn' (parameters: 'button')"]
    );
    // A name nothing declares is still an error, with no host flag involved.
    let got = unresolved("print(mouse_down(0))");
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("unknown function `mouse_down`"), "{got:?}");
}

// ── Linking ──────────────────────────────────────────────────────────────

#[test]
fn a_program_does_not_load_without_a_native_it_refers_to() {
    let mut env = Env::new();
    let src = "native fn beep(pitch: num)\nprint(\"ran\")\nbeep(440)";
    let err = env.load_program_diag(src, None).unwrap_err();
    assert_eq!(err.phase, Phase::Link);
    assert_eq!(
        err.to_string(),
        "module '<entry>' needs a native fn this host does not register: beep — the program \
         cannot run here; run it with a host that provides it"
    );
    // Before any line runs: nothing was printed, nothing was loaded.
    assert!(env.take_output().is_empty());
}

#[test]
fn a_reference_inside_an_uncalled_function_still_links() {
    let err = load_err("native fn beep(pitch: num)\nfn later() beep(1) end\nprint(1)");
    assert!(err.contains("needs a native fn this host does not register: beep"), "{err}");
    let err = load_err("native fn beep(pitch: num)\nlet f = beep\nprint(1)");
    assert!(err.contains("beep"), "{err}");
}

#[test]
fn a_declaration_nothing_refers_to_links_nothing() {
    assert_eq!(run("native fn beep(pitch: num)\nprint(\"ran\")"), ["ran"]);
}

#[test]
fn the_error_names_the_module_and_counts_past_a_few() {
    let mut env = Env::new();
    env.register_module(
        "audio",
        "pub native fn a1()\npub native fn a2()\npub native fn a3()\npub native fn a4()\n\
         pub native fn a5()\npub native fn a6()\npub native fn unused()\n\
         pub fn all() [a1(), a2(), a3(), a4(), a5(), a6()] end",
    );
    let err = env.load_program("import audio\naudio.all()").unwrap_err();
    assert_eq!(
        err,
        "module 'audio' needs native fns this host does not register: a1, a2, a3, a4, +2 more \
         — the program cannot run here; run it with a host that provides them"
    );
}

#[test]
fn registering_the_native_is_what_makes_it_load() {
    let mut env = Env::new();
    let src = "native fn count(...values) -> int = \"host_count\"\nprint(count(1, 2))";
    assert!(env.load_program(src).is_err());
    env.register_native("host_count", host_count, NativeEffects::PURE);
    assert_eq!(run_in(&mut env, src), ["2"]);
}

#[test]
fn check_only_compiles_without_linking() {
    let mut env = Env::new();
    assert!(!env.is_check_only());
    env.set_check_only(true);
    let src = "native fn beep(pitch: num)\nprint(\"ran\")\nbeep(440)";
    let pid = env.load_program(src).expect("check-only does not link");
    // The link requirement is still recorded, and still unmet.
    let program = env.get_program(pid).unwrap();
    assert_eq!(program.native_links.len(), 1);
    assert_eq!(program.native_links[0].host, "beep");
    assert_eq!(program.native_links[0].module, "<entry>");
    let err = env.link_program(program).unwrap_err();
    assert_eq!(err.phase, Phase::Link);
    // Running it anyway fails where the native is called, as it always did.
    let sid = env.create_stack(pid).unwrap();
    let err = env.run(sid).unwrap_err();
    assert!(err.contains("Unknown builtin: beep"), "{err}");
    assert_eq!(env.take_output(), ["ran"]);
}

#[test]
fn a_recompile_links_too() {
    let env = Env::new();
    let err = env
        .compile_program(petal::program::ProgramId(7), "native fn beep()\nbeep()")
        .err()
        .expect("does not link");
    assert!(err.contains("needs a native fn this host does not register: beep"), "{err}");
}

#[test]
fn the_core_builtins_link_nothing() {
    let mut env = Env::new();
    let pid = env.load_program("print(len([1]), sqrt(4))").unwrap();
    assert!(env.get_program(pid).unwrap().native_links.is_empty());
}

// ── Applying an interface to a host's natives ────────────────────────────

#[test]
fn declare_natives_gives_registered_natives_their_parameter_names() {
    let mut env = Env::new();
    env.register_native("host_add", host_add, NativeEffects::PURE);
    let iface = "pub native fn host_add(a: num, b?: num) -> int\npub native fn not_here(x)";
    // Before: the native takes arguments by position only.
    assert!(env.native_signatures("host_add").unwrap().is_empty());
    let missing = env.declare_natives(iface).unwrap();
    assert_eq!(missing, ["not_here"]);
    let sigs = env.native_signatures("host_add").unwrap();
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].params(), ["a", "b"]);
    assert_eq!(sigs[0].required(), 1);
    // A call the compiler cannot resolve statically binds by them at run time.
    assert_eq!(
        run_in(&mut env, "let f = host_add\nprint(f(b: 2, a: 1))"),
        ["3"]
    );
    // Applying it again changes nothing.
    env.declare_natives(iface).unwrap();
    assert_eq!(env.native_signatures("host_add").unwrap().len(), 1);
    assert!(env.declare_natives("native fn f(a = 1)").is_err());
}

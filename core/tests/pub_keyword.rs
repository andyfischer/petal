// `pub`, and `export` as its deprecated spelling (docs/module-system.md
// #exporting).
//
// The contract under test: the two words are one modifier. `pub` is accepted
// everywhere `export` was; `export` keeps working and compiles to the same
// program; the only places that tell them apart are the ones built to —
// `petal lint` (the `prefer-pub` rule) and `petal fmt` (which rewrites it).
// The compiler, and so `petal check` and the LSP, says nothing about it.

use petal::diagnostic::Diagnostic;
use petal::env::Env;
use petal::ir_equiv::ir_equivalent;
use petal::lint::{LintOptions, PREFER_PUB, RuleFilter, lint_source};

/// Every declaration the modifier can sit on, written with `{m}` where it
/// goes. `import` is first because imports form a prefix of the file.
const EVERY_FORM: &str = "\
{m} import shapes: area
{m} fn double(x)
  x * 2
end
{m} let limit = 10
{m} var hits = 0
{m} config let margin = 4
{m} state frame = 0
{m} enum Color
  Red,
  Green,
end
{m} class Point
  x: int,
  y: int,
end
";

const SHAPES: &str = "pub fn area(r)\n  r.w * r.h\nend\n";

fn every_form(modifier: &str) -> String {
    EVERY_FORM.replace("{m}", modifier)
}

/// An entry file that reaches every name `lib` exports, so a declaration
/// that stopped being exported fails to compile rather than passing quietly.
const USES_LIB: &str = "\
import lib
print(lib.double(lib.limit))
print(lib.area({w: 2, h: 3}))
print(lib.margin)
print(lib.hits)
print(lib.Color.Green == lib.Color.Green)
print(lib.Point(1, 2).y)
";

/// Compile `USES_LIB` against `lib` and return the environment and program.
fn compile_with_lib(lib: &str) -> (Env, petal::program::ProgramId) {
    let mut env = Env::new();
    env.register_module("shapes", SHAPES);
    env.register_module("lib", lib);
    let pid = env.load_program(USES_LIB).expect("compiles");
    (env, pid)
}

fn warnings(env: &Env, pid: petal::program::ProgramId) -> Vec<Diagnostic> {
    env.get_program(pid).expect("program").warnings.clone()
}

/// What compiling `source` warns about.
fn compile_warnings(source: &str) -> Vec<Diagnostic> {
    let mut env = Env::new();
    env.register_module("shapes", SHAPES);
    let pid = env.load_program(source).expect("compiles");
    warnings(&env, pid)
}

// ── `pub` and `export` are the same modifier ─────────────────────

#[test]
fn pub_exports_every_declaration_form() {
    let (mut env, pid) = compile_with_lib(&every_form("pub"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["20", "6", "4", "0", "true", "2"]);
}

#[test]
fn export_still_exports_every_declaration_form() {
    let (mut env, pid) = compile_with_lib(&every_form("export"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["20", "6", "4", "0", "true", "2"]);
}

#[test]
fn export_and_pub_compile_to_the_same_ir() {
    // As a module, where the modifier decides what the importer can reach...
    let (env_pub, pid_pub) = compile_with_lib(&every_form("pub"));
    let (env_export, pid_export) = compile_with_lib(&every_form("export"));
    if let Err(diff) = ir_equivalent(
        env_pub.get_program(pid_pub).unwrap(),
        env_export.get_program(pid_export).unwrap(),
    ) {
        panic!("`export` and `pub` modules differ in IR: {diff:?}");
    }

    // ...and as the entry file, where it is accepted and inert.
    let compile_entry = |modifier: &str| {
        let mut env = Env::new();
        env.register_module("shapes", SHAPES);
        let pid = env.load_program(&every_form(modifier)).expect("compiles");
        (env, pid)
    };
    let (env_pub, pid_pub) = compile_entry("pub");
    let (env_export, pid_export) = compile_entry("export");
    if let Err(diff) = ir_equivalent(
        env_pub.get_program(pid_pub).unwrap(),
        env_export.get_program(pid_export).unwrap(),
    ) {
        panic!("`export` and `pub` entry files differ in IR: {diff:?}");
    }
}

#[test]
fn the_two_spellings_mix_within_one_overload_set() {
    // "All overloads marked or none" is about the flag, not the word.
    let lib = "pub fn f(a)\n  1\nend\nexport fn f(a, b)\n  2\nend\n";
    let mut env = Env::new();
    env.register_module("lib", lib);
    let pid = env
        .load_program("import lib\nprint(lib.f(0), lib.f(0, 0))")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1 2"]);
}

#[test]
fn a_declaration_without_the_modifier_stays_private() {
    let mut env = Env::new();
    env.register_module("lib", "pub fn open()\n  1\nend\nfn hidden()\n  2\nend\n");
    let err = env.load_program("import lib: hidden\n").unwrap_err();
    assert!(err.contains("has no export 'hidden'"), "got: {err}");
}

// ── `pub` is a reserved word ─────────────────────────────────────

#[test]
fn pub_cannot_name_a_binding() {
    for src in ["let pub = 1", "fn pub()\n  1\nend", "var pub = 1"] {
        assert!(
            Env::new().load_program(src).is_err(),
            "`{src}` should not parse: `pub` is reserved"
        );
    }
}

#[test]
fn pub_and_export_remain_usable_as_field_names() {
    // Like every keyword: a record key and a field access are name slots.
    let mut env = Env::new();
    let pid = env
        .load_program("let r = {pub: 1, export: 2}\nprint(r.pub, r.export)")
        .unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    assert_eq!(env.take_output(), ["1 2"]);
    assert_eq!(warnings(&env, pid), vec![]);
}

#[test]
fn a_dangling_modifier_names_the_word_that_was_written() {
    let err = Env::new().load_program("pub 1 + 2").unwrap_err();
    assert!(
        err.contains(
            "`pub` must be followed by a fn, native fn, let, var, state, enum, class, or import"
        ),
        "got: {err}"
    );
    let err = Env::new().load_program("export 1 + 2").unwrap_err();
    assert!(err.contains("`export` must be followed by"), "got: {err}");
}

// ── The compiler does not comment on the spelling ────────────────

#[test]
fn neither_spelling_draws_a_compile_warning() {
    // The old spelling is lint's (`prefer-pub`) and fmt's to report and
    // rewrite; a compile, and so `petal check`, has nothing to say about it.
    assert_eq!(compile_warnings(&every_form("pub")), vec![]);
    assert_eq!(compile_warnings(&every_form("export")), vec![]);
}

#[test]
fn an_imported_modules_export_draws_no_warning_either() {
    let mut env = Env::new();
    env.register_module("lib", "\nexport let a = 1\n");
    let pid = env.load_program("import lib\nprint(lib.a)").unwrap();
    assert_eq!(warnings(&env, pid), vec![]);
}

#[test]
fn the_lsp_does_not_flag_export() {
    use petal::lsp::document::analyze;

    let analysis = analyze("let a = 1\nexport fn f()\n  a\nend\n");
    assert!(analysis.diagnostics.is_empty(), "{:?}", analysis.diagnostics);
}

// ── `petal lint`: the `prefer-pub` rule ──────────────────────────

fn lint(src: &str) -> petal::lint::LintOutcome {
    lint_source(src, &LintOptions::default()).expect("lint_source should succeed")
}

#[test]
fn lint_rewrites_every_export_form() {
    let out = lint(&every_form("export"));
    assert_eq!(out.count(PREFER_PUB), 8);
    assert_eq!(out.output, every_form("pub"));
    // Each finding sits on its keyword, in the original text.
    let at: Vec<(usize, usize)> = out.findings.iter().map(|f| (f.line, f.column)).collect();
    assert_eq!(
        at,
        [
            (1, 1),
            (2, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 1),
            (9, 1),
            (13, 1)
        ]
    );
    // A fixed point: the fixed text has nothing left to report.
    assert!(lint(&out.output).findings.is_empty());
}

#[test]
fn lint_leaves_pub_alone() {
    let src = every_form("pub");
    let out = lint(&src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn lint_fix_is_not_a_semantic_rewrite() {
    // `--verify` holds this rule to IR equality, like `prefer-compound-assign`.
    let src = "export let a = 1\nprint(a)\n";
    let out = lint(src);
    assert_eq!(out.count(PREFER_PUB), 1);
    assert!(!out.has_semantic_rewrite());
    let verdict = petal::lint::verify_rewrite(
        src,
        &out,
        petal::lint::VerifyMode::Strict,
        &LintOptions::default(),
    );
    assert!(matches!(verdict, Ok(petal::lint::VerifyVerdict::Equal)));
}

#[test]
fn lint_does_not_touch_export_used_as_a_name() {
    let src = "let r = {export: 1}\nprint(r.export)\n";
    let out = lint(src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn lint_ignore_keeps_the_old_spelling_even_when_another_rule_fires() {
    // The fixed file is formatted before it is written, and `petal fmt`
    // rewrites `export` on its own account. Lint must not let that apply a
    // fix the user silenced.
    let src = "\
// petal-lint-ignore prefer-pub -- kept for the old toolchain
export let a = 1
export let b = 2
let n = 0
n = n + 1
";
    let out = lint(src);
    assert_eq!(out.count(PREFER_PUB), 1, "only the un-ignored one");
    assert!(out.output.contains("export let a = 1"), "{}", out.output);
    assert!(out.output.contains("pub let b = 2"), "{}", out.output);
    assert!(out.output.contains("n += 1"), "{}", out.output);

    let excluded = lint_source(
        src,
        &LintOptions {
            rules: RuleFilter {
                include: None,
                exclude: vec![PREFER_PUB.to_string()],
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(excluded.count(PREFER_PUB), 0);
    assert!(
        excluded.output.contains("export let b = 2"),
        "{}",
        excluded.output
    );
    assert!(excluded.output.contains("n += 1"), "{}", excluded.output);
}

#[test]
fn prefer_pub_is_a_listed_rule() {
    assert!(petal::lint::is_rule(PREFER_PUB));
    assert_eq!(PREFER_PUB, "prefer-pub");
}

// ── `petal fmt` normalizes the keyword ───────────────────────────

fn fmt(src: &str) -> String {
    petal::fmt::format_source(src).expect("fmt should succeed")
}

#[test]
fn fmt_rewrites_every_export_form_and_is_idempotent() {
    let out = fmt(&every_form("export"));
    assert_eq!(out, every_form("pub"));
    assert_eq!(fmt(&out), out);
}

#[test]
fn fmt_rewrites_only_the_modifier() {
    assert_eq!(
        fmt("export let r = {export: 1}\nprint(r.export) // export this\n"),
        "pub let r = {export: 1}\nprint(r.export) // export this\n"
    );
    assert_eq!(
        fmt("let s = \"export fn f\"\n"),
        "let s = \"export fn f\"\n"
    );
}

#[test]
fn fmt_respects_its_opt_outs() {
    let ignored = "// petal-fmt-ignore\nexport let a = 1\nexport let b = 2\n";
    assert_eq!(
        fmt(ignored),
        "// petal-fmt-ignore\nexport let a = 1\npub let b = 2\n"
    );

    let region = "// petal-fmt-off\nexport let a = 1\n// petal-fmt-on\nexport let b = 2\n";
    assert_eq!(
        fmt(region),
        "// petal-fmt-off\nexport let a = 1\n// petal-fmt-on\npub let b = 2\n"
    );

    let file = "// petal-fmt-ignore-file\nexport let a = 1\n";
    assert_eq!(fmt(file), file);
}

#[test]
fn fmt_keeps_trailing_comments_lined_up_after_the_keyword_shrinks() {
    // `export` → `pub` pulls each line three columns left; a comment column
    // shared with a line that did not move is still shared afterwards.
    let src = "\
export let black = 15   // $0F
let other_name = 0      // $00
";
    let out = fmt(src);
    let cols: Vec<usize> = out.lines().map(|l| l.find("//").unwrap()).collect();
    assert_eq!(cols[0], cols[1], "{out}");
    assert!(out.starts_with("pub let black = 15"), "{out}");
}

#[test]
fn a_comment_column_moves_with_its_whole_group_or_not_at_all() {
    // Every line of the group is rewritten: the table shifts left as one.
    let all = "\
export let black = 15   // $0F
export let white = 48   // $30
";
    assert_eq!(
        fmt(all),
        "pub let black = 15   // $0F\npub let white = 48   // $30\n"
    );
    // A lone trailing comment has nothing to stay lined up with.
    assert_eq!(fmt("export let a = 1 // one\n"), "pub let a = 1 // one\n");
    // `lint --fix` plans the same edits, so it agrees with `fmt` on both.
    assert_eq!(lint(all).output, fmt(all));
    let mixed = "\
export let black = 15   // $0F
let other_name = 0      // $00
";
    assert_eq!(lint(mixed).output, fmt(mixed));
    assert_eq!(lint(mixed).count(PREFER_PUB), 1);
}

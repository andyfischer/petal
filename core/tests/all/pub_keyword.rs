// `pub`, and `export` as its deprecated spelling (docs/module-system.md
// #exporting).
//
// The contract under test: the two words are one modifier. `pub` is accepted
// everywhere `export` was; `export` keeps working and compiles to the same
// program; the only places that tell them apart are the ones built to —
// `petal migrate-old-code` (the `export-to-pub` migration, which rewrites it)
// and `petal lint`'s report-only `old-syntax` rule, which points there.
// The compiler, and so the compile check and the LSP, says nothing about it;
// `petal fmt` leaves it as written.

use petal::diagnostic::Diagnostic;
use petal::env::Env;
use petal::ir_equiv::ir_equivalent;
use petal::lint::{LintOptions, OLD_SYNTAX, RuleFilter, lint_source};
use petal::migrate::{EXPORT_TO_PUB, MigrateOptions, MigrateOutcome, migrate_source};

/// Every declaration the modifier can sit on, written with `{m}` where it
/// goes. `import` is first because imports form a prefix of the file.
const EVERY_FORM: &str = "\
{m} import shapes: area
{m} fn double(x)
  x * 2
end
{m} let limit = 10
{m} var hits = 0
{m} let margin = tunable(4)
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
            "`pub` must be followed by a fn, native fn, let, const, var, state, enum, class, or import"
        ),
        "got: {err}"
    );
    let err = Env::new().load_program("export 1 + 2").unwrap_err();
    assert!(err.contains("`export` must be followed by"), "got: {err}");
}

// ── The compiler does not comment on the spelling ────────────────

#[test]
fn neither_spelling_draws_a_compile_warning() {
    // The old spelling is `petal migrate-old-code`'s to rewrite; a compile
    // has nothing to say about it.
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

// ── `petal migrate-old-code`: the `export-to-pub` migration ──────

fn migrate(src: &str) -> MigrateOutcome {
    migrate_source(src, &MigrateOptions::default()).expect("migrate_source should succeed")
}

fn lint(src: &str) -> petal::lint::LintOutcome {
    lint_source(src, &LintOptions::default()).expect("lint_source should succeed")
}

#[test]
fn migrate_rewrites_every_export_form() {
    let out = migrate(&every_form("export"));
    assert_eq!(out.count(EXPORT_TO_PUB), 8);
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
    // Idempotent: the migrated text has nothing left to migrate.
    assert!(migrate(&out.output).findings.is_empty());
}

#[test]
fn migrate_leaves_pub_alone() {
    let src = every_form("pub");
    let out = migrate(&src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn the_migration_is_ir_invisible() {
    let src = "export let a = 1\nprint(a)\n";
    let out = migrate(src);
    assert_eq!(out.count(EXPORT_TO_PUB), 1);
    let verdict = petal::ir_equiv::sources_equivalent(src, &out.output, &[], None).unwrap();
    assert!(verdict.is_ok());
}

#[test]
fn migrate_does_not_touch_export_used_as_a_name() {
    let src = "let r = {export: 1}\nprint(r.export)\n";
    let out = migrate(src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
    assert_eq!(
        migrate("export let r = {export: 1}\nprint(r.export) # export this\n").output,
        "pub let r = {export: 1}\nprint(r.export) # export this\n"
    );
    assert_eq!(migrate("let s = \"export fn f\"\n").output, "let s = \"export fn f\"\n");
}

#[test]
fn an_ignore_comment_keeps_the_old_spelling() {
    let src = "\
// petal-lint-ignore export-to-pub -- kept for the old toolchain
export let a = 1
export let b = 2
";
    let out = migrate(src);
    assert_eq!(out.count(EXPORT_TO_PUB), 1, "only the un-ignored one");
    assert_eq!(
        out.output,
        "# petal-lint-ignore export-to-pub -- kept for the old toolchain\nexport let a = 1\npub let b = 2\n"
    );
    // `--only` another migration leaves it alone.
    let only = migrate_source(
        src,
        &MigrateOptions {
            only: Some(vec![petal::migrate::CONST_DECL_DEPRECATED.to_string()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(only.output, src);
}

#[test]
fn export_to_pub_is_a_migration_not_a_lint_rule() {
    assert!(petal::migrate::is_migration(EXPORT_TO_PUB));
    assert!(!petal::lint::is_rule(EXPORT_TO_PUB));
    assert!(!petal::lint::is_rule("prefer-pub"));
}

#[test]
fn trailing_comments_stay_lined_up_after_the_keyword_shrinks() {
    // `export` → `pub` pulls each line three columns left; a comment column
    // shared with a line that did not move is still shared afterwards.
    let mixed = "\
export let black = 15   # $0F
let other_name = 0      # $00
";
    let out = migrate(mixed).output;
    let cols: Vec<usize> = out.lines().map(|l| l.find('#').unwrap()).collect();
    assert_eq!(cols[0], cols[1], "{out}");
    assert!(out.starts_with("pub let black = 15"), "{out}");
    // Every line of the group is rewritten: the table shifts left as one.
    let all = "\
export let black = 15   # $0F
export let white = 48   # $30
";
    assert_eq!(
        migrate(all).output,
        "pub let black = 15   # $0F\npub let white = 48   # $30\n"
    );
    // A lone trailing comment has nothing to stay lined up with.
    assert_eq!(migrate("export let a = 1 # one\n").output, "pub let a = 1 # one\n");
}

// ── `petal lint` reports it and leaves it ────────────────────────

#[test]
fn lint_reports_the_old_spelling_and_does_not_fix_it() {
    let src = "export let a = 1\nlet n = 0\nn = n + 1\nprint(a + n)\n";
    let out = lint(src);
    assert_eq!(out.count(OLD_SYNTAX), 1);
    let f = out.findings.iter().find(|f| f.rule == OLD_SYNTAX).unwrap();
    assert!(!f.fixable);
    assert_eq!((f.line, f.column), (1, 1));
    assert_eq!(
        f.message,
        "`export` is deprecated; write `pub`; run `petal migrate-old-code` [export-to-pub]"
    );
    // `--fix` makes its own fixes and leaves the spelling.
    assert!(out.output.starts_with("export let a = 1\n"), "{}", out.output);
    assert!(out.output.contains("n += 1"), "{}", out.output);
    // Silenced under either name.
    for rule in [OLD_SYNTAX, EXPORT_TO_PUB] {
        let quiet = lint(&format!("# petal-lint-ignore {rule}\n{src}"));
        assert_eq!(quiet.count(OLD_SYNTAX), 0, "{rule}");
    }
    let excluded = lint_source(
        src,
        &LintOptions {
            rules: RuleFilter {
                include: None,
                exclude: vec![OLD_SYNTAX.to_string()],
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(excluded.count(OLD_SYNTAX), 0);
}

// ── `petal fmt` leaves the spelling as written ───────────────────

#[test]
fn fmt_does_not_rewrite_the_keyword() {
    let src = every_form("export");
    assert_eq!(petal::fmt::format_source(&src).unwrap(), src);
}

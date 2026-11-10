//! `petal migrate-old-code` at the library level ([`petal::migrate`]): one
//! fixture per migration, run on its own (`--only`) and with every other
//! migration on; every fixture is idempotent, compiles before and after, and
//! leaves current syntax alone. The CLI half (`--check`, the directory walk,
//! `old-imports`) is cli/tests/migrate_old_code_cli.rs.

use petal::env::Env;
use petal::migrate::{
    AT_REBIND, CONFIG_LET_DEPRECATED, CONST_DECL_DEPRECATED, EXPORT_TO_PUB, MIGRATIONS, MigrateOptions,
    MigrateOutcome, STATE_KEY_DEPRECATED, migrate_source,
};

/// (migration, old source, migrated source). Each old source compiles (or,
/// for a removed form, compiled before its removal), and is written only in
/// the one old form its migration rewrites.
const FIXTURES: &[(&str, &str, &str)] = &[
    // The old test of the operator (test/at-arg-update), migrated.
    (
        AT_REBIND,
        "fn double(x)\n  x * 2\nend\nfn inc(x)\n  x + 1\nend\n\
         let a = 5\ndouble(@a)\nprint(a) // 10\n\
         let b = 3\nlet r = inc(double(@b))\nprint(b, r) // 6 7\n\
         let c = 4\ndouble(@c)\ndouble(@ c)\nprint(c) // 16\n",
        "fn double(x)\n  x * 2\nend\nfn inc(x)\n  x + 1\nend\n\
         let a = 5\na = double(a)\nprint(a) // 10\n\
         let b = 3\nb = double(b)\nlet r = inc(b)\nprint(b, r) // 6 7\n\
         let c = 4\nc = double(c)\nc = double(c)\nprint(c) // 16\n",
    ),
    (
        EXPORT_TO_PUB,
        "export fn double(x)\n  x * 2\nend\nexport let limit = 10 // max\nprint(double(limit))\n",
        "pub fn double(x)\n  x * 2\nend\npub let limit = 10 // max\nprint(double(limit))\n",
    ),
    (
        CONST_DECL_DEPRECATED,
        "const BASE = 10\npub const TABLE = map([1, 2], fn(x) -> x * BASE) // computed\n\
         const P = {\n  x: 1,\n  y: [2, 3],\n}\nprint(TABLE, P)\n",
        "let BASE = const(10)\npub let TABLE = const(map([1, 2], fn(x) -> x * BASE)) // computed\n\
         let P = const({\n  x: 1,\n  y: [2, 3],\n})\nprint(TABLE, P)\n",
    ),
    (
        CONFIG_LET_DEPRECATED,
        "config let SPEED = 4.0   // px\nconfig let G = tunable(9.8, 0, 20)\nprint(SPEED, G)\n",
        "let SPEED = tunable(4.0) // px\nlet G = tunable(9.8, 0, 20)\nprint(SPEED, G)\n",
    ),
    (
        STATE_KEY_DEPRECATED,
        "fn width(text)\n  state(text) w = text.length * 8\n  w\nend\nprint(width(\"hi\"))\n",
        "fn width(text)\n  memo(text) w = text.length * 8\n  w\nend\nprint(width(\"hi\"))\n",
    ),
];

fn migrate_with(src: &str, only: Option<&[&str]>) -> MigrateOutcome {
    let opts = MigrateOptions {
        only: only.map(|o| o.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    migrate_source(src, &opts).unwrap_or_else(|e| panic!("migrate: {e}\n{src}"))
}

/// Whether `src` compiles; a removed form is checked as the operator's
/// desugaring would have seen it, which for `@` is "with the `@`s read as
/// nothing" (the program the old compiler checked had the same names).
fn compiled_before(rule: &str, src: &str) -> bool {
    if rule == AT_REBIND {
        !compiles(src) && compiles(&src.replace('@', " "))
    } else {
        compiles(src)
    }
}

fn run_out(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap_or_else(|e| panic!("{e}\n{src}"));
    env.take_output()
}

fn compiles(src: &str) -> bool {
    let mut env = Env::new();
    env.set_echo(false);
    env.load_program(src).is_ok()
}

fn deprecation_warnings(src: &str) -> Vec<String> {
    let mut env = Env::new();
    env.set_echo(false);
    let pid = env.load_program(src).expect("compiles");
    env.get_program(pid)
        .unwrap()
        .warnings
        .iter()
        .map(|d| d.message.clone())
        .filter(|m| m.contains("petal migrate-old-code"))
        .collect()
}

#[test]
fn every_text_migration_has_a_fixture() {
    for m in MIGRATIONS.iter().filter(|m| !m.batch) {
        assert!(
            FIXTURES.iter().any(|(rule, _, _)| *rule == m.name),
            "no fixture for {}",
            m.name
        );
    }
}

#[test]
fn each_migration_rewrites_its_fixture_on_its_own_and_with_the_rest() {
    for (rule, before, after) in FIXTURES {
        assert!(compiled_before(rule, before), "{rule}: the old source does not compile");
        let only = migrate_with(before, Some(&[rule]));
        assert_eq!(only.output, *after, "{rule} (--only)");
        assert!(only.findings.iter().all(|f| f.rule == *rule && f.fixable), "{rule}: {:?}", only.findings);
        assert!(only.rewritten() > 0, "{rule}");
        let all = migrate_with(before, None);
        assert_eq!(all.output, *after, "{rule} (every migration)");
        assert!(compiles(after), "{rule}: the migrated source does not compile");
        // The compiler warned about the old form, and points nowhere now.
        assert_eq!(deprecation_warnings(after), Vec::<String>::new(), "{rule}");
    }
}

#[test]
fn migrating_twice_changes_nothing() {
    for (rule, before, after) in FIXTURES {
        let once = migrate_with(before, None).output;
        let twice = migrate_with(&once, None);
        assert_eq!(twice.output, once, "{rule}");
        assert!(twice.findings.is_empty(), "{rule}: {:?}", twice.findings);
        assert_eq!(migrate_with(after, None).output, *after, "{rule}");
    }
}

#[test]
fn another_migration_leaves_a_fixture_alone() {
    for (rule, before, _) in FIXTURES {
        for (other, _, _) in FIXTURES.iter().filter(|(o, _, _)| o != rule) {
            if *rule == AT_REBIND {
                // Without its own migration, an `@` file does not parse.
                let opts = MigrateOptions {
                    only: Some(vec![other.to_string()]),
                    ..Default::default()
                };
                assert!(migrate_source(before, &opts).is_err(), "--only {other}");
                continue;
            }
            let out = migrate_with(before, Some(&[other]));
            assert_eq!(out.output, *before, "--only {other} touched the {rule} fixture");
        }
    }
}

#[test]
fn every_old_form_in_one_file() {
    let src = "export const N = 4\n\
               pub config let SPEED = 2.5\n\
               fn w(t)\n  state(t) x = t * 2\n  x\nend\n\
               print(N + SPEED + w(1))\n";
    assert_eq!(deprecation_warnings(src).len(), 3, "{:?}", deprecation_warnings(src));
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "pub let N = const(4)\n\
         pub let SPEED = tunable(2.5)\n\
         fn w(t)\n  memo(t) x = t * 2\n  x\nend\n\
         print(N + SPEED + w(1))\n"
    );
    let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
    assert_eq!(rules, [EXPORT_TO_PUB, CONST_DECL_DEPRECATED, CONFIG_LET_DEPRECATED, STATE_KEY_DEPRECATED]);
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
    assert_eq!(migrate_with(&out.output, None).output, out.output);
}

#[test]
fn current_syntax_is_left_exactly_as_written() {
    // Unformatted, too: the tool does not reformat.
    let src = "pub   let N = const( 4 )\nlet  S = tunable(1)\nfn f(k)\n  memo(k) v = k\n    v\nend\nprint(N+S+f(1))\n";
    let out = migrate_with(src, None);
    assert!(out.findings.is_empty(), "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn a_site_that_needs_a_hand_edit_is_reported_and_left() {
    let src = "fn health(id, d)\n  state(id) hp = 100\n  hp -= d\n  hp\nend\nprint(health(1, 2))\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert_eq!((out.rewritten(), out.left()), (0, 1));
    assert_eq!(out.findings[0].rule, STATE_KEY_DEPRECATED);
}

#[test]
fn an_ignore_comment_keeps_the_old_form_and_silences_the_warning() {
    let src = "// petal-lint-ignore const-decl-deprecated -- a fixture of the old form\n\
               const A = 1\nconst B = 2\nprint(A + B)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "// petal-lint-ignore const-decl-deprecated -- a fixture of the old form\n\
         const A = 1\nlet B = const(2)\nprint(A + B)\n"
    );
    assert_eq!(deprecation_warnings(&out.output), Vec::<String>::new());
}

#[test]
fn a_file_that_does_not_parse_is_an_error() {
    assert!(migrate_source("const = \n", &MigrateOptions::default()).is_err());
}

#[test]
fn a_file_that_does_not_compile_is_still_migrated() {
    // The import resolves nowhere here (a module of some app): the compile
    // gate has nothing to hold the rewrite to, and the migration applies.
    let src = "import no_such_package_here\nconst N = 1\nprint(N)\n";
    assert!(!compiles(src));
    assert_eq!(
        migrate_with(src, None).output,
        "import no_such_package_here\nlet N = const(1)\nprint(N)\n"
    );
}

#[test]
fn the_registry_lists_old_imports_last_as_a_batch_migration() {
    let last = MIGRATIONS.last().unwrap();
    assert_eq!(last.name, petal::migrate::OLD_IMPORTS);
    assert!(last.batch);
    assert!(MIGRATIONS[..MIGRATIONS.len() - 1].iter().all(|m| !m.batch));
}

// ---------------------------------------------------------------------------
// `at-rebind`: the removed `f(@x)` operator
// ---------------------------------------------------------------------------

#[test]
fn an_at_rebind_is_a_parse_error_that_names_the_tool() {
    let mut env = Env::new();
    env.set_echo(false);
    let err = format!("{}", env.load_program("let a = [1]\nappend(@a, 2)\n").unwrap_err());
    assert!(err.contains("`@a` is not an expression"), "{err}");
    assert!(err.contains("Write `a = f(a)` instead of `f(@a)`"), "{err}");
    assert!(err.contains("petal migrate-old-code"), "{err}");
    // Where a statement starts, `@` opens a declaration annotation
    // (docs/proposals/annotation-syntax.md), and the error says what one is.
    let err = format!("{}", env.load_program("@ 1\n").unwrap_err());
    assert!(err.contains("`@` must be followed by an annotation name"), "{err}");
}

#[test]
fn an_at_rebind_migrates_to_the_program_it_desugared_to() {
    // The values the old operator's test expected: 10, 6 7, 16.
    let (_, _, after) = FIXTURES.iter().find(|(r, _, _)| *r == AT_REBIND).unwrap();
    assert_eq!(run_out(after), ["10", "6 7", "16"]);
}

#[test]
fn an_at_rebind_lifts_within_each_block_and_keeps_its_indentation() {
    let src = "fn double(x)\n  x * 2\nend\nfn add(x, y)\n  x + y\nend\n\
               fn go(a, b)\n  if a > 0 then double(@a) end\n  if b > 0 then\n    \
               let r = add(double(@a), @b) // note\n    print(r)\n  end\n  \
               for i in [1, 2] do\n    a = add(double(@a), 1)\n  end\n  \
               print(\"@a stays {a}\")\n  [a, b]\nend\nprint(go(1, 2))\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\nfn add(x, y)\n  x + y\nend\n\
         fn go(a, b)\n  if a > 0 then a = double(a) end\n  if b > 0 then\n    \
         a = double(a)\n    b = add(a, b)\n    let r = b // note\n    print(r)\n  end\n  \
         for i in [1, 2] do\n    a = double(a)\n    a = add(a, 1)\n  end\n  \
         print(\"@a stays {a}\")\n  [a, b]\nend\nprint(go(1, 2))\n"
    );
    assert_eq!((out.rewritten(), out.left()), (3, 0));
    assert_eq!(run_out(&out.output), ["6", "@a stays 19", "[19, 6]"]);
    assert_eq!(migrate_with(&out.output, None).output, out.output);
}

#[test]
fn an_at_rebind_the_operator_could_not_lift_is_left_for_a_hand_edit() {
    for (src, why) in [
        ("fn add(x, y)\n  x + y\nend\nlet a = 1\nlet b = 2\nadd(@a, @b)\n", "two in one call"),
        (
            "fn double(x)\n  x * 2\nend\nlet a = 1\nwhile double(@a) < 10 do\n  print(a)\nend\n",
            "a while condition",
        ),
        (
            "fn double(x)\n  x * 2\nend\nlet a = 1\nlet m = match a\n  when 1 -> double(@a)\n  \
             when _ -> 0\nend\n",
            "a match arm",
        ),
        (
            "fn double(x)\n  x * 2\nend\nlet b = 1\nif true then print(double(@b)) end\n",
            "an assignment that would share its line",
        ),
    ] {
        let out = migrate_with(src, None);
        assert_eq!(out.output, src, "{why}");
        assert!(out.left() > 0 && out.rewritten() == 0, "{why}: {:?}", out.findings);
        assert!(out.findings.iter().all(|f| f.rule == AT_REBIND && !f.fixable), "{why}");
    }
}

#[test]
fn an_at_rebind_and_a_deprecated_form_migrate_together() {
    let src = "fn double(x)\n  x * 2\nend\nexport let n = 2\nconst K = 3\nlet a = 1\ndouble(@a)\nprint(a + n + K)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\npub let n = 2\nlet K = const(3)\nlet a = 1\na = double(a)\nprint(a + n + K)\n"
    );
    let rules: Vec<&str> = out.findings.iter().map(|f| f.rule).collect();
    assert_eq!(rules, [EXPORT_TO_PUB, CONST_DECL_DEPRECATED, AT_REBIND]);
    assert_eq!(run_out(&out.output), ["7"]);
}

#[test]
fn an_at_in_a_string_or_a_comment_is_not_a_rebind() {
    let src = "let a = \"@a\" // f(@a)\nprint(a)\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty());
}

#[test]
fn a_declaration_annotation_is_not_a_rebind() {
    // `@tunable`, `@rigid` and `@const` annotate a declaration
    // (docs/proposals/annotation-syntax.md); they never were rebinds, since
    // those names cannot be bound. migrate-old-code leaves them alone, also
    // beside a real rebind and a deprecated form it rewrites.
    let src = "@tunable(0, 10) let S = 4.0\n@const\npub let T = [1, 2]\n@rigid state A = 1\n\
               fn f(x)\n  @tunable let g = x\n  g\nend\nprint(S, T, A, f(1))\n";
    let out = migrate_with(src, None);
    assert_eq!(out.output, src);
    assert!(out.findings.is_empty(), "{:?}", out.findings);

    let src = "fn double(x)\n  x * 2\nend\nlet a = 1\n@tunable(0, 9) let k = 2\n\
               @rigid const G = 9.8\ndouble(@a)\nprint(a, k, G)\n";
    let out = migrate_with(src, None);
    assert_eq!(
        out.output,
        "fn double(x)\n  x * 2\nend\nlet a = 1\n@tunable(0, 9) let k = 2\n\
         @const @rigid let G = 9.8\na = double(a)\nprint(a, k, G)\n"
    );
    assert_eq!(run_out(&out.output), ["2 2 9.8"]);
}

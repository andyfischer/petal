//! `petal migrate-old-code` at the library level ([`petal::migrate`]): one
//! fixture per migration, run on its own (`--only`) and with every other
//! migration on; every fixture is idempotent, compiles before and after, and
//! leaves current syntax alone. The CLI half (`--check`, the directory walk,
//! `old-imports`) is cli/tests/migrate_old_code_cli.rs.

use petal::env::Env;
use petal::migrate::{
    CONFIG_LET_DEPRECATED, CONST_DECL_DEPRECATED, EXPORT_TO_PUB, MIGRATIONS, MigrateOptions,
    MigrateOutcome, STATE_KEY_DEPRECATED, migrate_source,
};

/// (migration, old source, migrated source). Each old source compiles, and
/// is written only in the one old form its migration rewrites.
const FIXTURES: &[(&str, &str, &str)] = &[
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
        assert!(compiles(before), "{rule}: the old source does not compile");
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

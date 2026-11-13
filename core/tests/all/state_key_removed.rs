//! `state(key) name = init`, absolute keyed state, was removed on 2026-11-13
//! (deprecated 2026-10-22): the parser rejects it with an error that names
//! what replaces it and `petal migrate-old-code`, and the migration
//! `state-key-deprecated` still reads the removed form
//! (core/src/migrate/state_key.rs): it reports every site and rewrites the
//! never-written ones whose initializer reads only its key to `memo(key)`.
//! `memo(key)` and keyed loops (`for … key`) are untouched.

use petal::env::Env;
use petal::migrate::{MigrateOptions, MigrateOutcome, STATE_KEY_DEPRECATED, migrate_source};

/// The error `src` fails to compile with.
fn compile_error(src: &str) -> String {
    match Env::new().load_program(src) {
        Ok(_) => panic!("should not compile: {src}"),
        Err(e) => e,
    }
}

fn run(src: &str) -> String {
    let mut env = Env::new();
    let pid = env.load_program(src).expect("should compile");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("should run");
    env.take_output().join("\n").trim().to_string()
}

fn migrate(src: &str) -> MigrateOutcome {
    let opts = MigrateOptions {
        only: Some(vec![STATE_KEY_DEPRECATED.to_string()]),
        ..MigrateOptions::default()
    };
    migrate_source(src, &opts).expect("migrate_source should succeed")
}

const HEALTH: &str = "\
fn health(id, damage)
  state(id) hp = 100
  hp -= damage
  hp
print(health(\"goblin\", 10), health(\"orc\", 30), health(\"goblin\", 5))
";

#[test]
fn a_keyed_state_is_an_error_naming_the_replacements_and_the_tool() {
    let e = compile_error(HEALTH);
    assert!(e.contains("`state(key)` was removed"), "{e}");
    for replacement in [
        "keyed loop (`for t in xs key t.id do`",
        "`state var` holding a `Map()`",
        "`memo(key) name = init`",
        "plain `state`",
    ] {
        assert!(e.contains(replacement), "{replacement}: {e}");
    }
    assert!(e.contains("run `petal migrate-old-code`"), "{e}");
    assert!(e.contains("[state-key-deprecated]"), "{e}");
}

#[test]
fn the_error_points_at_the_key_group() {
    let e = compile_error(HEALTH);
    assert!(e.contains("line 2, column 8"), "{e}");
}

#[test]
fn every_keyed_spelling_is_rejected() {
    for src in [
        "state(1) keyed = 2\n",
        "state (1) spaced = 2\n",
        "pub state(1) shared = 0\n",
        "fn f(k)\n  state(k) var cell = 0\n  get cell\nprint(f(1))\n",
        "state(1) n: int = 0\n",
        "for v in [1, 2] do\n  state(v) hits = 0\n  hits += 1\n",
        "# petal-lint-ignore state-key-deprecated\nstate(1) a = 1\n",
        "# petal-lint-ignore-file state-key-deprecated\nstate(1) a = 1\n",
    ] {
        let e = compile_error(src);
        assert!(e.contains("`state(key)` was removed"), "{src}: {e}");
    }
}

#[test]
fn an_imported_module_that_uses_it_is_rejected_too() {
    let mut env = Env::new();
    env.register_module("counter", "pub fn bump(id)\n  state(id) n = 0\n  n += 1\n  n\n");
    let e = env
        .load_program("import counter\nprint(counter.bump(1))\n")
        .expect_err("should not compile");
    assert!(e.contains("`state(key)` was removed"), "{e}");
}

#[test]
fn memo_keyed_loops_and_plain_state_are_untouched() {
    let src = "\
state top = 1
state var shared = Map()
fn f(k)
  state plain = 0
  memo(k) cached = k * 2
  set shared[k] = (get shared[k] ?? 0) + 1
  let n = get shared[k]
  cached + plain + n
for t in [{id: 7}, {id: 9}] key t.id do
  state seen = 0
  seen += 1
  print(seen)
print(top + f(3) + f(3))
";
    assert_eq!(run(src), "1\n1\n16");
}

#[test]
fn each_replacement_does_what_the_keyed_form_did() {
    // The Map in a top-level `state var`: one slot per key, shared by every
    // caller, which is what `state(id) hp` was.
    let shared = "\
state var hps = Map()
fn health(id, damage)
  set hps[id] = (get hps[id] ?? 100) - damage
  get hps[id]
print(health(\"goblin\", 10), health(\"orc\", 30), health(\"goblin\", 5))
";
    assert_eq!(run(shared), "90 70 85");
}

// ---------------------------------------------------------------------------
// `petal migrate-old-code`
// ---------------------------------------------------------------------------

#[test]
fn migrate_rewrites_a_never_written_keyed_state_to_memo() {
    let src = "\
fn width(text)
  state(text) w = text.length * 8
  w
end
print(width(\"hi\"), width(\"hi\"))
";
    let out = migrate(src);
    let [f] = out.findings.as_slice() else {
        panic!("one finding: {:?}", out.findings);
    };
    assert_eq!(
        (f.rule, f.line, f.column, f.fixable),
        (STATE_KEY_DEPRECATED, 2, 3, true)
    );
    let expected = src.replace("state(text)", "memo(text)");
    assert_eq!(out.output, expected);
    // A fixed point, and a program today's compiler runs.
    let again = migrate(&expected);
    assert_eq!(again.findings.len(), 0, "{:?}", again.findings);
    assert_eq!(run(&expected), "16 16");
}

#[test]
fn an_initializer_that_may_not_be_pure_of_its_key_is_not_rewritten() {
    // A `memo` shares one value between every caller with an equal key and
    // recomputes it after a reload, so each of these would change what the
    // program does: a name outside the key, an effectful native, a cell
    // read, and a call the rule cannot see into (even a pure one: the full
    // purity check is docs/tasks/memo-purity-check.md).
    for (head, init) in [
        ("fn f(k, scale)", "k * scale"),
        ("fn f(k, scale)", "random(0.0, 1.0)"),
        ("var table = [1, 2]\nfn f(k, scale)", "get table"),
        ("fn f(k, scale)", "len(\"{k}\")"),
        ("fn f(k, scale)", "[k, fn() k end]"),
        ("fn f(k, scale)", "if k > 0 then k else scale end"),
    ] {
        let src = format!("{head}\n  state(k) v = {init}\n  v\nend\nprint(f(2, 10), f(2, 100))\n");
        // The source compiles but for its `state(key)`.
        if let Err(e) = Env::new().load_program(&src.replace("state(k)", "state")) {
            panic!("does not compile: {src}: {e}");
        }
        let out = migrate(&src);
        assert_eq!(
            (out.count(STATE_KEY_DEPRECATED), out.rewritten()),
            (1, 0),
            "{init}: {}",
            out.output
        );
        assert_eq!(out.output, src, "{init}");
    }
}

#[test]
fn an_initializer_built_from_its_key_alone_is_rewritten() {
    for (key, init) in [
        ("k", "k * 2 + 1"),
        ("[a, b]", "{sum: a + b, label: \"{a}-{b}\"}"),
        ("k", "[k, -k, k.x ?? 0]"),
        ("k", "nil"),
    ] {
        let src = format!("fn f(k, a, b)\n  state({key}) v = {init}\n  v\nend\nprint(f(1, 2, 3))\n");
        let out = migrate(&src);
        assert_eq!(
            (out.count(STATE_KEY_DEPRECATED), out.rewritten()),
            (1, 1),
            "{key} / {init}: {:?}",
            out.findings
        );
        assert!(out.output.contains(&format!("memo({key}) v")), "{}", out.output);
    }
}

#[test]
fn migrate_reports_a_written_slot_without_a_fix() {
    let out = migrate(HEALTH);
    let [f] = out.findings.as_slice() else {
        panic!("one finding: {:?}", out.findings);
    };
    assert!(!f.fixable);
    assert_eq!(out.output, HEALTH);
}

#[test]
fn every_kind_of_write_keeps_the_state() {
    let mut checked = 0;
    for write in [
        "n = 2",
        "n += 1",
        "n.x = 1",
        "n[0] = 1",
        "n ??= 3",
        "let g = fn() n = 5 end",
    ] {
        let src =
            format!("fn f(id)\n  state(id) n = [{{x: 0}}]\n  {write}\n  n\nend\nprint(f(1))\n");
        // Only sources that compile but for the `state(key)` tell us
        // anything.
        if Env::new().load_program(&src.replace("state(id)", "state")).is_err() {
            continue;
        }
        let out = migrate(&src);
        assert_eq!(
            out.count(STATE_KEY_DEPRECATED),
            1,
            "{write}: {:?}",
            out.findings
        );
        assert_eq!(out.rewritten(), 0, "{write}: {}", out.output);
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} of the writes compiled");
}

#[test]
fn migrate_leaves_a_keyed_state_var_and_a_pub_one_unfixed() {
    let var = "fn f(id)\n  state(id) var c = 0\n  get c\nend\nprint(f(1))\n";
    let out = migrate(var);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.rewritten()), (1, 0));
    let exported = "pub state(1) shared = 0\nprint(shared)\n";
    let out = migrate(exported);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.rewritten()), (1, 0));
}

#[test]
fn migrate_rewrites_the_unwritten_ones_beside_a_written_one() {
    let src = "\
fn f(id)
  state(id) a = id * 2
  state(id) b = 0
  b += a
  b
end
print(f(1))
";
    let out = migrate(src);
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.rewritten()), (2, 1));
    assert!(out.output.contains("memo(id) a = id * 2"), "{}", out.output);
    assert!(out.output.contains("state(id) b = 0"), "{}", out.output);
}

#[test]
fn a_lint_ignore_keeps_the_migration_off_a_site() {
    let src = "\
fn width(text)
  # petal-lint-ignore state-key-deprecated
  state(text) w = text.length * 8
  w
end
print(width(\"hi\"))
";
    let out = migrate(src);
    assert_eq!(out.findings.len(), 0, "{:?}", out.findings);
    assert_eq!(out.output, src);
}

#[test]
fn the_migration_runs_by_default_and_is_not_a_lint_rule() {
    let out = migrate_source(HEALTH, &MigrateOptions::default()).expect("migrate");
    assert_eq!(out.count(STATE_KEY_DEPRECATED), 1, "{:?}", out.findings);
    assert!(petal::migrate::is_migration(STATE_KEY_DEPRECATED));
    assert!(!petal::lint::is_rule(STATE_KEY_DEPRECATED));
    assert_eq!(
        petal::migrate::migration(STATE_KEY_DEPRECATED).map(|m| m.old_form),
        Some(petal::migrate::OldForm::Removed)
    );
    // Lint carries the current syntax only: the file does not parse for it.
    let lint = petal::lint::lint_source(HEALTH, &petal::lint::LintOptions::default());
    assert!(lint.is_err_and(|e| e.contains("`state(key)` was removed")));
}

#[test]
fn the_other_migrations_still_run_on_a_file_with_a_site_left_for_a_hand_edit() {
    // `export` → `pub`, `const X = e` → `let X = const(e)`, an old `//`
    // comment, and the `end`s: all rewritten around a `state(key)` that
    // nothing can rewrite, which is reported.
    let src = "\
// shared per id
export fn health(id, damage)
  state(id) hp = 100
  hp -= damage
  hp
end
const START = 10
print(health(1, START))
";
    let opts = MigrateOptions {
        except: vec![petal::migrate::END_LINES.to_string()],
        ..MigrateOptions::default()
    };
    let out = migrate_source(src, &opts).expect("migrate");
    assert_eq!((out.count(STATE_KEY_DEPRECATED), out.left()), (1, 1), "{:?}", out.findings);
    assert_eq!(
        out.output,
        "\
# shared per id
pub fn health(id, damage)
  state(id) hp = 100
  hp -= damage
  hp
end
let START = const(10)
print(health(1, START))
"
    );
}

#[test]
fn a_migrated_file_is_laid_out_by_end_lines_too() {
    let src = "fn width(text)\n  state(text) w = text.length * 8\n  w\nend\nprint(width(\"hi\"))\n";
    let out = migrate_source(src, &MigrateOptions::default()).expect("migrate");
    assert_eq!(
        out.output,
        "fn width(text)\n  memo(text) w = text.length * 8\n  w\nprint(width(\"hi\"))\n"
    );
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
}

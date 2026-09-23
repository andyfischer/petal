//! Host-facing embedding APIs: typed errors from the recompile and
//! call-a-function paths, and source-file watching for hot reload.

use std::path::PathBuf;

use petal::env::Env;
use petal::error::{CallError, Phase};
use petal::value::Value;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "petal-host-embedding-{}-{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ran(source: &str) -> (Env, petal::stack::StackKey) {
    let mut env = Env::new();
    let pid = env.load_program(source).unwrap();
    let sid = env.create_stack(pid).unwrap();
    env.run(sid).unwrap();
    (env, sid)
}

// ---------------------------------------------------------------------------
// call_function_diag
// ---------------------------------------------------------------------------

#[test]
fn a_missing_function_is_typed_not_found() {
    let (mut env, sid) = ran("fn known()\n  1\nend\n");
    let err = env.call_function_diag(sid, "missing", &[]).unwrap_err();
    assert_eq!(
        err,
        CallError::FunctionNotFound {
            name: "missing".into()
        }
    );
    assert!(err.is_not_found());
    assert!(!env.has_function(sid, "missing"));
    assert!(env.has_function(sid, "known"));
    // The string facade is unchanged.
    let msg = env.call_function(sid, "missing", &[]).unwrap_err();
    assert_eq!(msg, err.to_string());
    assert!(msg.starts_with("No top-level function named 'missing'"));
}

#[test]
fn a_failing_function_is_typed_runtime() {
    let (mut env, sid) = ran("fn boom()\n  1 / \"x\"\nend\nfn add(a, b)\n  a + b\nend\n");
    let err = env.call_function_diag(sid, "boom", &[]).unwrap_err();
    assert!(matches!(err, CallError::Runtime(_)), "got {err:?}");
    assert!(!err.is_not_found());
    let arity = env
        .call_function_diag(sid, "add", &[Value::Int(1)])
        .unwrap_err();
    assert!(matches!(arity, CallError::Runtime(_)), "got {arity:?}");
    assert_eq!(
        env.call_function_diag(sid, "add", &[Value::Int(1), Value::Int(2)]),
        Ok(Value::Int(3))
    );
}

#[test]
fn an_unknown_stack_is_typed() {
    let (mut env, sid) = ran("fn f()\n  1\nend\n");
    env.drop_fork(sid);
    assert_eq!(
        env.call_function_diag(sid, "f", &[]),
        Err(CallError::StackNotFound)
    );
    assert!(!env.has_function(sid, "f"));
}

#[test]
fn call_error_converts_into_string_with_question_mark() {
    fn host(env: &mut Env, sid: petal::stack::StackKey) -> Result<Value, String> {
        Ok(env.call_function_diag(sid, "nope", &[])?)
    }
    let (mut env, sid) = ran("1");
    assert!(host(&mut env, sid).unwrap_err().contains("nope"));
}

// ---------------------------------------------------------------------------
// compile_program_diag
// ---------------------------------------------------------------------------

#[test]
fn recompile_reports_structured_diagnostics() {
    let mut env = Env::new();
    let pid = env.load_program("let x = 1\nx").unwrap();
    let Err(err) = env.compile_program_diag(pid, "let x = 1\nlet = 2\n", None) else {
        panic!("a parse error must not compile");
    };
    assert_eq!(err.phase, Phase::Parse);
    assert!(!err.items.is_empty());
    let span = err.items[0].span.expect("a parse error has a position");
    assert_eq!(span.start.line, 2);
    // Same failure, same text as the string API.
    let Err(msg) = env.compile_program(pid, "let x = 1\nlet = 2\n") else {
        panic!("a parse error must not compile");
    };
    assert_eq!(msg, err.to_string());
    // And a good recompile hands back a program to transfer state into.
    let Ok(program) = env.compile_program_diag(pid, "let x = 2\nx", None) else {
        panic!("a good recompile compiles");
    };
    assert_eq!(program.id, pid);
}

#[test]
fn recompile_at_an_origin_resolves_sibling_imports() {
    let dir = temp_dir("diag-origin");
    std::fs::write(dir.join("helper.ptl"), "export fn two()\n  2\nend\n").unwrap();
    let entry = dir.join("main.ptl");
    let src = "import helper\nhelper.two()\n";
    std::fs::write(&entry, src).unwrap();
    let mut env = Env::new();
    let pid = env.load_program_at(src, &entry).unwrap();
    assert!(env.compile_program_diag(pid, src, Some(&entry)).is_ok());
    let Err(err) = env.compile_program_diag(pid, "import nothere\n1\n", Some(&entry)) else {
        panic!("a missing import must not compile");
    };
    assert_eq!(err.phase, Phase::Module);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// program_source_paths / SourceWatch
// ---------------------------------------------------------------------------

#[test]
fn source_paths_list_the_entry_and_its_file_imports_once() {
    let dir = temp_dir("paths");
    std::fs::write(dir.join("helper.ptl"), "export fn two()\n  2\nend\n").unwrap();
    let entry = dir.join("main.ptl");

    let mut env = Env::new();
    env.register_module("inmem", "export fn one()\n  1\nend\n");
    let src = "import helper\nimport inmem\nhelper.two() + inmem.one()\n";
    std::fs::write(&entry, src).unwrap();
    let pid = env.load_program_at(src, &entry).unwrap();

    let paths = env.program_source_paths(pid, Some(&entry));
    assert_eq!(paths.len(), 2, "entry + helper, no in-memory modules: {paths:?}");
    assert_eq!(paths[0], entry, "the entry comes first, as spelled");
    assert!(paths[1].ends_with("helper.ptl"));

    // A differently spelled entry path is the same file: still two.
    let respelled = dir.join(".").join("main.ptl");
    assert_eq!(env.program_source_paths(pid, Some(&respelled)).len(), 2);

    // Without an entry: whatever the manifest has on disk.
    let paths = env.program_source_paths(pid, None);
    assert!(paths.iter().any(|p| p.ends_with("helper.ptl")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_single_file_program_watches_just_its_entry() {
    let dir = temp_dir("single");
    let entry = dir.join("main.ptl");
    std::fs::write(&entry, "1").unwrap();
    let mut env = Env::new();
    let pid = env.load_program("1").unwrap();
    assert_eq!(env.program_source_paths(pid, Some(&entry)), vec![entry.clone()]);
    assert!(env.program_source_paths(pid, None).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_watch_notices_an_edited_import() {
    let dir = temp_dir("watch");
    let helper = dir.join("helper.ptl");
    std::fs::write(&helper, "export fn two()\n  2\nend\n").unwrap();
    let entry = dir.join("main.ptl");
    let src = "import helper\nhelper.two()\n";
    std::fs::write(&entry, src).unwrap();
    let mut env = Env::new();
    let pid = env.load_program_at(src, &entry).unwrap();

    let mut watch = env.watch_program_sources(pid, Some(&entry));
    assert_eq!(watch.paths().count(), 2);
    assert!(!watch.changed());

    // A different length is a change even within one mtime tick.
    std::fs::write(&helper, "export fn two()\n  1 + 1\nend\n").unwrap();
    assert!(watch.changed());
    let changed = watch.changed_paths();
    assert_eq!(changed.len(), 1);
    assert!(changed[0].ends_with("helper.ptl"));

    watch.refresh();
    assert!(!watch.changed());
    let _ = std::fs::remove_dir_all(&dir);
}

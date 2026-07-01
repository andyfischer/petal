//! Differential tests: every snippet is run under both `Backend::Graph` and
//! `Backend::Bytecode` (optimizations off) and their results must agree. The
//! graph engine is the correctness oracle; "bytecode with opts off" must match
//! it exactly (see docs/bytecode-status.md, M1 step 6).

use crate::backend::Backend;
use crate::env::Env;
use crate::value;

/// Run `code` on `backend`, returning the rendered result value plus the print
/// output buffer. Values are compared by display string because heap ids are
/// not comparable across two independent runs.
fn run(code: &str, backend: Backend) -> Result<(String, Vec<String>), String> {
    let mut env = Env::new();
    env.set_backend(backend);
    let v = env.run_source(code)?;
    let rendered = value::value_to_display_string(&v, env.heap());
    Ok((rendered, env.take_output()))
}

/// Assert the two backends agree: either both error, or both succeed with an
/// equal rendered value and equal print output.
#[track_caller]
fn assert_parity(code: &str) {
    let graph = run(code, Backend::Graph);
    let bytecode = run(code, Backend::Bytecode);
    match (graph, bytecode) {
        (Ok((gv, go)), Ok((bv, bo))) => {
            assert_eq!(gv, bv, "value mismatch for:\n{code}");
            assert_eq!(go, bo, "output mismatch for:\n{code}");
        }
        (Err(_), Err(_)) => {} // both errored — parity holds (messages may differ)
        (g, b) => panic!("ok/err mismatch for:\n{code}\n  graph={g:?}\n  bytecode={b:?}"),
    }
}

#[test]
fn arithmetic() {
    assert_parity("let x = 1 + 2 * 3");
    assert_parity("let x = (10 - 4) / 2");
    assert_parity("let x = 17 % 5");
    assert_parity("let x = -42");
    assert_parity("let x = 3.5 * 2.0 + 1.0");
    assert_parity("let x = 2 + 3.5");
}

#[test]
fn comparisons_and_logic() {
    assert_parity("let x = 3 < 5");
    assert_parity("let x = 5 <= 5");
    assert_parity("let x = 9 > 2");
    assert_parity("let x = 4 >= 7");
    assert_parity("let x = 1 == 1");
    assert_parity("let x = 1 != 2");
    assert_parity("let x = !true");
}

#[test]
fn strings() {
    assert_parity(r#"let x = "foo" ++ "bar""#);
    assert_parity(r#"let x = "n=" ++ 42"#);
    assert_parity(r#"let x = "abc".length"#);
}

#[test]
fn containers_and_access() {
    assert_parity("let x = [1, 2, 3]");
    assert_parity("let p = { a: 1, b: 2 }\nlet y = p.a");
    assert_parity("let p = { a: 1, b: [2, 3] }\nlet y = p.b[1]");
    assert_parity("let xs = [10, 20, 30]\nlet y = xs[0] + xs[-1]");
    assert_parity("let xs = [10, 20, 30]\nlet y = xs.length");
    assert_parity("let p = { a: 1 }\nlet q = { ...p, b: 2 }\nlet y = q.b");
}

#[test]
fn value_semantics_setindex_setfield() {
    assert_parity("let xs = [1, 2, 3]\nlet ys = xs[1] = 99\nlet y = xs[1]");
    assert_parity("let p = { a: 1 }\nlet q = p.a = 5\nlet y = p.a");
}

#[test]
fn error_parity() {
    assert_parity("let x = 1 / 0");
    assert_parity("let xs = [1, 2]\nlet y = xs[5]");
    assert_parity(r#"let x = 1 + "a""#);
}

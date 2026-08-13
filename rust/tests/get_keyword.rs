//! `get` — the read half of the `var` escape hatch.
//!
//! `var` declares a cell, `set` writes it, and `get` reads it. The reason the
//! read needs a keyword at all is that a bare name means two different things
//! depending on a declaration that may be far away: reading a `let`/`state`
//! from inside a function yields the value captured at the function's
//! *definition point*, while reading a `var` yields the cell's contents *now*.
//! In a panel — where the script re-runs every frame — that difference shows up
//! as exactly one frame of lag, which is invisible until it isn't.
//!
//! So `get` is required wherever the read crosses a function boundary from the
//! declaration, which is exactly the set of positions where the two timings can
//! disagree. Inside the declaring scope a bare read is still fine: there is no
//! snapshot there to confuse it with.

use petal::env::Env;

fn run(src: &str) -> Result<String, String> {
    let mut env = Env::new();
    let pid = env.load_program(src)?;
    let sid = env.create_stack(pid)?;
    env.run(sid)?;
    Ok(env.take_output().join("\n").trim().to_string())
}

fn err(src: &str) -> String {
    match run(src) {
        Ok(out) => panic!("expected an error, got output: {out:?}"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// Parsing and evaluation
// ---------------------------------------------------------------------------

#[test]
fn get_reads_a_cell_across_a_function_boundary() {
    let out = run("var c = 1\nfn peek()\n  get c\nend\nset c = 2\nprint(peek())").unwrap();
    assert_eq!(out, "2", "a cell read must see the write, not a snapshot");
}

#[test]
fn get_is_permitted_but_not_required_in_the_declaring_scope() {
    // No function boundary is crossed, so both spellings are legal and equal.
    assert_eq!(run("var c = 1\nset c = 2\nprint(c)").unwrap(), "2");
    assert_eq!(run("var c = 1\nset c = 2\nprint(get c)").unwrap(), "2");
}

#[test]
fn get_binds_tighter_than_postfix_so_field_access_applies_to_the_contents() {
    // `get cfg.w` is `(get cfg).w`, not `get (cfg.w)` — the cell holds the
    // record, so the dereference has to happen first.
    let src = "var cfg = {w: 3}\nfn peek()\n  get cfg.w\nend\nset cfg = {w: 7}\nprint(peek())";
    assert_eq!(run(src).unwrap(), "7");
}

#[test]
fn get_composes_with_arithmetic_and_calls() {
    let src = "var n = 2\nfn twice()\n  get n * 2\nend\nset n = 5\nprint(twice())";
    assert_eq!(run(src).unwrap(), "10");
}

#[test]
fn get_works_on_a_state_var() {
    let src = "state var hits = 0\nfn peek()\n  get hits\nend\nset hits = 3\nprint(peek())";
    assert_eq!(run(src).unwrap(), "3");
}

#[test]
fn get_works_on_a_cell_captured_by_a_lambda() {
    // A lambda is a function boundary too, including one nested inside the
    // function that declared the cell.
    let src = "fn go()\n  var n = 1\n  let f = fn()\n    get n\n  end\n  set n = 9\n  f()\nend\nprint(go())";
    assert_eq!(run(src).unwrap(), "9");
}

// ---------------------------------------------------------------------------
// The rule: a bare cell read across a function boundary is an error
// ---------------------------------------------------------------------------

#[test]
fn bare_cell_read_inside_a_function_is_an_error() {
    let e = err("var c = 1\nfn peek()\n  c\nend\nprint(peek())");
    assert!(
        e.contains("`c` is a `var`") && e.contains("get c"),
        "error should name the var and suggest `get c`, got: {e}"
    );
}

#[test]
fn bare_state_var_read_inside_a_function_is_an_error() {
    let e = err("state var hits = 0\nfn peek()\n  hits\nend\nprint(peek())");
    assert!(e.contains("get hits"), "got: {e}");
}

#[test]
fn bare_cell_read_inside_a_lambda_is_an_error() {
    let e = err("fn go()\n  var n = 1\n  let f = fn()\n    n\n  end\n  f()\nend\nprint(go())");
    assert!(e.contains("get n"), "got: {e}");
}

#[test]
fn the_error_points_at_the_read_not_the_declaration() {
    let e = err("var c = 1\n\n\nfn peek()\n  c\nend\nprint(peek())");
    assert!(
        e.contains("line 5"),
        "should point at the read on line 5, got: {e}"
    );
}

// ---------------------------------------------------------------------------
// The other direction: `get` on something that is not a cell
// ---------------------------------------------------------------------------

#[test]
fn get_on_a_let_is_an_error() {
    let e = err("let x = 1\nfn peek()\n  get x\nend\nprint(peek())");
    assert!(
        e.contains("`x` is not a `var`"),
        "error should say x is not a var, got: {e}"
    );
}

#[test]
fn get_on_a_state_is_an_error() {
    let e = err("state s = 1\nfn peek()\n  get s\nend\nprint(peek())");
    assert!(e.contains("`s` is not a `var`"), "got: {e}");
}

#[test]
fn get_on_an_unknown_name_is_an_error() {
    let e = err("fn peek()\n  get nope\nend\nprint(peek())");
    assert!(e.contains("nope"), "got: {e}");
}

// ---------------------------------------------------------------------------
// What the keyword is for: the two timings are now visibly different
// ---------------------------------------------------------------------------

#[test]
fn a_bare_read_is_always_a_snapshot_and_a_get_is_always_live() {
    // The whole point. Same program shape, one keyword of difference, and the
    // reader can now tell which timing they are getting without looking up the
    // declaration.
    //
    // A module-scope `let` rebound below the function is the canonical shape:
    // capturing at the definition is the defined behaviour there, not a
    // mistake, so `capture_lag` says nothing about it. See
    // `crate::compiler::capture_lag` for the reactive case it does warn on.
    let snapshot = run(
        "let x = 1\nfn peek()\n  x\nend\nlet before = peek()\nx = 2\nprint(\"{before} {peek()}\")",
    )
    .unwrap();
    assert_eq!(snapshot, "1 1", "a captured let never moves");

    let live = run("var x = 1\nfn peek()\n  get x\nend\nlet before = peek()\nset x = 2\nprint(\"{before} {peek()}\")").unwrap();
    assert_eq!(live, "1 2", "a cell read tracks the write");
}

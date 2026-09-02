//! An overloaded function's variants carry an internal name — `box#1`, minted
//! in `compiler/function.rs` so the two `fn box` declarations can coexist as
//! separate `FunctionDef`s. That name is a compiler implementation detail: no
//! one wrote it, and nothing outside the compiler should ever see it.
//!
//! It reaches the outside world through three durable carriers — `FunctionDef.
//! name`, the `MakeClosure` term's name, and the self-reference phantom's name
//! — which between them feed every inspection command, the trace, the graph
//! dump, the disassembler, runtime error messages, and the public function
//! table a host calls through. Each of those is normalized with
//! `program::base_fn_name`; this file is the fence around that.
//!
//! Every test asserts the same two things: the output names the function the
//! way the source wrote it, and no `#` survives anywhere in it.

use std::process::Command;

use petal::env::Env;
use petal::program::base_fn_name;
use petal::value::Value;

/// Path to the freshly built `petal` binary, which Cargo sets for integration
/// tests. The inspection commands live in the private `cli::handlers` module,
/// so their output is only observable through the command itself.
const PETAL: &str = env!("CARGO_BIN_EXE_petal");

/// Two arities of one name, plus a top-level binding to explain, a call, and a
/// self-reference from the one-argument variant (which is what puts a mangled
/// name on the capture phantom as well as on the closure).
const OVL: &str = "\
fn box(w)
  box(w, w)
end
fn box(w, h)
  [w, h]
end
let x = 2
print(box(1))
";

/// Write `OVL` to a temp file and hand back the path — the inspection
/// subcommands take a file, not `-e`.
fn ovl_file() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("petal-ovl-{}.ptl", std::process::id()));
    std::fs::write(&path, OVL).expect("write the overload fixture");
    path
}

/// Run `petal <args…> <the fixture>` and return stdout + stderr together.
fn petal(args: &[&str]) -> String {
    let path = ovl_file();
    let out = Command::new(PETAL)
        .args(args)
        .arg(&path)
        .output()
        .expect("failed to run petal");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The two assertions every site shares.
#[track_caller]
fn names_the_source_function(what: &str, text: &str) {
    assert!(
        text.contains("box"),
        "{what} never named the function:\n{text}"
    );
    assert!(
        !text.contains('#'),
        "{what} leaked an internal overload name:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// The stripper itself
// ---------------------------------------------------------------------------

#[test]
fn base_fn_name_strips_only_the_arity_suffix() {
    assert_eq!(base_fn_name("box#1"), "box");
    assert_eq!(base_fn_name("box"), "box");
    // A qualified method keeps its class prefix; only the suffix goes.
    assert_eq!(base_fn_name("Point.shift#2"), "Point.shift");
    assert_eq!(base_fn_name("ui::button#1"), "ui::button");
}

// ---------------------------------------------------------------------------
// The public function table (`Stack::functions`)
// ---------------------------------------------------------------------------

/// The root-frame harvest that populates the table a host calls through skips
/// mangled names outright, so an internal name is not a public key. This is the
/// root fix: it is what keeps `push_closure_frame`'s arity message from ever
/// being reachable with a mangled name in the first place.
#[test]
fn an_internal_overload_name_is_not_a_callable_key() {
    let mut env = Env::new();
    let pid = env.load_program(OVL).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("runs");

    for internal in ["box#1", "box#2"] {
        let e = env
            .call_function(sid, internal, &[Value::Int(1)])
            .expect_err("an internal name is not callable");
        assert!(
            e.contains("No top-level function named"),
            "unexpected error for {internal}: {e}"
        );
    }
    // The name the source wrote is the one that works.
    assert!(env.call_function(sid, "box", &[Value::Int(1)]).is_ok());
}

/// Calling the source name at an arity no variant declares reports the arities
/// that exist, under the written name.
#[test]
fn a_host_arity_mismatch_names_the_source_function() {
    let mut env = Env::new();
    let pid = env.load_program(OVL).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    env.run(sid).expect("runs");

    let e = env
        .call_function(sid, "box", &[Value::Int(1), Value::Int(2), Value::Int(3)])
        .expect_err("no 3-argument variant");
    assert_eq!(e, "box() expects 1 or 2 arguments, got 3");
}

// ---------------------------------------------------------------------------
// Runtime error messages
// ---------------------------------------------------------------------------

/// The same arity message reached from source rather than from a host call.
#[test]
fn a_runtime_arity_error_names_the_source_function() {
    let mut env = Env::new();
    let pid = env
        .load_program("fn box(w)\n  w\nend\nfn box(w, h)\n  w\nend\nprint(box(1, 2, 3))\n")
        .expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    let e = env.run(sid).expect_err("no 3-argument variant");
    names_the_source_function("the arity error", &e);
    assert!(
        e.contains("box() expects 1 or 2 arguments, got 3"),
        "unexpected error: {e}"
    );
}

/// An overloaded *method*'s variant is `Point.shift#2` internally, and it is
/// the one the binder holds when the receiver plus one written argument select
/// it. The class prefix survives; the suffix does not.
#[test]
fn a_method_variants_binding_error_keeps_the_class_but_not_the_suffix() {
    let src = "class Point
  x,
end
fn Point.shift(p)
  p.x
end
fn Point.shift(p, dx)
  p.x + dx
end
let p = Point(1)
print(p.shift(nope: 2))
";
    let mut env = Env::new();
    let pid = env.load_program(src).expect("compiles");
    let sid = env.create_stack(pid).expect("stack");
    let e = env.run(sid).expect_err("no parameter named nope");
    assert!(
        e.contains("Point.shift() has no parameter named 'nope'") && !e.contains('#'),
        "unexpected error: {e}"
    );
}

// ---------------------------------------------------------------------------
// Library-level renderers
// ---------------------------------------------------------------------------

#[test]
fn show_ir_names_the_source_function() {
    let mut env = Env::new();
    let pid = env.load_program(OVL).expect("compiles");
    let text = petal::ir_display::display_program(env.get_program(pid).expect("program"));
    names_the_source_function("show-ir", &text);
}

#[test]
fn the_disassembly_names_the_source_function() {
    let text = petal::inspect::render(OVL, petal::inspect::Stage::Bytecode).expect("lowers");
    names_the_source_function("show-bytecode", &text);
}

#[test]
fn the_dot_graph_names_the_source_function() {
    let mut env = Env::new();
    let pid = env.load_program(OVL).expect("compiles");
    let text = petal::dot_graph::program_to_dot(env.get_program(pid).expect("program"), false);
    names_the_source_function("show-graph", &text);
}

// ---------------------------------------------------------------------------
// CLI surfaces
// ---------------------------------------------------------------------------

#[test]
fn explain_names_the_source_function() {
    // `--term box` walks the chain back through both variant closures, which
    // is where a mangled name would show up — in the header line and in the
    // chain rows.
    names_the_source_function("explain --term box", &petal(&["explain", "--term", "box"]));
    names_the_source_function(
        "explain --json",
        &petal(&["explain", "--term", "box", "--json"]),
    );
}

#[test]
fn the_provenance_commands_name_the_source_function() {
    // `--term box` lands on the `MakeOverloadSet`, whose ancestors are the two
    // variant `MakeClosure` terms — the rows that carry the mangled names.
    for cmd in ["show-provenance", "show-dependents"] {
        names_the_source_function(cmd, &petal(&[cmd, "--term", "box"]));
        names_the_source_function(cmd, &petal(&[cmd, "--term", "box", "--json"]));
    }
}

#[test]
fn the_graph_and_bytecode_commands_name_the_source_function() {
    names_the_source_function("show-graph", &petal(&["show-graph"]));
    names_the_source_function("show-ir", &petal(&["show-ir"]));
    names_the_source_function("show-bytecode", &petal(&["show-bytecode"]));
    names_the_source_function("show-bytecode --json", &petal(&["show-bytecode", "--json"]));
}

#[test]
fn a_recorded_trace_names_the_source_function() {
    let out = std::env::temp_dir().join(format!("petal-ovl-trace-{}.json", std::process::id()));
    petal(&["run", "--record-trace", out.to_str().expect("utf-8 path")]);
    let text = std::fs::read_to_string(&out).expect("the trace was written");
    names_the_source_function("run --record-trace", &text);
}

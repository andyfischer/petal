//! The `autodiff` package's finite-difference functions
//! (docs/features/differentiation.md), where a host is needed to see the
//! property: a probe's emits never reach the host's buffers, and its state
//! writes never outlive it, frame after frame. The behaviour of `deriv`,
//! `jacobian`, `solve_step` and `solve` themselves is tested in Petal, in
//! test/ptl/autodiff.ptl.

use petal::env::Env;
use petal::value;

/// Run `code` for `runs` frames, returning the `cmds` buffer's contents after
/// each frame and the final state as sorted `key=value` pairs.
fn frames(code: &str, runs: usize) -> (Vec<Vec<String>>, String) {
    let mut env = Env::new();
    let pid = env.load_program(code).unwrap();
    let sid = env.create_stack(pid).unwrap();
    let cmds = env.intern_symbol("cmds");
    let mut out = Vec::new();
    for i in 0..runs {
        if i > 0 {
            env.reset_stack(sid).unwrap();
        }
        env.run(sid).unwrap();
        out.push(
            env.take_output_buffer(cmds)
                .iter()
                .map(|v| value::value_to_display_string(v, env.heap()))
                .collect(),
        );
    }
    let mut pairs: Vec<String> =
        env.get_state_json(pid, sid).iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    (out, pairs.join(","))
}

const PROGRAM: &str = "import io
import autodiff
state frame = 0
state var total = 0.0
fn tip(x)
  state seen = 0
  seen += 1
  set total = (get total) + x
  push_output(symbol(\"cmds\"), x)
  x * x + seen
end
frame += 1
SOLVE
push_output(symbol(\"cmds\"), \"live {frame}\")
";

#[test]
fn probes_leave_no_emits_and_no_state_behind() {
    let calls = "let d = deriv(tip, 2.0)
let j = jacobian(fn(p) [tip(p[0]), tip(p[1])] end, [1.0, 2.0])
let s = solve(fn(p) tip(p[0]) end, [1.0], 5.0)
assert(abs(d.value - 4.0) < 0.000001)
assert(abs(s.xs[0] - 2.0) < 0.000001)";
    let with = frames(&PROGRAM.replace("SOLVE", calls), 3);
    let without = frames(&PROGRAM.replace("SOLVE", ""), 3);
    assert_eq!(with, without);
    assert_eq!(with.0[2], ["live 3"]);
}

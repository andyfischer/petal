// Minimal reproduction of: state assignment inside an `if` (or any child
// block) does NOT emit a StateWrite, so the new value is lost on the next
// frame/run (when reset_stack + run reads the state back from the persistent
// state map).
//
// Root cause: compile_assign calls scope_lookup(name). When the assignment is
// inside an `if` body, scope_lookup returns the Phi term (which references
// the outer StateInit), not the StateInit itself. The check
//   `if let TermOp::StateInit = &self.terms[existing_tid.0].op`
// fails, so no StateWrite is emitted — the new value is only rebound in SSA
// scope and is lost at the block boundary.
//
// Symptom at the application level: in petal-sdl / petal-fps, any game logic
// of the form
//   if key_pressed("space") { jumping = true }
// silently drops the assignment, making conditional state updates impossible.

import { describe, it, expect } from "vitest";
import { runPetal, showIrJson, termsByOp } from "./helpers";


describe("state assignment inside if block", () => {
  it("top-level state assignment emits StateWrite", () => {
    const ir = showIrJson("state x = 0\nx = x + 1");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(1);
  });

  it("state assignment inside `if true` block ALSO emits StateWrite", () => {
    const ir = showIrJson("state y = 0\nif true then\n  y = y + 1\nend");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(1);
  });

  it("state assignment inside `if/else` emits StateWrite in both arms", () => {
    const ir = showIrJson(
      "state y = 0\nif true then\n  y = y + 1\nelse\n  y = y + 2\nend"
    );
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(2);
  });

  // Repeat reassignments to the same state variable each emit a StateWrite —
  // the second `x = ...` was previously dropped because scope_lookup returned
  // the first assignment's Copy term, which find_state_init couldn't trace
  // back to the StateInit. See compiler.rs::find_state_init / state_inits.
  it("multiple top-level reassignments each emit StateWrite", () => {
    const ir = showIrJson("state x = 0\nx = 5\nx = 10");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(2);
  });

  it("three reassignments emit three StateWrites", () => {
    const ir = showIrJson("state z = 0\nz = 1\nz = 2\nz = 3");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(3);
  });

  // Under call-path keying a StateWrite also has to say which *slot* it means:
  // an assignment nested deeper in loops than its declaration pops back to the
  // declaration's slot (`Term::path_pop`). A conditional does not nest the
  // path — only a loop does — so the branch depth must not leak into it.
  it("a conditional write carries the loop depth, not the branch depth", () => {
    const ir = showIrJson(
      "state x = 0\nif true then\n  if true then\n    x = 1\n  end\nend",
    );
    const write = termsByOp(ir, "StateWrite")[0];
    expect(write.path_pop).toBeUndefined(); // omitted when zero
  });

  it("a write inside an `if` inside a loop pops the loop", () => {
    const ir = showIrJson(
      "state x = 0\nfor i in [1, 2] do\n  if i > 1 then\n    x = i\n  end\nend",
    );
    const write = termsByOp(ir, "StateWrite")[0];
    expect(write.path_pop).toBe(1);
  });

  it("a conditional write to a loop-local declaration pops nothing", () => {
    const ir = showIrJson(
      "for i in [1, 2] do\n  state x = 0\n  if i > 1 then\n    x = i\n  end\nend",
    );
    const write = termsByOp(ir, "StateWrite")[0];
    expect(write.path_pop).toBeUndefined();
  });

  it("a conditional write inside a function reaches the same slot on rerun", () => {
    // The runtime half of the same rule: `if` bodies do not fork the path, so a
    // write from inside one lands where the surrounding reads look.
    const out = runPetal(`
      fn tick(on)
        state n = 0
        if on then
          n = n + 1
        end
        n
      end
      for i in range(0, 3) do
        print(tick(true))
      end
    `);
    // One callsite inside one loop → per-iteration slots, each hit once.
    expect(out).toBe("1\n1\n1");
  });
});

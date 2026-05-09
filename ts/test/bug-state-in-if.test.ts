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

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showIrJson, termsByOp } from "./helpers";

beforeAll(() => ensureBuild());

describe("state assignment inside if block", () => {
  it("top-level state assignment emits StateWrite", () => {
    const ir = showIrJson("state x = 0\nx = x + 1");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(1);
  });

  it("state assignment inside `if true` block ALSO emits StateWrite", () => {
    const ir = showIrJson("state y = 0\nif true { y = y + 1 }");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBe(1);
  });

  it("state assignment inside `if/else` emits StateWrite in both arms", () => {
    const ir = showIrJson(
      "state y = 0\nif true { y = y + 1 } else { y = y + 2 }"
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
});

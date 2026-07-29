// `petal check` must answer "will this run?", which means lowering to bytecode
// and not just lexing/parsing/compiling. A program can compile cleanly and
// still fail to lower, and `check` is what CI and editors call — so a check
// that stops before lowering reports a green build for a program that aborts on
// first run. That is exactly how the shadowed-name phi bug survived in the
// shipped `petal-ui` prelude. See docs/dev/var-next-steps.md (Lexical shadowing).
//
// The negative direction is asserted on *injected IR*, not on a source program.
// Lowering has exactly two failure sites, and neither is reachable from source
// any more: the "unlowered op" arm is dead (every `TermOp` is handled), and
// `FnLowerer::flat` ("term tN in block bN not in this function") needs an input
// edge that crosses a function boundary. The compiler stopped emitting that
// edge when cross-function assignment became a compile error (see
// docs/dev/var-next-steps.md §2a); ~50 candidate shapes were probed — match-arm
// phi, loop-carried closures, nested capture chains, `state var` in nested
// scopes, an exported `var` written through a nested fn, break/continue phi
// carry-outs — and every one lowers cleanly.
//
// So the gate builds the edge itself: take a real program's IR, repoint one
// root-block term's input at a term inside a function body, and feed it to
// `check --ir -`. `Program::validate` (rust/src/ir_validate.rs) only
// range-checks ids and arities — it does not check function-boundary edges —
// so the corrupted IR imports fine and dies in lowering, which is precisely
// what the gate needs to observe. The IR is built here rather than checked in
// as a blob so it can never drift from the IR format.

import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  checkText,
  checkIrText,
  checkIrJsonAllowFail,
  showIrJsonRaw,
} from "./helpers";

beforeAll(() => ensureBuild());

const GOOD_SOURCE = `fn f()
  1
end
let a = 2
print(a + 3)`;

/** IR with one root-block input edge repointed into `f`'s body block. */
function badIr(): string {
  const ir = JSON.parse(showIrJsonRaw(GOOD_SOURCE));
  const fBody = ir.functions[0].body_block;
  const foreign = ir.terms.find((t: any) => t.block_id === fBody).id;
  const victim = ir.terms
    .filter((t: any) => t.block_id === ir.root_block && t.inputs.length)
    .pop();
  // Term ids equal their index, enforced by ir_validate.rs.
  ir.terms[victim.id].inputs[0] = foreign;
  return JSON.stringify(ir);
}

describe("petal check lowers to bytecode", () => {
  it("still passes a program that lowers", () => {
    const { code, stderr } = checkText(`let i = 0
if true then i = 1 end
print(i)`);
    expect(stderr).toBe("");
    expect(code).toBe(0);
  });

  it("a local shadowing a std name lowers (the ui.ptl _wrap_segment shape)", () => {
    // `take` collides with `std::take` from the auto-loaded core prelude.
    // Before the phi pre-scan became scope-aware this did not lower, and
    // `check` did not notice.
    const { code, stderr } = checkText(`fn f(words)
  for w in words do
    while len(w) > 2 do
      let take = 2
      while take < 3 do
        take = take + 1
      end
      w = slice(w, take, len(w))
    end
  end
end`);
    expect(stderr).toBe("");
    expect(code).toBe(0);
  });

  // The uncorrupted IR must pass, or the negative results below would only be
  // proving that `--ir` import is broken.
  it("accepts the uncorrupted IR via check --ir -", () => {
    const { code, stderr } = checkIrText(showIrJsonRaw(GOOD_SOURCE));
    expect(stderr).toBe("");
    expect(code).toBe(0);
  });

  it("fails on IR that compiles but cannot lower", () => {
    const { code, stderr } = checkIrText(badIr());
    expect(stderr).toContain("bytecode lowering failed");
    expect(code).toBe(1);
  });

  it("reports the lowering failure as phase 'lower' in --json", () => {
    const result = checkIrJsonAllowFail(badIr());
    expect(result.error).toBe(true);
    expect(result.phase).toBe("lower");
    expect(result.message).toContain("not in this function");
  });
});

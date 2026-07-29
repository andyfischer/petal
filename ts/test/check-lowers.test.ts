// `petal check` must answer "will this run?", which means lowering to bytecode
// and not just lexing/parsing/compiling. A program can compile cleanly and
// still fail to lower, and `check` is what CI and editors call — so a check
// that stops before lowering reports a green build for a program that aborts on
// first run. That is exactly how the shadowed-name phi bug survived in the
// shipped `petal-ui` prelude. See docs/dev/var-next-steps.md (Lexical shadowing).

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, checkText, checkJsonAllowFail } from "./helpers";

beforeAll(() => ensureBuild());

// Assignment to a binding from an outer function, inside control flow: compiles,
// does not lower. (Becoming a proper compile error is step 5 of the plan; until
// then it is the most convenient program that compiles but cannot run.)
const COMPILES_BUT_DOES_NOT_LOWER = `let i = 0
fn f()
  if false then i = 1 end
  i
end
print(f())`;

describe("petal check lowers to bytecode", () => {
  it("fails on a program that compiles but does not lower", () => {
    const { code, stderr } = checkText(COMPILES_BUT_DOES_NOT_LOWER);
    expect(code).toBe(1);
    expect(stderr).toContain("bytecode lowering failed");
  });

  it("reports the lowering failure as its own phase in JSON mode", () => {
    const out = checkJsonAllowFail(COMPILES_BUT_DOES_NOT_LOWER);
    expect(out.error).toBe(true);
    expect(out.phase).toBe("lower");
    expect(out.message).toContain("bytecode lowering failed");
  });

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
});

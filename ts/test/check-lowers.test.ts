// `petal check` must answer "will this run?", which means lowering to bytecode
// and not just lexing/parsing/compiling. A program can compile cleanly and
// still fail to lower, and `check` is what CI and editors call — so a check
// that stops before lowering reports a green build for a program that aborts on
// first run. That is exactly how the shadowed-name phi bug survived in the
// shipped `petal-ui` prelude. See docs/dev/var-next-steps.md (Lexical shadowing).
//
// NOTE: this file no longer asserts that `check` *fails* on a program that
// lowers badly — the only such program in the repo was a cross-function
// assignment, which is now a compile error and never reaches lowering. No
// replacement shape is known. What remains is the positive direction: programs
// that must lower, including the exact shape of the bug above. See
// docs/dev/var-next-steps.md (Followups) — restoring the negative gate is open.

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, checkText } from "./helpers";

beforeAll(() => ensureBuild());

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
});

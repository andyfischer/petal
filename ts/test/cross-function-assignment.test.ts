// Assigning to a name bound outside the current function does not modify that
// binding — it creates a function-local shadow, silently. The compiler warns at
// every such site; the warning is the measurement step for the planned
// `var`/`set` escape hatch, and becomes an error once that lands.
// See docs/dev/var-next-steps.md (Why the feature exists).

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, checkJson, checkJsonAllowFail } from "./helpers";

beforeAll(() => ensureBuild());

const outerAssignWarnings = (json: any): string[] =>
  (json.warnings ?? [])
    .map((w: any) => w.message as string)
    .filter((m: string) => m.includes("bound outside this function"));

describe("cross-function assignment warning", () => {
  it("fires on assignment to a module-level let", () => {
    const w = outerAssignWarnings(
      checkJson(`let i = 10
fn f()
  i = i + 1
  i
end
print(f())`),
    );
    expect(w).toHaveLength(1);
    expect(w[0]).toContain("`i` is bound outside this function");
  });

  it("fires on state, captures, an enclosing fn's local, and path targets", () => {
    const w = outerAssignWarnings(
      checkJson(`state s = 0
let xs = [1, 2, 3]
let r = { a: 1 }
fn f()
  s = s + 1
  xs[0] = 99
  r.a = 5
end
fn g()
  let local = 0
  let inner = fn()
    local = 1
  end
  inner()
  local
end
f()
print(g())`),
    );
    expect(w).toHaveLength(4);
  });

  it("fires once per site, not once per name", () => {
    // Every later lookup finds the phi the `if` installed, so without
    // `cross_fn_terms` only the first assignment would be reported.
    const w = outerAssignWarnings(
      checkJsonAllowFail(`let x = 1
fn f(c)
  if c then
    x = 1
    x = 2
  end
end
f(true)`),
    );
    expect(w).toHaveLength(2);
  });

  it("is silent for same-function and top-level rebinds", () => {
    const w = outerAssignWarnings(
      checkJson(`let t = 0
if true then t = 1 end
fn ok(n)
  let m = 0
  if n > 0 then m = 1 end
  n = n + m
  n
end
print(t + ok(2))`),
    );
    expect(w).toEqual([]);
  });

  it("is silent after a `let` in the function shadows the outer name", () => {
    const w = outerAssignWarnings(
      checkJson(`let x = 1
fn f(c)
  let x = 5
  if c then x = 6 end
  x
end
print(f(true))`),
    );
    expect(w).toEqual([]);
  });

  it("is reported even when the program fails to lower", () => {
    // The three shipped SDL examples are in exactly this state: they compile,
    // they warn, and they abort at lowering. A sweep that only looked at
    // programs which lower would score them as clean.
    const out = checkJsonAllowFail(`let i = 0
fn f()
  if false then i = 1 end
  i
end
print(f())`);
    expect(out.error).toBe(true);
    expect(out.phase).toBe("lower");
    expect(outerAssignWarnings(out)).toHaveLength(1);
  });
});

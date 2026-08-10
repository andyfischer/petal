// Assigning to a name bound outside the current function does not modify that
// binding — it would create a function-local shadow, silently. That is a
// compile error at the assignment site: the code reads as a dataflow edge that
// is not there, and one control-flow step further it did not even lower. The
// escape hatch for genuine mutation is `var` + `set`.
// See docs/dev/var-next-steps.md (Why the feature exists).

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, checkJson, checkJsonAllowFail } from "./helpers";

beforeAll(() => ensureBuild());

/**
 * Every reported site. Compiler errors arrive newline-joined in a single
 * `message`, each but the last carrying a `[line N, column M]` suffix — so a
 * split is what counts sites, not the `warnings` array.
 */
const outerAssignErrors = (json: any): string[] =>
  String(json.message ?? "")
    .split("\n")
    .filter((m: string) => m.includes("bound outside this function"));

describe("cross-function assignment is a compile error", () => {
  it("fires on assignment to a module-level let", () => {
    const out = checkJsonAllowFail(`let i = 10
fn f()
  i = i + 1
  i
end
print(f())`);
    expect(out.error).toBe(true);
    const e = outerAssignErrors(out);
    expect(e).toHaveLength(1);
    expect(e[0]).toContain("`i` is bound outside this function");
    // The message must name the escape hatch, not just refuse.
    expect(e[0]).toContain("var i = ...");
    expect(e[0]).toContain("set i = ...");
  });

  it("fires on state, captures, an enclosing fn's local, and path targets", () => {
    const out = checkJsonAllowFail(`state s = 0
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
print(g())`);
    expect(out.error).toBe(true);
    // All four declaration sites (module state, module let via index and
    // field, lambda capture of an enclosing fn's local) report.
    expect(outerAssignErrors(out)).toHaveLength(4);
  });

  it("reports every site, and locates each one", () => {
    // Every later lookup finds the phi the `if` installed, so without
    // `cross_fn_terms` only the first assignment would be reported.
    const out = checkJsonAllowFail(`let x = 1
fn f(c)
  if c then
    x = 1
    x = 2
  end
end
f(true)`);
    expect(out.error).toBe(true);
    const e = outerAssignErrors(out);
    expect(e).toHaveLength(2);
    // Earlier sites carry their own position; the last is the top-level one.
    expect(e[0]).toContain("[line 4, column 5]");
    expect(out.line).toBe(5);
  });

  it("fires on the `@` rebind operator, which desugars to `=`", () => {
    // `bump(@n)` is `n = bump(n)` — the `=` form, so it is caught by the same
    // check rather than sneaking past as sugar.
    const out = checkJsonAllowFail(`let n = 1
fn bump(v)
  v + 1
end
fn f()
  bump(@n)
end
print(f())`);
    expect(out.error).toBe(true);
    expect(outerAssignErrors(out)).toHaveLength(1);
  });

  it("is silent for same-function and top-level rebinds", () => {
    const out = checkJson(`let t = 0
if true then t = 1 end
fn ok(n)
  let m = 0
  if n > 0 then m = 1 end
  n = n + m
  n
end
print(t + ok(2))`);
    expect(out.ok).toBe(true);
    expect(outerAssignErrors(out)).toEqual([]);
  });

  it("is silent after a `let` in the function shadows the outer name", () => {
    const out = checkJson(`let x = 1
fn f(c)
  let x = 5
  if c then x = 6 end
  x
end
print(f(true))`);
    expect(out.ok).toBe(true);
    expect(outerAssignErrors(out)).toEqual([]);
  });

  it("is silent for a `var` written with `set` — the escape hatch", () => {
    const out = checkJson(`var i = 0
fn f()
  set i = get i + 1
end
f()
print(i)`);
    expect(out.ok).toBe(true);
    expect(outerAssignErrors(out)).toEqual([]);
  });

  it("does not emit code for the rejected assignment", () => {
    // The statement is abandoned once reported, so a program whose only fault
    // is the assignment fails at compile and never reaches lowering.
    const out = checkJsonAllowFail(`let i = 0
fn f()
  if false then i = 1 end
  i
end
print(f())`);
    expect(out.error).toBe(true);
    expect(out.message).not.toContain("bytecode lowering failed");
    expect(outerAssignErrors(out)).toHaveLength(1);
  });
});

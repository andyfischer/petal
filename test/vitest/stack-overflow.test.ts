// Runaway recursion fails with a stack overflow error instead of taking the
// process down. Before the guard, a recursive call with no base case grew the
// VM's heap-allocated frame stack until the OS killed the process (exit 137,
// no output, ~2 GB after ~45 s), and one that recursed through a `map`
// callback overflowed the native stack and aborted. See
// `MAX_CALL_DEPTH` / `SYNC_STACK_BUDGET` in rust/src/backend/bytecode/vm/mod.rs.

import { describe, it, expect } from "vitest";
import { petalCapture } from "./helpers";

function run(code: string) {
  return petalCapture(["run", "-e", code]);
}

describe("stack overflow", () => {
  it("stops unbounded direct recursion with an error", () => {
    const r = run("fn f(n) 1 + f(n + 1) end\nprint(f(0))");
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("Stack overflow: more than 5000 nested calls");
    expect(r.stderr).toContain("[line 1, column 13]");
  });

  it("collapses the repeated frames of the trace", () => {
    const r = run("fn f(n) 1 + f(n + 1) end\nprint(f(0))");
    expect(r.stderr).toMatch(/\.\.\. previous line repeated \d+ more times/);
    // One line per distinct frame, not one per call.
    expect(r.stderr.split("\n").length).toBeLessThan(20);
  });

  it("elides the middle of a long trace that does not repeat line for line", () => {
    const r = run(
      "fn even(n) if n == 0 then true else odd(n - 1) end end\n" +
        "fn odd(n) if n == 0 then false else even(n - 1) end end\n" +
        "print(even(-1))",
    );
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("Stack overflow");
    expect(r.stderr).toMatch(/\.\.\. \d+ more frames \.\.\./);
    expect(r.stderr.split("\n").length).toBeLessThan(60);
  });

  it("stops recursion through a builtin's callback before the native stack overflows", () => {
    const r = run("fn f(n) map([n], fn(x) -> f(x + 1)) end\nprint(f(0))");
    expect(r.code).toBe(1);
    expect(r.stderr).toMatch(/Stack overflow: callbacks nested \d+ deep/);
    expect(r.stderr).not.toContain("fatal runtime error");
  });

  it("keeps the output printed before the overflow", () => {
    const r = run('print("before")\nfn f(n) f(n + 1) end\nf(0)');
    expect(r.stdout).toBe("before\n");
    expect(r.stderr).toContain("Stack overflow");
  });

  it("allows recursion just under the limit", () => {
    const r = run("fn f(n) if n == 0 then 0 else 1 + f(n - 1) end end\nprint(f(4990))");
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("4990\n");
  });

  it("allows callbacks nested a normal depth", () => {
    // A tree walk through `map`, 12 levels of nested callbacks.
    const r = run(
      "fn walk(t) if len(t.kids) == 0 then 1 else " +
        "1 + reduce(map(t.kids, fn(k) -> walk(k)), 0, fn(a, b) -> a + b) end end\n" +
        "fn mk(d) if d == 0 then {kids: []} else {kids: [mk(d - 1), mk(d - 1)]} end end\n" +
        "print(walk(mk(12)))",
    );
    expect(r.code).toBe(0);
    expect(r.stdout).toBe("8191\n");
  });
});

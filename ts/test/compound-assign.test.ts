import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, showAstJson } from "./helpers";

beforeAll(() => ensureBuild());

describe("compound assignment operators", () => {
  it("handles += on variables", () => {
    const out = runPetal("let x = 10\nx += 5\nprint(x)");
    expect(out).toBe("15");
  });

  it("handles -= on variables", () => {
    const out = runPetal("let x = 10\nx -= 3\nprint(x)");
    expect(out).toBe("7");
  });

  it("handles *= on variables", () => {
    const out = runPetal("let x = 4\nx *= 3\nprint(x)");
    expect(out).toBe("12");
  });

  it("handles /= on variables", () => {
    const out = runPetal("let x = 20\nx /= 4\nprint(x)");
    expect(out).toBe("5");
  });

  it("handles %= on variables", () => {
    const out = runPetal("let x = 17\nx %= 5\nprint(x)");
    expect(out).toBe("2");
  });

  it("handles += with expressions", () => {
    const out = runPetal("let x = 1\nx += 2 + 3\nprint(x)");
    expect(out).toBe("6");
  });

  it("handles += on field access", () => {
    const out = runPetal('let r = { count: 0 }\nr.count += 5\nprint(r.count)');
    expect(out).toBe("5");
  });

  it("handles += on index access", () => {
    const out = runPetal("let a = [10, 20, 30]\na[1] += 5\nprint(a[1])");
    expect(out).toBe("25");
  });

  it("handles += in a loop", () => {
    const out = runPetal("let sum = 0\nfor i in range(1, 5) {\n  sum += i\n}\nprint(sum)");
    expect(out).toBe("10");
  });

  it("desugars += to x = x + expr in AST", () => {
    const ast = showAstJson("let x = 0\nx += 1");
    // Second statement should be a regular Assign
    const assignStmt = ast[1].kind;
    expect(assignStmt).toHaveProperty("Assign");
  });
});

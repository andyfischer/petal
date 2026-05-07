import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, runPetalError } from "./helpers";

beforeAll(() => ensureBuild());

describe("is_callable validation", () => {
  describe("rejects non-callable expressions at parse time", () => {
    it("rejects integer literal calls", () => {
      const err = runPetalError("42(1, 2)");
      expect(err).toContain("cannot be called");
    });

    it("rejects float literal calls", () => {
      const err = runPetalError("3.14(1)");
      expect(err).toContain("cannot be called");
    });

    it("rejects string literal calls", () => {
      const err = runPetalError('"hello"(1)');
      expect(err).toContain("cannot be called");
    });

    it("rejects boolean literal calls", () => {
      const err = runPetalError("true(1)");
      expect(err).toContain("cannot be called");
    });

    it("rejects list literal calls", () => {
      const err = runPetalError("[1, 2, 3](0)");
      expect(err).toContain("cannot be called");
    });

    it("rejects binary operation calls", () => {
      const err = runPetalError("(1 + 2)(3)");
      expect(err).toContain("cannot be called");
    });

    it("rejects unary operation calls", () => {
      const err = runPetalError("let x = 5\n(-x)(1)");
      expect(err).toContain("cannot be called");
    });
  });

  describe("allows callable expressions", () => {
    it("allows identifier calls", () => {
      const result = runPetal("fn f(x) { x + 1 }\nprint(f(5))");
      expect(result).toBe("6");
    });

    it("allows field access calls (method syntax)", () => {
      const result = runPetal("let xs = [1, 2, 3]\nprint(xs.len())");
      expect(result).toBe("3");
    });

    it("allows chained calls (call result called)", () => {
      const result = runPetal(
        "fn make(x) {\n  let inner = fn(y) { x + y }\n  inner\n}\nprint(make(10)(5))"
      );
      expect(result).toBe("15");
    });

    it("allows lambda IIFE", () => {
      const result = runPetal("print(fn(x) { x * 2 }(21))");
      expect(result).toBe("42");
    });

    it("allows index access calls", () => {
      const result = runPetal(
        "fn add1(x) { x + 1 }\nfn double(x) { x * 2 }\nlet fns = [add1, double]\nprint(fns[1](5))"
      );
      expect(result).toBe("10");
    });
  });
});

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, runPetalError } from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("forward-mode automatic differentiation", () => {
  describe("dual() builtin", () => {
    it("creates a dual number", () => {
      expect(runPetal(`print(dual(3, 1))`)).toBe("dual(3.0, 1.0)");
    });

    it("creates a dual with float inputs", () => {
      expect(runPetal(`print(dual(2.5, 0.5))`)).toBe("dual(2.5, 0.5)");
    });
  });

  describe("value_of() and deriv_of() builtins", () => {
    it("extracts value from dual", () => {
      expect(runPetal(`let x = dual(3, 1)\nprint(value_of(x))`)).toBe("3.0");
    });

    it("extracts derivative from dual", () => {
      expect(runPetal(`let x = dual(3, 1)\nprint(deriv_of(x))`)).toBe("1.0");
    });

    it("value_of on plain number returns float", () => {
      expect(runPetal(`print(value_of(5))`)).toBe("5.0");
    });

    it("deriv_of on plain number returns 0", () => {
      expect(runPetal(`print(deriv_of(5))`)).toBe("0.0");
    });
  });

  describe("arithmetic propagation", () => {
    it("addition propagates derivatives", () => {
      // d/dx (x + 2) = 1
      const code = `let x = dual(3, 1)\nlet y = x + 2\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("5.0 1.0");
    });

    it("subtraction propagates derivatives", () => {
      // d/dx (x - 2) = 1
      const code = `let x = dual(5, 1)\nlet y = x - 2\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 1.0");
    });

    it("multiplication uses product rule", () => {
      // d/dx (x * x) = 2x, at x=3: value=9, deriv=6
      const code = `let x = dual(3, 1)\nlet y = x * x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("division uses quotient rule", () => {
      // d/dx (1/x) at x=2: value=0.5, deriv=-0.25
      const code = `let x = dual(2, 1)\nlet y = 1 / x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("0.5 -0.25");
    });

    it("negation propagates derivative", () => {
      // d/dx (-x) = -1
      const code = `let x = dual(3, 1)\nlet y = -x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("-3.0 -1.0");
    });
  });

  describe("compound expressions", () => {
    it("computes derivative of x^2 via x*x", () => {
      // f(x) = x^2, f'(x) = 2x, at x=3: f=9, f'=6
      const code = `let x = dual(3, 1)\nlet f = x * x\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("computes derivative of x^3 via x*x*x", () => {
      // f(x) = x^3, f'(x) = 3x^2, at x=2: f=8, f'=12
      const code = `let x = dual(2, 1)\nlet f = x * x * x\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("8.0 12.0");
    });

    it("computes derivative of (x+2)*(x-3) via product rule", () => {
      // f(x) = (x+2)(x-3) = x^2 - x - 6, f'(x) = 2x - 1
      // at x=1: f = 3*(-2) = -6, f' = 2(1)-1 = 1
      const code = `let x = dual(1, 1)\nlet f = (x + 2) * (x - 3)\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("-6.0 1.0");
    });

    it("computes derivative of a polynomial 3x^2 + 2x + 1", () => {
      // f(x) = 3x^2 + 2x + 1, f'(x) = 6x + 2
      // at x=4: f = 48+8+1 = 57, f' = 24+2 = 26
      const code = `let x = dual(4, 1)\nlet f = 3 * x * x + 2 * x + 1\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("57.0 26.0");
    });
  });

  describe("dual with functions", () => {
    it("works through function calls", () => {
      const code = `fn square(x) { x * x }\nlet x = dual(3, 1)\nlet y = square(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("works with higher-order functions", () => {
      // Apply f(x) = x*x through a function that calls its argument
      const code = `fn apply(f, x) { f(x) }\nlet x = dual(3, 1)\nlet y = apply(fn(a) { a * a }, x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });
  });

  describe("type() builtin", () => {
    it("reports dual type", () => {
      expect(runPetal(`print(type(dual(1, 0)))`)).toBe("dual");
    });
  });

  describe("comparisons with dual", () => {
    it("dual compared to int", () => {
      expect(runPetal(`let x = dual(5, 1)\nprint(x > 3)`)).toBe("true");
    });

    it("dual compared to dual", () => {
      expect(runPetal(`let a = dual(3, 1)\nlet b = dual(5, 1)\nprint(a < b)`)).toBe("true");
    });
  });

  describe("AD through math builtins", () => {
    it("sqrt propagates derivatives", () => {
      // d/dx sqrt(x) = 1/(2*sqrt(x)), at x=4: value=2, deriv=0.25
      const code = `let x = dual(4, 1)\nlet y = sqrt(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("2.0 0.25");
    });

    it("abs propagates sign for positive", () => {
      const code = `let x = dual(3, 1)\nlet y = abs(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 1.0");
    });

    it("abs propagates sign for negative", () => {
      const code = `let x = dual(-3, 1)\nlet y = abs(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 -1.0");
    });

    it("floor has zero derivative", () => {
      const code = `let x = dual(3.7, 1)\nlet y = floor(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 0.0");
    });

    it("ceil has zero derivative", () => {
      const code = `let x = dual(3.2, 1)\nlet y = ceil(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("4.0 0.0");
    });

    it("round has zero derivative", () => {
      const code = `let x = dual(3.7, 1)\nlet y = round(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("4.0 0.0");
    });

    it("sqrt composes with chain rule", () => {
      // d/dx sqrt(x^2 + 1) at x=3: f=sqrt(10), f'= 2x / (2*sqrt(x^2+1)) = 3/sqrt(10)
      const code = `let x = dual(3, 1)\nlet y = sqrt(x * x + 1)\nprint(value_of(y))`;
      const output = runPetal(code);
      expect(output).toContain("3.16");
    });
  });

  describe("error handling", () => {
    it("dual() requires 2 arguments", () => {
      const err = runPetalError(`dual(1)`);
      expect(err).toContain("2 arguments");
    });

    it("dual() requires numeric arguments", () => {
      const err = runPetalError(`dual("a", 1)`);
      expect(err).toContain("number");
    });
  });
});

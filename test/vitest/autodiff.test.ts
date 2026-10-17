import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";


describe("forward-mode automatic differentiation", () => {
  describe("dual() builtin", () => {
    it("creates a dual number", () => {
      expect(runPetal(`import autodiff; print(dual(3, 1))`)).toBe("dual(3.0, 1.0)");
    });

    it("creates a dual with float inputs", () => {
      expect(runPetal(`import autodiff; print(dual(2.5, 0.5))`)).toBe("dual(2.5, 0.5)");
    });
  });

  describe("value_of() and deriv_of() builtins", () => {
    it("extracts value from dual", () => {
      expect(runPetal(`import autodiff; let x = dual(3, 1)\nprint(value_of(x))`)).toBe("3.0");
    });

    it("extracts derivative from dual", () => {
      expect(runPetal(`import autodiff; let x = dual(3, 1)\nprint(deriv_of(x))`)).toBe("1.0");
    });

    it("value_of on plain number returns float", () => {
      expect(runPetal(`import autodiff; print(value_of(5))`)).toBe("5.0");
    });

    it("deriv_of on plain number returns 0", () => {
      expect(runPetal(`import autodiff; print(deriv_of(5))`)).toBe("0.0");
    });
  });

  describe("arithmetic propagation", () => {
    it("addition propagates derivatives", () => {
      // d/dx (x + 2) = 1
      const code = `import autodiff; let x = dual(3, 1)\nlet y = x + 2\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("5.0 1.0");
    });

    it("subtraction propagates derivatives", () => {
      // d/dx (x - 2) = 1
      const code = `import autodiff; let x = dual(5, 1)\nlet y = x - 2\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 1.0");
    });

    it("multiplication uses product rule", () => {
      // d/dx (x * x) = 2x, at x=3: value=9, deriv=6
      const code = `import autodiff; let x = dual(3, 1)\nlet y = x * x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("division uses quotient rule", () => {
      // d/dx (1/x) at x=2: value=0.5, deriv=-0.25
      const code = `import autodiff; let x = dual(2, 1)\nlet y = 1 / x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("0.5 -0.25");
    });

    it("negation propagates derivative", () => {
      // d/dx (-x) = -1
      const code = `import autodiff; let x = dual(3, 1)\nlet y = -x\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("-3.0 -1.0");
    });
  });

  describe("compound expressions", () => {
    it("computes derivative of x^2 via x*x", () => {
      // f(x) = x^2, f'(x) = 2x, at x=3: f=9, f'=6
      const code = `import autodiff; let x = dual(3, 1)\nlet f = x * x\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("computes derivative of x^3 via x*x*x", () => {
      // f(x) = x^3, f'(x) = 3x^2, at x=2: f=8, f'=12
      const code = `import autodiff; let x = dual(2, 1)\nlet f = x * x * x\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("8.0 12.0");
    });

    it("computes derivative of (x+2)*(x-3) via product rule", () => {
      // f(x) = (x+2)(x-3) = x^2 - x - 6, f'(x) = 2x - 1
      // at x=1: f = 3*(-2) = -6, f' = 2(1)-1 = 1
      const code = `import autodiff; let x = dual(1, 1)\nlet f = (x + 2) * (x - 3)\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("-6.0 1.0");
    });

    it("computes derivative of a polynomial 3x^2 + 2x + 1", () => {
      // f(x) = 3x^2 + 2x + 1, f'(x) = 6x + 2
      // at x=4: f = 48+8+1 = 57, f' = 24+2 = 26
      const code = `import autodiff; let x = dual(4, 1)\nlet f = 3 * x * x + 2 * x + 1\nprint(value_of(f), deriv_of(f))`;
      expect(runPetal(code)).toBe("57.0 26.0");
    });
  });

  describe("dual with functions", () => {
    it("works through function calls", () => {
      const code = `import autodiff; fn square(x)\n  x * x\nend\nlet x = dual(3, 1)\nlet y = square(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });

    it("works with higher-order functions", () => {
      // Apply f(x) = x*x through a function that calls its argument
      const code = `import autodiff; fn apply(f, x)\n  f(x)\nend\nlet x = dual(3, 1)\nlet y = apply(fn(a) -> a * a, x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("9.0 6.0");
    });
  });

  describe("type() builtin", () => {
    it("reports dual type", () => {
      expect(runPetal(`import autodiff; print(type(dual(1, 0)))`)).toBe("dual");
    });
  });

  describe("comparisons with dual", () => {
    it("dual compared to int", () => {
      expect(runPetal(`import autodiff; let x = dual(5, 1)\nprint(x > 3)`)).toBe("true");
    });

    it("dual compared to dual", () => {
      expect(runPetal(`import autodiff; let a = dual(3, 1)\nlet b = dual(5, 1)\nprint(a < b)`)).toBe("true");
    });
  });

  describe("AD through math builtins", () => {
    it("sqrt propagates derivatives", () => {
      // d/dx sqrt(x) = 1/(2*sqrt(x)), at x=4: value=2, deriv=0.25
      const code = `import autodiff; let x = dual(4, 1)\nlet y = sqrt(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("2.0 0.25");
    });

    it("abs propagates sign for positive", () => {
      const code = `import autodiff; let x = dual(3, 1)\nlet y = abs(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 1.0");
    });

    it("abs propagates sign for negative", () => {
      const code = `import autodiff; let x = dual(-3, 1)\nlet y = abs(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 -1.0");
    });

    it("floor has zero derivative", () => {
      const code = `import autodiff; let x = dual(3.7, 1)\nlet y = floor(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("3.0 0.0");
    });

    it("ceil has zero derivative", () => {
      const code = `import autodiff; let x = dual(3.2, 1)\nlet y = ceil(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("4.0 0.0");
    });

    it("round has zero derivative", () => {
      const code = `import autodiff; let x = dual(3.7, 1)\nlet y = round(x)\nprint(value_of(y), deriv_of(y))`;
      expect(runPetal(code)).toBe("4.0 0.0");
    });

    it("sqrt composes with chain rule", () => {
      // d/dx sqrt(x^2 + 1) at x=3: f=sqrt(10), f'= 2x / (2*sqrt(x^2+1)) = 3/sqrt(10)
      const code = `import autodiff; let x = dual(3, 1)\nlet y = sqrt(x * x + 1)\nprint(value_of(y))`;
      const output = runPetal(code);
      expect(output).toContain("3.16");
    });
  });

  describe("multi-argument and creative builtins carry a derivative", () => {
    // Each case: an expression in `x` (or `t`), checked against a central
    // difference of the same expression on plain numbers.
    const cases: [string, number][] = [
      ["exp(x)", 0.7],
      ["log(x)", 2.0],
      ["pow(x, 3)", 1.5],
      ["pow(2, x)", 1.5],
      ["pow(x, x)", 1.5],
      ["atan2(x, 2.0)", 0.5],
      ["atan2(1.0, x)", 0.5],
      ["hypot(x, 3.0)", 4.0],
      ["clamp(x * 2, 0.0, 5.0)", 1.0],
      ["fract(x * 3)", 0.4],
      ["lerp(1.0, 3.0, x)", 0.25],
      ["lerp(x, x * x, 0.5)", 2.0],
      ["map_range(x, 0.0, 4.0, 10.0, 30.0)", 1.0],
      ["map_range(2.0, 0.0, x, 0.0, 1.0)", 4.0],
      ["smoothstep(0.0, 4.0, x)", 1.0],
      ["radians(x)", 30.0],
      ["degrees(x)", 0.5],
      ["mag(x, 2.0)", 1.0],
      ["distance(0, 0, x, 1.0)", 2.0],
      ["interp([[0, 0], [4, 8]], x)", 1.0],
    ];
    for (const [expr, at] of cases) {
      it(`${expr} at ${at}`, () => {
        const code = `import autodiff
import creative
fn f(x) ${expr} end
let h = 0.000001
let d = deriv_of(f(dual(${at}, 1.0)))
let fd = (f(${at} + h) - f(${at} - h)) / (2 * h)
print(type(f(dual(${at}, 1.0))), abs(d - fd) < 0.0001, d)`;
        expect(runPetal(code)).toMatch(/^dual true /);
      });
    }

    it("plain arguments still give a plain float", () => {
      expect(runPetal(`print(type(exp(1)), type(pow(2, 3)), type(atan2(1, 2)))`)).toBe(
        "float float float",
      );
    });

    it("clamp hands back the bound's derivative outside the range", () => {
      expect(runPetal(`import autodiff; print(clamp(dual(9.0, 1.0), 0.0, 5.0))`)).toBe(
        "dual(5.0, 0.0)",
      );
    });

    it("sign of a dual is a dual with derivative 0", () => {
      expect(runPetal(`import autodiff; print(sign(dual(-2.0, 1.0)), sign(dual(0.0, 1.0)))`)).toBe(
        "dual(-1.0, 0.0) dual(0.0, 0.0)",
      );
    });

    it("lerp of lists carries element derivatives", () => {
      expect(
        runPetal(`import autodiff; import creative; print(lerp([dual(1.0, 1.0), 2.0], [3.0, 4.0], 0.5))`),
      ).toBe("[dual(2.0, 0.5), 3.0]");
    });
  });

  describe("a builtin with no derivative rule refuses a dual", () => {
    it("vec2 and vec3 name the problem", () => {
      expect(runPetalError(`import autodiff; import vec2; vec2(dual(1.0, 1.0), 2.0)`)).toContain(
        "vec2() cannot hold a dual number (argument 1)",
      );
      expect(runPetalError(`import autodiff; import vec2; vec3(1.0, 2.0, dual(1.0, 1.0))`)).toContain(
        "vec3() cannot hold a dual number (argument 3)",
      );
    });

    it("any float-reading native without a rule raises instead of dropping it", () => {
      const err = runPetalError(`import autodiff; random(dual(1.0, 1.0))`);
      expect(err).toContain("got dual: this function has no derivative rule");
      expect(err).toContain("value_of(x)");
    });

    it("a lerp of vectors refuses a dual t", () => {
      expect(
        runPetalError(`import autodiff; import creative; import vec2; lerp(vec2(0, 0), vec2(1, 1), dual(0.5, 1.0))`),
      ).toContain("got dual");
    });
  });

  describe("error handling", () => {
    it("dual() requires 2 arguments", () => {
      const err = runPetalError(`import autodiff; dual(1)`);
      expect(err).toContain("2 arguments");
    });

    it("dual() requires numeric arguments", () => {
      const err = runPetalError(`import autodiff; dual("a", 1)`);
      expect(err).toContain("number");
    });
  });
});

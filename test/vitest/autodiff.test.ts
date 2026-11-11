import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";

// The `autodiff` package: derivatives and solving by finite differences
// (docs/features/differentiation.md). The numbers are approximations, so the
// checks compare within a tolerance. test/ptl/autodiff.ptl covers the column
// qualities, bounds and speculation in more depth.

/** Run `body` after `import autodiff`, with a `near(a, b)` helper in scope. */
function ad(body: string): string {
  return runPetal(`import autodiff
fn near(a, b)
  abs(a - b) < 0.000001
end
${body}`);
}

describe("autodiff: derivatives by finite differences", () => {
  describe("deriv(f, x)", () => {
    it("returns the value, quality and step", () => {
      expect(ad(`let d = deriv(fn(x) x * x end, 3.0)
print(near(d.value, 6.0), d.quality, d.step > 0.0)`)).toBe("true smooth true");
    });

    it("differentiates polynomials", () => {
      // f(x) = 3x^2 + 2x + 1, f'(x) = 6x + 2, at x = 4: 26
      expect(ad(`print(near(deriv(fn(x) 3 * x * x + 2 * x + 1 end, 4.0).value, 26.0))`)).toBe("true");
      // (x + 2)(x - 3), f'(x) = 2x - 1, at x = 1: 1
      expect(ad(`print(near(deriv(fn(x) (x + 2) * (x - 3) end, 1.0).value, 1.0))`)).toBe("true");
    });

    it("differentiates through named functions and closures", () => {
      expect(ad(`fn square(x)
  x * x
end
fn apply(f, x)
  f(x)
end
print(near(deriv(square, 3.0).value, 6.0), near(deriv(fn(x) apply(square, x) end, 3.0).value, 6.0))`)).toBe(
        "true true",
      );
    });

    it("takes an int input as a float", () => {
      expect(ad(`print(near(deriv(fn(x) x * x end, 3).value, 6.0))`)).toBe("true");
    });

    it("reports a kink, and a flat function as zero", () => {
      expect(ad(`print(deriv(abs, 0.0).quality, deriv(fn(x) 5.0 end, 1.0).quality)`)).toBe("kink zero");
    });
  });

  describe("the math builtins against their closed-form derivatives", () => {
    // Each case: an expression in `x`, the point, and the exact derivative.
    const cases: [string, number, number][] = [
      ["sqrt(x)", 4.0, 0.25],
      ["sin(x)", 0.0, 1.0],
      ["cos(x)", 0.0, 0.0],
      ["atan(x)", 1.0, 0.5],
      ["exp(x)", 0.7, Math.exp(0.7)],
      ["log(x)", 2.0, 0.5],
      ["pow(x, 3)", 1.5, 3 * 1.5 * 1.5],
      ["pow(2, x)", 1.5, Math.pow(2, 1.5) * Math.log(2)],
      ["atan2(x, 2.0)", 0.5, 2.0 / (0.25 + 4.0)],
      ["hypot(x, 3.0)", 4.0, 0.8],
      ["lerp(1.0, 3.0, x)", 0.25, 2.0],
      ["map_range(x, 0.0, 4.0, 10.0, 30.0)", 1.0, 5.0],
      ["smoothstep(0.0, 4.0, x)", 1.0, (6 * 0.25 * 0.75) / 4],
      ["radians(x)", 30.0, Math.PI / 180],
      ["mag(x, 2.0)", 1.0, 1 / Math.sqrt(5)],
      ["distance(0, 0, x, 1.0)", 2.0, 2 / Math.sqrt(5)],
      ["interp([[0, 0], [4, 8]], x)", 1.0, 2.0],
    ];
    for (const [expr, at, exact] of cases) {
      it(`${expr} at ${at}`, () => {
        const out = runPetal(`import autodiff
fn f(x) ${expr} end
let d = deriv(f, ${at})
print(d.quality, d.value)`);
        const [quality, value] = out.split(" ");
        expect(quality).toBe("smooth");
        expect(Math.abs(Number(value) - exact)).toBeLessThan(1e-6);
      });
    }

    it("the math builtins answer plain floats", () => {
      expect(runPetal(`print(type(exp(1)), type(pow(2, 3)), type(atan2(1, 2)), type(clamp(1.5, 0, 1)))`)).toBe(
        "float float float float",
      );
    });
  });

  describe("jacobian(f, xs)", () => {
    it("has one row per output and one column per input", () => {
      expect(ad(`let j = jacobian(fn(xs) [xs[0] * xs[1], xs[0] + xs[1]] end, [2.0, 3.0])
print(near(j.rows[0][0], 3.0), near(j.rows[0][1], 2.0), near(j.rows[1][0], 1.0), near(j.rows[1][1], 1.0))
print(j.quality, len(j.step))`)).toBe('true true true true\n["smooth", "smooth"] 2');
    });
  });

  describe("solve_step and solve", () => {
    it("one step of a linear goal lands on it", () => {
      expect(ad(`let r = solve_step(fn(xs) xs[0] * 2.0 end, [1.0], 10.0)
print(near(r.xs[0], 5.0), r.improved, r.stuck, r.quality)`)).toBe('true true [] ["smooth"]');
    });

    it("solve reaches a two-input goal", () => {
      expect(ad(`let s = solve(fn(xs) [xs[0] + xs[1], xs[0] - xs[1]] end, [0.0, 0.0], [4.0, 2.0])
print(near(s.xs[0], 3.0), near(s.xs[1], 1.0), s.residual < 0.000001)`)).toBe("true true true");
    });

    it("an input that does not move the output is reported stuck", () => {
      expect(ad(`print(solve_step(fn(xs) xs[0] end, [0.0, 0.0], 3.0).stuck)`)).toBe("[1]");
    });

    it("keeps inputs inside their bounds", () => {
      expect(ad(`let r = solve(fn(xs) xs[0] end, [0.0], 10.0, {bounds: [[-1.0, 2.0]]})
print(r.xs[0] <= 2.0)`)).toBe("true");
    });
  });

  describe("the dual-number functions are gone", () => {
    for (const name of ["dual", "value_of", "deriv_of"]) {
      it(`${name} is a compile error naming the replacement`, () => {
        const call = name === "dual" ? "dual(1.0, 1.0)" : `${name}(1.0)`;
        const err = runPetalError(`import autodiff\nprint(${call})`);
        expect(err).toContain(`\`${name}\` was removed from \`autodiff\` (dual numbers are gone).`);
        expect(err).toContain("Use `deriv(f, x)` for a derivative, or `solve_step(f, xs, target)` to move");
        expect(err).toContain("See docs/features/differentiation.md.");
      });
    }

    it("a file's own function of the same name still works", () => {
      expect(runPetal(`import autodiff\nfn dual(a, b)\n  a + b\nend\nprint(dual(1, 2))`)).toBe("3");
    });
  });
});

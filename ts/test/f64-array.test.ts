import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, runPetalError } from "./helpers";

beforeAll(ensureBuild);

describe("f64_array — construction and length", () => {
  it("f64_array(n) creates a zero-filled array of length n", () => {
    expect(runPetal(`print(f64_array(3))`)).toBe("[0.0, 0.0, 0.0]");
  });

  it("f64_array(0) creates an empty array", () => {
    expect(runPetal(`print(f64_array(0))`)).toBe("[]");
  });

  it("len(a) returns the number of elements", () => {
    expect(runPetal(`print(len(f64_array(5)))`)).toBe("5");
  });

  it("type(a) reports f64_array", () => {
    expect(runPetal(`print(type(f64_array(2)))`)).toBe("f64_array");
  });

  it("two arrays of equal contents compare equal", () => {
    expect(runPetal(`print(f64_array(3) == f64_array(3))`)).toBe("true");
  });
});

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

describe("f64_array — element access", () => {
  it("set then get round-trips a value", () => {
    expect(
      runPetal(`let a = f64_array(3)
a = set(a, 1, 5.5)
print(get(a, 1))`),
    ).toBe("5.5");
  });

  it("set returns a new array (value semantics); the input is unchanged", () => {
    // `set` no longer mutates in place: the original handle still sees zeros…
    expect(
      runPetal(`let a = f64_array(2)
let b = set(a, 0, 9.0)
print(a)`),
    ).toBe("[0.0, 0.0]");
  });

  it("set result holds the updated value", () => {
    // …while the returned array carries the update.
    expect(
      runPetal(`let a = f64_array(2)
let b = set(a, 0, 9.0)
print(b)`),
    ).toBe("[9.0, 0.0]");
  });

  it("set accepts an int and stores it as a float", () => {
    expect(
      runPetal(`let a = f64_array(1)
a = set(a, 0, 7)
print(get(a, 0))`),
    ).toBe("7.0");
  });

  it("swap returns a new array with two elements exchanged", () => {
    expect(
      runPetal(`let a = f64_array(3)
a = set(a, 0, 1.0)
a = set(a, 2, 3.0)
a = swap(a, 0, 2)
print(a)`),
    ).toBe("[3.0, 0.0, 1.0]");
  });

  it("swap does not mutate the input array", () => {
    expect(
      runPetal(`let a = f64_array(3)
a = set(a, 0, 1.0)
let b = swap(a, 0, 2)
print(a)`),
    ).toBe("[1.0, 0.0, 0.0]");
  });

  it("index read a[i] returns the element", () => {
    expect(
      runPetal(`let a = f64_array(2)
a = set(a, 1, 4.0)
print(a[1])`),
    ).toBe("4.0");
  });

  it("index write a[i] = v mutates the array", () => {
    expect(
      runPetal(`let a = f64_array(2)
a[0] = 2.5
print(a)`),
    ).toBe("[2.5, 0.0]");
  });

  it("get out of bounds is an error", () => {
    expect(runPetalError(`get(f64_array(2), 5)`)).toMatch(/bounds|range/i);
  });

  it("set out of bounds is an error", () => {
    expect(runPetalError(`set(f64_array(2), 5, 1.0)`)).toMatch(/bounds|range/i);
  });
});

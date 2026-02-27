import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("enumerate builtin", () => {
  it("returns index-value pairs", () => {
    expect(runPetal(`print(enumerate(["a", "b", "c"]))`)).toBe(`[[0, "a"], [1, "b"], [2, "c"]]`);
  });

  it("handles empty list", () => {
    expect(runPetal(`print(enumerate([]))`)).toBe("[]");
  });

  it("preserves value types", () => {
    expect(runPetal(`print(enumerate([10, 20]))`)).toBe("[[0, 10], [1, 20]]");
  });
});

describe("zip builtin", () => {
  it("pairs elements from two lists", () => {
    expect(runPetal(`print(zip([1, 2, 3], ["a", "b", "c"]))`)).toBe(`[[1, "a"], [2, "b"], [3, "c"]]`);
  });

  it("truncates to shorter list", () => {
    expect(runPetal(`print(zip([1, 2], ["a", "b", "c"]))`)).toBe(`[[1, "a"], [2, "b"]]`);
  });

  it("handles empty lists", () => {
    expect(runPetal(`print(zip([], [1, 2, 3]))`)).toBe("[]");
  });
});

describe("slice builtin", () => {
  it("slices a list with start and end", () => {
    expect(runPetal(`print(slice([1, 2, 3, 4, 5], 1, 3))`)).toBe("[2, 3]");
  });

  it("slices from start to end of list", () => {
    expect(runPetal(`print(slice([1, 2, 3, 4, 5], 2))`)).toBe("[3, 4, 5]");
  });

  it("supports negative indices", () => {
    expect(runPetal(`print(slice([1, 2, 3, 4, 5], -2))`)).toBe("[4, 5]");
  });

  it("slices a string", () => {
    expect(runPetal(`print(slice("hello", 1, 3))`)).toBe("el");
  });

  it("handles empty slice", () => {
    expect(runPetal(`print(slice([1, 2, 3], 3))`)).toBe("[]");
  });
});

describe("flat builtin", () => {
  it("flattens nested lists one level", () => {
    expect(runPetal(`print(flat([[1, 2], [3, 4], [5]]))`)).toBe("[1, 2, 3, 4, 5]");
  });

  it("leaves non-list elements as-is", () => {
    expect(runPetal(`print(flat([1, [2, 3], 4]))`)).toBe("[1, 2, 3, 4]");
  });

  it("handles empty list", () => {
    expect(runPetal(`print(flat([]))`)).toBe("[]");
  });

  it("handles list with empty sublists", () => {
    expect(runPetal(`print(flat([[], [1], []]))`)).toBe("[1]");
  });
});

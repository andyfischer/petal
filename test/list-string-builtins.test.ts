import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, runPetalError } from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("sort builtin", () => {
  it("sorts a list of integers", () => {
    expect(runPetal(`print(sort([3, 1, 2]))`)).toBe("[1, 2, 3]");
  });

  it("sorts a list of floats", () => {
    expect(runPetal(`print(sort([3.1, 1.2, 2.5]))`)).toBe("[1.2, 2.5, 3.1]");
  });

  it("sorts a list of strings", () => {
    expect(runPetal(`print(sort(["banana", "apple", "cherry"]))`)).toBe(`["apple", "banana", "cherry"]`);
  });

  it("sorts empty list", () => {
    expect(runPetal(`print(sort([]))`)).toBe("[]");
  });

  it("sorts single element", () => {
    expect(runPetal(`print(sort([42]))`)).toBe("[42]");
  });

  it("returns a new list (does not mutate)", () => {
    expect(runPetal(`let xs = [3, 1, 2]\nlet ys = sort(xs)\nprint(xs, ys)`)).toBe("[3, 1, 2] [1, 2, 3]");
  });
});

describe("reverse builtin", () => {
  it("reverses a list", () => {
    expect(runPetal(`print(reverse([1, 2, 3]))`)).toBe("[3, 2, 1]");
  });

  it("reverses empty list", () => {
    expect(runPetal(`print(reverse([]))`)).toBe("[]");
  });

  it("reverses a string", () => {
    expect(runPetal(`print(reverse("hello"))`)).toBe("olleh");
  });

  it("returns a new list (does not mutate)", () => {
    expect(runPetal(`let xs = [1, 2, 3]\nlet ys = reverse(xs)\nprint(xs, ys)`)).toBe("[1, 2, 3] [3, 2, 1]");
  });
});

describe("join builtin", () => {
  it("joins a list of strings", () => {
    expect(runPetal(`print(join(["a", "b", "c"], ", "))`)).toBe("a, b, c");
  });

  it("joins with empty separator", () => {
    expect(runPetal(`print(join(["a", "b", "c"], ""))`)).toBe("abc");
  });

  it("joins empty list", () => {
    expect(runPetal(`print(join([], ", "))`)).toBe("");
  });

  it("converts non-string elements", () => {
    expect(runPetal(`print(join([1, 2, 3], "-"))`)).toBe("1-2-3");
  });
});

describe("split builtin", () => {
  it("splits a string by separator", () => {
    expect(runPetal(`print(split("a,b,c", ","))`)).toBe(`["a", "b", "c"]`);
  });

  it("splits with no matches returns single element", () => {
    expect(runPetal(`print(split("hello", ","))`)).toBe(`["hello"]`);
  });

  it("splits empty string", () => {
    expect(runPetal(`print(split("", ","))`)).toBe(`[""]`);
  });
});

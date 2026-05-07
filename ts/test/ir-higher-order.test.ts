import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

describe("map", () => {
  it("applies a function to each element", () => {
    const out = runPetal("let xs = [1, 2, 3]\nprint(map(xs, fn(x) { x * 2 }))");
    expect(out).toBe("[2, 4, 6]");
  });

  it("works with named functions", () => {
    const out = runPetal("fn double(x) { x * 2 }\nlet xs = [1, 2, 3]\nprint(map(xs, double))");
    expect(out).toBe("[2, 4, 6]");
  });

  it("works with empty list", () => {
    const out = runPetal("print(map([], fn(x) { x }))");
    expect(out).toBe("[]");
  });

  it("preserves order", () => {
    const out = runPetal("print(map([3, 1, 2], fn(x) { x + 10 }))");
    expect(out).toBe("[13, 11, 12]");
  });
});

describe("filter", () => {
  it("keeps elements where predicate is true", () => {
    const out = runPetal("let xs = [1, 2, 3, 4, 5]\nprint(filter(xs, fn(x) { x > 3 }))");
    expect(out).toBe("[4, 5]");
  });

  it("returns empty list when nothing matches", () => {
    const out = runPetal("print(filter([1, 2, 3], fn(x) { x > 10 }))");
    expect(out).toBe("[]");
  });

  it("works with empty list", () => {
    const out = runPetal("print(filter([], fn(x) { x > 0 }))");
    expect(out).toBe("[]");
  });
});

describe("reduce", () => {
  it("reduces a list with an accumulator", () => {
    const out = runPetal("let xs = [1, 2, 3, 4]\nprint(reduce(xs, 0, fn(acc, x) { acc + x }))");
    expect(out).toBe("10");
  });

  it("works with string accumulation", () => {
    const out = runPetal('let words = ["hello", " ", "world"]\nprint(reduce(words, "", fn(acc, w) { acc ++ w }))');
    expect(out).toBe("hello world");
  });

  it("returns initial value for empty list", () => {
    const out = runPetal("print(reduce([], 42, fn(acc, x) { acc + x }))");
    expect(out).toBe("42");
  });
});

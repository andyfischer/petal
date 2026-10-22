import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";


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

describe("immutable list ops — last / drop_last / pop", () => {
  it("last returns the final element", () => {
    expect(runPetal(`print(last([1, 2, 3]))`)).toBe("3");
  });

  it("last of an empty list is nil", () => {
    expect(runPetal(`print(last([]))`)).toBe("nil");
  });

  it("drop_last returns a new list without the final element", () => {
    expect(runPetal(`print(drop_last([1, 2, 3]))`)).toBe("[1, 2]");
  });

  it("drop_last does not mutate the input (value semantics)", () => {
    expect(
      runPetal(`let xs = [1, 2, 3]
let ys = drop_last(xs)
print(xs)`),
    ).toBe("[1, 2, 3]");
  });

  it("drop_last of an empty list is an empty list", () => {
    expect(runPetal(`print(drop_last([]))`)).toBe("[]");
  });

  it("pop is a deprecated immutable alias of drop_last", () => {
    expect(runPetal(`print(pop([1, 2, 3]))`)).toBe("[1, 2]");
  });

  it("pop does not mutate the input list", () => {
    expect(
      runPetal(`let xs = [1, 2, 3]
let ys = pop(xs)
print(xs)`),
    ).toBe("[1, 2, 3]");
  });
});

describe("immutable map op — remove", () => {
  it("remove returns a new map without the key", () => {
    expect(runPetal(`print(remove({a: 1, b: 2}, "a"))`)).toBe("{ b: 2 }");
  });

  it("remove does not mutate the input map (value semantics)", () => {
    expect(
      runPetal(`let m = {a: 1, b: 2}
let n = remove(m, "a")
print(m)`),
    ).toBe("{ a: 1, b: 2 }");
  });

  it("removing an absent key yields an equivalent map", () => {
    expect(runPetal(`print(remove({a: 1}, "missing"))`)).toBe("{ a: 1 }");
  });
});

describe("Map: a map keyed by any hashable value", () => {
  it("prints as the call that rebuilds it", () => {
    expect(runPetal(`let m = Map()
m[1] = "one"
m[[2, 3]] = true
print(m, type(m), len(m))`)).toBe(`Map([[1, "one"], [[2, 3], true]]) map 2`);
  });

  it("finds a key by content, and reads an absent key as nil", () => {
    expect(runPetal(`let m = Map([[[1, 2], "pair"], [2, "two"]])
print(m[[1, 2]], m[2.0], m["nope"], m["nope"] ?? "fallback")`)).toBe("pair two nil fallback");
  });

  it("takes the record builtins and iterates keys in insertion order", () => {
    expect(runPetal(`let m = Map([["b", 1], ["a", 2]])
print(keys(m), values(m), entries(m), contains(m, "a"))
for k in m do print(k) end`)).toBe(`["b", "a"] [1, 2] [["b", 1], ["a", 2]] true\nb\na`);
  });

  it("rejects a key that cannot be hashed", () => {
    expect(runPetalError(`let m = Map()
m[fn(x) -> x] = 1`)).toContain("Cannot use a function as a map key");
  });
});

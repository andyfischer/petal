import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(() => ensureBuild());

// -----------------------------------------------------------------------
// Every position in which a `for` loop collects into a list.
//
// docs/language-guide.md enumerates these; the list was incomplete (it
// omitted record fields, string interpolation, and the `if`/`match` arm
// tails that commit 92fef1d made collect). These tests are what keeps the
// documented list honest.
// -----------------------------------------------------------------------

describe("`for` in value position collects", () => {
  it("assigned to a name", () => {
    expect(runPetal("let xs = for i in range(0, 3) do i * 2 end\nprint(xs)").trim()).toBe(
      "[0, 2, 4]",
    );
  });

  it("returned explicitly", () => {
    const src = `fn f(n)
    return for i in range(0, n) do i end
end
print(f(3))`;
    expect(runPetal(src).trim()).toBe("[0, 1, 2]");
  });

  it("passed as an argument", () => {
    expect(runPetal("print(len(for i in range(0, 3) do i end))").trim()).toBe("3");
  });

  it("as a list element", () => {
    expect(runPetal("print([for i in range(0, 2) do i end, 9])").trim()).toBe("[[0, 1], 9]");
  });

  it("as a record field value", () => {
    expect(runPetal("print({a: 1, b: for i in range(0, 2) do i end})").trim()).toBe(
      "{ a: 1, b: [0, 1] }",
    );
  });

  it("interpolated into a string", () => {
    expect(runPetal('print("{for i in range(0, 2) do i end}")').trim()).toBe("[0, 1]");
  });
});

describe("`for` in tail position collects", () => {
  it("as a function's implicit return", () => {
    const src = `fn doubled(xs)
    for x in xs do x * 2 end
end
print(doubled([1, 2, 3]))`;
    expect(runPetal(src).trim()).toBe("[2, 4, 6]");
  });

  it("as an `if` branch tail", () => {
    const src = `fn rows(n)
    if n > 0 then
        for i in range(0, n) do i end
    else
        []
    end
end
print(rows(3))
print(rows(0))`;
    expect(runPetal(src).trim().split("\n")).toEqual(["[0, 1, 2]", "[]"]);
  });

  it("as a `match` arm tail", () => {
    const src = `fn tagged(n)
    match n
        when 0 -> []
        when m -> for i in range(0, m) do i + 100 end
    end
end
print(tagged(2))`;
    expect(runPetal(src).trim()).toBe("[100, 101]");
  });

  it("as the body of an enclosing collecting loop", () => {
    const src = `let grid = for row in range(0, 3) do
    for col in range(0, 3) do
        row * 10 + col
    end
end
print(grid)`;
    expect(runPetal(src).trim()).toBe("[[0, 1, 2], [10, 11, 12], [20, 21, 22]]");
  });
});

describe("a bare `for` statement does not collect", () => {
  it("runs for side effects only", () => {
    const src = `for i in range(0, 3) do
    print(i)
end`;
    expect(runPetal(src).trim().split("\n")).toEqual(["0", "1", "2"]);
  });

  it("a trailing nil opts a tail-position loop out of collecting", () => {
    const src = `fn draw_all(items)
    for it in items do it end
    nil
end
print(draw_all([1, 2]))`;
    expect(runPetal(src).trim()).toBe("nil");
  });
});

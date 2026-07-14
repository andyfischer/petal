import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  runPetal,
  runPetalError,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

// -----------------------------------------------------------------------
// if / match as value-producing expressions
//
// These were already expressions in Petal; these tests lock in that they
// yield the last statement of the taken branch/arm.
// -----------------------------------------------------------------------

describe("if as an expression", () => {
  it("yields the taken branch's value", () => {
    expect(runPetal("x = if true then 1 else 2 end\nprint(x)").trim()).toBe("1");
    expect(runPetal("x = if false then 1 else 2 end\nprint(x)").trim()).toBe("2");
  });

  it("yields the last statement of a multi-statement branch", () => {
    const src = `x = if true then
  a = 5
  a + 1
end
print(x)`;
    expect(runPetal(src).trim()).toBe("6");
  });

  it("an if with no else yields nil on the untaken path", () => {
    expect(runPetal("x = if false then 1 end\nprint(x)").trim()).toBe("nil");
  });

  it("elsif chains yield the matching arm", () => {
    const src = `fn classify(n)
  if n < 0 then
    "neg"
  elsif n == 0 then
    "zero"
  else
    "pos"
  end
end
print(classify(-3))
print(classify(0))
print(classify(7))`;
    expect(runPetal(src).trim().split("\n")).toEqual(["neg", "zero", "pos"]);
  });
});

describe("match as an expression", () => {
  it("yields the matched arm's value", () => {
    const src = `x = match 2
  when 1 -> "one"
  when 2 -> "two"
  when _ -> "other"
end
print(x)`;
    expect(runPetal(src).trim()).toBe("two");
  });

  it("yields the last statement of a do-block arm", () => {
    const src = `x = match 2
  when 2 do
    a = "t"
    a ++ "wo"
  end
  when _ -> "other"
end
print(x)`;
    expect(runPetal(src).trim()).toBe("two");
  });
});

// -----------------------------------------------------------------------
// for as a mapping expression
// -----------------------------------------------------------------------

describe("for as a mapping expression", () => {
  it("collects each iteration's last expression into a list (range)", () => {
    expect(runPetal("print(for i in range(3) do i * 10 end)").trim()).toBe(
      "[0, 10, 20]"
    );
  });

  it("collects when iterating a list", () => {
    expect(runPetal("print(for n in [1, 2, 3] do n * n end)").trim()).toBe(
      "[1, 4, 9]"
    );
  });

  it("collects when used as a call argument", () => {
    expect(runPetal("print(for i in range(4) do i end)").trim()).toBe(
      "[0, 1, 2, 3]"
    );
  });

  it("collects when returned from a function", () => {
    const src = `fn doubled(xs)
  return for x in xs do x * 2 end
end
print(doubled([1, 2, 3, 4]))`;
    expect(runPetal(src).trim()).toBe("[2, 4, 6, 8]");
  });

  it("collects when used as a list element", () => {
    expect(
      runPetal("print([0, for i in range(3) do i + 1 end, 99])").trim()
    ).toBe("[0, [1, 2, 3], 99]");
  });

  it("yields an empty list for an empty iterable", () => {
    expect(runPetal("print(for i in [] do i end)").trim()).toBe("[]");
  });

  it("the body's last expression (not the first) is collected", () => {
    const src = `print(for i in range(3) do
  doubled = i * 2
  doubled + 1
end)`;
    expect(runPetal(src).trim()).toBe("[1, 3, 5]");
  });

  it("an if-expression body maps each element", () => {
    const src = `print(for i in range(4) do
  if i % 2 == 0 then "even" else "odd" end
end)`;
    expect(runPetal(src).trim()).toBe('["even", "odd", "even", "odd"]');
  });
});

describe("for-expression control flow", () => {
  it("continue skips the element (filter behavior)", () => {
    const src = `print(for i in range(6) do
  if i % 2 == 0 then continue end
  i
end)`;
    expect(runPetal(src).trim()).toBe("[1, 3, 5]");
  });

  it("break ends collection with the elements gathered so far", () => {
    const src = `print(for i in range(10) do
  if i == 3 then break end
  i * 10
end)`;
    expect(runPetal(src).trim()).toBe("[0, 10, 20]");
  });

  it("loop-carried state still works alongside collection", () => {
    const src = `total = 0
squares = for i in range(4) do
  total = total + i
  i * i
end
print(squares)
print(total)`;
    expect(runPetal(src).trim().split("\n")).toEqual(["[0, 1, 4, 9]", "6"]);
  });
});

describe("for as a statement (no collection)", () => {
  it("a bare for statement runs for side effects and allocates no list", () => {
    const src = `for i in range(3) do
  print(i)
end`;
    expect(runPetal(src).trim().split("\n")).toEqual(["0", "1", "2"]);
  });

  it("only value-position loops set the collect flag in the IR", () => {
    // Bare statement form: no collect flag.
    const stmtIr = showIrJson("for i in range(3) do\n  i\nend");
    for (const t of termsByOp(stmtIr, "NumericForLoop")) {
      expect(t.collect ?? false).toBe(false);
    }
    // Value position: collect flag set.
    const exprIr = showIrJson("x = for i in range(3) do\n  i\nend");
    const exprLoops = termsByOp(exprIr, "NumericForLoop");
    expect(exprLoops.length).toBeGreaterThanOrEqual(1);
    expect(exprLoops.some((t: any) => t.collect === true)).toBe(true);
  });
});

describe("nested for (strict collection: inner loop must be bound)", () => {
  it("an unbound inner loop yields nil per outer iteration", () => {
    // Documented behavior: a loop only collects when its value is bound. The
    // inner loop here is a bare statement, so each outer iteration collects nil.
    const src = `print(for r in range(2) do
  for c in range(2) do r * 10 + c end
end)`;
    expect(runPetal(src).trim()).toBe("[nil, nil]");
  });

  it("binding the inner loop produces a nested list (the idiom)", () => {
    const src = `print(for r in range(2) do
  row = for c in range(2) do r * 10 + c end
  row
end)`;
    expect(runPetal(src).trim()).toBe("[[0, 1], [10, 11]]");
  });
});

describe("while remains statement-only", () => {
  it("a while statement runs for side effects", () => {
    const src = `n = 0
while n < 3 do
  print(n)
  n = n + 1
end`;
    expect(runPetal(src).trim().split("\n")).toEqual(["0", "1", "2"]);
  });

  it("while in value position is a parse error", () => {
    expect(runPetalError("x = while true do 1 end\nprint(x)")).toContain(
      "While"
    );
  });
});

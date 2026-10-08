import { describe, it, expect } from "vitest";
import { runPetal, runPetalError } from "./helpers";

// -----------------------------------------------------------------------
// Named call arguments — `f(a: 1, b: 2)`.
//
// Selection is still by total argument count; once a callee is picked, each
// named argument claims the slot its name picks out of that function's
// parameter list. A call that writes no name is unaffected.
// -----------------------------------------------------------------------

const SUB = `fn sub(a, b)
  a - b
end
`;

describe("named arguments", () => {
  it("bind by name, in any order", () => {
    expect(runPetal(`${SUB}print(sub(b: 2, a: 10))`).trim()).toBe("8");
    expect(runPetal(`${SUB}print(sub(a: 10, b: 2))`).trim()).toBe("8");
  });

  it("mix with a positional prefix", () => {
    expect(runPetal(`${SUB}print(sub(10, b: 2))`).trim()).toBe("8");
  });

  it("leave an all-positional call alone", () => {
    expect(runPetal(`${SUB}print(sub(10, 2))`).trim()).toBe("8");
  });

  it("select an overload by total count, then bind by name", () => {
    const src = `fn g(a)
  a
end
fn g(a, b)
  a - b
end
`;
    expect(runPetal(`${src}print(g(a: 5))`).trim()).toBe("5");
    expect(runPetal(`${src}print(g(b: 2, a: 5))`).trim()).toBe("3");
  });

  it("bind after a method's receiver", () => {
    const src = `class Point
  x,
  y,
end
fn Point.shift(p, dx)
  p.x - dx
end
let p = Point(10, 0)
`;
    expect(runPetal(`${src}print(p.shift(dx: 2))`).trim()).toBe("8");
    expect(runPetal(`${src}print(Point(y: 1, x: 7).x)`).trim()).toBe("7");
  });

  it("work on a lambda", () => {
    const src = `let k = 3
let f = fn(a, b)
  (a - b) * k
end
print(f(b: 1, a: 5))`;
    expect(runPetal(src).trim()).toBe("12");
  });

  it("survive recursion", () => {
    const src = `fn fact(n, acc)
  if n <= 1 then acc else fact(acc: acc * n, n: n - 1) end
end
print(fact(n: 5, acc: 1))`;
    expect(runPetal(src).trim()).toBe("120");
  });
});

describe("named-argument errors", () => {
  it("reject an unknown parameter name", () => {
    expect(runPetalError(`${SUB}print(sub(c: 1, a: 2))`)).toContain(
      "sub() has no parameter named 'c'",
    );
  });

  it("reject a slot filled twice", () => {
    expect(runPetalError(`${SUB}print(sub(1, a: 2))`)).toContain(
      "sub() got multiple values for parameter 'a'",
    );
    expect(runPetalError(`${SUB}print(sub(b: 1, b: 2))`)).toContain(
      "sub() got multiple values for parameter 'b'",
    );
  });

  it("refuse to rebind a method's receiver", () => {
    const src = `class Point
  x,
  y,
end
fn Point.shift(p, dx)
  p.x - dx
end
let p = Point(10, 0)
print(p.shift(p: 1))`;
    expect(runPetalError(src)).toContain(
      "Point.shift() got multiple values for parameter 'p'",
    );
  });

  it("refuse a named argument to a builtin that declares no parameters", () => {
    expect(runPetalError("print(1, sep: 2)")).toContain(
      "builtin 'print' does not accept named arguments",
    );
  });

  it("report a builtin's unknown, repeated and missing parameters", () => {
    expect(runPetalError("print(append([1], x: 2))")).toContain(
      "append() has no parameter named 'x'",
    );
    expect(runPetalError("print(clamp(5, value: 0, hi: 3))")).toContain(
      "clamp() got multiple values for parameter 'value'",
    );
    expect(runPetalError("print(noise(x: 1, z: 2))")).toContain(
      "noise() is missing a value for parameter 'y'",
    );
  });

  it("refuse a positional argument after a named one", () => {
    expect(runPetalError(`${SUB}print(sub(a: 1, 2))`)).toContain("named");
  });
});

describe("named arguments to builtins", () => {
  it("bind against the builtin's declared parameters", () => {
    expect(runPetal("print(clamp(hi: 10, value: 15, lo: 0))").trim()).toBe("10");
    expect(runPetal("print(append([1], value: 2))").trim()).toBe("[1, 2]");
  });

  it("leave trailing optional parameters off", () => {
    expect(runPetal("print(slice([1, 2, 3, 4], start: 1))").trim()).toBe("[2, 3, 4]");
    expect(runPetal("print(slice([1, 2, 3, 4], start: 1, end: 3))").trim()).toBe("[2, 3]");
  });

  it("bind through method syntax, a native value and a pipe", () => {
    expect(runPetal("print([1, 2, 3, 4].slice(end: 3, start: 1))").trim()).toBe("[2, 3]");
    expect(runPetal("let c = clamp\nprint(c(hi: 10, value: 15, lo: 0))").trim()).toBe("10");
    expect(runPetal("print(15 |> clamp(hi: 10, lo: 0))").trim()).toBe("10");
  });

  it("bind a built-in class's constructor and methods", () => {
    expect(runPetal("print(Rect(w: 4, h: 2, x: 1, y: 1).inset(n: 1).w)").trim()).toBe("2");
  });
});

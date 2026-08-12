import { describe, it, expect } from "vitest";
import { runPetal, runPetalError, showTokensJson } from "./helpers";

// Papercuts reported by testbed app authors: line continuation, keywords as
// field names, escaped quotes inside interpolation holes, scientific-notation
// floats, and the diagnostic for an unclosed inline `if`.

describe("line continuation", () => {
  it("continues an expression on a line that starts with +", () => {
    expect(runPetal("let x = 1\n  + 2\nprint(x)").trim()).toBe("3");
  });

  it("continues across several operator-led lines, respecting precedence", () => {
    expect(runPetal("let x = 1\n  + 2\n  * 3\nprint(x)").trim()).toBe("7");
  });

  it("continues a blank-line-separated operator line", () => {
    expect(runPetal("let x = 10\n\n  % 4\nprint(x)").trim()).toBe("2");
  });

  it("wraps a long boolean condition in an if", () => {
    const src = [
      "let a = true",
      "let b = false",
      "if a",
      "  && !b",
      "  || false then",
      '  print("yes")',
      "end",
    ].join("\n");
    expect(runPetal(src).trim()).toBe("yes");
  });

  it("wraps a while condition", () => {
    const src = [
      "var i = 0",
      "while i < 3",
      "  && true do",
      "  set i = i + 1",
      "end",
      "print(i)",
    ].join("\n");
    expect(runPetal(src).trim()).toBe("3");
  });

  it("continues a pipeline on a leading |>", () => {
    expect(runPetal("let x = [1, 2, 3]\n  |> len()\nprint(x)").trim()).toBe("3");
  });

  it("continues on leading comparison and concat operators", () => {
    expect(runPetal('let s = "a"\n  ++ "b"\nprint(s)').trim()).toBe("ab");
    expect(runPetal("let c = 3\n  >= 2\nprint(c)").trim()).toBe("true");
    expect(runPetal("let c = 3\n  == 3\nprint(c)").trim()).toBe("true");
    expect(runPetal("let c = nil\n  ?? 7\nprint(c)").trim()).toBe("7");
  });

  it("still allows an operator at the end of a line", () => {
    expect(runPetal("let x = 1 +\n  2\nprint(x)").trim()).toBe("3");
  });

  it("does NOT swallow a statement that begins with unary minus", () => {
    // `- y` on its own line stays a fresh expression, not a subtraction.
    const src = ["fn f(y)", "  let a = 1", "  -y", "end", "print(f(5))"].join("\n");
    expect(runPetal(src).trim()).toBe("-5");
  });

  it("does NOT treat a leading `<` as a continuation (JSX still parses)", () => {
    const src = ["let name = 1", '<div id="a"></div>'].join("\n");
    // Parses; the point is that `<` starts a JSX element, not a comparison.
    expect(() => runPetal(src)).not.toThrow();
  });
});

describe("keywords as field names", () => {
  it("allows `when` as a record key and in field access", () => {
    expect(runPetal("let r = {when: 3}\nprint(r.when)").trim()).toBe("3");
  });

  it("allows other keywords as record keys", () => {
    const src = "let r = {end: 1, match: 2, state: 3, then: 4, in: 5}\n" +
      "print(r.end, r.match, r.state, r.then, r.in)";
    expect(runPetal(src).trim()).toBe("1 2 3 4 5");
  });

  it("allows a keyword key in a record pattern", () => {
    const src = ["let r = {when: 7}", "let v = match r", "  when {when: w} -> w", "end", "print(v)"].join("\n");
    expect(runPetal(src).trim()).toBe("7");
  });

  it("keeps `when` working as a match arm keyword", () => {
    const src = ["let v = match 2", "  when 1 -> \"one\"", "  when 2 -> \"two\"", "end", "print(v)"].join("\n");
    expect(runPetal(src).trim()).toBe("two");
  });
});

describe("escaped quotes inside an interpolation hole", () => {
  it("accepts \\\" as a string delimiter inside a hole", () => {
    expect(runPetal('let t = true\nprint("v {if t then \\"a\\" else \\"b\\" end}")').trim()).toBe("v a");
  });

  it("still accepts bare nested quotes inside a hole", () => {
    expect(runPetal('let t = false\nprint("v {if t then "a" else "b" end}")').trim()).toBe("v b");
  });

  it("leaves escaped quotes outside a hole alone", () => {
    expect(runPetal('print("she said \\"hi\\"")').trim()).toBe('she said "hi"');
    expect(runPetal('let n = "x"\nprint("a {n} \\"q\\" b")').trim()).toBe('a x "q" b');
  });

  it("reports an error inside a hole at the right line", () => {
    const err = runPetalError('let a = 1\nlet b = 2\nprint("x {nope}")');
    expect(err).toContain("line 3");
  });
});

describe("scientific notation floats", () => {
  it("lexes exponent forms as floats", () => {
    expect(showTokensJson("1e9")).toEqual([{ Float: 1e9 }, "Eof"]);
    expect(runPetal("print(1.0e9)").trim()).toBe("1000000000.0");
    expect(runPetal("print(1e3)").trim()).toBe("1000.0");
    expect(runPetal("print(1.5e-3)").trim()).toBe("0.0015");
    expect(runPetal("print(2E+4)").trim()).toBe("20000.0");
  });

  it("does not steal an identifier that only looks like an exponent", () => {
    expect(runPetal("let e9 = 5\nprint(e9)").trim()).toBe("5");
    expect(runPetal("print(3.max(4))").trim()).toBe("4");
  });
});

describe("unclosed `if` diagnostic", () => {
  it("names where the unclosed if started", () => {
    const src = "fn rr(a, b, c, d)\n  print(a)\nend\nlet q = true\nrr(1, 9, if q then 2 else 3, if q then 4 else 5)";
    const err = runPetalError(src);
    expect(err).toContain("started at line 5 column 10");
    expect(err).toContain("expected `end`");
  });

  it("names the unclosed if with no else branch", () => {
    const err = runPetalError('let q = true\nprint(if q then 1)');
    expect(err).toContain("is unclosed; expected `end`");
  });

  it("a closed inline if still parses", () => {
    expect(runPetal("let q = true\nprint(if q then 1 else 2 end)").trim()).toBe("1");
  });
});

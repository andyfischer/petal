import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showTokensJson, runPetal, runPetalError } from "./helpers";

beforeAll(() => ensureBuild());

describe("semicolons", () => {
  it("lexes semicolon as Newline token", () => {
    const tokens = showTokensJson("let x = 1; let y = 2");
    // Semicolons should produce Newline tokens, same as actual newlines
    const newlineCount = tokens.filter((t: any) => t === "Newline").length;
    expect(newlineCount).toBeGreaterThanOrEqual(1);
  });

  it("semicolons separate statements", () => {
    const out = runPetal('let x = 1; let y = 2; print(x + y)');
    expect(out.trim()).toBe("3");
  });

  it("trailing semicolons are allowed", () => {
    const out = runPetal('print("hello");');
    expect(out.trim()).toBe("hello");
  });

  it("semicolons and newlines can be mixed", () => {
    const out = runPetal('let a = 1; let b = 2\nlet c = a + b; print(c)');
    expect(out.trim()).toBe("3");
  });

  it("semicolons work inside function bodies", () => {
    const out = runPetal('fn add(a, b)\n  let sum = a + b; sum\nend\nprint(add(3, 4))');
    expect(out.trim()).toBe("7");
  });
});

describe("DotDotDot token", () => {
  it("lexes ... as a single DotDotDot token", () => {
    const tokens = showTokensJson("...x");
    expect(tokens).toContain("DotDotDot");
    // Should NOT be DotDot + Dot
    const dotDotCount = tokens.filter((t: any) => t === "DotDot").length;
    expect(dotDotCount).toBe(0);
  });

  it("still lexes .. as DotDot", () => {
    const tokens = showTokensJson("1..10");
    expect(tokens).not.toContain("DotDotDot");
    expect(tokens).toContain("DotDot");
  });

  it("lexes . as Dot", () => {
    const tokens = showTokensJson("a.b");
    expect(tokens).toContain("Dot");
  });

  it("lexes ... in list pattern context", () => {
    const tokens = showTokensJson("[first, ...rest]");
    expect(tokens).toEqual([
      "LBracket",
      { Ident: "first" },
      "Comma",
      "DotDotDot",
      { Ident: "rest" },
      "RBracket",
      "Eof",
    ]);
  });
});

describe("triple-quoted raw strings", () => {
  it("lexes a triple-quoted string as a single String token", () => {
    const tokens = showTokensJson('"""hello"""');
    expect(tokens).toEqual([{ String: "hello" }, "Eof"]);
  });

  it("captures raw newlines verbatim", () => {
    const out = runPetal('print("""line one\nline two""")');
    expect(out.trim()).toBe("line one\nline two");
  });

  it("treats braces as literal (no interpolation)", () => {
    // Inside a raw string, `{` does not start an interpolation hole.
    const out = runPetal('print("""fn c() { 1 }""")');
    expect(out.trim()).toBe("fn c() { 1 }");
  });

  it("does not process backslash escapes", () => {
    const out = runPetal('print("""a\\nb""")');
    expect(out.trim()).toBe("a\\nb");
  });

  it("allows embedded double quotes", () => {
    const out = runPetal('print("""say "hi" now""")');
    expect(out.trim()).toBe('say "hi" now');
  });

  it("supports embedding multi-line source code with braces and quotes", () => {
    const out = runPetal(
      'let src = """\n  fn step(input) {\n    str(input) ++ "!"\n  }\n"""\nprint(src)'
    );
    expect(out).toContain("fn step(input) {");
    expect(out).toContain('str(input) ++ "!"');
  });

  it("lexes an empty triple-quoted string", () => {
    const tokens = showTokensJson('""""""');
    expect(tokens).toEqual([{ String: "" }, "Eof"]);
  });
});

describe("a literal brace in a double-quoted string", () => {
  // A bare `{` used to open an interpolation hole, whose first `"` opened a
  // *nested* string that ran on until the next quote anywhere in the file.
  // Quote parity inverted from there, and the first character that is illegal
  // outside a string got the blame — hundreds of lines away from the cause.
  const laterStrings = [
    'print("mid · dot")',
    'print("dash — here")',
    'print("arrow ↑ up")',
  ].join("\n");

  it("blames the brace, not a non-ASCII character much later in the file", () => {
    const err = runPetalError(
      ['let name = "x"', 'let tok = "{" ++ name ++ "}"', "", laterStrings].join("\n")
    );
    expect(err).toMatch(/line 2/);
    expect(err).toContain("interpolation hole");
    expect(err).toContain('"""{"""');
    expect(err).not.toContain("·");
    expect(err).not.toContain("—");
    expect(err).not.toContain("↑");
  });

  it("rejects a lone bare-brace literal at the brace", () => {
    const err = runPetalError(['let open = "{"', "", laterStrings].join("\n"));
    expect(err).toMatch(/line 1, column 13/);
    expect(err).toContain("interpolation hole");
  });

  it("names the escape form, and that form runs clean alongside non-ASCII text", () => {
    const out = runPetal(
      ['let tok = "\\{" ++ "x" ++ "\\}"', "print(tok)", laterStrings].join("\n")
    );
    expect(out.trim().split("\n")).toEqual([
      "{x}",
      "mid · dot",
      "dash — here",
      "arrow ↑ up",
    ]);
  });

  it("names the raw-string form, and that form runs clean too", () => {
    const out = runPetal(
      ['let tok = """{""" ++ "x" ++ """}"""', "print(tok)", laterStrings].join("\n")
    );
    expect(out.trim().split("\n")[0]).toBe("{x}");
  });

  it("still allows a hole whose first token is a string but which computes", () => {
    expect(runPetal('let x = "B"\nprint("{"pre" ++ x}")').trim()).toBe("preB");
  });

  it("rejects a string inside a hole that runs past the end of its line", () => {
    const err = runPetalError('print("{ 1 ++ "y\nz" }")');
    expect(err).toContain("must close on the same line");
  });
});

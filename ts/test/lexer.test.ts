import { describe, it, expect } from "vitest";
import {
  showTokensJson,
  showTokensText,
  tokenKinds,
  runPetal,
  runPetalError,
} from "./helpers";


describe("token dump format", () => {
  // JSON rows are uniform: {kind, value?, span} where span is
  // [startLine, startCol, endLine, endCol] (1-based, end-exclusive).
  it("emits uniform rows with spans", () => {
    const tokens = showTokensJson("let x = 1");
    expect(tokens).toEqual([
      { kind: "Let", span: [1, 1, 1, 4] },
      { kind: "Ident", value: "x", span: [1, 5, 1, 6] },
      { kind: "Assign", span: [1, 7, 1, 8] },
      { kind: "Int", value: 1, span: [1, 9, 1, 10] },
      { kind: "Eof", span: [1, 10, 1, 10] },
    ]);
  });

  it("keeps numeric values as JSON numbers and strings as JSON strings", () => {
    const tokens = showTokensJson('f(1, 2.5, "s")');
    const byKind = Object.fromEntries(
      tokens.filter((t: any) => "value" in t).map((t: any) => [t.kind, t.value])
    );
    expect(byKind.Int).toBe(1);
    expect(byKind.Float).toBe(2.5);
    expect(byKind.String).toBe("s");
  });

  it("omits value on unit tokens", () => {
    const tokens = showTokensJson("let x = 1");
    for (const t of tokens.filter((t: any) => t.kind === "Let" || t.kind === "Assign")) {
      expect(t).not.toHaveProperty("value");
    }
  });

  it("spans track lines", () => {
    const tokens = showTokensJson("let x = 1\nlet y = 2");
    const y = tokens.find((t: any) => t.kind === "Ident" && t.value === "y");
    expect(y.span).toEqual([2, 5, 2, 6]);
  });

  it("text form is one token per line with index, value, and span", () => {
    const lines = showTokensText("let x = 1").split("\n");
    expect(lines).toEqual([
      "0: Let @1:1-1:4",
      '1: Ident "x" @1:5-1:6',
      "2: Assign @1:7-1:8",
      "3: Int 1 @1:9-1:10",
      "4: Eof @1:10-1:10",
    ]);
  });
});

describe("semicolons", () => {
  it("lexes semicolon as Newline token", () => {
    const tokens = showTokensJson("let x = 1; let y = 2");
    // Semicolons should produce Newline tokens, same as actual newlines
    const newlineCount = tokenKinds(tokens).filter((k) => k === "Newline").length;
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
    const kinds = tokenKinds(showTokensJson("...x"));
    expect(kinds).toContain("DotDotDot");
    // Should NOT be DotDot + Dot
    const dotDotCount = kinds.filter((k) => k === "DotDot").length;
    expect(dotDotCount).toBe(0);
  });

  it("still lexes .. as DotDot", () => {
    const kinds = tokenKinds(showTokensJson("1..10"));
    expect(kinds).not.toContain("DotDotDot");
    expect(kinds).toContain("DotDot");
  });

  it("lexes . as Dot", () => {
    expect(tokenKinds(showTokensJson("a.b"))).toContain("Dot");
  });

  it("lexes ... in list pattern context", () => {
    const tokens = showTokensJson("[first, ...rest]");
    expect(tokens).toEqual([
      { kind: "LBracket", span: [1, 1, 1, 2] },
      { kind: "Ident", value: "first", span: [1, 2, 1, 7] },
      { kind: "Comma", span: [1, 7, 1, 8] },
      { kind: "DotDotDot", span: [1, 9, 1, 12] },
      { kind: "Ident", value: "rest", span: [1, 12, 1, 16] },
      { kind: "RBracket", span: [1, 16, 1, 17] },
      { kind: "Eof", span: [1, 17, 1, 17] },
    ]);
  });
});

describe("triple-quoted raw strings", () => {
  it("lexes a triple-quoted string as a single String token", () => {
    const tokens = showTokensJson('"""hello"""');
    expect(tokens).toEqual([
      { kind: "String", value: "hello", span: [1, 1, 1, 12] },
      { kind: "Eof", span: [1, 12, 1, 12] },
    ]);
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
    expect(tokens).toEqual([
      { kind: "String", value: "", span: [1, 1, 1, 7] },
      { kind: "Eof", span: [1, 7, 1, 7] },
    ]);
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

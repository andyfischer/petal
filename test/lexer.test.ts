import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showTokensJson, runPetal } from "./helpers";

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
    const out = runPetal('fn add(a, b) { let sum = a + b; sum }; print(add(3, 4))');
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

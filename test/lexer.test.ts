import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showTokensJson } from "./helpers";

beforeAll(() => ensureBuild());

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

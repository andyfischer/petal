import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showTokensJson,
  runPetal,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("string interpolation", () => {
  it("lexes a string with interpolation into parts", () => {
    const tokens = showTokensJson('"hello {name}"');
    expect(tokens).toContainEqual({ String: "hello " });
    expect(tokens).toContain("InterpStart");
    expect(tokens).toContainEqual({ Ident: "name" });
    expect(tokens).toContain("InterpEnd");
  });

  it("evaluates simple variable interpolation", () => {
    const result = runPetal('let name = "world"\nprint("hello {name}")');
    expect(result).toBe("hello world");
  });

  it("evaluates expression interpolation", () => {
    const result = runPetal("let x = 5\nprint(\"{x + 1}\")");
    expect(result).toBe("6");
  });

  it("evaluates multiple interpolations", () => {
    const result = runPetal(
      'let a = "foo"\nlet b = "bar"\nprint("{a} and {b}")'
    );
    expect(result).toBe("foo and bar");
  });

  it("handles string with no interpolation normally", () => {
    const result = runPetal('print("hello world")');
    expect(result).toBe("hello world");
  });

  it("handles escaped braces", () => {
    const result = runPetal('print("value: \\{not interpolated\\}")');
    expect(result).toBe("value: {not interpolated}");
  });

  it("converts non-string values to strings", () => {
    const result = runPetal('let n = 42\nprint("n is {n}")');
    expect(result).toBe("n is 42");
  });

  it("handles adjacent interpolations", () => {
    const result = runPetal('let a = 1\nlet b = 2\nprint("{a}{b}")');
    expect(result).toBe("12");
  });
});

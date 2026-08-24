import { describe, it, expect } from "vitest";
import {
  showTokensJson,
  runPetal,
} from "./helpers";


describe("string interpolation", () => {
  it("lexes a string with interpolation into parts", () => {
    const tokens = showTokensJson('"hello {name}"');
    const parts = tokens.map((t: any) => [t.kind, t.value]);
    expect(parts).toContainEqual(["String", "hello "]);
    expect(parts).toContainEqual(["InterpStart", undefined]);
    expect(parts).toContainEqual(["Ident", "name"]);
    expect(parts).toContainEqual(["InterpEnd", undefined]);
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

describe("escaped quotes inside an interpolation hole", () => {
  // A nested string may be written bare or backslash-escaped; both spellings
  // mean the same thing, so both must lex.
  it("accepts an escaped empty string next to ??", () => {
    const result = runPetal(
      'let a = "A"\nlet b = nil\nlet c = "C"\nprint("{a} · {b ?? \\"\\"} · {c}")'
    );
    expect(result).toBe("A ·  · C");
  });

  // Spans differ between the two spellings (the escapes occupy columns), so
  // compare kind+value only.
  const kindsAndValues = (tokens: any[]) =>
    tokens.map((t: any) => ({ kind: t.kind, value: t.value }));

  it("lexes the escaped and bare spellings of a hole identically", () => {
    const escaped = showTokensJson('"v {if t then \\"a\\" else \\"b\\" end}"');
    const bare = showTokensJson('"v {if t then "a" else "b" end}"');
    expect(kindsAndValues(escaped)).toEqual(kindsAndValues(bare));
  });

  it("accepts an escaped string in a JSX child hole", () => {
    const escaped = showTokensJson('<t>{b ?? \\"q\\"}</t>');
    const bare = showTokensJson('<t>{b ?? "q"}</t>');
    expect(kindsAndValues(escaped)).toEqual(kindsAndValues(bare));
  });
});

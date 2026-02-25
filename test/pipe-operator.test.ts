import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showTokensJson,
  showAstJson,
  runPetal,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("pipe operator |>", () => {
  it("lexes |> as a Pipe token", () => {
    const tokens = showTokensJson("1 |> f");
    expect(tokens).toContainEqual("Pipe");
  });

  it("parses x |> f as f(x)", () => {
    const ast = showAstJson("5 |> str");
    // Should desugar to: str(5)
    const expr = ast[0].Expr;
    expect(expr.Call).toBeDefined();
    expect(expr.Call.function.Ident).toBe("str");
    expect(expr.Call.args).toHaveLength(1);
    expect(expr.Call.args[0].Literal.Int).toBe(5);
  });

  it("parses x |> f(y) as f(x, y)", () => {
    const ast = showAstJson("10 |> min(20)");
    // Should desugar to: min(10, 20)
    const expr = ast[0].Expr;
    expect(expr.Call).toBeDefined();
    expect(expr.Call.function.Ident).toBe("min");
    expect(expr.Call.args).toHaveLength(2);
    expect(expr.Call.args[0].Literal.Int).toBe(10);
    expect(expr.Call.args[1].Literal.Int).toBe(20);
  });

  it("chains multiple pipes", () => {
    const output = runPetal('[1, 2, 3, 4, 5] |> filter(fn(x) { x > 2 }) |> map(fn(x) { x * 10 }) |> print');
    expect(output).toBe("[30, 40, 50]");
  });

  it("pipes a value into a simple function", () => {
    const output = runPetal('42 |> str |> print');
    expect(output).toBe("42");
  });

  it("pipes into a function with extra args", () => {
    const output = runPetal('[1, 2, 3] |> map(fn(x) { x + 10 }) |> print');
    expect(output).toBe("[11, 12, 13]");
  });

  it("has lower precedence than arithmetic", () => {
    const output = runPetal('1 + 2 |> str |> print');
    expect(output).toBe("3");
  });

  it("works with lambda expressions", () => {
    const output = runPetal('5 |> fn(x) { x * 2 } |> print');
    expect(output).toBe("10");
  });
});

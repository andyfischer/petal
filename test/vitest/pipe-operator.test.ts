import { describe, it, expect } from "vitest";
import {
  showTokensJson,
  showAstJson,
} from "./helpers";


describe("pipe operator |>", () => {
  it("lexes |> as a Pipe token", () => {
    const tokens = showTokensJson("1 |> f");
    expect(tokens.map((t: any) => t.kind)).toContain("Pipe");
  });

  it("parses x |> f as f(x)", () => {
    const ast = showAstJson("5 |> str");
    // Should desugar to: str(5)
    const expr = ast[0].kind.Expr.kind;
    expect(expr.Call).toBeDefined();
    expect(expr.Call.function.kind.Ident).toBe("str");
    expect(expr.Call.args).toHaveLength(1);
    expect(expr.Call.args[0].kind.Literal.Int).toBe(5);
  });

  it("parses x |> f(y) as f(x, y)", () => {
    const ast = showAstJson("10 |> min(20)");
    // Should desugar to: min(10, 20)
    const expr = ast[0].kind.Expr.kind;
    expect(expr.Call).toBeDefined();
    expect(expr.Call.function.kind.Ident).toBe("min");
    expect(expr.Call.args).toHaveLength(2);
    expect(expr.Call.args[0].kind.Literal.Int).toBe(10);
    expect(expr.Call.args[1].kind.Literal.Int).toBe(20);
  });

  // What a pipe evaluates to is tested in Petal: test/ptl/pipe_operator.ptl
  // (`petal test test/ptl`, run by core/tests/ptl_tests.rs).
});

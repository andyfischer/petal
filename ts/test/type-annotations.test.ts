import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, showAstJson } from "./helpers";

beforeAll(() => ensureBuild());

// Chunk B: optional type annotations on `let` bindings and function/lambda
// parameters. Annotations are parsed and surfaced on the AST but not yet
// checked or used at runtime. In the serialized AST the resolved type appears
// under `ty` as the Rust variant name ("Int", "Float", "String", ...) or null.

function letStmt(ast: any) {
  return ast.find((s: any) => s.kind.Let)?.kind.Let;
}
function fnDecl(ast: any) {
  return ast.find((s: any) => s.kind.FnDecl)?.kind.FnDecl;
}

describe("optional type annotations", () => {
  it("parses a typed let binding and exposes the type", () => {
    const ast = showAstJson("let x: int = 5");
    expect(letStmt(ast).name).toBe("x");
    expect(letStmt(ast).ty).toBe("Int");
  });

  it("leaves un-annotated let with ty: null", () => {
    const ast = showAstJson("let y = 5");
    expect(letStmt(ast).ty).toBeNull();
  });

  it("accepts str as an alias for string", () => {
    const ast = showAstJson('let s: str = "hi"');
    expect(letStmt(ast).ty).toBe("String");
    expect(showAstJson('let s: string = "hi"').find((x: any) => x.kind.Let).kind.Let.ty).toBe(
      "String",
    );
  });

  it("parses per-parameter annotations, mixing typed and bare params", () => {
    const ast = showAstJson("fn f(a: int, b, c: string) a end");
    const params = fnDecl(ast).params;
    expect(params).toEqual([
      { name: "a", ty: "Int" },
      { name: "b", ty: null },
      { name: "c", ty: "String" },
    ]);
  });

  it("leaves fully un-annotated params with ty: null", () => {
    const params = fnDecl(showAstJson("fn g(a, b) a end")).params;
    expect(params).toEqual([
      { name: "a", ty: null },
      { name: "b", ty: null },
    ]);
  });

  it("parses a function return-type annotation", () => {
    const ast = showAstJson("fn area(r: float) -> float\n  r\nend");
    expect(fnDecl(ast).ret).toBe("Float");
  });

  it("leaves an un-annotated function with ret: null", () => {
    const ast = showAstJson("fn greet(n)\n  n\nend");
    expect(fnDecl(ast).ret).toBeNull();
  });

  it("runs a function with a return-type annotation (ignored at runtime)", () => {
    expect(runPetal("fn dbl(n: int) -> int\n  n * 2\nend\nprint(dbl(21))")).toBe("42");
  });

  it("parses lambda parameter annotations", () => {
    const ast = showAstJson("let d = fn(n: int) -> n * 2");
    const lambda = ast.find((s: any) => s.kind.Let).kind.Let.value.kind.Lambda;
    expect(lambda.params).toEqual([{ name: "n", ty: "Int" }]);
  });

  it("accepts an unknown type name syntactically but drops it (ty: null)", () => {
    // TODO(types): a later chunk should preserve the raw name for diagnostics.
    const ast = showAstJson("let z: banana = 3");
    expect(letStmt(ast).ty).toBeNull();
  });

  it("ignores annotations at runtime (dynamic execution unchanged)", () => {
    const out = runPetal("let x: int = 5\nfn sq(n: int) n * n end\nprint(x, sq(x))");
    expect(out).toBe("5 25");
  });

  it("runs a lambda with an annotated parameter", () => {
    expect(runPetal("let d = fn(n: int) -> n * 2\nprint(d(21))")).toBe("42");
  });
});

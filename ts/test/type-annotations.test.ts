import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, showAstJson } from "./helpers";

beforeAll(() => ensureBuild());

// Chunk B/E1: optional type annotations on `let` bindings and function/lambda
// parameters. Annotations are parsed and surfaced on the AST but not yet
// checked or used at runtime. In the serialized AST an annotation appears under
// `ty`/`ret` as an object `{ name, resolved }`: `name` is the raw type name as
// written (`"int"`, `"str"`, `"banana"`), and `resolved` is the Rust `Type`
// variant name ("Int", "Float", "String", ...) or null for an unknown name.
// An absent annotation is null.

function letStmt(ast: any) {
  return ast.find((s: any) => s.kind.Let)?.kind.Let;
}
function fnDecl(ast: any) {
  return ast.find((s: any) => s.kind.FnDecl)?.kind.FnDecl;
}
function stateStmt(ast: any) {
  return ast.find((s: any) => s.kind.State)?.kind.State;
}

describe("optional type annotations", () => {
  it("parses a typed let binding and exposes the type", () => {
    const ast = showAstJson("let x: int = 5");
    expect(letStmt(ast).name).toBe("x");
    expect(letStmt(ast).ty).toEqual({ name: "int", resolved: "Int" });
  });

  it("leaves un-annotated let with ty: null", () => {
    const ast = showAstJson("let y = 5");
    expect(letStmt(ast).ty).toBeNull();
  });

  it("accepts str as an alias for string", () => {
    const ast = showAstJson('let s: str = "hi"');
    expect(letStmt(ast).ty).toEqual({ name: "str", resolved: "String" });
    expect(
      showAstJson('let s: string = "hi"').find((x: any) => x.kind.Let).kind.Let.ty,
    ).toEqual({ name: "string", resolved: "String" });
  });

  it("parses per-parameter annotations, mixing typed and bare params", () => {
    const ast = showAstJson("fn f(a: int, b, c: string) a end");
    const params = fnDecl(ast).params;
    expect(params).toEqual([
      { name: "a", ty: { name: "int", resolved: "Int" } },
      { name: "b", ty: null },
      { name: "c", ty: { name: "string", resolved: "String" } },
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
    expect(fnDecl(ast).ret).toEqual({ name: "float", resolved: "Float" });
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
    expect(lambda.params).toEqual([{ name: "n", ty: { name: "int", resolved: "Int" } }]);
  });

  it("preserves an unknown type name (raw name kept, resolved: null)", () => {
    const ast = showAstJson("let z: banana = 3");
    expect(letStmt(ast).ty).toEqual({ name: "banana", resolved: null });
  });

  it("ignores annotations at runtime (dynamic execution unchanged)", () => {
    const out = runPetal("let x: int = 5\nfn sq(n: int) n * n end\nprint(x, sq(x))");
    expect(out).toBe("5 25");
  });

  it("runs a lambda with an annotated parameter", () => {
    expect(runPetal("let d = fn(n: int) -> n * 2\nprint(d(21))")).toBe("42");
  });

  // A lambda's `->` already introduces its body (`fn(n) -> n * 2`), so a lambda
  // return annotation would need two arrows and is deliberately not supported
  // (type-declarations-plan.md §2). Parameter annotations are unambiguous and do
  // work; this pins the decision so it isn't "fixed" by accident.
  it("rejects a return-type annotation on a lambda", () => {
    expect(() => showAstJson("let f = fn(x: int) -> int -> x + 1")).toThrow();
  });
});

// `state` takes the same `: type` slot as `let`/`var` — it is a binding form,
// and the annotation is what lets the checker say anything about a reactive
// cell at all (see type-declarations-progress.md, `state` annotations).
describe("type annotations on `state`", () => {
  it("parses a typed state binding and exposes the type", () => {
    const ast = showAstJson("state n: int = 0");
    expect(stateStmt(ast).name).toBe("n");
    expect(stateStmt(ast).ty).toEqual({ name: "int", resolved: "Int" });
  });

  it("leaves an un-annotated state with ty: null", () => {
    expect(stateStmt(showAstJson("state n = 0")).ty).toBeNull();
  });

  it("parses annotations on `state var` and on a keyed state", () => {
    expect(stateStmt(showAstJson("state var n: float = 0.0")).ty).toEqual({
      name: "float",
      resolved: "Float",
    });
    const keyed = stateStmt(showAstJson('state(1) n: string = "a"'));
    expect(keyed.ty).toEqual({ name: "string", resolved: "String" });
    expect(keyed.key).not.toBeNull();
  });

  it("preserves an unknown type name on state", () => {
    expect(stateStmt(showAstJson("state n: banana = 0")).ty).toEqual({
      name: "banana",
      resolved: null,
    });
  });

  it("ignores state annotations at runtime", () => {
    expect(runPetal("state n: int = 41\nprint(n + 1)")).toBe("42");
    expect(runPetal('state var s: string = "hi"\nset s = "bye"\nprint(s)')).toBe("bye");
  });
});

// A type is a single bare name — there are no parameterized types. Written
// naively, `list<int>` used to fail three different confusing ways depending on
// position (a missing-initializer error, a comma error, and an unclosed-JSX
// error, because `<int>` lexes as a JSX tag). Say what is actually wrong.
describe("parameterized type names are rejected with a targeted error", () => {
  const cases: [string, string][] = [
    ["let xs: list<int> = [1]", "let"],
    ["state xs: list<int> = []", "state"],
    ["fn f(a: list<int>)\n  a\nend", "parameter"],
    ["fn f() -> list<int>\n  []\nend", "return type"],
  ];
  for (const [src, what] of cases) {
    it(`explains the error in ${what} position`, () => {
      expect(() => showAstJson(src)).toThrow(/parameterized types/);
    });
  }

  it("still accepts the bare name", () => {
    expect(runPetal("let xs: list = [1, 2]\nprint(len(xs))")).toBe("2");
  });

  it("does not disturb a real comparison after a binding", () => {
    expect(runPetal("let a = 1\nlet b = 2\nprint(a < b)")).toBe("true");
  });
});

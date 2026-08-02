import { describe, it, expect, beforeAll } from "vitest";
import { execSync } from "child_process";
import { resolve } from "path";
import {
  ensureBuild,
  runPetal,
  checkJson,
  checkJsonAllowFail,
  showAstJson,
  runWithStderr,
} from "./helpers";

beforeAll(() => ensureBuild());

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");
const FIXTURES = resolve(__dirname, "fixtures/classes");

// Classes & user-declared methods. See docs/language-guide.md (Classes &
// Methods). A class is a named record type: instances are ordinary records
// carrying a class tag, so every record operation still works on them.

describe("class declarations", () => {
  it("declares a class and constructs it positionally", () => {
    const out = runPetal(
      "class Point\n  x: int\n  y: int\nend\nlet p = Point(3, 4)\nprint(p.x)\nprint(p.y)"
    );
    expect(out.trim().split("\n")).toEqual(["3", "4"]);
  });

  it("an instance is a record — record builtins still work", () => {
    const out = runPetal(
      "class Point\n  x: int\n  y: int\nend\nlet p = Point(1, 2)\nprint(keys(p))\nprint(p)"
    );
    expect(out.trim().split("\n")).toEqual(['["x", "y"]', "{ x: 1, y: 2 }"]);
  });

  it("type() reports the class name", () => {
    const out = runPetal("class Point\n  x: int\n  y: int\nend\nprint(type(Point(1, 2)))");
    expect(out.trim()).toBe("Point");
  });

  it("a field write keeps the class tag", () => {
    const out = runPetal(
      "class Point\n  x: int\n  y: int\nend\nlet p = Point(1, 2)\nlet q = {...p, x: 9}\nprint(type(q))\nlet r = Point(1, 2)\nr.x = 5\nprint(type(r))\nprint(r.x)"
    );
    expect(out.trim().split("\n")).toEqual(["record", "Point", "5"]);
  });

  it("reports the constructor arity in an error", () => {
    const { stderr } = runWithStderr(
      "class Point\n  x: int\n  y: int\nend\nlet p = Point(1)"
    );
    expect(stderr).toMatch(/Point/);
    expect(stderr).toMatch(/2/);
  });

  it("appears in the AST as a ClassDecl", () => {
    const ast = showAstJson("class Point\n  x: int\n  y: int\nend");
    const decl = ast.find((s: any) => s.kind && s.kind.ClassDecl);
    expect(decl).toBeTruthy();
    expect(decl.kind.ClassDecl.name).toBe("Point");
    expect(decl.kind.ClassDecl.fields.map((f: any) => f.name)).toEqual(["x", "y"]);
  });
});

describe("class names as type annotations", () => {
  it("accepts a matching argument with no warning", () => {
    const out = checkJson(
      "class Point\n  x: int\n  y: int\nend\nfn px(p: Point) -> int\n  p.x\nend\nprint(px(Point(1, 2)))"
    );
    expect(out.ok).toBe(true);
    expect(out.warnings).toEqual([]);
  });

  it("warns when a non-instance is passed", () => {
    const out = checkJson(
      "class Point\n  x: int\n  y: int\nend\nfn px(p: Point) -> int\n  p.x\nend\nprint(px(7))"
    );
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/Point/);
    expect(out.warnings[0].message).toMatch(/argument 1/);
  });

  it("infers a field's declared type", () => {
    const out = checkJson(
      "class Point\n  x: int\n  y: int\nend\nlet s: string = Point(1, 2).x"
    );
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/mismatch/);
  });

  it("does not warn on an unknown-type-name for a declared class", () => {
    const out = checkJson("class Point\n  x: int\nend\nlet p: Point = Point(1)");
    expect(out.warnings).toEqual([]);
  });
});

describe("user-declared methods", () => {
  it("declares and dispatches a method", () => {
    const out = runPetal(
      "class Rect2\n  x: int\n  y: int\n  w: int\n  h: int\nend\n" +
        "fn Rect2.center_x(r: Rect2)\n  r.x + r.w / 2\nend\n" +
        "let r = Rect2(0, 0, 100, 40)\nprint(r.center_x())"
    );
    expect(out.trim()).toBe("50");
  });

  it("passes extra arguments after the receiver", () => {
    const out = runPetal(
      "class Point\n  x: int\n  y: int\nend\n" +
        "fn Point.shifted(p: Point, dx: int, dy: int) -> Point\n  Point(p.x + dx, p.y + dy)\nend\n" +
        "let p = Point(1, 2).shifted(10, 20)\nprint(p.x)\nprint(p.y)"
    );
    expect(out.trim().split("\n")).toEqual(["11", "22"]);
  });

  it("dispatches per class for the same method name", () => {
    const out = runPetal(
      "class A\n  v: int\nend\nclass B\n  v: int\nend\n" +
        'fn A.describe(a: A)\n  "A" ++ str(a.v)\nend\n' +
        'fn B.describe(b: B)\n  "B" ++ str(b.v)\nend\n' +
        "print(A(1).describe())\nprint(B(2).describe())"
    );
    expect(out.trim().split("\n")).toEqual(["A1", "B2"]);
  });

  it("a callable record field still wins over a method", () => {
    const out = runPetal(
      "class Point\n  x: int\n  f: any\nend\n" +
        'fn Point.f(p: Point)\n  "method"\nend\n' +
        'print(Point(1, fn() -> "field").f())'
    );
    expect(out.trim()).toBe("field");
  });

  it("errors when calling an undefined method on an instance", () => {
    const { stderr } = runWithStderr(
      "class Point\n  x: int\nend\nprint(Point(1).nope())"
    );
    expect(stderr).toMatch(/nope/);
    expect(stderr).toMatch(/Point/);
  });

});

describe("class and method diagnostics", () => {
  it("rejects a method declared on an unknown type", () => {
    const out = checkJsonAllowFail("fn Nope.thing(n: any)\n  1\nend");
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/no class of that name/);
    expect(out.errors[0].line).toBe(1);
  });

  it("rejects duplicate class fields", () => {
    const out = checkJsonAllowFail("class Point\n  x: int\n  x: int\nend");
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/duplicate field `x`/);
  });

  it("lets a user `class Rect` shadow the built-in of that name", () => {
    // The whole target syntax, spelled out, even though `Rect` is built in.
    const out = runPetal(
      "class Rect\n  x: int\n  y: int\n  w: int\n  h: int\nend\n" +
        "fn Rect.center_x(rect: Rect)\n  rect.x + rect.w / 2\nend\n" +
        "let r = Rect(0, 0, 100, 40)\nprint(r.center_x())"
    );
    expect(out.trim()).toBe("50");
  });

  it("rejects a duplicate class declaration", () => {
    const out = checkJsonAllowFail("class Point\n  x: int\nend\nclass Point\n  y: int\nend");
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/class `Point` is already declared/);
  });

  it("rejects two methods with the same name and arity", () => {
    const out = checkJsonAllowFail(
      "class Point\n  x: int\nend\nfn Point.f(p: Point)\n  1\nend\nfn Point.f(p: Point)\n  2\nend"
    );
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/`Point.f` is already declared/);
  });
});

describe("the built-in Rect class", () => {
  it("is available with no declaration", () => {
    const out = runPetal("let r = Rect(10, 20, 100, 40)\nprint(r.x)\nprint(type(r))");
    expect(out.trim().split("\n")).toEqual(["10", "Rect"]);
  });

  it("computes center_x / center_y", () => {
    const out = runPetal("let r = Rect(0, 0, 100, 40)\nprint(r.center_x())\nprint(r.center_y())");
    expect(out.trim().split("\n")).toEqual(["50", "20"]);
  });

  it("computes right / bottom", () => {
    const out = runPetal("let r = Rect(10, 20, 100, 40)\nprint(r.right())\nprint(r.bottom())");
    expect(out.trim().split("\n")).toEqual(["110", "60"]);
  });

  it("insets by a margin", () => {
    const out = runPetal("let r = Rect(0, 0, 100, 40).inset(5)\nprint([r.x, r.y, r.w, r.h])");
    expect(out.trim()).toBe("[5, 5, 90, 30]");
  });

  it("offsets by a delta", () => {
    const out = runPetal("let r = Rect(1, 2, 3, 4).offset(10, 20)\nprint([r.x, r.y, r.w, r.h])");
    expect(out.trim()).toBe("[11, 22, 3, 4]");
  });

  it("returns a Rect from inset, so methods chain", () => {
    const out = runPetal("print(Rect(0, 0, 100, 40).inset(5).center_x())");
    expect(out.trim()).toBe("50");
  });

  it("checks a Rect annotation", () => {
    const out = checkJson('fn cx(r: Rect) -> int\n  r.center_x()\nend\nprint(cx("nope"))');
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/Rect/);
  });

  it("lets a user method be declared on the built-in class", () => {
    const out = runPetal(
      "fn Rect.area(r: Rect) -> int\n  r.w * r.h\nend\nprint(Rect(0, 0, 10, 4).area())"
    );
    expect(out.trim()).toBe("40");
  });
});

describe("classes across modules", () => {
  // Classes and methods are program-wide: the class table and the runtime
  // method table span the whole compilation, so a module may declare a method
  // on a built-in class and an importer may extend an imported one. `export`
  // governs only the constructor *name*, like any other binding.
  it("dispatches methods declared in an imported module", () => {
    const out = execSync(`${PETAL} run ${FIXTURES}/main.ptl`, {
      encoding: "utf-8",
      timeout: 10000,
    }).trim();
    expect(out.split("\n")).toEqual(["40", "Circle 12", "4"]);
  });
});

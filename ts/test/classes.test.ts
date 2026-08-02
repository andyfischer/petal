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

/// `petal check --json` on a real file, so module visibility is exercised.
function checkFileJson(path: string): any {
  return JSON.parse(
    execSync(`${PETAL} check --json ${path}`, { encoding: "utf-8", timeout: 10000 })
  );
}

/// The stderr of a run that is expected to fail.
function runFileError(path: string): string {
  try {
    execSync(`${PETAL} run ${path}`, {
      encoding: "utf-8",
      timeout: 10000,
      stdio: ["pipe", "pipe", "pipe"],
    });
    throw new Error(`expected ${path} to fail, but it succeeded`);
  } catch (e: any) {
    return (e.stderr || "").trim();
  }
}

// Classes & user-declared methods. See docs/language-guide.md (Classes &
// Methods). A class is a named record type: instances are ordinary records
// carrying a class tag, so every record operation still works on them.

describe("class declarations", () => {
  it("declares a class and constructs it positionally", () => {
    const out = runPetal(
      "class Point\n  x: int,\n  y: int,\nend\nlet p = Point(3, 4)\nprint(p.x)\nprint(p.y)"
    );
    expect(out.trim().split("\n")).toEqual(["3", "4"]);
  });

  it("an instance is a record — record builtins still work", () => {
    const out = runPetal(
      "class Point\n  x: int,\n  y: int,\nend\nlet p = Point(1, 2)\nprint(keys(p))\nprint(p)"
    );
    expect(out.trim().split("\n")).toEqual(['["x", "y"]', "{ x: 1, y: 2 }"]);
  });

  it("type() reports the class name", () => {
    const out = runPetal("class Point\n  x: int,\n  y: int,\nend\nprint(type(Point(1, 2)))");
    expect(out.trim()).toBe("Point");
  });

  it("a field write keeps the class tag", () => {
    const out = runPetal(
      "class Point\n  x: int,\n  y: int,\nend\nlet p = Point(1, 2)\nlet q = {...p, x: 9}\nprint(type(q))\nlet r = Point(1, 2)\nr.x = 5\nprint(type(r))\nprint(r.x)"
    );
    expect(out.trim().split("\n")).toEqual(["record", "Point", "5"]);
  });

  it("reports the constructor arity in an error", () => {
    const { stderr } = runWithStderr(
      "class Point\n  x: int,\n  y: int,\nend\nlet p = Point(1)"
    );
    expect(stderr).toMatch(/Point/);
    expect(stderr).toMatch(/2/);
  });

  it("appears in the AST as a ClassDecl", () => {
    const ast = showAstJson("class Point\n  x: int,\n  y: int,\nend");
    const decl = ast.find((s: any) => s.kind && s.kind.ClassDecl);
    expect(decl).toBeTruthy();
    expect(decl.kind.ClassDecl.name).toBe("Point");
    expect(decl.kind.ClassDecl.fields.map((f: any) => f.name)).toEqual(["x", "y"]);
  });
});

describe("class names as type annotations", () => {
  it("accepts a matching argument with no warning", () => {
    const out = checkJson(
      "class Point\n  x: int,\n  y: int,\nend\nfn px(p: Point) -> int\n  p.x\nend\nprint(px(Point(1, 2)))"
    );
    expect(out.ok).toBe(true);
    expect(out.warnings).toEqual([]);
  });

  it("warns when a non-instance is passed", () => {
    const out = checkJson(
      "class Point\n  x: int,\n  y: int,\nend\nfn px(p: Point) -> int\n  p.x\nend\nprint(px(7))"
    );
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/Point/);
    expect(out.warnings[0].message).toMatch(/argument 1/);
  });

  it("infers a field's declared type", () => {
    const out = checkJson(
      "class Point\n  x: int,\n  y: int,\nend\nlet s: string = Point(1, 2).x"
    );
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/mismatch/);
  });

  it("does not warn on an unknown-type-name for a declared class", () => {
    const out = checkJson("class Point\n  x: int,\nend\nlet p: Point = Point(1)");
    expect(out.warnings).toEqual([]);
  });
});

describe("user-declared methods", () => {
  it("declares and dispatches a method", () => {
    const out = runPetal(
      "class Rect2\n  x: int,\n  y: int,\n  w: int,\n  h: int,\nend\n" +
        "fn Rect2.center_x(r: Rect2)\n  r.x + r.w / 2\nend\n" +
        "let r = Rect2(0, 0, 100, 40)\nprint(r.center_x())"
    );
    expect(out.trim()).toBe("50");
  });

  it("passes extra arguments after the receiver", () => {
    const out = runPetal(
      "class Point\n  x: int,\n  y: int,\nend\n" +
        "fn Point.shifted(p: Point, dx: int, dy: int) -> Point\n  Point(p.x + dx, p.y + dy)\nend\n" +
        "let p = Point(1, 2).shifted(10, 20)\nprint(p.x)\nprint(p.y)"
    );
    expect(out.trim().split("\n")).toEqual(["11", "22"]);
  });

  it("dispatches per class for the same method name", () => {
    const out = runPetal(
      "class A\n  v: int,\nend\nclass B\n  v: int,\nend\n" +
        'fn A.describe(a: A)\n  "A" ++ str(a.v)\nend\n' +
        'fn B.describe(b: B)\n  "B" ++ str(b.v)\nend\n' +
        "print(A(1).describe())\nprint(B(2).describe())"
    );
    expect(out.trim().split("\n")).toEqual(["A1", "B2"]);
  });

  it("a callable record field still wins over a method", () => {
    const out = runPetal(
      "class Point\n  x: int,\n  f: any,\nend\n" +
        'fn Point.f(p: Point)\n  "method"\nend\n' +
        'print(Point(1, fn() -> "field").f())'
    );
    expect(out.trim()).toBe("field");
  });

  it("errors when calling an undefined method on an instance", () => {
    const { stderr } = runWithStderr(
      "class Point\n  x: int,\nend\nprint(Point(1).nope())"
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

  it("rejects duplicate class fields, at the second field", () => {
    const out = checkJsonAllowFail("class Point\n  x: int,\n  x: int,\nend");
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/duplicate field `x`/);
    // Every field-level diagnostic used to land on line 1, column 1: the
    // class's span, because `ClassFieldDecl` carried no span of its own.
    expect(out.errors[0].line).toBe(3);
    expect(out.errors[0].column).toBe(3);
  });

  it("underlines a field's own type annotation, not the class", () => {
    const out = checkJsonAllowFail("class Point\n  x: int,\n  y: nosuchtype,\nend");
    const warning = out.warnings.find((w: any) =>
      /unknown type name `nosuchtype`/.test(w.message)
    );
    expect(warning, JSON.stringify(out)).toBeTruthy();
    expect([warning.line, warning.column]).toEqual([3, 6]);
  });

  it("lets a user `class Rect` shadow the built-in of that name", () => {
    // The whole target syntax, spelled out, even though `Rect` is built in.
    const out = runPetal(
      "class Rect\n  x: int,\n  y: int,\n  w: int,\n  h: int,\nend\n" +
        "fn Rect.center_x(rect: Rect)\n  rect.x + rect.w / 2\nend\n" +
        "let r = Rect(0, 0, 100, 40)\nprint(r.center_x())"
    );
    expect(out.trim()).toBe("50");
  });

  it("rejects a duplicate class declaration", () => {
    const out = checkJsonAllowFail("class Point\n  x: int,\nend\nclass Point\n  y: int,\nend");
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/class `Point` is already declared/);
  });

  it("rejects two methods with the same name and arity", () => {
    const out = checkJsonAllowFail(
      "class Point\n  x: int,\nend\nfn Point.f(p: Point)\n  1\nend\nfn Point.f(p: Point)\n  2\nend"
    );
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/`Point.f` is already declared/);
  });

  // A method's receiver is always an instance of the class it is declared on,
  // so an annotation that cannot accept one describes a call that can never
  // happen — fatal, not a warning.
  it("rejects a receiver annotated as a different class", () => {
    const out = checkJsonAllowFail(
      "class A\n  a: int\nend\nclass B\n  b: int\nend\nfn A.go(x: B)\n  x.a\nend"
    );
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/`A.go`/);
    expect(out.errors[0].message).toMatch(/receiver `x` as `B`/);
    expect(out.errors[0].line).toBe(7);
  });

  it("accepts any receiver slot an instance fits, including none", () => {
    for (const src of [
      "class A\n  a: int\nend\nfn A.go(x: A)\n  x.a\nend\nprint(A(1).go())",
      "class A\n  a: int\nend\nfn A.go(x)\n  x.a\nend\nprint(A(1).go())",
      "class A\n  a: int\nend\nfn A.go(x: any)\n  x.a\nend\nprint(A(1).go())",
    ]) {
      expect(checkJson(src).warnings).toEqual([]);
    }
  });
});

describe("field and method checks on a class-typed value", () => {
  it("warns on a field the declared class does not have", () => {
    const out = checkJson("class B\n  b: int\nend\nfn f(x: B)\n  x.nosuch\nend\nprint(f(B(1)))");
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("class `B` has no field `nosuch`");
    expect(out.warnings[0].line).toBe(5);
  });

  it("stays quiet on declared fields, plain records and `any`", () => {
    for (const src of [
      "class B\n  b: int\nend\nfn f(x: B)\n  x.b\nend\nprint(f(B(1)))",
      "let r = {a: 1}\nprint(r.nosuch)",
      "fn f(x)\n  x.nosuch\nend\nprint(f({a: 1}))",
      "fn f(x: record)\n  x.nosuch\nend\nprint(f({a: 1}))",
    ]) {
      expect(checkJson(src).warnings).toEqual([]);
    }
  });

  it("does not read a method name as a field", () => {
    const out = checkJson(
      "class B\n  b: int\nend\nfn B.go(x: B)\n  x.b\nend\nprint(B(1).go())\nprint(B(1).keys())"
    );
    expect(out.warnings).toEqual([]);
  });

  it("warns when no overload of a method takes that many arguments", () => {
    const src =
      "class P\n  x: int\n  y: int\nend\nfn P.shift(p: P, dx: int)\n  p.x + dx\nend\n";
    const out = checkJson(`${src}print(P(1, 2).shift())`);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("method `P.shift` expects 1 argument, got 0");
    // The same call is a hard error at runtime — that is the gap `check` closes.
    const { stderr } = runWithStderr(`${src}print(P(1, 2).shift())`);
    expect(stderr).toMatch(/P.shift\(\) expected 2 arguments/);
    expect(checkJson(`${src}print(P(1, 2).shift(3))`).warnings).toEqual([]);
  });

  it("warns when a constructor is given the wrong number of fields", () => {
    const out = checkJson("class P\n  x: int\n  y: int\nend\nprint(P(1))");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("`P` expects 2 arguments, got 1");
  });
});

// An undefined method whose name collides with a global builtin used to fall
// through to "global native with the receiver prepended" and report the
// builtin's own complaint — which never mentions the class, and which is what
// a live edit that deletes `fn P.get` fails with.
describe("an undefined method on a class instance names the class", () => {
  const P = "class P\n  a: int,\nend\n";

  it.each([
    ["get", "P(1).get()"],
    ["len", "P(1).len()"],
    ["upper", "P(1).upper()"],
    ["nope", "P(1).nope()"],
  ])("reports 'No method %s on class P'", (name, call) => {
    const { stderr } = runWithStderr(`${P}print(${call})`);
    expect(stderr).toContain(`No method '${name}' on class P`);
  });

  // The fallback keeps working when it works — an instance is still a record.
  it("still routes a builtin that accepts the receiver", () => {
    expect(runPetal(`${P}print(P(1).str())`).trim()).toBe("{ a: 1 }");
    expect(runPetal(`${P}print(P(1).keys())`).trim()).toBe('["a"]');
    // And a plain record keeps the builtin's own message.
    const { stderr } = runWithStderr("print({a: 1}.upper())");
    expect(stderr).toContain("upper() expects a string");
  });

  it("names the class when a field does not exist either", () => {
    const { stderr } = runWithStderr(`class B\n  a: int,\nend\nprint(B(1).b)`);
    expect(stderr).toContain("No field 'b' on class B");
  });
});

// Arity messages count what the user wrote. The receiver is a parameter the
// *call site* supplies, so `C(1).foo()` wrote zero arguments, not one.
describe("arity errors", () => {
  it("excludes the receiver from a method's argument count", () => {
    const { stderr } = runWithStderr(
      "class C\n  a: int,\nend\nfn C.foo(c: C, n: int)\n  n\nend\nprint(C(1).foo())"
    );
    expect(stderr).toContain("C.foo() expects 1 argument, got 0");
  });

  it("counts a plain function's arguments as written", () => {
    const { stderr } = runWithStderr("fn f(a, b)\n  a\nend\nprint(f(1))");
    expect(stderr).toContain("f() expects 2 arguments, got 1");
  });

  it("words a builtin's arity error the same way", () => {
    const { stderr } = runWithStderr("print(len())");
    expect(stderr).toContain("len() expects 1 argument, got 0");
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

  // Fractional geometry. A Rect field holds whatever number it was given —
  // Petal has no implicit casting, so a constructor may not quietly truncate
  // its argument. Sub-pixel layout and animation depend on this.
  it("keeps float arguments as floats", () => {
    const out = runPetal("print(Rect(10.5, 20.9, 100.4, 40.6))");
    expect(out.trim()).toBe("{ x: 10.5, y: 20.9, w: 100.4, h: 40.6 }");
  });

  it("keeps a float that only arrives at runtime", () => {
    const out = runPetal("fn mk(a)\n  Rect(a, a, a * 2, a * 2)\nend\nprint(mk(10.5))");
    expect(out.trim()).toBe("{ x: 10.5, y: 10.5, w: 21.0, h: 21.0 }");
  });

  it("does not warn on a float argument", () => {
    const out = checkJson("let r = Rect(10.5, 20.9, 100.4, 40.6)");
    expect(out.warnings).toEqual([]);
  });

  it("still rejects a non-numeric argument, naming the field", () => {
    const { stderr } = runWithStderr('let r = Rect("a", 0, 0, 0)');
    expect(stderr).toMatch(/Rect/);
    expect(stderr).toMatch(/`x`/);
    expect(stderr).toMatch(/string/);
  });

  it("computes float geometry without truncating", () => {
    const out = runPetal(
      "let r = Rect(10.5, 20.5, 101.0, 41.0)\n" +
        "print(r.center_x())\nprint(r.center_y())\nprint(r.right())\nprint(r.bottom())"
    );
    expect(out.trim().split("\n")).toEqual(["61.0", "41.0", "111.5", "61.5"]);
  });

  it("insets and offsets float geometry", () => {
    const out = runPetal(
      "let r = Rect(0.5, 0.5, 100.0, 40.0).inset(2.5)\nprint([r.x, r.y, r.w, r.h])\n" +
        "let o = Rect(1, 2, 3, 4).offset(0.5, 0.5)\nprint([o.x, o.y, o.w, o.h])"
    );
    expect(out.trim().split("\n")).toEqual([
      "[3.0, 3.0, 95.0, 35.0]",
      "[1.5, 2.5, 3, 4]",
    ]);
  });

  it("clamps an over-inset float rect at zero, still a float", () => {
    const out = runPetal("let r = Rect(0.0, 0.0, 4.0, 4.0).inset(10.0)\nprint([r.w, r.h])");
    expect(out.trim()).toBe("[0.0, 0.0]");
  });

  it("keeps int geometry integral", () => {
    // `/` on two ints truncates in Petal, and `r.x + r.w / 2` is the
    // documented equivalent of center_x — so an int rect stays an int rect.
    const out = runPetal(
      "let r = Rect(0, 0, 101, 41)\nprint(r.center_x())\nprint(type(r.center_x()))"
    );
    expect(out.trim().split("\n")).toEqual(["50", "int"]);
  });

  // Arity. Every built-in method checks it, so a stray argument is a mistake
  // reported at the call rather than silently ignored. The count in the
  // message is the one written at the call site — the receiver is implicit.
  it("rejects extra arguments to the zero-argument methods", () => {
    for (const call of [
      "center_x(1, 2, 3, 4, 5)",
      "center_y(1)",
      'right("nonsense")',
      "bottom(0)",
    ]) {
      const { stderr } = runWithStderr(`print(Rect(0, 0, 10, 10).${call})`);
      expect(stderr, `Rect(0, 0, 10, 10).${call}`).toMatch(/expects no arguments/);
    }
  });

  it("rejects the wrong argument count to inset / offset", () => {
    const inset = runWithStderr("print(Rect(0, 0, 10, 10).inset(1, 2))");
    expect(inset.stderr).toMatch(/Rect\.inset\(\) expects 1 argument, got 2/);
    const offset = runWithStderr("print(Rect(0, 0, 10, 10).offset(1))");
    expect(offset.stderr).toMatch(/Rect\.offset\(\) expects 2 arguments, got 1/);
  });

  it("rejects a non-numeric argument to inset / offset", () => {
    const { stderr } = runWithStderr('print(Rect(0, 0, 10, 10).inset("wide"))');
    expect(stderr).toMatch(/Rect.inset/);
    expect(stderr).toMatch(/string/);
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
  // The runtime method table is program-wide, so a module may declare a method
  // on a built-in class and an importer may extend an imported one. The class
  // *name* — constructor and type alike — follows `export`.
  it("dispatches methods declared in an imported module", () => {
    const out = execSync(`${PETAL} run ${FIXTURES}/main.ptl`, {
      encoding: "utf-8",
      timeout: 10000,
    }).trim();
    expect(out.split("\n")).toEqual(["40", "Circle 12", "4"]);
  });

  it("a module-private class is not a type name in an importer", () => {
    const out = checkFileJson(`${FIXTURES}/private-type.ptl`);
    expect(out.error).toBeFalsy();
    expect(out.warnings.map((w: any) => w.message)).toContain(
      "unknown type name `Secret`"
    );
  });

  it("the private class still runs — the annotation is warning-only", () => {
    const out = execSync(`${PETAL} run ${FIXTURES}/private-type.ptl`, {
      encoding: "utf-8",
      timeout: 10000,
    }).trim();
    expect(out).toBe("7");
  });

  it("names both files when two modules declare the same class", () => {
    const stderr = runFileError(`${FIXTURES}/dup/entry.ptl`);
    expect(stderr).toMatch(/class `Dup` is already declared/);
    expect(stderr).toMatch(/dup_a\.ptl/);
    expect(stderr).toMatch(/dup_b\.ptl/);
  });
});

// A class is a top-level, file-scoped declaration. It is hoisted like the type
// name it introduces, and it may not be nested, collide with a built-in type
// name, or leak out of the module that declares it.
describe("class scoping", () => {
  it("hoists the constructor, like the type name", () => {
    const out = runPetal("print(Later(1).a)\nclass Later\n  a: int\nend");
    expect(out.trim()).toBe("1");
  });

  it("hoisting agrees with `check`", () => {
    const out = checkJson("let l = Later(1)\nclass Later\n  a: int\nend\nprint(l.a)");
    expect(out.warnings).toHaveLength(0);
  });

  it("rejects a class declared inside a function", () => {
    const out = checkJsonAllowFail(
      "fn f()\n  class Inner\n    a: int\n  end\n  Inner(1)\nend\nprint(f())"
    );
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/`Inner`/);
    expect(out.errors[0].message).toMatch(/top level/);
  });

  it("a nested class no longer aliases a top-level one of the same name", () => {
    // Before: the inner `Inner` was invisible to the class table but shared the
    // outer's heap tag, so the *outer* class's method ran on it.
    const out = checkJsonAllowFail(
      "class Inner\n  a: int\nend\n" +
        "fn f()\n  class Inner\n    b: int\n    c: int\n  end\n  Inner(1, 2)\nend\n" +
        'fn Inner.who(i: Inner)\n  "outer"\nend\n' +
        "print(f().who())"
    );
    expect(out.error).toBe(true);
    expect(out.errors[0].message).toMatch(/top level/);
  });

  it("rejects a class named after a built-in type", () => {
    for (const name of ["int", "string", "list", "record"]) {
      const out = checkJsonAllowFail(`class ${name}\n  a: any\nend`);
      expect(out.error).toBe(true);
      expect(out.errors[0].message).toMatch(/built-in type name/);
      expect(out.errors[0].message).toContain(name);
    }
  });

  it("allows a class that shadows a built-in function name", () => {
    const out = runPetal("class len\n  a: int\nend\nprint(type(len(1)))");
    expect(out.trim()).toBe("len");
  });
});

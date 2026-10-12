import { describe, it, expect } from "vitest";
import { mkdtempSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import {
  runPetal,
  runPetalError,
  runPetalFile,
  runPetalFileError,
  checkJson,
  checkJsonAllowFail,
  checkStrict,
  showAstJson,
  showIrJson,
  showIrJsonRaw,
  runIr,
  termsByOp,
} from "./helpers";

// Enums through the CLI. See docs/language-guide.md (Enums): a variant is a
// member of its enum (`Color.red`), may carry payload fields and constant
// data, and is read, printed, compared and reflected on through the enum.

const lines = (code: string) => runPetal(code).split("\n");

const COLOR = "enum Color\n  red,\n  green,\n  custom(r, g, b),\nend\n";
const KIND =
  'enum Kind\n  parkway(shape: "route", use: Use.road),\n  park(shape: "area", use: Use.green),\nend\n' +
  "enum Use\n  road,\n  green,\nend\n";
const OP = 'enum Op\n  fight(place, won, tone: "bad"),\n  gift(amount, tone: "good"),\nend\n';

/** Write a module beside an entry file in a fresh directory; returns the entry's path. */
function project(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "petal-enums-"));
  for (const [name, source] of Object.entries(files)) writeFileSync(join(dir, name), source);
  return join(dir, "main.ptl");
}

describe("enum declarations", () => {
  it("parses payload fields and `key: expr` constant data, alone or mixed", () => {
    const ast = showAstJson(OP);
    const variants = ast[0].kind.EnumDecl.variants;
    expect(variants.map((v: any) => v.name)).toEqual(["fight", "gift"]);
    expect(variants[0].fields).toEqual(["place", "won"]);
    expect(variants[0].data.map((d: any) => d.key)).toEqual(["tone"]);
    // A variant with neither omits both.
    const plain = showAstJson(COLOR)[0].kind.EnumDecl.variants[0];
    expect(plain).toEqual({ name: "red" });
  });

  it("a variant name is any identifier, lowercase or uppercase", () => {
    expect(runPetal("enum M\n  lower,\n  Upper,\n  X1(v),\nend\nprint(M.lower, M.Upper, M.X1(1))")).toBe(
      "M.lower M.Upper M.X1(1)"
    );
  });

  it("every variant declares the same data keys", () => {
    const missing = runPetalError("enum K\n  a(s: 1, u: 2),\n  b(s: 1),\n  c(s: 3, u: 4),\nend");
    expect(missing).toMatch(/variant `K\.b` is missing the data key `u`, which `K\.a` declares/);
    const extra = runPetalError("enum K\n  a(s: 1),\n  b(s: 1, w: 2),\n  c(s: 3),\nend");
    expect(extra).toMatch(/variant `K\.b` has the extra data key `w`/);
    // `check` reports it as the compile error it is.
    const checked = checkJsonAllowFail("enum K\n  a(s: 1),\n  b,\nend");
    expect(checked.phase).toBe("compile");
    expect(checked.errors[0].line).toBe(3);
  });

  it("constant data is evaluated once per declaration, not per read or construction", () => {
    const code =
      'fn noisy(v)\n  print("evaluated", v)\n  v\nend\n' +
      'enum Op\n  fight(place, tone: noisy("bad")),\nend\n' +
      'let a = Op.fight("x")\nlet b = Op.fight("y")\nprint(a.tone, b.tone, a.tone)';
    expect(lines(code)).toEqual(["evaluated bad", "bad bad bad"]);
  });

  it("constant data may name a variant of any enum, whichever is declared first", () => {
    expect(runPetal(KIND + "print(Kind.parkway.use, Kind.park.use == Use.green)")).toBe("Use.road true");
  });

  it("constant data cannot read a value the file computes as it runs", () => {
    const code = "fn compute()\n  3\nend\nlet x = compute()\nenum K\n  a(v: x),\nend\nprint(K.a.v)";
    expect(runPetalError(code)).toMatch(/`x` cannot be used in an enum's constant data/);
    expect(checkJsonAllowFail(code).ok).toBe(false);
  });

  it("rejects reserved and duplicate names", () => {
    expect(runPetalError("enum E\n  all,\nend")).toMatch(/a variant may not be named `all`/);
    expect(runPetalError("enum E\n  from,\nend")).toMatch(/a variant may not be named `from`/);
    expect(runPetalError("enum E\n  a(name),\nend")).toMatch(/may not have a payload field named `name`/);
    expect(runPetalError("enum E\n  a(name: 1),\nend")).toMatch(/may not have a data key named `name`/);
    expect(runPetalError("enum E\n  a(x, x: 1),\nend")).toMatch(/variant `E\.a` declares `x` twice/);
    expect(runPetalError("enum E\n  a,\n  a,\nend")).toMatch(/enum `E` declares the variant `a` twice/);
  });

  it("is hoisted, top-level only, and declared once", () => {
    expect(runPetal("print(Color.red)\n" + COLOR)).toBe("Color.red");
    expect(runPetalError("fn f()\n  enum Inner\n    a,\n  end\nend")).toMatch(
      /enum `Inner` must be declared at the top level of a file/
    );
    expect(runPetalError(COLOR + COLOR)).toMatch(/enum `Color` is already declared/);
    expect(runPetalError("class E\n  x,\nend\nenum E\n  a,\nend")).toMatch(
      /enum `E` has the same name as class `E`/
    );
  });

  it("an enum name is a type name", () => {
    const code = COLOR + "fn keep(c: Color) -> Color\n  c\nend\nprint(keep(Color.red))";
    expect(checkStrict(code).code).toBe(0);
    expect(runPetal(code)).toBe("Color.red");
  });
});

describe("variants are members of their enum", () => {
  it("a variant is a value, a construction, or the constructor function", () => {
    const code =
      COLOR +
      "let make = Color.custom\nprint(Color.red)\nprint(Color.custom(1, 2, 3))\nprint(make(4, 5, 6))";
    expect(lines(code)).toEqual(["Color.red", "Color.custom(1, 2, 3)", "Color.custom(4, 5, 6)"]);
  });

  it("compiles `Enum.variant` straight to the variant", () => {
    const ir = showIrJson(COLOR + "let a = Color.red\nlet b = Color.custom(1, 2, 3)");
    const made = termsByOp(ir, "MakeEnumVariant");
    expect(made).toHaveLength(2);
    expect(made[1].inputs).toHaveLength(3);
    // The declarations are recorded on the program.
    expect(ir.enums.Color.variants.map((v: any) => v.name)).toEqual(["red", "green", "custom"]);
    expect(ir.enums.Color.variants[2].fields).toEqual(["r", "g", "b"]);
  });

  it("a bare variant is an undefined name, with a did-you-mean", () => {
    expect(runPetalError(COLOR + "print(red)")).toMatch(/Undefined variable: red — did you mean Color\.red\?/);
    expect(runPetalError(COLOR + "print(custom(1, 2, 3))")).toMatch(
      /Unknown function: custom — did you mean Color\.custom\?/
    );
    const checked = checkJsonAllowFail(COLOR + "print(red)");
    expect(checked.ok).toBe(false);
    expect(JSON.stringify(checked)).toMatch(/undefined variable `red` — did you mean Color\.red\?/);
  });

  it("an unknown variant of a known enum is a compile error, with a did-you-mean", () => {
    expect(runPetalError(COLOR + "print(Color.rde)")).toMatch(
      /enum `Color` has no variant `rde` — did you mean Color\.red\?/
    );
    // Even where the line never runs.
    expect(runPetalError(COLOR + "fn unused()\n  Color.rde\nend")).toMatch(/has no variant `rde`/);
    const checked = checkJsonAllowFail(COLOR + "print(Color.rde)");
    expect(checked.phase).toBe("compile");
    expect(checked.errors[0].line).toBe(6);
  });

  it("a construction is held to the payload the variant declares", () => {
    expect(runPetalError(COLOR + "print(Color.custom(1, 2))")).toMatch(
      /Color\.custom takes 3 arguments \(r, g, b\), got 2/
    );
    expect(runPetalError(COLOR + "print(Color.red(1))")).toMatch(/Color\.red has no payload/);
  });

  it("is exported and imported by the enum's name", () => {
    const shapes = "pub enum Shape\n  circle(r),\n  dot,\nend\nenum Hidden\n  h,\nend\n";
    const qualified = project({
      "shapes.ptl": shapes,
      "main.ptl": "import ./shapes as s\nprint(s.Shape.dot, s.Shape.circle(2), s.Shape.all)",
    });
    expect(runPetalFile(qualified)).toBe("Shape.dot Shape.circle(2) [Shape.dot]");
    const selective = project({
      "shapes.ptl": shapes,
      "main.ptl":
        "import ./shapes: Shape\nprint(match Shape.circle(3)\n  when Shape.circle(r) -> r\n  when Shape.dot -> 0\nend)",
    });
    expect(runPetalFile(selective)).toBe("3");
    const priv = project({ "shapes.ptl": shapes, "main.ptl": "import ./shapes as s\nprint(s.Hidden.h)" });
    expect(runPetalFileError(priv)).toMatch(/has no export 'Hidden'/);
    const dup = project({ "shapes.ptl": shapes, "main.ptl": "import ./shapes\nenum Shape\n  a,\nend" });
    expect(runPetalFileError(dup)).toMatch(/enum `Shape` is already declared in `.*shapes.*`/);
  });

  it("two enums may declare one variant name; equality is enum, variant and payload", () => {
    const code =
      "enum Kind\n  park,\nend\nenum Use\n  park,\nend\n" +
      COLOR +
      "print(Kind.park == Kind.park, Kind.park == Use.park, Kind.park != Use.park)\n" +
      "print(Color.custom(1, 2, 3) == Color.custom(1, 2, 3), Color.custom(1, 2, 3) == Color.custom(1, 2, 4))";
    expect(lines(code)).toEqual(["true false true", "true false"]);
    // The collisions `check` used to warn about no longer exist.
    expect(checkJson(code).warnings).toEqual([]);
    expect(checkJson("let red = 5\nenum Color\n  red,\nend\nprint(red, Color.red)").warnings).toEqual([]);
  });
});

describe("reading a variant", () => {
  it("reads constant data, payload fields by name, and `.name`", () => {
    const code =
      KIND +
      OP +
      'let k = Kind.park\nlet op = Op.fight("bridge", true)\n' +
      "print(k.shape, k.use, k.name)\nprint(op.place, op.won, op.tone, op.name)";
    expect(lines(code)).toEqual(["area Use.green park", "bridge true bad fight"]);
  });

  it("an unknown field is an error, as on a record", () => {
    expect(runPetalError(KIND + "print(Kind.park.colour)")).toMatch(
      /No field 'colour' on enum variant Kind\.park/
    );
    expect(runPetal(KIND + 'print(Kind.park.colour ?? "none")')).toBe("none");
  });

  it("`type` is the enum's name; `str` and `print` show the qualified variant", () => {
    const code =
      KIND +
      OP +
      'print(type(Kind.park), type(Op.gift(1)))\nprint(str(Kind.park), Op.fight("x", true))\nprint("{Kind.park}")';
    expect(lines(code)).toEqual(["Kind Op", 'Kind.park Op.fight("x", true)', "Kind.park"]);
  });
});

describe("Enum.all and Enum.from", () => {
  it("`all` lists the payload-free variants in declaration order", () => {
    expect(runPetal(KIND + COLOR + "print(Kind.all, Color.all)")).toBe(
      "[Kind.parkway, Kind.park] [Color.red, Color.green]"
    );
  });

  it("`from` is the payload-free variant of that name, or nil", () => {
    const code =
      KIND +
      COLOR +
      'print(Kind.from("park"), Kind.from("lake"), Color.from("custom"))\n' +
      'print(Kind.from("parkway").shape, map(["park", "x"], Kind.from))';
    expect(lines(code)).toEqual(["Kind.park nil nil", "route [Kind.park, nil]"]);
  });
});

describe("qualified variant patterns", () => {
  it("match a variant through its enum, binding payload fields", () => {
    const code =
      KIND +
      OP +
      "fn describe(v)\n  match v\n" +
      '    when Kind.park -> "a park"\n' +
      '    when Op.fight(place, true) -> "won at {place}"\n' +
      '    when Op.fight(place, _) -> "lost at {place}"\n' +
      '    when _ -> "other"\n  end\nend\n' +
      'print(describe(Kind.park), describe(Op.fight("x", true)), describe(Op.fight("y", false)), describe(Use.road))';
    expect(runPetal(code)).toBe("a park won at x lost at y other");
  });

  it("a same-named variant of another enum does not match", () => {
    const code =
      "enum Kind\n  park,\nend\nenum Use\n  park,\nend\n" +
      'print(match Use.park\n  when Kind.park -> "kind"\n  when Use.park -> "use"\nend)';
    expect(runPetal(code)).toBe("use");
  });

  it("a bare identifier is a binding, and a bare call-shaped pattern is an error", () => {
    expect(runPetal(COLOR + "print(match Color.green\n  when red -> red\nend)")).toBe("Color.green");
    expect(runPetalError(COLOR + "match Color.red\n  when custom(r, g, b) -> 1\nend")).toMatch(
      /`custom\(…\)` is not a pattern.*did you mean Color\.custom\?/
    );
  });

  it("a pattern naming no variant, or the wrong payload, is a compile error", () => {
    expect(runPetalError(COLOR + "match Color.red\n  when Color.rde -> 1\nend")).toMatch(
      /has no variant `rde` — did you mean Color\.red\?/
    );
    expect(runPetalError(COLOR + "match Color.red\n  when Colour.red -> 1\nend")).toMatch(
      /`Colour` is not an enum in scope/
    );
    expect(runPetalError(COLOR + "match Color.red\n  when Color.custom(r) -> 1\nend")).toMatch(
      /the pattern `Color\.custom\(…\)` has 1 field, but Color\.custom has 3 payload fields \(r, g, b\)/
    );
  });
});

describe("enums in the IR", () => {
  it("a program with enums runs the same from its JSON IR", () => {
    const code =
      KIND +
      OP +
      'let op = Op.fight("bridge", true)\n' +
      "print(op, op.place, op.tone, Kind.park.shape, Kind.all, Kind.from(\"park\"))\n" +
      'print(match op\n  when Op.fight(p, _) -> p\n  when _ -> "?"\nend)';
    expect(runIr(showIrJsonRaw(code))).toBe(runPetal(code));
  });
});

import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("lists", () => {
  it("emits AllocList with correct input count", () => {
    const ir = showIrJson("let xs = [1, 2, 3]");
    const allocs = termsByOp(ir, "AllocList");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].inputs).toHaveLength(3);
  });

  it("AllocList with empty list has 0 inputs", () => {
    const ir = showIrJson("let xs = []");
    const allocs = termsByOp(ir, "AllocList");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].inputs).toHaveLength(0);
  });
});

describe("records", () => {
  it("emits AllocMap with field constants", () => {
    const ir = showIrJson('let r = { name: "Alice", age: 30 }');
    const allocs = termsByOp(ir, "AllocMap");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    const allocMap = allocs[0];
    expect(allocMap.op.AllocMap.fields).toHaveLength(2);
    // inputs should match number of fields
    expect(allocMap.inputs).toHaveLength(2);
  });

  it("field names are stored as constant IDs", () => {
    const ir = showIrJson('let r = { x: 1 }');
    const allocs = termsByOp(ir, "AllocMap");
    const fieldIds = allocs[0].op.AllocMap.fields;
    // Each field ID should reference a string constant
    for (const cid of fieldIds) {
      const constVal = ir.constants.values[cid];
      expect(constVal).toEqual({ String: expect.any(String) });
    }
  });
});

describe("field access", () => {
  it("emits GetField for dot access", () => {
    const ir = showIrJson('let r = { x: 1 }\nlet v = r.x');
    const gets = termsByOp(ir, "GetField");
    expect(gets.length).toBeGreaterThanOrEqual(1);
    expect(gets[0].inputs).toHaveLength(1);
  });

  it("emits SetField for field assignment", () => {
    const ir = showIrJson('let r = { x: 1 }\nr.x = 2');
    const sets = termsByOp(ir, "SetField");
    expect(sets.length).toBeGreaterThanOrEqual(1);
  });

  it("field mutation persists", () => {
    expect(runPetal('let r = { x: 1, y: 2 }\nr.x = 99\nprint(r.x, r.y)')).toBe("99 2");
  });

  it("field mutation preserves other fields", () => {
    expect(
      runPetal('let r = { a: 1, b: 2, c: 3 }\nr.b = 20\nprint(r)'),
    ).toBe("{ a: 1, b: 20, c: 3 }");
  });

  it("field mutation on record inside a list", () => {
    expect(
      runPetal(
        'let pts = [{x: 1}, {x: 2}, {x: 3}]\npts[1].x = 99\nprint(pts[0].x, pts[1].x, pts[2].x)',
      ),
    ).toBe("1 99 3");
  });

  it("nested field mutation", () => {
    expect(
      runPetal('let r = { inner: { a: 1, b: 2 } }\nr.inner.a = 42\nprint(r.inner.a, r.inner.b)'),
    ).toBe("42 2");
  });

  it("field mutation uses right-hand expression", () => {
    expect(
      runPetal('let r = { count: 5 }\nr.count = r.count + 1\nprint(r.count)'),
    ).toBe("6");
  });
});

describe("index access", () => {
  it("emits GetIndex for bracket access", () => {
    const ir = showIrJson("let xs = [1,2,3]\nlet v = xs[0]");
    const gets = termsByOp(ir, "GetIndex");
    expect(gets.length).toBeGreaterThanOrEqual(1);
    // inputs: [object, index]
    expect(gets[0].inputs).toHaveLength(2);
  });

  it("emits SetIndex for index assignment", () => {
    const ir = showIrJson("let xs = [1,2,3]\nxs[0] = 99");
    const sets = termsByOp(ir, "SetIndex");
    expect(sets.length).toBeGreaterThanOrEqual(1);
  });

  it("index mutation persists", () => {
    expect(runPetal("let xs = [1, 2, 3]\nxs[1] = 99\nprint(xs)")).toBe("[1, 99, 3]");
  });
});

describe("enums", () => {
  it("emits MakeEnumVariant for enum construction", () => {
    const ir = showIrJson(
      "enum Color { Red, Green, Blue }\nlet c = Color.Red()"
    );
    const variants = termsByOp(ir, "MakeEnumVariant");
    expect(variants.length).toBeGreaterThanOrEqual(1);
  });
});

describe("concat operator (++)", () => {
  it("emits Concat for ++", () => {
    const ir = showIrJson('let s = "hello" ++ " world"');
    const concats = termsByOp(ir, "Concat");
    expect(concats.length).toBeGreaterThanOrEqual(1);
    expect(concats[0].inputs).toHaveLength(2);
  });

  it("concatenates two lists", () => {
    expect(runPetal("print([1, 2] ++ [3, 4])")).toBe("[1, 2, 3, 4]");
  });

  it("concatenates empty list with non-empty", () => {
    expect(runPetal("print([] ++ [1, 2])")).toBe("[1, 2]");
  });

  it("concatenates non-empty list with empty", () => {
    expect(runPetal("print([1, 2] ++ [])")).toBe("[1, 2]");
  });

  it("concatenates strings", () => {
    expect(runPetal('print("hello" ++ " world")')).toBe("hello world");
  });

  it("converts non-string to string when one side is string", () => {
    expect(runPetal('print("count: " ++ 42)')).toBe("count: 42");
  });
});

describe("color literals", () => {
  it("emits AllocMap with 3 fields for #rgb", () => {
    const ir = showIrJson("let c = #f80");
    const allocs = termsByOp(ir, "AllocMap");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].op.AllocMap.fields).toHaveLength(3);
  });

  it("emits AllocMap with 4 fields for #rgba", () => {
    const ir = showIrJson("let c = #f80a");
    const allocs = termsByOp(ir, "AllocMap");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].op.AllocMap.fields).toHaveLength(4);
  });

  it("emits AllocMap with 3 fields for #rrggbb", () => {
    const ir = showIrJson("let c = #ff8800");
    const allocs = termsByOp(ir, "AllocMap");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].op.AllocMap.fields).toHaveLength(3);
  });

  it("emits AllocMap with 4 fields for #rrggbbaa", () => {
    const ir = showIrJson("let c = #ff8800aa");
    const allocs = termsByOp(ir, "AllocMap");
    expect(allocs.length).toBeGreaterThanOrEqual(1);
    expect(allocs[0].op.AllocMap.fields).toHaveLength(4);
  });

  it("#rgb produces correct r, g, b values", () => {
    expect(runPetal("let c = #f80\nprint(c.r, c.g, c.b)")).toBe("255 136 0");
  });

  it("#rgba produces correct r, g, b, a values", () => {
    expect(runPetal("let c = #f80a\nprint(c.r, c.g, c.b, c.a)")).toBe("255 136 0 170");
  });

  it("#rrggbb produces correct values", () => {
    expect(runPetal("let c = #ff8800\nprint(c.r, c.g, c.b)")).toBe("255 136 0");
  });

  it("#rrggbbaa produces correct values", () => {
    expect(runPetal("let c = #ff8800aa\nprint(c.r, c.g, c.b, c.a)")).toBe("255 136 0 170");
  });

  it("#000000 produces all zeros", () => {
    expect(runPetal("let c = #000000\nprint(c.r, c.g, c.b)")).toBe("0 0 0");
  });

  it("#ffffff produces all 255s", () => {
    expect(runPetal("let c = #ffffff\nprint(c.r, c.g, c.b)")).toBe("255 255 255");
  });

  it("case insensitive hex digits", () => {
    expect(runPetal("let c = #FF8800\nprint(c.r, c.g, c.b)")).toBe("255 136 0");
  });
});

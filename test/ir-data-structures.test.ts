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

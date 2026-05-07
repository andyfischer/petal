import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, showIrJson, userTerms } from "./helpers";

beforeAll(() => ensureBuild());

describe("range() single argument", () => {
  it("range(n) produces 0..n", () => {
    const out = runPetal("print(range(5))");
    expect(out.trim()).toBe("[0, 1, 2, 3, 4]");
  });

  it("range(0) produces empty list", () => {
    const out = runPetal("print(range(0))");
    expect(out.trim()).toBe("[]");
  });

  it("range(start, end) still works", () => {
    const out = runPetal("print(range(2, 5))");
    expect(out.trim()).toBe("[2, 3, 4]");
  });

  it("range(n) works in for loops", () => {
    const out = runPetal(`
      for x in range(3) {
        print(x)
      }
    `);
    expect(out.trim()).toBe("0\n1\n2");
  });
});

describe(".length field access", () => {
  it("list.length returns element count", () => {
    const out = runPetal("print([1, 2, 3].length)");
    expect(out.trim()).toBe("3");
  });

  it("empty list .length is 0", () => {
    const out = runPetal("print([].length)");
    expect(out.trim()).toBe("0");
  });

  it("string.length returns character count", () => {
    const out = runPetal('print("hello".length)');
    expect(out.trim()).toBe("5");
  });

  it("empty string .length is 0", () => {
    const out = runPetal('print("".length)');
    expect(out.trim()).toBe("0");
  });
});

describe(".includes() method", () => {
  it("returns true when element is present", () => {
    const out = runPetal("print([1, 2, 3].includes(2))");
    expect(out.trim()).toBe("true");
  });

  it("returns false when element is absent", () => {
    const out = runPetal("print([1, 2, 3].includes(5))");
    expect(out.trim()).toBe("false");
  });

  it("works on strings", () => {
    const out = runPetal('print("hello world".includes("world"))');
    expect(out.trim()).toBe("true");
  });
});

describe(".forEach() method", () => {
  it("calls function for each element", () => {
    const out = runPetal("[10, 20, 30].forEach(fn(x) { print(x) })");
    expect(out.trim()).toBe("10\n20\n30");
  });

  it("returns nil", () => {
    const out = runPetal("let result = [1].forEach(fn(x) { x }); print(result)");
    expect(out.trim()).toBe("nil");
  });
});

describe("JS idiom error hints", () => {
  it("console suggests print()", () => {
    const ir = showIrJson('console.log("hi")');
    const terms = userTerms(ir);
    const errorTerm = terms.find((t: any) => typeof t.op === "object" && "Error" in t.op);
    expect(errorTerm).toBeDefined();
    const errorMsg = ir.constants.values[errorTerm.op.Error];
    expect(errorMsg.String).toContain("print()");
  });

  it("null suggests nil", () => {
    const ir = showIrJson("let x = null");
    const terms = userTerms(ir);
    const errorTerm = terms.find((t: any) => typeof t.op === "object" && "Error" in t.op);
    expect(errorTerm).toBeDefined();
    const errorMsg = ir.constants.values[errorTerm.op.Error];
    expect(errorMsg.String).toContain("nil");
  });

  it("typeof suggests type()", () => {
    const ir = showIrJson("typeof 5");
    const terms = userTerms(ir);
    const errorTerm = terms.find((t: any) => typeof t.op === "object" && "Error" in t.op);
    expect(errorTerm).toBeDefined();
    const errorMsg = ir.constants.values[errorTerm.op.Error];
    expect(errorMsg.String).toContain("type()");
  });
});

import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  userTerms,
  termByName,
  termsByOp,
  BUILTIN_COUNT,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("constants", () => {
  it("interns integer constants", () => {
    const ir = showIrJson("let x = 42");
    const vals = ir.constants.values;
    expect(vals).toContainEqual({ Int: 42 });
  });

  it("interns string constants", () => {
    const ir = showIrJson('let s = "hello"');
    const vals = ir.constants.values;
    expect(vals).toContainEqual({ String: "hello" });
  });

  it("interns nil constant", () => {
    const ir = showIrJson("let n = nil");
    const vals = ir.constants.values;
    expect(vals).toContainEqual("Nil");
  });

  it("interns bool constants", () => {
    const ir = showIrJson("let a = true\nlet b = false");
    const vals = ir.constants.values;
    expect(vals).toContainEqual({ Bool: true });
    expect(vals).toContainEqual({ Bool: false });
  });

  it("deduplicates identical constants", () => {
    const ir = showIrJson("let a = 5\nlet b = 5");
    const fives = ir.constants.values.filter(
      (v: any) => typeof v === "object" && v.Int === 5
    );
    expect(fives).toHaveLength(1);
  });
});

describe("arithmetic terms", () => {
  it("emits Add term for +", () => {
    const ir = showIrJson("let x = 1 + 2");
    const adds = termsByOp(ir, "Add");
    expect(adds.length).toBeGreaterThanOrEqual(1);
    expect(adds[0].inputs).toHaveLength(2);
  });

  it("emits Sub term for -", () => {
    const ir = showIrJson("let x = 5 - 3");
    expect(termsByOp(ir, "Sub").length).toBeGreaterThanOrEqual(1);
  });

  it("emits Mul term for *", () => {
    const ir = showIrJson("let x = 2 * 3");
    expect(termsByOp(ir, "Mul").length).toBeGreaterThanOrEqual(1);
  });

  it("emits Div term for /", () => {
    const ir = showIrJson("let x = 10 / 2");
    expect(termsByOp(ir, "Div").length).toBeGreaterThanOrEqual(1);
  });

  it("emits Mod term for %", () => {
    const ir = showIrJson("let x = 10 % 3");
    expect(termsByOp(ir, "Mod").length).toBeGreaterThanOrEqual(1);
  });
});

describe("variables and registers", () => {
  it("assigns sequential registers to user terms", () => {
    const ir = showIrJson("let a = 1\nlet b = 2\nlet c = 3");
    const ut = userTerms(ir);
    const regs = ut.map((t: any) => t.register);
    // Registers should be monotonically increasing
    for (let i = 1; i < regs.length; i++) {
      expect(regs[i]).toBeGreaterThan(regs[i - 1]);
    }
  });

  it("gives named terms their variable name", () => {
    const ir = showIrJson("let x = 42");
    const x = termByName(ir, "x");
    expect(x).toBeDefined();
    expect(x.op).toEqual({ Constant: expect.any(Number) });
  });

  it("emits Copy for variable references", () => {
    const ir = showIrJson("let x = 1\nlet y = x");
    const y = termByName(ir, "y");
    expect(y).toBeDefined();
    expect(y.op).toBe("Copy");
    expect(y.inputs).toHaveLength(1);
  });

  it("root block has correct root_block id", () => {
    const ir = showIrJson("let x = 1");
    expect(ir.root_block).toBe(0);
    const rootBlock = ir.blocks.find((b: any) => b.id === 0);
    expect(rootBlock).toBeDefined();
    expect(rootBlock.parent_term_id).toBeNull();
  });

  it("root block register_count covers all terms", () => {
    const ir = showIrJson("let a = 1\nlet b = 2");
    const rootBlock = ir.blocks.find((b: any) => b.id === ir.root_block);
    // Must be at least BUILTIN_COUNT + number of user terms
    const ut = userTerms(ir);
    expect(rootBlock.register_count).toBeGreaterThanOrEqual(
      BUILTIN_COUNT + ut.length
    );
  });
});

describe("comparison terms", () => {
  it("emits Eq for ==", () => {
    const ir = showIrJson("let x = 1 == 2");
    expect(termsByOp(ir, "Eq").length).toBeGreaterThanOrEqual(1);
  });

  it("emits Lt for <", () => {
    const ir = showIrJson("let x = 1 < 2");
    expect(termsByOp(ir, "Lt").length).toBeGreaterThanOrEqual(1);
  });
});

describe("unary terms", () => {
  it("emits Neg for unary minus", () => {
    const ir = showIrJson("let x = -5");
    expect(termsByOp(ir, "Neg").length).toBeGreaterThanOrEqual(1);
  });

  it("emits Not for !", () => {
    const ir = showIrJson("let x = !true");
    expect(termsByOp(ir, "Not").length).toBeGreaterThanOrEqual(1);
  });
});

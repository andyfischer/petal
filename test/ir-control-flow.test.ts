import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("if/else (Branch)", () => {
  it("emits Branch with 2 child_blocks for if/else", () => {
    const ir = showIrJson("let x = if true { 1 } else { 2 }");
    const branches = termsByOp(ir, "Branch");
    expect(branches.length).toBeGreaterThanOrEqual(1);
    expect(branches[0].child_blocks).toHaveLength(2);
  });

  it("Branch inputs include condition", () => {
    const ir = showIrJson("let x = if true { 1 } else { 2 }");
    const branch = termsByOp(ir, "Branch")[0];
    expect(branch.inputs).toHaveLength(1);
  });

  it("child blocks have parent_term_id pointing to Branch", () => {
    const ir = showIrJson("let x = if true { 1 } else { 2 }");
    const branch = termsByOp(ir, "Branch")[0];
    for (const blockId of branch.child_blocks) {
      const block = ir.blocks.find((b: any) => b.id === blockId);
      expect(block).toBeDefined();
      expect(block.parent_term_id).toBe(branch.id);
    }
  });
});

describe("for loops", () => {
  it("emits ForLoop with 1 child_block", () => {
    const ir = showIrJson("for i in [1, 2, 3] { i }");
    const loops = termsByOp(ir, "ForLoop");
    expect(loops.length).toBeGreaterThanOrEqual(1);
    expect(loops[0].child_blocks).toHaveLength(1);
  });

  it("ForLoop body block has loop variable as param", () => {
    const ir = showIrJson("for i in [1, 2, 3] { i }");
    const loop_ = termsByOp(ir, "ForLoop")[0];
    const bodyBlock = ir.blocks.find(
      (b: any) => b.id === loop_.child_blocks[0]
    );
    expect(bodyBlock).toBeDefined();
    expect(bodyBlock.param_names).toContain("i");
  });

  it("ForLoop inputs include iterable", () => {
    const ir = showIrJson("for i in [1, 2, 3] { i }");
    const loop_ = termsByOp(ir, "ForLoop")[0];
    expect(loop_.inputs).toHaveLength(1);
  });
});

describe("while loops", () => {
  it("emits WhileLoop with 2 child_blocks", () => {
    const ir = showIrJson("let x = 0\nwhile x < 5 { x = x + 1 }");
    const loops = termsByOp(ir, "WhileLoop");
    expect(loops.length).toBeGreaterThanOrEqual(1);
    expect(loops[0].child_blocks).toHaveLength(2);
  });
});

describe("match", () => {
  it("emits Match term", () => {
    const ir = showIrJson('let x = 1\nmatch x {\n  1 -> "one"\n  _ -> "other"\n}');
    const matches = termsByOp(ir, "Match");
    expect(matches.length).toBeGreaterThanOrEqual(1);
  });

  it("Match has child_blocks for each arm", () => {
    const ir = showIrJson('let x = 1\nmatch x {\n  1 -> "one"\n  2 -> "two"\n  _ -> "other"\n}');
    const match_ = termsByOp(ir, "Match")[0];
    // At least 3 child blocks for 3 arms
    expect(match_.child_blocks.length).toBeGreaterThanOrEqual(3);
  });

  it("Match inputs include subject", () => {
    const ir = showIrJson('let x = 1\nmatch x {\n  1 -> "one"\n  _ -> "other"\n}');
    const match_ = termsByOp(ir, "Match")[0];
    expect(match_.inputs).toHaveLength(1);
  });
});

describe("short-circuit operators", () => {
  it("emits And with child_block for &&", () => {
    const ir = showIrJson("let x = true && false");
    const ands = termsByOp(ir, "And");
    expect(ands.length).toBeGreaterThanOrEqual(1);
    expect(ands[0].child_blocks).toHaveLength(1);
  });

  it("emits Or with child_block for ||", () => {
    const ir = showIrJson("let x = false || true");
    const ors = termsByOp(ir, "Or");
    expect(ors.length).toBeGreaterThanOrEqual(1);
    expect(ors[0].child_blocks).toHaveLength(1);
  });
});

describe("break and return", () => {
  it("emits Break inside loop", () => {
    const ir = showIrJson("for i in [1,2,3] { if i == 2 { break } }");
    const breaks = termsByOp(ir, "Break");
    expect(breaks.length).toBeGreaterThanOrEqual(1);
  });

  it("emits Return inside function", () => {
    const ir = showIrJson("fn f() { return 1 }");
    const returns = termsByOp(ir, "Return");
    expect(returns.length).toBeGreaterThanOrEqual(1);
  });
});

describe("continue", () => {
  it("emits Continue inside for loop", () => {
    const ir = showIrJson("for i in [1,2,3] { if i == 2 { continue } }");
    const continues = termsByOp(ir, "Continue");
    expect(continues.length).toBeGreaterThanOrEqual(1);
  });

  it("continue skips rest of for-loop iteration", () => {
    const result = runPetal(`
      let result = []
      for i in [1, 2, 3, 4, 5] {
        if i == 3 { continue }
        push(result, i)
      }
      print(result)
    `);
    expect(result).toBe("[1, 2, 4, 5]");
  });

  it("continue works in while loops", () => {
    const result = runPetal(`
      let i = 0
      let result = []
      while i < 5 {
        i = i + 1
        if i == 3 { continue }
        push(result, i)
      }
      print(result)
    `);
    expect(result).toBe("[1, 2, 4, 5]");
  });

  it("continue in nested loops only affects inner loop", () => {
    const result = runPetal(`
      let result = []
      for i in [1, 2] {
        for j in [10, 20, 30] {
          if j == 20 { continue }
          push(result, i * 100 + j)
        }
      }
      print(result)
    `);
    expect(result).toBe("[110, 130, 210, 230]");
  });
});

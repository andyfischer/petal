import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("state keyword", () => {
  it("emits StateInit for state declaration", () => {
    const ir = showIrJson("state count = 0");
    const inits = termsByOp(ir, "StateInit");
    expect(inits.length).toBeGreaterThanOrEqual(1);
  });

  it("StateInit has state_key set", () => {
    const ir = showIrJson("state count = 0");
    const init = termsByOp(ir, "StateInit")[0];
    expect(init.state_key).not.toBeNull();
  });

  it("StateInit has its init expression in a child block (lazy init)", () => {
    // The init expression is compiled into a child block so it only runs
    // on the first encounter of the runtime state key. StateInit's `inputs`
    // hold only the explicit key (if any), not the init value.
    const ir = showIrJson("state count = 0");
    const init = termsByOp(ir, "StateInit")[0];
    expect(init.inputs).toHaveLength(0);
    expect(init.child_blocks).toHaveLength(1);
    const initBlock = ir.blocks.find((b: any) => b.id === init.child_blocks[0]);
    expect(initBlock).toBeDefined();
    expect(initBlock.entry).not.toBeNull();
  });

  it("state assignment emits StateWrite", () => {
    const ir = showIrJson("state count = 0\ncount = 5");
    const writes = termsByOp(ir, "StateWrite");
    expect(writes.length).toBeGreaterThanOrEqual(1);
  });

  it("StateWrite has state_key set", () => {
    const ir = showIrJson("state count = 0\ncount = 5");
    const write = termsByOp(ir, "StateWrite")[0];
    expect(write.state_key).not.toBeNull();
  });

  it("state reference produces Copy of StateInit", () => {
    const ir = showIrJson("state count = 0\nlet x = count");
    // In the same scope, referencing state produces a Copy pointing at the StateInit term
    const x = ir.terms.find((t: any) => t.name === "x");
    expect(x).toBeDefined();
    expect(x.op).toBe("Copy");
    const source = ir.terms.find((t: any) => t.id === x.inputs[0]);
    expect(source.op).toBe("StateInit");
  });
});

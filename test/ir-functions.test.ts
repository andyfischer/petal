import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("function definitions", () => {
  it("creates a FunctionDef for fn declarations", () => {
    const ir = showIrJson("fn add(a, b) { a + b }");
    expect(ir.functions).toHaveLength(1);
    expect(ir.functions[0].name).toBe("add");
    expect(ir.functions[0].params).toEqual(["a", "b"]);
  });

  it("assigns a body_block to the function", () => {
    const ir = showIrJson("fn greet() { 1 }");
    const func = ir.functions[0];
    expect(func.body_block).toBeGreaterThan(0);
    const bodyBlock = ir.blocks.find((b: any) => b.id === func.body_block);
    expect(bodyBlock).toBeDefined();
  });

  it("emits MakeClosure in root block", () => {
    const ir = showIrJson("fn f() { 1 }");
    const closures = termsByOp(ir, "MakeClosure");
    expect(closures.length).toBeGreaterThanOrEqual(1);
    const mc = closures[0];
    expect(typeof mc.op).toBe("object");
    expect(mc.op.MakeClosure).toBe(ir.functions[0].id);
  });

  it("sets self_ref_register for recursive functions", () => {
    const ir = showIrJson("fn fib(n) { if n < 2 { n } else { fib(n-1) + fib(n-2) } }");
    const func = ir.functions[0];
    expect(func.self_ref_register).not.toBeNull();
  });
});

describe("closures and captures", () => {
  it("populates capture_names for closures", () => {
    const ir = showIrJson("let x = 10\nfn get_x() { x }");
    const func = ir.functions.find((f: any) => f.name === "get_x");
    expect(func).toBeDefined();
    expect(func.capture_names).toContain("x");
  });

  it("has capture_registers matching capture_names length", () => {
    const ir = showIrJson("let a = 1\nlet b = 2\nfn f() { a + b }");
    const func = ir.functions.find((f: any) => f.name === "f");
    expect(func.capture_registers).toHaveLength(func.capture_names.length);
  });

  it("MakeClosure inputs correspond to captured values", () => {
    const ir = showIrJson("let x = 10\nfn get_x() { x }");
    const closures = termsByOp(ir, "MakeClosure");
    const mc = closures.find((t: any) => {
      const fid = t.op.MakeClosure;
      return ir.functions[fid]?.name === "get_x";
    });
    expect(mc).toBeDefined();
    // Should have 1 input (the captured x)
    expect(mc.inputs.length).toBeGreaterThanOrEqual(1);
  });
});

describe("lambdas", () => {
  it("creates a FunctionDef with null name for lambdas", () => {
    const ir = showIrJson("let f = fn(x) { x + 1 }");
    const lambda = ir.functions.find((f: any) => f.name === null);
    expect(lambda).toBeDefined();
    expect(lambda.params).toEqual(["x"]);
  });
});

describe("function calls", () => {
  it("emits Call term", () => {
    const ir = showIrJson("fn f() { 1 }\nf()");
    const calls = termsByOp(ir, "Call");
    expect(calls.length).toBeGreaterThanOrEqual(1);
    // Call inputs: [callable, ...args]
    expect(calls[0].inputs.length).toBeGreaterThanOrEqual(1);
  });

  it("Call with arguments has correct input count", () => {
    const ir = showIrJson("fn add(a, b) { a + b }\nadd(1, 2)");
    const calls = termsByOp(ir, "Call");
    expect(calls.length).toBeGreaterThanOrEqual(1);
    // callable + 2 args = 3 inputs
    const call = calls[calls.length - 1];
    expect(call.inputs).toHaveLength(3);
  });
});

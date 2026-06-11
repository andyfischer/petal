import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  showIrJsonRaw,
  runPetal,
  runIr,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

/** Resolve the string constant a ConstantId-carrying op points at. */
function constString(ir: any, cid: number): any {
  return ir.constants.values[cid];
}

describe("BuiltinCall static dispatch", () => {
  it("compiles a bare unshadowed builtin call to BuiltinCall, not Call", () => {
    const ir = showIrJson('print("hello")');
    const builtinCalls = termsByOp(ir, "BuiltinCall");
    expect(builtinCalls.length).toBe(1);
    // No dynamic Call should remain for the bare builtin invocation.
    expect(termsByOp(ir, "Call").length).toBe(0);
    // The op carries the builtin name as a String constant.
    const cid = builtinCalls[0].op.BuiltinCall;
    expect(constString(ir, cid)).toEqual({ String: "print" });
    // BuiltinCall inputs are the args only (no callable input).
    expect(builtinCalls[0].inputs.length).toBe(1);
  });

  it("still runs correctly via BuiltinCall", () => {
    expect(runPetal('print("hello")')).toBe("hello");
    expect(runPetal("print(len([1, 2, 3]))")).toBe("3");
  });

  it("does NOT use BuiltinCall when the name is shadowed by a user fn", () => {
    const code = `fn print(x)\n  x + 1\nend\nprint(2)`;
    const ir = showIrJson(code);
    // Shadowed: must dynamic-dispatch through a normal Call.
    expect(termsByOp(ir, "BuiltinCall").length).toBe(0);
    expect(termsByOp(ir, "Call").length).toBeGreaterThanOrEqual(1);
    // And the user fn semantics win.
    expect(runPetal(code)).toBe("");
  });

  it("does NOT use BuiltinCall when the name is shadowed by a let binding", () => {
    const code = `let len = 99\nprint(len)`;
    const ir = showIrJson(code);
    // len(...) is not even called here, but referencing a shadowed builtin
    // as a value must stay a Copy and print() is the only builtin call.
    const builtinCalls = termsByOp(ir, "BuiltinCall");
    // Only print is an unshadowed builtin call.
    for (const bc of builtinCalls) {
      const cid = bc.op.BuiltinCall;
      expect(constString(ir, cid)).toEqual({ String: "print" });
    }
    expect(runPetal(code)).toBe("99");
  });

  it("keeps a value reference (not BuiltinCall) when a builtin is passed by name", () => {
    // Passing `str` by name to map is a value reference, NOT a call site,
    // so it must compile to a Copy of the `str` phantom rather than BuiltinCall.
    const code = `let f = str\nprint(2)`;
    const ir = showIrJson(code);
    // `str` referenced as a value resolves through a (named or unnamed) Copy
    // of the str phantom — not a BuiltinCall.
    const strCalls = termsByOp(ir, "BuiltinCall").filter(
      (bc: any) => constString(ir, bc.op.BuiltinCall)?.String === "str"
    );
    expect(strCalls.length).toBe(0);
  });

  it("round-trips through show-ir --json | run --ir -", () => {
    const raw = showIrJsonRaw('print(1 + 2)');
    expect(runIr(raw)).toBe("3");
  });

  it("handles higher-order builtin called directly (map) via BuiltinCall", () => {
    const code = `let double = fn(x) x * 2 end\nlet xs = map([1, 2, 3], double)\nprint(xs[1])`;
    expect(runPetal(code)).toBe("4");
    const ir = showIrJson(code);
    // map is an unshadowed builtin call site → BuiltinCall.
    const builtinCalls = termsByOp(ir, "BuiltinCall");
    const names = builtinCalls.map((bc: any) =>
      constString(ir, bc.op.BuiltinCall)
    );
    expect(names).toContainEqual({ String: "map" });
  });
});

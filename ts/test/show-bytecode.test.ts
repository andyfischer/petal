// `show-bytecode --json` is the tool-facing view of the lowered bytecode: each
// instruction row carries both a structured operand-level encoding (`inst`, the
// Inst enum's externally-tagged serde form — the shape a tool consumes) and the
// disassembled string (`text` — the shape a human reads). These tests pin the
// row shape and the text-form spacing conventions documented in docs/CLI.md.

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showBytecodeJson, showBytecodeText } from "./helpers";

beforeAll(() => ensureBuild());

const SNIPPET = "let xs = map([1, 2, 3], fn(x) x * 2 end)\nprint(xs)";

/** All instruction rows of every function, flattened. */
function allRows(bc: any): any[] {
  return bc.functions.flatMap((f: any) => f.code);
}

/** The rows whose single inst key is `op`, unwrapped to their operand objects. */
function instsOf(bc: any, op: string): any[] {
  return allRows(bc)
    .filter((row: any) => op in row.inst)
    .map((row: any) => row.inst[op]);
}

describe("show-bytecode --json structure", () => {
  it("emits one object per function with register metadata", () => {
    const bc = showBytecodeJson(SNIPPET);
    expect(bc.functions.length).toBe(2); // root + the lambda
    const [root, lambda] = bc.functions;
    expect(root.fn).toBeNull();
    expect(typeof root.reg_count).toBe("number");
    expect(lambda.fn).toBe(0);
    expect(lambda.param_regs.length).toBe(1);
  });

  it("each code row carries ip, structured inst, and rendered text", () => {
    const bc = showBytecodeJson(SNIPPET);
    for (const f of bc.functions) {
      f.code.forEach((row: any, i: number) => {
        expect(row.ip).toBe(i);
        expect(typeof row.text).toBe("string");
        // Externally tagged: exactly one key, the opcode name, mapping to the
        // operand object.
        expect(typeof row.inst).toBe("object");
        const keys = Object.keys(row.inst);
        expect(keys.length).toBe(1);
        expect(keys[0]).toMatch(/^[A-Z]/);
        expect(typeof row.inst[keys[0]]).toBe("object");
      });
    }
  });

  it("operands are structured, with ConstantId operands as numbers", () => {
    const bc = showBytecodeJson(SNIPPET);

    const builtins = instsOf(bc, "BuiltinCall");
    expect(builtins.length).toBeGreaterThanOrEqual(2); // map, print
    for (const b of builtins) {
      expect(typeof b.dst).toBe("number");
      expect(typeof b.name).toBe("number"); // ConstantId index
      expect(Array.isArray(b.args)).toBe(true);
      expect(typeof b.in_place).toBe("boolean");
    }

    const [mul] = instsOf(bc, "Mul");
    expect(mul).toEqual({
      dst: expect.any(Number),
      a: expect.any(Number),
      b: expect.any(Number),
    });

    const [closure] = instsOf(bc, "MakeClosure");
    expect(closure.func).toBe(0); // FunctionId as a bare number
    expect(closure.caps).toEqual([]);
  });

  it("inst and text agree on the operands", () => {
    const bc = showBytecodeJson(SNIPPET);
    const row = allRows(bc).find((r: any) => "Mul" in r.inst);
    const { dst, a, b } = row.inst.Mul;
    expect(row.text).toBe(`r${dst} = r${a} * r${b}`);
  });
});

describe("show-bytecode text form", () => {
  it("puts a space between a builtin's name and its arg list", () => {
    const text = showBytecodeText(SNIPPET);
    expect(text).toMatch(/= builtin "map" \[/);
    expect(text).not.toMatch(/"map"\[/);
  });

  it("puts a space between a method's name and its arg list", () => {
    const text = showBytecodeText('let p = { x: 3 }\nfn dist(m)\n  m.x\nend\nprint(p.dist())');
    expect(text).toMatch(/\."dist" \[/);
  });
});

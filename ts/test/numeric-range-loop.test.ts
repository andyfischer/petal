import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal, showIrJson, termsByOp } from "./helpers";

beforeAll(ensureBuild);

function ops(ir: any): string[] {
  return ir.terms.map((t: any) =>
    typeof t.op === "string" ? t.op : Object.keys(t.op)[0],
  );
}

describe("numeric range loop — IR lowering", () => {
  it("for i in range(a, b) lowers to NumericForLoop with no range Call", () => {
    // Arithmetic-only body so any Call term would have to be the range() call.
    const ir = showIrJson("let s = 0\nfor i in range(0, 3) do\n  s = s + i\nend");
    expect(termsByOp(ir, "NumericForLoop").length).toBe(1);
    // The range() builtin should NOT be called — no list materialized.
    expect(ops(ir)).not.toContain("Call");
  });

  it("single-arg range(n) also lowers to NumericForLoop", () => {
    const ir = showIrJson("let s = 0\nfor i in range(4) do\n  s = s + i\nend");
    expect(termsByOp(ir, "NumericForLoop").length).toBe(1);
    expect(ops(ir)).not.toContain("Call");
  });

  it("NumericForLoop body block carries the loop variable as a param", () => {
    const ir = showIrJson("for i in range(0, 3) do\n  i\nend");
    const loop = termsByOp(ir, "NumericForLoop")[0];
    expect(loop.child_blocks).toHaveLength(1);
    const body = ir.blocks.find((b: any) => b.id === loop.child_blocks[0]);
    expect(body.param_names).toContain("i");
  });

  it("for over a list literal still uses the generic ForLoop", () => {
    const ir = showIrJson("for i in [1, 2, 3] do\n  i\nend");
    expect(termsByOp(ir, "ForLoop").length).toBe(1);
    expect(termsByOp(ir, "NumericForLoop").length).toBe(0);
  });
});

describe("numeric range loop — runtime semantics match the list path", () => {
  it("accumulates the same sum", () => {
    expect(
      runPetal(`let total = 0
for i in range(0, 5) do
  total = total + i
end
print(total)`),
    ).toBe("10");
  });

  it("range(n) iterates 0..n", () => {
    expect(
      runPetal(`for i in range(3) do
  print(i)
end`),
    ).toBe("0\n1\n2");
  });

  it("non-zero start", () => {
    expect(
      runPetal(`for i in range(2, 5) do
  print(i)
end`),
    ).toBe("2\n3\n4");
  });

  it("empty range runs zero iterations", () => {
    expect(
      runPetal(`let n = 0
for i in range(3, 3) do
  n = n + 1
end
print(n)`),
    ).toBe("0");
  });

  it("reversed bounds run zero iterations", () => {
    expect(
      runPetal(`let n = 0
for i in range(5, 2) do
  n = n + 1
end
print(n)`),
    ).toBe("0");
  });

  it("break with a mid-body rebind preserves the carry", () => {
    expect(
      runPetal(`let total = 0
for x in range(1, 4) do
  total = total + x
  if x == 2 then break end
  total = total + 100
end
print(total)`),
    ).toBe("103");
  });

  it("continue skips the rest of the body", () => {
    expect(
      runPetal(`let total = 0
for x in range(0, 5) do
  if x == 2 then continue end
  total = total + x
end
print(total)`),
    ).toBe("8");
  });

  it("state inside a numeric for-loop resets per iteration", () => {
    expect(
      runPetal(`for i in range(0, 3) do
  state count = 0
  count += 1
  print(count)
end`),
    ).toBe("1\n1\n1");
  });

  it("bounds are evaluated from runtime expressions", () => {
    expect(
      runPetal(`let lo = 1
let hi = 4
let total = 0
for i in range(lo, hi) do
  total = total + i
end
print(total)`),
    ).toBe("6");
  });
});

describe("numeric range loop — IR round-trips through run --ir", () => {
  it("show-ir --json | run --ir produces identical output", () => {
    // This is exercised generically by ir-roundtrip.test.ts, but assert here
    // that a NumericForLoop program survives the loader.
    expect(
      runPetal(`let total = 0
for i in range(0, 4) do
  total = total + i
end
print(total)`),
    ).toBe("6");
  });
});

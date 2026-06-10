import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runIr, runIrError, runPetal } from "./helpers";
import { compileCalcToIr, compileCalcToIrJson } from "../tools/calc-to-ir";

beforeAll(ensureBuild);

// The reference external emitter (M4 of idea-34b8348d): a toy "calc" language
// that compiles directly to Petal IR JSON and runs via `petal run --ir`. These
// tests prove a third-party front-end can target the documented IR contract and
// execute on Petal's evaluator.

function runCalc(source: string): string {
  return runIr(compileCalcToIrJson(source));
}

/** Translate a calc program into the equivalent Petal source. The calc
 *  expression grammar is a subset of Petal's, so only the statements differ. */
function calcToPetal(source: string): string {
  return source
    .split("\n")
    .map((line) => {
      const t = line.trim();
      if (t.startsWith("print ")) return `print(${t.slice("print ".length)})`;
      return line; // `let ... = ...` is identical in both languages
    })
    .join("\n");
}

describe("calc emitter — runs on Petal's evaluator via run --ir", () => {
  it("evaluates a single expression with correct precedence", () => {
    expect(runCalc("print 1 + 2 * 3")).toBe("7");
    expect(runCalc("print (1 + 2) * 3")).toBe("9");
  });

  it("handles variables, parentheses, and unary minus", () => {
    expect(
      runCalc(`let x = 10
let y = x * 2 + 1
print y
print (x - 3) * 2
print -x + 100`),
    ).toBe("21\n14\n90");
  });

  it("dedups repeated constants and chains arithmetic", () => {
    expect(runCalc("print 2 + 2 + 2 + 2")).toBe("8");
    expect(runCalc("print 100 - 50 - 25")).toBe("25");
  });

  it("supports comments and blank lines", () => {
    expect(
      runCalc(`# a calc program
let a = 5   # five

print a * a`),
    ).toBe("25");
  });
});

describe("calc emitter — output matches the real Petal compiler", () => {
  const programs = [
    "print 1 + 2 * 3 - 4",
    "print (8 - 2) * (3 + 1)",
    "let x = 7\nlet y = x * x\nprint y\nprint y + x",
    "let a = 2\nlet b = 3\nlet c = a * b + a\nprint c\nprint -c",
    "print 1000 / 10 / 5",
  ];

  for (const prog of programs) {
    it(`matches Petal for: ${prog.replace(/\n/g, " ; ")}`, () => {
      const fromIr = runCalc(prog);
      const fromSource = runPetal(calcToPetal(prog));
      expect(fromIr).toBe(fromSource);
    });
  }
});

describe("calc emitter — produces a valid IR contract", () => {
  it("emits the leading print phantom before the entry term", () => {
    const ir = compileCalcToIr("print 1 + 2") as any;
    expect(ir.terms[0]).toMatchObject({ op: "Copy", name: "print", inputs: [] });
    // entry points at the first real (listed) term, not the phantom.
    expect(ir.blocks[0].entry).toBe(1);
    expect(ir.has_errors).toBe(false);
  });

  it("a program with no print emits no phantom and no output", () => {
    const ir = compileCalcToIr("let x = 1 + 1") as any;
    expect(ir.terms.every((t: any) => t.name !== "print")).toBe(true);
    expect(runIr(JSON.stringify(ir))).toBe("");
  });

  it("the validator rejects a tampered graph (dangling input)", () => {
    const ir = compileCalcToIr("print 1 + 2") as any;
    // Point the Add at a non-existent term id.
    const add = ir.terms.find((t: any) => t.op === "Add");
    add.inputs = [999, 999];
    expect(runIrError(JSON.stringify(ir))).toMatch(/invalid|input|bounds|dangling|reference/i);
  });
});

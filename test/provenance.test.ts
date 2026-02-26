import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showIrJson, BUILTIN_COUNT } from "./helpers";
import { execSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../rust/target/debug/petal");

beforeAll(() => ensureBuild());

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

function showProvenance(code: string, termName: string): any {
  const result = execSync(
    [PETAL, "show-provenance", "--json", "--term", termName, "-e", shellEscape(code)].join(" "),
    { encoding: "utf-8", timeout: 10000 }
  ).trim();
  return JSON.parse(result);
}

/** Check if an op matches a simple string or an object key */
function hasOp(term: any, op: string): boolean {
  if (term.op === op) return true;
  if (typeof term.op === "object" && op in term.op) return true;
  return false;
}

/** Check if the root or any ancestor has the given op */
function anyTermHasOp(prov: any, op: string): boolean {
  if (hasOp(prov.root, op)) return true;
  return prov.ancestors.some((a: any) => hasOp(a, op));
}

describe("provenance queries", () => {
  it("traces a simple variable to its constant", () => {
    const prov = showProvenance("let x = 42", "x");
    expect(prov.root.name).toBe("x");
    expect(prov.root.id).toBeGreaterThanOrEqual(BUILTIN_COUNT);
  });

  it("traces arithmetic through operands", () => {
    const prov = showProvenance("let a = 10\nlet b = 20\nlet c = a + b", "c");
    expect(prov.root.name).toBe("c");
    // c is the Add term; its ancestors include Copy terms for a and b,
    // plus the Constant terms for 10 and 20
    expect(anyTermHasOp(prov, "Add")).toBe(true);
    expect(prov.ancestors.length).toBeGreaterThanOrEqual(2);
    // Should trace back to the named constants a and b
    const namedAncestors = prov.ancestors.filter((a: any) => a.name !== null);
    expect(namedAncestors.map((a: any) => a.name).sort()).toEqual(["a", "b"]);
  });

  it("traces through function calls", () => {
    const prov = showProvenance(
      "fn double(x) { x * 2 }\nlet result = double(5)",
      "result"
    );
    expect(prov.root.name).toBe("result");
    // result is a Copy of the Call result; the Call or Copy-of-Call should be in the chain
    expect(anyTermHasOp(prov, "Call")).toBe(true);
  });

  it("returns empty ancestors for a leaf constant", () => {
    const ir = showIrJson("let x = 42");
    // Find the first user constant term
    const constTerm = ir.terms.find(
      (t: any) => typeof t.op === "object" && "Constant" in t.op && t.id >= BUILTIN_COUNT
    );
    if (constTerm) {
      const prov = showProvenance("let x = 42", `t${constTerm.id}`);
      expect(prov.ancestors.length).toBe(0);
    }
  });

  it("traces by term id", () => {
    const ir = showIrJson("let x = 42");
    const xTerm = ir.terms.find((t: any) => t.name === "x");
    const prov = showProvenance("let x = 42", `t${xTerm.id}`);
    expect(prov.root.id).toBe(xTerm.id);
    expect(prov.root.name).toBe("x");
  });

  it("shows edges between terms", () => {
    const prov = showProvenance("let a = 1\nlet b = a + 2", "b");
    expect(prov.edges).toBeDefined();
    expect(prov.edges.length).toBeGreaterThan(0);
    // Each edge should have from/to fields
    for (const edge of prov.edges) {
      expect(edge).toHaveProperty("from");
      expect(edge).toHaveProperty("to");
    }
  });

  it("traces list allocation inputs", () => {
    const prov = showProvenance("let a = 1\nlet b = 2\nlet xs = [a, b]", "xs");
    expect(prov.root.name).toBe("xs");
    // xs is the AllocList or a Copy of it
    expect(anyTermHasOp(prov, "AllocList")).toBe(true);
  });
});

import { describe, it, expect } from "vitest";
import {
  showIrJson,
  userTerms,
  showProvenanceJson,
  showDependentsJson,
  showSliceJson,
  explainJson,
  dataflowText,
} from "./helpers";


/** Local alias kept so the existing assertions below read unchanged. */
const showProvenance = showProvenanceJson;

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
    // root term should be a user term, not a builtin
    const ut = userTerms(showIrJson("let x = 42"));
    const xTerm = ut.find((t: any) => t.name === "x");
    expect(prov.root.id).toBe(xTerm.id);
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
      "fn double(x)\n  x * 2\nend\nlet result = double(5)",
      "result"
    );
    expect(prov.root.name).toBe("result");
    // result is a Copy of the Call result; the Call or Copy-of-Call should be in the chain
    expect(anyTermHasOp(prov, "Call")).toBe(true);
  });

  it("returns empty ancestors for a leaf constant", () => {
    const ir = showIrJson("let x = 42");
    // Find the first user constant term
    const ut = userTerms(ir);
    const constTerm = ut.find(
      (t: any) => typeof t.op === "object" && "Constant" in t.op
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

/**
 * The cell frontier (docs/var.md, Provenance). A backward walk
 * must stop at a cell read *and say so*: an unannounced truncation reads as
 * "nothing further influenced this", which is the same lie shorter.
 */
describe("cell boundaries in provenance", () => {
  const REPRO = "var x = 0\nset x = x + 1\nlet y = x * 2\n";
  const VIA_FN = "var x = 0\nfn bump()\n  set x = get x + 1\n  get x\nend\nlet y = bump()\n";

  it("stops at the cell read and reports the frontier", () => {
    const prov = showProvenance(REPRO, "y");
    expect(prov.complete).toBe(false);
    expect(prov.frontier).toHaveLength(1);
    expect(prov.frontier[0].var).toBe("x");
    expect(prov.frontier[0].writes.map((w: any) => w.line)).toContain(2);
    // The measured bug: `y` was reported as descending from the cell's
    // initializer, which never reached it.
    const ops = prov.ancestors.map((a: any) => a.op);
    expect(ops).not.toContain("CellNew");
  });

  it("degrades to the static answer when the program was not run", () => {
    // show-provenance never runs the program, so the dynamic writer is
    // unavailable by construction — but the write set still is.
    const prov = showProvenance(REPRO, "y");
    expect(prov.frontier[0].resolution).toBe("not_traced");
    expect(prov.frontier[0].writes.length).toBeGreaterThan(0);
  });

  it("stops at a closure capture, not only at a direct read", () => {
    // The value comes back through a call, so there is no CellRead in the
    // root block. Without treating the capture as an identity edge this would
    // come back `complete: true` — the §6e lie, certified.
    const prov = showProvenance(VIA_FN, "y");
    expect(prov.complete).toBe(false);
    expect(prov.frontier[0].var).toBe("x");
    expect(prov.frontier[0].captured).toBe(true);
    const ops = prov.ancestors.map((a: any) => a.op);
    expect(ops).not.toContain("CellNew");
    expect(prov.ancestors.map((a: any) => a.name)).not.toContain("x");
  });

  it("marks a state var's write set as host-writable", () => {
    const prov = showProvenance("state var h = 0\nset h = h + 1\nlet y = h * 2\n", "y");
    expect(prov.frontier[0].var).toBe("h");
    expect(prov.frontier[0].host_writable).toBe(true);
  });

  it("reports cell-free programs as complete, with today's exact answer", () => {
    const prov = showProvenance("let x = 1\nlet y = x * 2\n", "y");
    expect(prov.complete).toBe(true);
    expect(prov.frontier).toEqual([]);
    expect(prov.ancestors.length).toBeGreaterThan(0);
  });
});

describe("explain across a cell boundary", () => {
  const REPRO = "var x = 0\nset x = x + 1\nlet y = x * 2\n";

  it("names the writer and continues the chain through it", () => {
    const out = explainJson(REPRO, "y");
    expect(out.complete).toBe(true);
    const boundary = out.chain.map((e: any) => e.boundary).filter(Boolean)[0];
    expect(boundary.var).toBe("x");
    expect(boundary.resolution).toBe("resolved");
    expect(boundary.last_write.line).toBe(2);
    // The chain continues past the boundary: the Add, its `1`, and the
    // initial `0` all appear, which they cannot if the walk merely truncated.
    const values = out.chain.map((e: any) => e.value);
    expect(values).toContain("2");
    expect(values).toContain("1");
    expect(values).toContain("0");
  });

  it("names a write site inside a function", () => {
    const out = explainJson(
      "var x = 0\nfn bump()\n  set x = get x + 1\n  get x\nend\nlet y = bump()\n",
      "y"
    );
    const boundary = out.chain.map((e: any) => e.boundary).filter(Boolean)[0];
    expect(boundary.var).toBe("x");
    expect(boundary.captured).toBe(true);
    expect(boundary.resolution).toBe("resolved");
    expect(boundary.last_write.line).toBe(3);
  });

  it("resolves a loop to the final iteration and lists every write in order", () => {
    const out = explainJson(
      "var acc = 0\nfor i in [1, 2, 3] do\n  set acc = acc + i\nend\nlet y = acc * 2\n",
      "y"
    );
    const boundary = out.chain.map((e: any) => e.boundary).filter(Boolean)[0];
    expect(boundary.resolution).toBe("resolved");
    expect(boundary.last_write.value).toBe("6");
    // One entry per iteration, in order — the listing the MCP ExplainTerm
    // description already advertises.
    expect(boundary.writes.map((w: any) => w.value)).toEqual(["1", "3", "6"]);
    const seqs = boundary.writes.map((w: any) => w.seq);
    expect(boundary.last_write.seq).toBe(seqs[seqs.length - 1]);
    // Values on the continued chain come from *that write's* execution: the
    // read it consumed saw 3, not the final 6.
    const continued = out.chain.filter((e: any) => e.boundary && e.value === "3");
    expect(continued.length).toBeGreaterThan(0);
  });

  it("reports a never-written var as holding its initializer", () => {
    const out = explainJson("var x = 7\nlet y = x * 2\n", "y");
    const boundary = out.chain.map((e: any) => e.boundary).filter(Boolean)[0];
    expect(boundary.resolution).toBe("initial");
    expect(boundary.last_write.value).toBe("7");
    expect(out.complete).toBe(true);
  });

  it("leaves a cell-free chain unannotated and complete", () => {
    const out = explainJson("let a = 1\nlet y = a * 2\n", "y");
    expect(out.complete).toBe(true);
    expect(out.truncated).toBe(false);
    expect(out.chain.every((e: any) => e.boundary === null)).toBe(true);
  });

  it("says so in text mode — a chain that just ends is the same lie", () => {
    const text = dataflowText("explain", REPRO, ["y"]);
    expect(text).toContain("read of var 'x'");
    expect(text).toContain("line 2");
    expect(text).toContain("chain continues from there");
  });
});

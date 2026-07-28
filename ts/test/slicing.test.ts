import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showIrJson,
  showDependentsJson,
  showSliceJson,
  dataflowText,
} from "./helpers";

beforeAll(() => {
  ensureBuild();
});

describe("dataflow slicing", () => {
  describe("show-dependents", () => {
    it("finds direct dependents of a variable", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 2", "a");
      expect(result.root.name).toBe("a");
      // b depends on a (through Copy and Add)
      expect(result.dependents.length).toBeGreaterThan(0);
    });

    it("finds transitive dependents", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 1\nlet c = b + 1", "a");
      // c transitively depends on a
      const names = result.dependents.map((d: any) => d.name).filter(Boolean);
      expect(names).toContain("b");
      expect(names).toContain("c");
    });

    it("returns empty for terminal values", () => {
      const ir = showIrJson("let a = 1\nlet b = a + 1\nlet c = b + 1");
      // c is the last named variable, find its term id
      const cTerm = ir.terms.find((t: any) => t.name === "c");
      const result = showDependentsJson("let a = 1\nlet b = a + 1\nlet c = b + 1", `t${cTerm.id}`);
      expect(result.dependents.length).toBe(0);
    });
  });

  describe("show-slice", () => {
    it("returns minimal subgraph for a single target", () => {
      const result = showSliceJson("let a = 1\nlet b = 2\nlet c = a + b\nlet d = 99", ["c"]);
      const sliceNames = result.slice.map((t: any) => t.name).filter(Boolean);
      // c's slice should include a and b but not d
      expect(sliceNames).toContain("a");
      expect(sliceNames).toContain("b");
      expect(sliceNames).toContain("c");
      expect(sliceNames).not.toContain("d");
    });

    it("returns terms in topological order", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet c = b + 1", ["c"]);
      const ids = result.slice.map((t: any) => t.id);
      // IDs should be in ascending order (topological = program order)
      for (let i = 1; i < ids.length; i++) {
        expect(ids[i]).toBeGreaterThan(ids[i - 1]);
      }
    });

    it("merges slices for multiple targets", () => {
      // a -> b, c -> d. Slice for [b, d] should include a, b, c, d
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet c = 2\nlet d = c + 1", ["b", "d"]);
      const sliceNames = result.slice.map((t: any) => t.name).filter(Boolean);
      expect(sliceNames).toContain("a");
      expect(sliceNames).toContain("b");
      expect(sliceNames).toContain("c");
      expect(sliceNames).toContain("d");
    });

    it("excludes unrelated terms", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet unrelated = 42", ["b"]);
      const sliceNames = result.slice.map((t: any) => t.name).filter(Boolean);
      expect(sliceNames).not.toContain("unrelated");
    });
  });
});

/**
 * Cells in the two slicing directions (§6e). Backward is a *must* question, so
 * may-writes are inadmissible as edges and go in the frontier instead; forward
 * is already a *may* question, so the may-edges belong in it.
 */
describe("cells and dataflow slicing", () => {
  const REPRO = "var x = 0\nset x = x + 1\nlet y = x * 2\n";

  describe("show-slice", () => {
    it("is not minimal once a cell is crossed, and says which", () => {
      const result = showSliceJson(REPRO, ["y"]);
      expect(result.minimal).toBe(false);
      expect(result.complete).toBe(false);
      expect(result.frontier.length).toBeGreaterThan(0);
      expect(result.frontier[0].var).toBe("x");
    });

    it("includes the write chain the minimal slice dropped", () => {
      // Measured before this change: the slice for `y` omitted the write and
      // its operands, so re-evaluating it produced 0 instead of 2.
      const result = showSliceJson(REPRO, ["y"]);
      const ops = result.slice.map((t: any) => t.op);
      expect(ops).toContain("CellWrite");
      expect(ops).toContain("CellNew");
      expect(ops).toContain("Add");
    });

    it("closes over chained vars, not just the first one", () => {
      // `set a = 5` is two cells deep from `y`; one level of writes gets b's
      // chain but not a's, and the slice evaluates to 2 instead of 12.
      const result = showSliceJson(
        "var a = 0\nvar b = 0\nset a = 5\nset b = a + 1\nlet y = b * 2\n",
        ["y"]
      );
      const writes = result.slice.filter((t: any) => t.op === "CellWrite");
      expect(writes.map((w: any) => w.name).sort()).toEqual(["a", "b"]);
      expect(result.frontier.map((f: any) => f.var).sort()).toEqual(["a", "b"]);
    });

    it("still reports a let-only program as minimal and complete", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet y = b * 2\n", ["y"]);
      expect(result.minimal).toBe(true);
      expect(result.complete).toBe(true);
      expect(result.frontier).toEqual([]);
      const names = result.slice.map((t: any) => t.name).filter(Boolean);
      expect(names).toContain("a");
      expect(names).toContain("b");
      expect(names).toContain("y");
    });

    it("says `Not minimal` in text mode", () => {
      const text = dataflowText("show-slice", REPRO, ["y"]);
      expect(text).toContain("Not minimal");
      expect(text).toContain("read of var 'x'");
      // ...and does not on the cell-free equivalent.
      const clean = dataflowText("show-slice", "let a = 1\nlet y = a * 2\n", ["y"]);
      expect(clean).not.toContain("Not minimal");
    });
  });

  describe("show-dependents", () => {
    it("reaches later reads from a set", () => {
      // Measured before this change: `Downstream (0)` — the mutation was
      // reported as affecting nothing.
      const result = showDependentsJson(REPRO, "x");
      expect(result.root.op).toBe("CellWrite");
      expect(result.dependents.length).toBeGreaterThan(0);
      expect(result.dependents.map((d: any) => d.name)).toContain("y");
      const may = result.edges.filter((e: any) => e.kind === "may");
      expect(may.length).toBeGreaterThan(0);
    });

    it("lists the set sites downstream of the declaration", () => {
      const ir = showIrJson(REPRO);
      const decl = ir.terms.find((t: any) => t.op === "CellNew");
      const result = showDependentsJson(REPRO, `t${decl.id}`);
      expect(result.dependents.map((d: any) => d.op)).toContain("CellWrite");
    });

    it("tags value edges dataflow and cell edges may", () => {
      const result = showDependentsJson(REPRO, "x");
      expect(result.edges.some((e: any) => e.kind === "dataflow")).toBe(true);
      expect(result.edges.some((e: any) => e.kind === "may")).toBe(true);
    });

    it("marks the cell edge as may in text mode", () => {
      const text = dataflowText("show-dependents", REPRO, ["x"]);
      expect(text).toMatch(/~> t\d+ \(cell 'x', may\)/);
    });

    it("tags every edge dataflow on a cell-free program", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 1\n", "a");
      expect(result.edges.length).toBeGreaterThan(0);
      expect(result.edges.every((e: any) => e.kind === "dataflow")).toBe(true);
    });
  });
});

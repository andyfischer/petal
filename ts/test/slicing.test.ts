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


// A user method is dispatched by name at runtime, so a `MethodCall` term has
// no operand naming the function it calls. Without the dispatch edge, every
// dataflow answer over class-using code silently omitted the code that
// computes the value — the one thing a slice must never do.
describe("method dispatch is a dataflow edge", () => {
  const METHOD = [
    "class Point",
    "  x: int,",
    "  y: int,",
    "end",
    "fn Point.dist2(p: Point) -> int",
    "  p.x * p.x + p.y * p.y",
    "end",
    "let base = Point(3, 4)",
    "let d = base.dist2()",
    "print(d)",
  ].join("\n");

  // The same program written with a plain function, whose slice has always
  // included the MakeClosure.
  const PLAIN = METHOD.replace("fn Point.dist2(", "fn dist2(").replace(
    "base.dist2()",
    "dist2(base)"
  );

  /** Ops serialize as `"Copy"` or `{ MakeClosure: 1 }`; match either. */
  const isOp = (op: any, name: string) =>
    op === name || (op !== null && typeof op === "object" && name in op);

  const closureNames = (slice: any[]) =>
    slice.filter((t: any) => isOp(t.op, "MakeClosure")).map((t: any) => t.name);

  it("puts the method's function term in the slice", () => {
    const slice = showSliceJson(METHOD, ["d"]).slice;
    const closure = slice.find((t: any) => t.name === "Point.dist2");
    expect(closure, JSON.stringify(slice)).toBeTruthy();
    expect(isOp(closure.op, "MakeClosure")).toBe(true);
  });

  it("agrees with the plain-function spelling of the same program", () => {
    expect(closureNames(showSliceJson(METHOD, ["d"]).slice)).toEqual([
      "Point",
      "Point.dist2",
    ]);
    expect(closureNames(showSliceJson(PLAIN, ["d"]).slice)).toEqual([
      "Point",
      "dist2",
    ]);
  });

  it("reaches the call site from the declaration by an exact edge", () => {
    // `base` is statically a Point, so the compiler binds `base.dist2()`
    // straight to the declaration: an ordinary Call naming its callee, not a
    // dispatch the analysis has to over-approximate.
    const result = showDependentsJson(METHOD, "Point.dist2");
    expect(result.dependents.some((t: any) => isOp(t.op, "Call"))).toBe(true);
    expect(result.edges.every((e: any) => e.kind === "dataflow")).toBe(true);
  });

  // A receiver the checker cannot pin to one class keeps runtime dispatch, and
  // with it the may-edge: the call links to every method of that name, because
  // which one runs is not knowable until the receiver arrives.
  const DYNAMIC = [
    "class Point",
    "  x: int,",
    "  y: int,",
    "end",
    "fn Point.dist2(p: Point) -> int",
    "  p.x * p.x + p.y * p.y",
    "end",
    "fn go(p)",           // un-annotated parameter: type is `any`
    "  p.dist2()",
    "end",
    "let d = go(Point(3, 4))",
    "print(d)",
  ].join("\n");

  it("keeps a may-edge where the receiver's class is not statically known", () => {
    const result = showDependentsJson(DYNAMIC, "Point.dist2");
    expect(result.dependents.some((t: any) => isOp(t.op, "MethodCall"))).toBe(true);
    expect(result.edges.some((e: any) => e.kind === "dispatch")).toBe(true);
  });

  it("marks that dispatch edge as may in text mode", () => {
    const text = dataflowText("show-dependents", DYNAMIC, ["Point.dist2"]);
    expect(text).toMatch(/~> t\d+ \(dispatch, may\)/);
  });

  it("gives the declarations a source position instead of [no location]", () => {
    const text = dataflowText("explain", METHOD, ["d"]);
    // Term numbering is an implementation detail (it shifts whenever a
    // declaration is hoisted); the position is what the reader needs.
    expect(text).toMatch(/t\d+ Point\.dist2 \[line 5, column 1\]/);
    // …and the class constructor, which reported no location at all.
    expect(text).toMatch(/t\d+ Point \[line 1, column 1\]/);
    expect(text).not.toContain("[no location]");
  });
});

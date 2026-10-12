import { describe, it, expect } from "vitest";
import {
  showIrJson,
  showDependentsJson,
  showSliceJson,
  showProvenanceJson,
  graphJson,
  dataflowText,
} from "./helpers";


describe("dataflow slicing", () => {
  describe("dependents (graph --direction forward)", () => {
    it("finds direct dependents of a variable", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 2", "a");
      expect(result.target_terms[0].name).toBe("a");
      // b depends on a (through Copy and Add)
      expect(result.terms.length).toBeGreaterThan(0);
    });

    it("finds transitive dependents", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 1\nlet c = b + 1", "a");
      // c transitively depends on a
      const names = result.terms.map((d: any) => d.name).filter(Boolean);
      expect(names).toContain("b");
      expect(names).toContain("c");
    });

    it("returns empty for terminal values", () => {
      const ir = showIrJson("let a = 1\nlet b = a + 1\nlet c = b + 1");
      // c is the last named variable, find its term id
      const cTerm = ir.terms.find((t: any) => t.name === "c");
      const result = showDependentsJson("let a = 1\nlet b = a + 1\nlet c = b + 1", `t${cTerm.id}`);
      expect(result.terms.length).toBe(0);
    });
  });

  describe("slice (graph --slice)", () => {
    it("returns minimal subgraph for a single target", () => {
      const result = showSliceJson("let a = 1\nlet b = 2\nlet c = a + b\nlet d = 99", ["c"]);
      const sliceNames = result.terms.map((t: any) => t.name).filter(Boolean);
      // c's slice should include a and b but not d
      expect(sliceNames).toContain("a");
      expect(sliceNames).toContain("b");
      expect(sliceNames).toContain("c");
      expect(sliceNames).not.toContain("d");
    });

    it("returns terms in topological order", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet c = b + 1", ["c"]);
      const ids = result.terms.map((t: any) => t.id);
      // IDs should be in ascending order (topological = program order)
      for (let i = 1; i < ids.length; i++) {
        expect(ids[i]).toBeGreaterThan(ids[i - 1]);
      }
    });

    it("merges slices for multiple targets", () => {
      // a -> b, c -> d. Slice for [b, d] should include a, b, c, d
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet c = 2\nlet d = c + 1", ["b", "d"]);
      const sliceNames = result.terms.map((t: any) => t.name).filter(Boolean);
      expect(sliceNames).toContain("a");
      expect(sliceNames).toContain("b");
      expect(sliceNames).toContain("c");
      expect(sliceNames).toContain("d");
    });

    it("excludes unrelated terms", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet unrelated = 42", ["b"]);
      const sliceNames = result.terms.map((t: any) => t.name).filter(Boolean);
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

  describe("slice (graph --slice)", () => {
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
      const ops = result.terms.map((t: any) => t.op);
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
      const writes = result.terms.filter((t: any) => t.op === "CellWrite");
      expect(writes.map((w: any) => w.name).sort()).toEqual(["a", "b"]);
      expect(result.frontier.map((f: any) => f.var).sort()).toEqual(["a", "b"]);
    });

    it("still reports a let-only program as minimal and complete", () => {
      const result = showSliceJson("let a = 1\nlet b = a + 1\nlet y = b * 2\n", ["y"]);
      expect(result.minimal).toBe(true);
      expect(result.complete).toBe(true);
      expect(result.frontier).toEqual([]);
      const names = result.terms.map((t: any) => t.name).filter(Boolean);
      expect(names).toContain("a");
      expect(names).toContain("b");
      expect(names).toContain("y");
    });

    it("says `Not minimal` in text mode", () => {
      const text = dataflowText("graph", REPRO, ["y"], ["--slice"]);
      expect(text).toContain("Not minimal");
      expect(text).toContain("read of var 'x'");
      // ...and does not on the cell-free equivalent.
      const clean = dataflowText("graph", "let a = 1\nlet y = a * 2\n", ["y"], ["--slice"]);
      expect(clean).not.toContain("Not minimal");
    });
  });

  describe("dependents (graph --direction forward)", () => {
    it("reaches later reads from a set", () => {
      // Measured before this change: `Downstream (0)` — the mutation was
      // reported as affecting nothing.
      const result = showDependentsJson(REPRO, "x");
      expect(result.target_terms[0].op).toBe("CellWrite");
      expect(result.terms.length).toBeGreaterThan(0);
      expect(result.terms.map((d: any) => d.name)).toContain("y");
      const may = result.edges.filter((e: any) => e.kind === "may");
      expect(may.length).toBeGreaterThan(0);
    });

    it("lists the set sites downstream of the declaration", () => {
      const ir = showIrJson(REPRO);
      const decl = ir.terms.find((t: any) => t.op === "CellNew");
      const result = showDependentsJson(REPRO, `t${decl.id}`);
      expect(result.terms.map((d: any) => d.op)).toContain("CellWrite");
    });

    it("tags value edges dataflow and cell edges may", () => {
      const result = showDependentsJson(REPRO, "x");
      expect(result.edges.some((e: any) => e.kind === "dataflow")).toBe(true);
      expect(result.edges.some((e: any) => e.kind === "may")).toBe(true);
    });

    it("marks the cell edge as may in text mode", () => {
      const text = dataflowText("graph", REPRO, ["x"], ["--direction", "forward"]);
      expect(text).toMatch(/~> t\d+ \(cell 'x', may\)/);
    });

    it("carries a frontier for a var read it reaches through a may-edge", () => {
      // Before `petal graph` unified the result shape, this command reported
      // neither `frontier` nor `complete` — though it is the one query that
      // over-approximates most.
      const result = showDependentsJson(REPRO, "x");
      expect(result.complete).toBe(false);
      expect(result.minimal).toBe(false);
      expect(result.frontier.length).toBeGreaterThan(0);
      expect(result.frontier.every((f: any) => f.var === "x")).toBe(true);
      expect(result.frontier[0].writes.length).toBeGreaterThan(0);
    });

    it("says so in text mode", () => {
      const text = dataflowText("graph", REPRO, ["x"], ["--direction", "forward"]);
      expect(text).toContain("Frontier (");
      expect(text).toContain("read of var 'x'");
    });

    it("is complete on a cell-free program", () => {
      const result = showDependentsJson("let a = 1\nlet b = a + 1\n", "a");
      expect(result.complete).toBe(true);
      expect(result.frontier).toEqual([]);
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
    const slice = showSliceJson(METHOD, ["d"]).terms;
    const closure = slice.find((t: any) => t.name === "Point.dist2");
    expect(closure, JSON.stringify(slice)).toBeTruthy();
    expect(isOp(closure.op, "MakeClosure")).toBe(true);
  });

  it("agrees with the plain-function spelling of the same program", () => {
    expect(closureNames(showSliceJson(METHOD, ["d"]).terms)).toEqual([
      "Point",
      "Point.dist2",
    ]);
    expect(closureNames(showSliceJson(PLAIN, ["d"]).terms)).toEqual([
      "Point",
      "dist2",
    ]);
  });

  it("reaches the call site from the declaration by an exact edge", () => {
    // `base` is statically a Point, so the compiler binds `base.dist2()`
    // straight to the declaration: an ordinary Call naming its callee, not a
    // dispatch the analysis has to over-approximate.
    const result = showDependentsJson(METHOD, "Point.dist2");
    expect(result.terms.some((t: any) => isOp(t.op, "Call"))).toBe(true);
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
    expect(result.terms.some((t: any) => isOp(t.op, "MethodCall"))).toBe(true);
    expect(result.edges.some((e: any) => e.kind === "dispatch")).toBe(true);
  });

  it("marks that dispatch edge as may in text mode", () => {
    const text = dataflowText("graph", DYNAMIC, ["Point.dist2"], ["--direction", "forward"]);
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

// `petal graph` is the one dataflow query: provenance, dependents and slice.
describe("petal graph", () => {
  const REPRO = "var x = 0\nset x = x + 1\nlet y = x * 2\n";
  const SHAPE = [
    "complete",
    "direction",
    "edges",
    "frontier",
    "minimal",
    "target_terms",
    "targets",
    "terms",
  ];
  const ids = (terms: any[]) => terms.map((t: any) => t.id);

  it("has one result shape in every direction", () => {
    for (const r of [
      graphJson(REPRO, ["y"]),
      graphJson(REPRO, ["x"], "forward"),
      graphJson(REPRO, ["y", "x"]),
      graphJson(REPRO, ["y"], "back", true),
    ]) {
      expect(Object.keys(r).sort()).toEqual(SHAPE);
      expect(ids(r.target_terms)).toEqual(r.targets);
    }
  });

  it("back is the default direction", () => {
    const g = graphJson(REPRO, ["y"], "back");
    const p = graphJson(REPRO, ["y"]);
    expect(g.direction).toBe("back");
    expect(g).toEqual(p);
  });

  it("forward says so", () => {
    expect(graphJson(REPRO, ["x"], "forward").direction).toBe("forward");
  });

  it("several terms is a slice, with edges inside the slice", () => {
    const g = graphJson(REPRO, ["y", "x"]);
    expect(g).toEqual(graphJson(REPRO, ["y", "x"], "back", true));
    expect(g.minimal).toBe(false);
    const inSlice = new Set(ids(g.terms));
    expect(g.edges.length).toBeGreaterThan(0);
    for (const e of g.edges) {
      expect(inSlice.has(e.from) && inSlice.has(e.to)).toBe(true);
    }
  });

  it("one term is its provenance unless --slice is passed", () => {
    // The slice crosses the cell and pulls in the write; provenance stops there.
    const prov = graphJson(REPRO, ["y"]);
    const slice = graphJson(REPRO, ["y"], "back", true);
    expect(slice.terms.map((t: any) => t.op)).toContain("CellWrite");
    expect(prov.terms.map((t: any) => t.op)).not.toContain("CellWrite");
  });

  it("--slice refuses --direction forward", () => {
    expect(() => graphJson(REPRO, ["y"], "forward", true)).toThrow();
  });

  it("forward from several terms is the union of their walks", () => {
    const code = "let a = 1\nlet b = a + 1\nlet c = 2\nlet d = c + 1\n";
    const names = graphJson(code, ["a", "c"], "forward").terms.map((t: any) => t.name);
    expect(names).toContain("b");
    expect(names).toContain("d");
  });
});

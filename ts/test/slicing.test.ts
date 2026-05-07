import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, showIrJson } from "./helpers";
import { execSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");

beforeAll(() => {
  ensureBuild();
});

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

function showDependentsJson(code: string, term: string): any {
  const cmd = [PETAL, "show-dependents", "--json", "--term", term, "-e", shellEscape(code)].join(" ");
  return JSON.parse(execSync(cmd, { encoding: "utf-8", timeout: 10000 }).trim());
}

function showSliceJson(code: string, terms: string[]): any {
  const termArgs = terms.flatMap(t => ["--term", t]);
  const cmd = [PETAL, "show-slice", "--json", ...termArgs, "-e", shellEscape(code)].join(" ");
  return JSON.parse(execSync(cmd, { encoding: "utf-8", timeout: 10000 }).trim());
}

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

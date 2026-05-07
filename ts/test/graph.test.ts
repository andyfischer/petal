import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild } from "./helpers";
import { execSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");

beforeAll(() => {
  ensureBuild();
});

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

function showGraph(code: string): string {
  const cmd = [PETAL, "show-graph", "-e", shellEscape(code)].join(" ");
  return execSync(cmd, { encoding: "utf-8", timeout: 10000 }).trim();
}

describe("show-graph", () => {
  it("produces valid DOT format", () => {
    const dot = showGraph("let x = 1");
    expect(dot).toContain("digraph dataflow {");
    expect(dot).toContain("}");
  });

  it("includes term nodes", () => {
    const dot = showGraph("let a = 1\nlet b = a + 1");
    // Should have nodes for constants, Copy terms, and Add
    expect(dot).toContain("t");
    expect(dot).toContain("Add");
  });

  it("includes dataflow edges", () => {
    const dot = showGraph("let a = 1\nlet b = a + 1");
    // Should have edges (->)
    expect(dot).toMatch(/t\d+ -> t\d+/);
  });

  it("colors state terms differently", () => {
    const dot = showGraph("state x = 0");
    expect(dot).toContain("lightyellow");
  });

  it("colors branch terms differently", () => {
    const dot = showGraph("if true { 1 } else { 2 }");
    expect(dot).toContain("lightsalmon");
  });
});

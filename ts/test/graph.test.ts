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
    const dot = showGraph("if true then 1 else 2 end");
    expect(dot).toContain("lightsalmon");
  });

  // A `MethodCall` has no operand naming the function it dispatches to, so the
  // graph used to have no path at all from `fn Point.twice` to the call site.
  // Where the receiver's class is statically known the call is bound straight
  // to the declaration, so the path is an ordinary solid dataflow edge; where
  // it is not, the may-edge below recovers it.
  it("draws a solid edge from a declaration to a statically resolved call", () => {
    const dot = showGraph(
      "class Point\n  x: int,\nend\nfn Point.twice(p: Point)\n  p.x * 2\nend\nlet d = Point(3).twice()\n"
    );
    const decl = dot.match(/t(\d+) \[label="t\d+: Point\.twice \(MakeClosure/);
    const call = dot.match(/t(\d+) \[label="t\d+: d \(Call/);
    expect(decl && call, dot).toBeTruthy();
    expect(dot).not.toContain('label="dispatch"');
  });

  it("draws the method-dispatch may-edge when the receiver is not pinned down", () => {
    const dot = showGraph(
      "class Point\n  x: int,\nend\nfn Point.twice(p: Point)\n  p.x * 2\nend\nfn go(p)\n  p.twice()\nend\nlet d = go(Point(3))\n"
    );
    const decl = dot.match(/t(\d+) \[label="t\d+: Point\.twice \(MakeClosure/);
    const call = dot.match(/t(\d+) \[label="t\d+: MethodCall/);
    expect(decl && call, dot).toBeTruthy();
    expect(dot).toContain(
      `t${decl![1]} -> t${call![1]} [style=dashed, color=darkgreen, label="dispatch"];`
    );
  });
});

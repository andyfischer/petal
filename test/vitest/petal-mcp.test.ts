// Smoke tests for the dev MCP server's ProposeEdit tool (tools/petal-mcp.ts).
// The tool's input → argv mapping lives in tools/petal-mcp-propose.ts so it
// can run here without the MCP SDK; each test builds the argv the tool would
// and runs the built `petal` on it, as the tool does.

import { describe, expect, it } from "vitest";
import { mkdtempSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { proposeEditArgs, type ProposeInput } from "../../tools/petal-mcp-propose.ts";
import { petalCapture } from "./helpers";

const HELPER = `import io
config let gap = 30.0
fn place(i)
  return 20 + i * gap
end
for i in range(0, 4) do
  push_output(symbol("shapes"), place(i))
end
`;

function propose(input: ProposeInput, code: string): any {
  const built = proposeEditArgs(input);
  if ("error" in built) throw new Error(built.error);
  const file = join(mkdtempSync(join(tmpdir(), "petal-mcp-")), "sketch.ptl");
  writeFileSync(file, code);
  const r = petalCapture([...built.args, file]);
  expect(r.code, r.stderr || r.stdout).toBe(0);
  return JSON.parse(r.stdout);
}

describe("ProposeEdit (MCP)", () => {
  it("v2 solves through a helper function", () => {
    const out = propose(
      { channel: "shapes", emit: 2, engine: "v2", nativeArg: 1, by: 25, explain: true },
      HELPER,
    );
    expect(out.engine).toBe("v2");
    const top = out.alternatives[0];
    expect(top.verified).toBe(true);
    expect(top.changes[0].binding).toBe("gap");
    expect(top.changes[0].new_text).toBe("42.5");
    expect(top.achieved[0]).toBe(105);
    expect(out.candidates.length).toBeGreaterThan(0);
    expect(out.explain.length).toBeGreaterThan(0);
  });

  it("re-asking with allow moves the named literal instead", () => {
    const out = propose(
      { channel: "shapes", emit: 2, arg: 1, to: 105, allow: ["fn:place"] },
      HELPER,
    );
    expect(out.engine).toBe("v2");
    expect(out.alternatives).toHaveLength(1);
    expect(out.alternatives[0].changes[0].function).toBe("place");
  });

  it("arg + to with no engine keeps the v1 answer and shape", () => {
    const sketch = `import io
let offset = 10
let x = 20
push_output(symbol("shapes"), x + offset)
`;
    const out = propose({ channel: "shapes", emit: 0, arg: 1, to: "42.5" }, sketch);
    expect(out.proposals.map((p: any) => p.variable)).toEqual(["x", "offset"]);
  });

  it("builds multi-goal argv across emits", () => {
    const built = proposeEditArgs({
      channel: "shapes",
      emit: 0,
      goals: [
        { arg: 1, to: 3 },
        { emit: 1, arg: 1, to: 5 },
      ],
    });
    expect(built).toEqual({
      args: [
        "propose-edit", "--json",
        "--channel", "shapes", "--emit", "0", "--arg", "1", "--to", "3",
        "--emit", "1", "--arg", "1", "--to", "5",
      ],
    });
  });

  it("rejects malformed goals", () => {
    expect(proposeEditArgs({ channel: "s", emit: 0 })).toHaveProperty("error");
    expect(proposeEditArgs({ channel: "s", emit: 0, arg: 1 })).toHaveProperty("error");
    expect(proposeEditArgs({ channel: "s", emit: 0, arg: 1, nativeArg: 1, to: 1 })).toHaveProperty("error");
  });
});

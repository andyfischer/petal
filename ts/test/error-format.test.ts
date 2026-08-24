import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");

function stderrOf(args: string[]) {
  const r = spawnSync(PETAL, args, { encoding: "utf-8", timeout: 10000 });
  return { text: r.stderr, status: r.status };
}

const RUNTIME_ERR = "let x = 1\nprint(x.foo.bar)";
const PARSE_ERR = "let x = =";

describe("--error-format bare", () => {
  it("drops the position, the caret block and the Error: prefix", () => {
    const full = stderrOf(["run", "-e", RUNTIME_ERR]);
    const bare = stderrOf(["run", "--error-format", "bare", "-e", RUNTIME_ERR]);

    expect(full.status).toBe(1);
    expect(bare.status).toBe(1);

    expect(full.text).toMatch(/\[line 2, column \d+\]/);
    expect(full.text).toContain("print(x.foo.bar)");
    expect(full.text).toContain("^");

    expect(bare.text.trim()).toBe("Cannot access field 'foo' on int");
  });

  it("defaults to full", () => {
    const dflt = stderrOf(["run", "-e", RUNTIME_ERR]);
    const full = stderrOf(["run", "--error-format", "full", "-e", RUNTIME_ERR]);
    expect(dflt.text).toBe(full.text);
  });

  it("is identical for sources differing only in indentation and blank lines", () => {
    const a = "fn f()\n  let x = 1\n  x.foo\nend\nf()";
    const b = "\n\nfn f()\n      let x = 1\n      x.foo\nend\n\nf()";
    const bareA = stderrOf(["run", "--error-format", "bare", "-e", a]);
    const bareB = stderrOf(["run", "--error-format", "bare", "-e", b]);
    expect(bareA.text).toBe(bareB.text);
    expect(bareA.text.trim()).not.toBe("");
    // The point of the flag: the default output does differ for these two.
    expect(stderrOf(["run", "-e", a]).text).not.toBe(stderrOf(["run", "-e", b]).text);
  });

  it("works for parse errors too", () => {
    const bare = stderrOf(["run", "--error-format", "bare", "-e", PARSE_ERR]);
    expect(bare.text.trim()).toBe("Unexpected token: '='");
  });

  it("works on check", () => {
    const full = stderrOf(["check", "-e", PARSE_ERR]);
    const bare = stderrOf(["check", "--error-format", "bare", "-e", PARSE_ERR]);
    expect(full.text).toMatch(/\[line 1, column \d+\]/);
    expect(bare.text.trim()).toBe("Unexpected token: '='");
    expect(bare.status).toBe(1);
  });

  it("rejects an unknown format", () => {
    const r = stderrOf(["run", "--error-format", "sideways", "-e", "1"]);
    expect(r.status).toBe(1);
    expect(r.text).toMatch(/error-format/);
  });
});

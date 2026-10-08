import { describe, it, expect } from "vitest";
import { petalCapture } from "./helpers";

function stderrOf(args: string[]) {
  const r = petalCapture(args);
  return { text: r.stderr, status: r.code };
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

const WARNS = "state s = 0\nfn f()\n  s\nend\nprint(f())\ns = 1";

describe("--error-format bare on type-checker warnings", () => {
  it("keeps the message but drops the position line and the caret block", () => {
    const full = stderrOf(["check", "-e", WARNS]);
    const bare = stderrOf(["check", "--error-format", "bare", "-e", WARNS]);

    expect(full.status).toBe(0);
    expect(bare.status).toBe(0);
    expect(full.text).toMatch(/ --> \[line 3, column \d+\]/);
    expect(full.text).toContain("^");

    expect(bare.text).toContain("warning: `s` is a `state` binding written");
    expect(bare.text).not.toContain("-->");
    expect(bare.text).not.toContain("^");
    // The whole point: re-indenting the function body must not move the text.
    const indented = stderrOf(["check", "--error-format", "bare", "-e", WARNS.replace("\n  s\n", "\n      s\n")]);
    expect(indented.text).toBe(bare.text);
  });
});

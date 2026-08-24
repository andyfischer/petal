import { describe, test, expect, beforeAll, afterAll } from "vitest";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { ensureBuild, petalCapture } from "./helpers";

beforeAll(() => ensureBuild());

// `petal ir-equal` and `petal lint --verify` — the refactor-verification
// primitives from docs/dev/refactor-verification.md (§5 ir-equal, §7
// self-checking lint). Both are exit-code contracts, so every assertion here
// checks the code as well as the text.

let dir: string;
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "petal-ir-equal-"));
});
afterAll(() => rmSync(dir, { recursive: true, force: true }));

let counter = 0;
function file(contents: string): string {
  const path = join(dir, `f${counter++}.ptl`);
  writeFileSync(path, contents);
  return path;
}

const CHAIN = `fn label(ch)
  if ch == "@" then "spawn"
  elsif ch == "o" then "coin"
  elsif ch == "w" then "walker"
  else "none"
  end
end

print(label("@"))
`;

describe("petal ir-equal", () => {
  test("identical files are equivalent (exit 0)", () => {
    const a = file("let x = 1\nprint(x)\n");
    const b = file("let x = 1\nprint(x)\n");
    const r = petalCapture(["ir-equal", a, b]);
    expect(r.code).toBe(0);
    expect(r.stdout).toContain("equivalent");
  });

  test("whitespace and comments do not count", () => {
    const a = file("let x = 1\nlet y = x + 2\nprint(y)\n");
    const b = file(
      "\n// a comment\nlet x   =  1\n\n      let y = x + 2  // trailing\nprint(y)\n"
    );
    expect(petalCapture(["ir-equal", a, b]).code).toBe(0);
  });

  test("a changed constant is a difference (exit 1) and is named", () => {
    const a = file("let x = 1\nprint(x)\n");
    const b = file("let x = 2\nprint(x)\n");
    const r = petalCapture(["ir-equal", a, b]);
    expect(r.code).toBe(1);
    expect(r.stdout).toContain("int 1");
    expect(r.stdout).toContain("int 2");
  });

  test("--json reports the diff structurally", () => {
    const a = file("let x = 1\nprint(x)\n");
    const b = file("let x = 2\nprint(x)\n");
    const r = petalCapture(["ir-equal", "--json", a, b]);
    expect(r.code).toBe(1);
    const out = JSON.parse(r.stdout);
    expect(out.equal).toBe(false);
    expect(out.diff.what).toBe("op");
    expect(out.diff.line).toBe(1);
  });

  test("--json on equal files", () => {
    const a = file("print(1)\n");
    const b = file("print(1)\n");
    const r = petalCapture(["ir-equal", "--json", a, b]);
    expect(r.code).toBe(0);
    expect(JSON.parse(r.stdout).equal).toBe(true);
  });

  test("a file that does not compile exits 2, not 1", () => {
    const a = file("print(1)\n");
    const b = file("print(1) let let\n");
    const r = petalCapture(["ir-equal", a, b]);
    expect(r.code).toBe(2);
  });
});

describe("petal lint --verify", () => {
  test("a formatting-only fix is proven and written", () => {
    const path = file("fn f(a)\nif a > 1 then\nreturn a\nend\nend\n");
    const r = petalCapture(["lint", "--fix", "--verify", path]);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain("IR unchanged");
    expect(readFileSync(path, "utf-8")).toContain("  if a > 1 then");
  });

  test("--check --verify leaves the file alone and still exits 1", () => {
    const before = "fn f(a)\nif a > 1 then\nreturn a\nend\nend\n";
    const path = file(before);
    const r = petalCapture(["lint", "--check", "--verify", path]);
    expect(r.code).toBe(1);
    expect(readFileSync(path, "utf-8")).toBe(before);
  });

  test("the if-chain to match rewrite is reported as a semantic IR change", () => {
    const path = file(CHAIN);
    const r = petalCapture(["lint", "--fix", "--verify", path]);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain("rewrite changed IR");
    expect(r.stderr).toContain("run-diff verification needed");
    // Still written: `--verify` (=ir) proves the formatting pass only.
    expect(readFileSync(path, "utf-8")).toContain("match ch");
  });

  test("--verify=strict refuses the semantic rewrite with exit 3", () => {
    const path = file(CHAIN);
    const r = petalCapture(["lint", "--fix", "--verify=strict", path]);
    expect(r.code).toBe(3);
    expect(r.stderr).toContain("not IR-equal");
    expect(r.stderr).toContain("refusing to write");
    expect(readFileSync(path, "utf-8")).toBe(CHAIN);
  });

  test("an unknown --verify mode is rejected", () => {
    const path = file("print(1)\n");
    const r = petalCapture(["lint", "--verify=bogus", path]);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("Unknown --verify mode");
  });

  test("a file needing no changes verifies quietly", () => {
    const path = file("print(1)\n");
    const r = petalCapture(["lint", "--fix", "--verify", path]);
    expect(r.code).toBe(0);
    expect(r.stderr).toBe("");
  });
});

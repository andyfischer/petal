import { describe, test, expect, beforeAll, afterAll } from "vitest";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { petalCapture, checkJson, checkStrict, showAstJson } from "./helpers";

// `pub` and its deprecated spelling `export`, at the CLI (the library-level
// contract is in core/tests/pub_keyword.rs). `export` keeps working; `check`
// warns about it, `lint --fix` and `fmt` rewrite it, and `ir-equal` cannot
// tell the two apart.

let dir: string;
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "petal-pub-"));
});
afterAll(() => rmSync(dir, { recursive: true, force: true }));

let counter = 0;
function file(contents: string): string {
  const path = join(dir, `f${counter++}.ptl`);
  writeFileSync(path, contents);
  return path;
}

const OLD = "export fn double(x)\n  x * 2\nend\nexport let limit = 10\nprint(double(limit))\n";
const NEW = "pub fn double(x)\n  x * 2\nend\npub let limit = 10\nprint(double(limit))\n";
const DEPRECATED = "`export` is deprecated";

describe("pub / export", () => {
  test("both spellings run, and print the same thing", () => {
    const a = petalCapture(["run", "-e", OLD]);
    const b = petalCapture(["run", "-e", NEW]);
    expect(a.code).toBe(0);
    expect(a.stdout).toBe("20\n");
    expect(b.stdout).toBe(a.stdout);
  });

  test("ir-equal: the two spellings are the same program", () => {
    const r = petalCapture(["ir-equal", file(OLD), file(NEW)]);
    expect(r.code).toBe(0);
  });

  test("the AST carries one flag for both", () => {
    expect(showAstJson("export let a = 1")[0].exported).toBe(true);
    expect(showAstJson("pub let a = 1")[0].exported).toBe(true);
  });

  test("check warns about each `export`, at the keyword, and still passes", () => {
    const r = petalCapture(["check", "-e", OLD]);
    expect(r.code).toBe(0);
    expect(r.stderr.split(DEPRECATED).length - 1).toBe(2);
    expect(r.stderr).toContain("write `pub` instead");

    const diags = checkJson(OLD).warnings.filter((w: any) =>
      w.message.includes(DEPRECATED)
    );
    expect(diags.map((w: any) => [w.severity, w.line, w.column])).toEqual([
      ["warning", 1, 1],
      ["warning", 4, 1],
    ]);
  });

  test("check is silent about `pub`", () => {
    const r = petalCapture(["check", "-e", NEW]);
    expect(r.code).toBe(0);
    expect(r.stderr).not.toContain("deprecated");
    expect(checkStrict(NEW).code).toBe(0);
  });

  test("lint reports prefer-pub and --fix rewrites the file", () => {
    const path = file(OLD);
    const report = petalCapture(["lint", path]);
    expect(report.code).toBe(1);
    expect(report.stdout).toContain(":1:1: prefer-pub: `export` is deprecated; write `pub`");
    expect(report.stdout).toContain(":4:1: prefer-pub:");

    const fix = petalCapture(["lint", "--fix", "--verify=strict", path]);
    expect(fix.code).toBe(0);
    expect(readFileSync(path, "utf-8")).toBe(NEW);
    expect(petalCapture(["lint", path]).code).toBe(0);
  });

  test("lint --rules lists prefer-pub", () => {
    expect(petalCapture(["lint", "--rules"]).stdout).toContain("prefer-pub");
  });

  test("fmt normalizes export to pub, and --check flags the old spelling", () => {
    expect(petalCapture(["fmt", "-e", OLD]).stdout).toBe(NEW);
    const path = file(OLD);
    expect(petalCapture(["fmt", "--check", path]).code).toBe(1);
    expect(petalCapture(["fmt", path]).code).toBe(0);
    expect(readFileSync(path, "utf-8")).toBe(NEW);
    expect(petalCapture(["fmt", "--check", path]).code).toBe(0);
  });

  test("`pub` is reserved; a dangling modifier is a parse error naming it", () => {
    expect(petalCapture(["run", "-e", "let pub = 1"]).code).not.toBe(0);
    const r = petalCapture(["run", "-e", "pub print(1)"]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("`pub` must be followed by");
  });
});

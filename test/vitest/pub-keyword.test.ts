import { describe, test, expect, beforeAll, afterAll } from "vitest";
import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { petalCapture, checkJson, checkStrict, showAstJson } from "./helpers";

// `pub` and its deprecated spelling `export`, at the CLI (the library-level
// contract is in core/tests/all/pub_keyword.rs). `export` keeps working; `lint`
// reports it (`old-syntax`), `migrate-old-code` rewrites it, `fmt` and
// `lint --fix` leave it, and `ir-equal` cannot tell the two apart.

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

// The keyword's migration alone: the whole command would also drop the `end`
// line (`end-lines`).
const MIGRATE = ["migrate-old-code", "--only", "export-to-pub"];
const OLD = "export fn double(x)\n  x * 2\nend\nexport let limit = 10\nprint(double(limit))\n";
const NEW = "pub fn double(x)\n  x * 2\nend\npub let limit = 10\nprint(double(limit))\n";

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

  test("the compile check is silent about either spelling: the old one is a rule's", () => {
    for (const source of [OLD, NEW]) {
      const r = petalCapture(["lint", "-e", source]);
      expect(r.code).toBe(0);
      expect(r.stderr).not.toContain("warning:");
      expect(checkJson(source).warnings).toEqual([]);
    }
    // A finding of a rule fails only a strict lint.
    expect(checkStrict(NEW).code).toBe(0);
    expect(checkStrict(OLD).code).toBe(1);
  });

  test("lint reports old-syntax and --fix leaves it; migrate-old-code rewrites it", () => {
    const path = file(OLD);
    const report = petalCapture(["lint", path]);
    expect(report.code).toBe(0);
    expect(report.stdout).toContain(
      ":1:1: old-syntax: `export` is deprecated; write `pub`; run `petal migrate-old-code` [export-to-pub]"
    );
    expect(report.stdout).toContain(":4:1: old-syntax:");
    expect(petalCapture(["lint", "--strict", path]).code).toBe(1);

    expect(petalCapture(["lint", "--fix", path]).code).toBe(0);
    expect(readFileSync(path, "utf-8")).toBe(OLD);

    expect(petalCapture([...MIGRATE, "--check", path]).code).toBe(1);
    const migrated = petalCapture([...MIGRATE, path]);
    expect(migrated.code).toBe(0);
    expect(readFileSync(path, "utf-8")).toBe(NEW);
    expect(petalCapture(["lint", "--strict", path]).code).toBe(0);
    expect(petalCapture([...MIGRATE, "--check", path]).code).toBe(0);
  });

  test("lint --rules lists old-syntax, and migrate-old-code --list lists export-to-pub", () => {
    expect(petalCapture(["lint", "--rules"]).stdout).toContain("old-syntax");
    expect(petalCapture(["lint", "--rules"]).stdout).not.toContain("prefer-pub");
    expect(petalCapture(["migrate-old-code", "--list"]).stdout).toContain("export-to-pub");
  });

  test("fmt leaves the old spelling as written", () => {
    expect(petalCapture(["fmt", "-e", OLD]).stdout).toBe(OLD);
    expect(petalCapture([...MIGRATE, "-e", OLD]).stdout).toBe(NEW);
  });

  test("`pub` is reserved; a dangling modifier is a parse error naming it", () => {
    expect(petalCapture(["run", "-e", "let pub = 1"]).code).not.toBe(0);
    const r = petalCapture(["run", "-e", "pub print(1)"]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("`pub` must be followed by");
  });
});

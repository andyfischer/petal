// `petal check` over several files, the literal key-name warning, and the
// missing-record-field hint (petal-movies follow-ups, 2026-10).
import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { mkdtempSync, writeFileSync, rmSync, readFileSync } from "fs";
import { tmpdir } from "os";
import { join, resolve } from "path";
import { petalCapture } from "./helpers";
import { mapKey } from "../../integrations/petal-web-canvas/src/input";

let dir: string;
let ok: string, warn: string, bad: string;

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "petal-check-many-"));
  ok = join(dir, "ok.ptl");
  warn = join(dir, "warn.ptl");
  bad = join(dir, "bad.ptl");
  writeFileSync(ok, "print(1)\n");
  writeFileSync(warn, 'import ui\nif key_pressed("enter") then print(1) end\n');
  writeFileSync(bad, "print(nope)\n");
});

afterAll(() => rmSync(dir, { recursive: true, force: true }));

describe("check: literal key names", () => {
  it("warns on a key name that is not one, naming the likely key", () => {
    const r = petalCapture(["check", warn]);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain(
      'argument 1 to `key_pressed`: `"enter"` is not a key name; did you mean `"return"`?',
    );
  });

  it("is silent on canonical names", () => {
    const r = petalCapture(["check", "-e", 'import ui\nprint(key_down("return"), key_released("leftbracket"))']);
    expect(r.code).toBe(0);
    expect(r.stderr).toBe("");
  });

  it("--strict fails on it", () => {
    expect(petalCapture(["check", "--strict", warn]).code).toBe(1);
  });
});

describe("check: several files", () => {
  it("passes when every file passes, printing nothing for clean files", () => {
    const r = petalCapture(["check", ok, ok]);
    expect(r.code).toBe(0);
    expect(r.stdout + r.stderr).toBe("");
  });

  it("checks every file and fails when one fails", () => {
    const r = petalCapture(["check", ok, warn, bad]);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain(`==> ${warn} <==`);
    expect(r.stderr).toContain(`==> ${bad} <==`);
    expect(r.stderr).not.toContain(`==> ${ok} <==`);
    expect(r.stderr).toContain("undefined variable `nope`");
    expect(r.stderr).toContain(`check failed for 1 of 3 files: ${bad}`);
  });

  it("--json prints one object with an entry per file", () => {
    const r = petalCapture(["check", "--json", ok, bad]);
    expect(r.code).toBe(1);
    const out = JSON.parse(r.stdout);
    expect(out.ok).toBe(false);
    expect(out.files.map((f: any) => [f.file, f.ok])).toEqual([
      [ok, true],
      [bad, false],
    ]);
  });

  it("refuses -e alongside a file", () => {
    const r = petalCapture(["check", "-e", "print(1)", ok]);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("a source was already given");
  });
});

describe("run: missing record field", () => {
  it("suggests ?? and has_field for an absent field", () => {
    const r = petalCapture(["run", "-e", "let r = {a: 1}\nprint(r.font != nil)"]);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain(
      "No field 'font' on record (for a field that may be absent, read `.font ?? nil` or test `has_field(rec, \"font\")` first)",
    );
  });

  it("suggests the near field for a slip", () => {
    const r = petalCapture(["run", "-e", "let r = {font: 1}\nprint(r.fnt)"]);
    expect(r.stderr).toContain("No field 'fnt' on record (did you mean 'font'?)");
  });
});

describe("petal-web-canvas key map", () => {
  // The canonical list, read from its one home.
  const keysRs = readFileSync(resolve(__dirname, "../../core/src/keys.rs"), "utf-8");
  const list = keysRs.slice(keysRs.indexOf("pub const KEY_NAMES"), keysRs.indexOf("];"));
  const canonical = new Set([...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]));

  it("maps keypad Enter to return, like Enter", () => {
    expect(mapKey("Enter", "NumpadEnter")).toBe("return");
    expect(mapKey("Enter", "Enter")).toBe("return");
  });

  it("maps every printable US-keyboard key onto a canonical name", () => {
    const keys = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-=[]\\;',./`~!@#$%^&*()_+{}|:\"<>?";
    for (const k of keys) {
      const name = mapKey(k, "");
      expect(canonical.has(name ?? ""), `${k} -> ${name}`).toBe(true);
    }
    for (const k of ["ArrowUp", "Home", "PageDown", "Delete", "F5", "Escape", " "]) {
      expect(canonical.has(mapKey(k, "") ?? ""), k).toBe(true);
    }
  });

  it("maps the keypad by physical key", () => {
    expect(mapKey("1", "Numpad1")).toBe("1");
    expect(mapKey("End", "Numpad1")).toBe("1");
    expect(mapKey("-", "NumpadSubtract")).toBe("minus");
    expect(mapKey("+", "NumpadAdd")).toBe(null);
  });
});

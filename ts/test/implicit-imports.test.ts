// A host's implicit imports (`Env::set_implicit_imports`) reach every module of
// a program, not just the entry file.
//
// The gap these pin (docs/dev/sharing-petal-libraries.md, "Language-level gaps"
// §1): a host registers a prelude — petal-ui registers `ui` — and promises that
// scripts get its names bare. That promise used to stop at the entry file, so a
// library module calling `draw_rect(rect, color)` resolved to the raw native's
// int-arity overload and died at runtime with "Expected int at arg 1, got
// record". Every library therefore had to hard-code `import ui: …` in each file.
//
// These run the real petal-ui host through its headless driver, since that is
// the only place in the tree where a host prelude is registered; the precedence
// and cycle cases live beside the implementation in `rust/src/module.rs`.

import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { existsSync } from "fs";
import { resolve } from "path";

const ROOT = resolve(__dirname, "../..");
const UI_RUN = resolve(ROOT, "petal-ui/target/debug/petal-ui-run");
const FIXTURES = resolve(__dirname, "fixtures/implicit-imports");

interface Frame {
  commands: any[];
  error: string | null;
  prints: string[];
}

/** One headless frame of a petal-ui app, parsed from the driver's JSONL. */
function runUiApp(file: string): Frame {
  const r = spawnSync(
    UI_RUN,
    [resolve(FIXTURES, file), "--frames", "1", "--seed", "1", "-I", FIXTURES],
    { encoding: "utf-8", timeout: 20000 }
  );
  if (r.status !== 0) {
    throw new Error(`petal-ui-run exited ${r.status}: ${r.stderr}`);
  }
  const line = (r.stdout || "").trim().split("\n")[0];
  return JSON.parse(line);
}

// The driver is built by CI's vitest job; a local tree that has never built
// petal-ui skips rather than failing.
const maybe = existsSync(UI_RUN) ? describe : describe.skip;

maybe("host implicit imports in imported modules", () => {
  it("a prelude name resolves inside an imported module", () => {
    const frame = runUiApp("app.ptl");
    expect(frame.error).toBe(null);
    // The prelude's record overload ran (not the native's int arity), so the
    // rect carries the record's fields and the screen width the prelude reports.
    expect(frame.commands).toEqual([
      { op: "rect", x: 8, y: 8, w: 784, h: 24, r: 10, g: 20, b: 30 },
    ]);
    expect(frame.prints).toEqual(["banner 8"]);
  });

  it("a module's own declaration shadows the implicit import, silently", () => {
    const frame = runUiApp("shadow_app.ptl");
    // No collision error: implicit imports stay weak. Inside the module its own
    // `screen_width` wins; in the entry the prelude's still does.
    expect(frame.error).toBe(null);
    expect(frame.prints).toEqual(["width 42", "entry width 800"]);
    expect(frame.commands).toEqual([
      { op: "rect", x: 0, y: 0, w: 42, h: 10, r: 1, g: 2, b: 3 },
    ]);
  });
});

// A library module reaches a host's names the way an entry file does: by
// importing the host's interface package (`import ui`).
//
// There are no implicit imports. These pin what a module that imports `ui`
// gets: the package's overloads (not the raw natives under them), at the
// plain-import tier — so the module's own declarations shadow them silently,
// wherever in the file they are written.
//
// These run the real petal-ui host through its headless driver; the
// precedence cases live beside the implementation in `core/src/module.rs`.

import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { existsSync } from "fs";
import { resolve } from "path";

const ROOT = resolve(__dirname, "../..");
const UI_RUN = resolve(ROOT, "core-libs/petal-ui/target/debug/petal-ui-run");
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

maybe("a module that imports ui", () => {
  it("a ui name resolves inside an imported module", () => {
    const frame = runUiApp("app.ptl");
    expect(frame.error).toBe(null);
    // The prelude's record overload ran (not the native's int arity), so the
    // rect carries the record's fields and the screen width the prelude reports.
    expect(frame.commands).toEqual([
      { op: "rect", x: 8, y: 8, w: 784, h: 24, r: 10, g: 20, b: 30 },
    ]);
    expect(frame.prints).toEqual(["banner 8"]);
  });

  it("a module's own declaration shadows the plain import, silently", () => {
    const frame = runUiApp("shadow_app.ptl");
    // No collision error: a plain import is weaker than a declaration. Inside the module its own
    // `screen_width` wins; in the entry the prelude's still does.
    expect(frame.error).toBe(null);
    expect(frame.prints).toEqual(["width 42", "entry width 800"]);
    expect(frame.commands).toEqual([
      { op: "rect", x: 0, y: 0, w: 42, h: 10, r: 1, g: 2, b: 3 },
    ]);
  });

  it("a module's own declaration wins even when it is written below its use", () => {
    // The trap a ~261-name import arms: a top-level `fn` whose name was
    // already in scope used to be left where it stands, so that a read of
    // the pre-shadow meaning above it still worked. With `ui` bound bare in
    // the module, that rule fired on any library function
    // whose name happened to collide — here `spinner`, which `ui` exports with
    // 3 or 4 parameters. `banner` called it with 1 and died on arity, with no
    // warning. Only a read that actually captures the old meaning holds a
    // declaration back now, and this module has none.
    const frame = runUiApp("late_app.ptl");
    expect(frame.error).toBe(null);
    expect(frame.prints).toEqual(["label own"]);
    // The module's own 1-argument `spinner` ran (h: 4 + 6); the entry file,
    // which shadows nothing, still reaches the prelude's arc-drawing one.
    expect(frame.commands[0]).toEqual({
      op: "rect",
      x: 0,
      y: 0,
      w: 20,
      h: 10,
      r: 4,
      g: 5,
      b: 6,
    });
    expect(frame.commands[1].op).toBe("arc");
  });
});

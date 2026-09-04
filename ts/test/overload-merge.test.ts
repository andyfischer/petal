// Overload sets merge across module boundaries (docs/function-overloading.md,
// "Across modules").
//
// The gap these pin (docs/dev/sharing-petal-libraries.md, "Language-level gaps"
// §6): a library could not *add* an arity to a name another module owned — a
// declaration replaced the whole set, so bloom could not give `ui`'s `draw_text`
// a fourth variant and shipped `ts_a(...)` instead. A declaration (or a
// higher-precedence import) now joins the set it lands on: it wins every arity
// it defines, and the arities only the outgoing binding had stay callable.
//
// Two lower-precedence sets are exercised: the core prelude `std`, reachable
// through the plain CLI, and the *host* prelude `ui`, which needs petal-ui's
// headless driver since that is the only place a host prelude is registered.

import { describe, it, expect } from "vitest";
import { spawnSync } from "child_process";
import { existsSync } from "fs";
import { resolve } from "path";
import { petalCapture, runPetalFile } from "./helpers";

const ROOT = resolve(__dirname, "../..");
const UI_RUN = resolve(ROOT, "petal-ui/target/debug/petal-ui-run");
const FIXTURES = resolve(__dirname, "fixtures/overload-merge");

const fixture = (name: string) => resolve(FIXTURES, name);

/** One headless frame of a petal-ui app, parsed from the driver's JSONL. */
function runUiApp(file: string): { commands: any[]; error: string | null } {
  const r = spawnSync(
    UI_RUN,
    [fixture(file), "--frames", "1", "--seed", "1", "-I", FIXTURES],
    { encoding: "utf-8", timeout: 20000 }
  );
  if (r.status !== 0) {
    throw new Error(`petal-ui-run exited ${r.status}: ${r.stderr}`);
  }
  return JSON.parse((r.stdout || "").trim().split("\n")[0]);
}

describe("overload sets merge across modules", () => {
  it("a selective import adds its arity to the core prelude's set", () => {
    // `counter` exports `count(xs)`; `std` exports `count(xs, pred)`. The
    // import is the higher-precedence binding and owns arity 1; std's arity 2
    // stays reachable instead of vanishing with the shadowed set.
    expect(runPetalFile(fixture("app_import.ptl"))).toBe("999\n2");
  });

  it("a module's own declaration joins a set the prelude owns", () => {
    // Both from inside the declaring module (`both()`) and from its importer.
    expect(runPetalFile(fixture("app_lib.ptl"))).toBe("[999, 2]\n999\n2");
  });

  it("an arity both sides define resolves to the higher-precedence one", () => {
    // The file's own `count(xs, pred)` wins arity 2; nothing errors, and the
    // rest of the prelude is untouched.
    expect(runPetalFile(fixture("app_collide.ptl"))).toBe("mine\n6");
  });

  it("a non-function binding still shadows the whole set", () => {
    expect(runPetalFile(fixture("app_nonfn.ptl"))).toBe("7\n6");
    // …and the set really is gone: calling it is the ordinary "not callable".
    const err = petalCapture(["run", fixture("app_nonfn_call.ptl")]);
    expect(err.code).not.toBe(0);
    expect(err.stderr).toContain("Cannot call");
  });

  it("a second declaration of the same arity in one file still replaces", () => {
    // Merging is a *cross-module* rule; within one file the later declaration
    // wins outright, as docs/function-overloading.md has always said.
    expect(runPetalFile(fixture("app_same_file.ptl"))).toBe("second\n2");
  });

  it("a nested fn shadows the merged set for the rest of its body", () => {
    expect(runPetalFile(fixture("app_nested.ptl"))).toBe("nested\n999\n2");
  });
});

// The driver is built by CI's vitest job; a local tree that has never built
// petal-ui skips rather than failing.
const maybe = existsSync(UI_RUN) ? describe : describe.skip;

maybe("overload sets merge with a host prelude", () => {
  it("a library module adds an arity to the host prelude's `draw_rect`", () => {
    // No `import ui` anywhere in the library: the host's implicit import is the
    // lower-precedence binding, and the module's 1-argument `draw_rect` joins
    // it. Both the added arity and the prelude's record form draw.
    const frame = runUiApp("ui_extend_app.ptl");
    expect(frame.error).toBe(null);
    expect(frame.commands).toEqual([
      { op: "rect", x: 0, y: 0, w: 10, h: 10, r: 9, g: 9, b: 9 },
      { op: "rect", x: 20, y: 0, w: 10, h: 10, r: 1, g: 2, b: 3 },
    ]);
  });

  it("a redeclared arity wins while the prelude's others stay reachable", () => {
    const frame = runUiApp("ui_collide_app.ptl");
    expect(frame.error).toBe(null);
    // x: 1 — the module's own 2-argument variant ran, not the prelude's.
    expect(frame.commands[0]).toEqual({
      op: "rect", x: 1, y: 0, w: 10, h: 10, r: 1, g: 2, b: 3,
    });
    // The prelude's 3-argument alpha form is still callable under the same name.
    expect(frame.commands[1]).toMatchObject({ op: "rect", x: 40, r: 4, g: 5, b: 6 });
    expect(frame.commands[1]).toHaveProperty("a");
  });
});

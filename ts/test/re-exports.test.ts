// Re-exports: `export import m: *` and friends (docs/module-system.md).
//
// A library's facade used to be a hand-maintained list of
// `export let button = widgets_button.button` lines — correct, but silent when
// a name was forgotten. `export import` makes it declarative: the facade names
// the modules, not their exports. fixtures/re-exports/ is a small library in
// that shape (widgets.ptl over widgets/{button,menu,theme}.ptl) plus the
// error cases.

import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { petalCapture, runPetalFile } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/re-exports");
const fixture = (name: string) => resolve(FIXTURES, name);

/** Run `petal run <file>` expecting failure; return stderr. */
function runFileError(file: string): string {
  const r = petalCapture(["run", fixture(file)]);
  if (r.code === 0) throw new Error("Expected petal to fail but it succeeded");
  return r.stderr.trim();
}

describe("re-exports", () => {
  it("a star re-export carries a whole overload set through the facade", () => {
    // Both arities of `button` reach the app through widgets.ptl, which never
    // names `button` at all.
    const out = runPetalFile(fixture("app.ptl"));
    expect(out).toBe("[ok]\n[ok|4]\ncyan\nmenu open\n[x]cyan");
  });

  it("a re-export chain passes names along", () => {
    // chain_top.ptl star-re-exports widgets.ptl, which star-re-exports
    // widgets/button.ptl — and the overload set survives both hops.
    const out = runPetalFile(fixture("chain_app.ptl"));
    expect(out).toBe("[chain]\n[chain|1]\ncyan");
  });

  it("a local declaration wins over a star re-export, silently", () => {
    const out = runPetalFile(fixture("local_wins_app.ptl"));
    expect(out).toBe("<a>");
  });

  it("naming an export the module does not have is an error", () => {
    const err = runFileError("missing_app.ptl");
    expect(err).toContain("module 'widgets/menu' has no export 'nope'");
    expect(err).toContain("close, open");
  });

  it("two star re-exports of one value name collide loudly", () => {
    const err = runFileError("collide_app.ptl");
    expect(err).toContain(
      "'shared' is re-exported by both 'collide_a' and 'collide_b'"
    );
  });

  it("a re-export cycle is an error, not a hang", () => {
    const err = runFileError("cycle_app.ptl");
    expect(err).toContain("import cycle");
  });

  it("a star re-export does not widen the module's privacy rule", () => {
    // widgets/theme.ptl's `private_helper` has no `export`, so no star can
    // pick it up.
    const err = runFileError("private_app.ptl");
    expect(err).toContain("module 'widgets' has no export 'private_helper'");
  });
});

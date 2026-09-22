// `petal check` reports a call to, or read of, a global that nothing defines.
// The compiler lets both through (a call becomes a static builtin call that is
// resolved when it runs, a read becomes a deferred error), so before this a
// misspelled function passed `check --strict` and died with "Unknown builtin"
// on first run. The host's natives decide what is known: `--host` picks a set
// (default `ui`: core + petal-ui + the `ui` prelude), `--native` adds names.
// See rust/src/typecheck/globals.rs.

import { describe, it, expect } from "vitest";
import { petalCapture, checkStrict } from "./helpers";

function checkWith(args: string[], code: string) {
  const r = petalCapture(["check", "--json", ...args, "-e", code]);
  return JSON.parse(r.stdout).warnings.map((w: any) => w.message) as string[];
}

describe("check: unknown globals", () => {
  it("flags a call to an unknown function under --strict", () => {
    const r = checkStrict("totally_bogus_fn(1)");
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("unknown function `totally_bogus_fn`");
  });

  it("flags a read of an undefined variable, keeping the compiler's hint", () => {
    const msgs = checkWith([], "print(null)");
    expect(msgs).toEqual([
      "undefined variable `null` — use 'nil' for null/empty values in Petal",
    ]);
  });

  it("accepts core builtins, petal-ui natives and the ui prelude by default", () => {
    const r = checkStrict(`draw_rect(0, 0, 4, 4, 255, 0, 0)
let f = mouse_x
let b = button({x: 0, y: 0, w: 40, h: 20}, "ok")
print(len([1]), theme)`);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });

  it("accepts a declared function called through a parameter", () => {
    const r = checkStrict(`fn g(k) k(1) end
print(g(fn(x) -> x + 1))`);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });

  it("--host core does not know the petal-ui natives", () => {
    expect(checkWith(["--host", "core"], "draw_rect(0, 0, 4, 4, 255, 0, 0)")).toEqual([
      expect.stringContaining("unknown function `draw_rect`"),
    ]);
  });

  it("--host garden adds Garden's natives, which the ui default lacks", () => {
    expect(checkWith([], "print(palette())")).toEqual([
      expect.stringContaining("unknown function `palette`"),
    ]);
    expect(checkWith(["--host", "garden"], "print(palette())")).toEqual([]);
  });

  it("--native names extra host natives", () => {
    expect(checkWith(["--native", "wf_action,wf_goto"], "wf_action(1)\nwf_goto(2)")).toEqual([]);
  });

  it("rejects an unknown --host", () => {
    const r = petalCapture(["check", "--host", "nope", "-e", "1"]);
    expect(r.code).toBe(2);
    expect(r.stderr).toContain("Unknown --host 'nope'");
  });
});

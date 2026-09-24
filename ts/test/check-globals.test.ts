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

  it("--host garden resolves the packages Garden registers, with no -I", () => {
    const src = "import text_layout\nimport bloom\nprint(text_layout.line_step({size: 12}))";
    expect(checkWith(["--host", "garden"], src)).toEqual([]);
    const r = petalCapture(["check", "-e", "import text_layout"]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("cannot find module 'text_layout'");
  });

  it("--host garden-config checks a layout script without the ui prelude", () => {
    // Garden's config host registers `row(children)`; the ui prelude's
    // 3-argument `row` must not shadow it.
    const layout = `layout(column([row([editor("a"), editor("b")]), editor("c")], [0.7, 0.3]))`;
    expect(checkWith(["--host", "garden"], layout)).toEqual([
      expect.stringContaining("`row` expects 3 arguments"),
    ]);
    expect(checkWith(["--host", "garden-config"], layout)).toEqual([]);
    expect(checkWith(["--host", "garden-config"], "button(1)")).toEqual([
      expect.stringContaining("unknown function `button`"),
    ]);
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

// Natives declare argument slots, and the ui prelude's un-annotated draw
// overloads are held to what their bodies do with each parameter, so a call
// whose argument count selects the wrong overload is caught by `check`
// instead of failing inside the prelude. See rust/src/typecheck/param_reqs.rs.
describe("check: native arguments and prelude overloads", () => {
  it("flags a native argument of the wrong type", () => {
    expect(checkWith([], 'print(sqrt("x"), range({r: 1}))')).toEqual([
      "argument 1 to `sqrt`: expected a number, found `string`",
      "argument 1 to `range`: expected a number, found `record`",
    ]);
  });

  it("flags a prelude overload that cannot take its arguments", () => {
    const msgs = checkWith([], "draw_rect(0, 0, {r: 1}, 4, 1, 2, 3)\ndraw_rect(0, 0, 10, 10, 5)");
    expect(msgs).toHaveLength(2);
    expect(msgs[0]).toContain(
      "argument 3 to `draw_rect`: the 7-argument `draw_rect` uses it as a number, found `record`",
    );
    expect(msgs[1]).toContain(
      "argument 5 to `draw_rect`: the 5-argument `draw_rect` reads field `r` from it, found `int`",
    );
  });

  it("accepts every shape the prelude really takes", () => {
    const r = checkStrict(`let C = {r: 255, g: 0, b: 0}
let R = {x: 0, y: 0, w: 10, h: 10}
draw_rect(0, 0, 10, 10, C)
draw_rect(0, 0, 10, 10, 255, 0, 0)
draw_rect(R, C)
draw_rect(R, C, 128)
draw_line(0, 0, 10, 10, C, 255, 2)
draw_line(0, 0, 10, 10, 1, 2, 3)
draw_circle_outline(5, 5, 3, C)
draw_polyline([{x: 0, y: 0}, {x: 1, y: 1}], C, 235, 1)`);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });
});

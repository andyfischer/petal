// `petal check` reports a call to, or read of, a name that nothing defines.
// The compiler lets both through (each becomes a deferred error that fires if
// the line runs), so before this a misspelled function passed
// `check --strict` and died on first run. A name is known because the file
// declares it, an import binds it, or it is a core/std builtin — and for no
// other reason: a host's names come from the interface package the script
// imports (`import ui`), bundled in the binary or found with `-I`. There is
// no `--host`. See core/src/typecheck/globals.rs.

import { describe, it, expect } from "vitest";
import { petalCapture, checkStrict } from "./helpers";

function checkWith(args: string[], code: string) {
  const r = petalCapture(["check", "--json", ...args, "-e", code]);
  return JSON.parse(r.stdout).warnings.map((w: any) => w.message) as string[];
}

describe("check: unknown globals", () => {
  it("fails plain check (no --strict) on a name nothing defines", () => {
    const r = petalCapture(["check", "-e", "print(nope)"]);
    expect(r.code).toBe(1);
    expect(r.stderr).toContain("error: undefined variable `nope`");
    // No host profile is involved, so there is nothing to say about one.
    expect(r.stderr).not.toContain("--host");
    expect(r.stderr).toContain("1 error found by check");
  });

  it("reports it as severity error with ok false in --json", () => {
    const r = petalCapture(["check", "--json", "-e", "frob(1)"]);
    expect(r.code).toBe(1);
    const out = JSON.parse(r.stdout);
    expect(out.ok).toBe(false);
    expect(out.warnings.map((w: any) => w.severity)).toEqual(["error"]);
  });

  it("--lenient reports the error but exits 0", () => {
    const r = petalCapture(["check", "--lenient", "-e", "print(nope)"]);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain("error: undefined variable `nope`");
  });

  it("leaves a warnings-only program at exit 0", () => {
    const r = petalCapture(["check", "-e", 'let x: int = "s"']);
    expect(r.code).toBe(0);
    expect(r.stderr).toContain("warning: type mismatch");
  });

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

  it("accepts core builtins bare, and the ui package through its import", () => {
    const body = `draw_rect(0, 0, 4, 4, 255, 0, 0)
let f = mouse_x
let b = button({x: 0, y: 0, w: 40, h: 20}, "ok")
print(len([1]), theme)`;
    const r = checkStrict(`import ui\n${body}`);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
    // Without the import the ui names are unknown: nothing is implicit.
    const msgs = checkWith([], body);
    expect(msgs.some((m) => m.includes("unknown function `draw_rect`"))).toBe(true);
    expect(msgs.some((m) => m.includes("undefined variable `mouse_x`"))).toBe(true);
  });

  it("accepts a declared function called through a parameter", () => {
    const r = checkStrict(`fn g(k) k(1) end
print(g(fn(x) -> x + 1))`);
    expect(r.stderr).toBe("");
    expect(r.code).toBe(0);
  });

  it("check and run agree on a ui script: check passes, run refuses it at load", () => {
    const src = 'import ui\nprint("ran")\ndraw_rect(0, 0, 4, 4, 255, 0, 0)';
    expect(checkWith([], src)).toEqual([]);
    const r = petalCapture(["run", "-e", src]);
    expect(r.code).toBe(1);
    expect(r.stdout).not.toContain("ran");
    expect(r.stderr).toContain("module 'ui' needs native fns this host does not register");
    expect(r.stderr).toContain("petal-ui runner");
  });

  it("finds Garden's natives through its interface package on -I", () => {
    const garden = ["-I", "garden/garden-script/packages"];
    expect(checkWith(garden, "print(palette())")).toEqual([
      expect.stringContaining("unknown function `palette`"),
    ]);
    expect(checkWith(garden, "import garden\nprint(palette())")).toEqual([]);
    // With no -I the package is not found: core knows nothing about Garden.
    const r = petalCapture(["check", "-e", "import garden\nprint(palette())"]);
    expect(r.code).not.toBe(0);
    expect(r.stderr).toContain("garden");
  });

  it("resolves the bundled libraries with no -I", () => {
    const src = "import text_layout\nimport bloom\nprint(text_layout.line_step({size: 12}))";
    expect(checkWith([], src)).toEqual([]);
  });

  it("checks a Garden layout script against garden_config alone", () => {
    // The config host registers `row(children)`; the ui package's `row` is
    // not in scope unless the script imports it.
    const garden = ["-I", "garden/garden-script/packages"];
    const layout = `layout(column([row([editor("a"), editor("b")]), editor("c")], [0.7, 0.3]))`;
    expect(checkWith(garden, `import garden_config\n${layout}`)).toEqual([]);
    expect(checkWith(garden, "import garden_config\nbutton(1)")).toEqual([
      expect.stringContaining("unknown function `button`"),
    ]);
  });

  it("a host native is known through a declaration in the file", () => {
    const decls = "native fn wf_action(n)\nnative fn wf_goto(n)\n";
    expect(checkWith([], `${decls}wf_action(1)\nwf_goto(2)`)).toEqual([]);
    expect(checkWith([], "wf_action(1)")).toEqual([
      expect.stringContaining("unknown function `wf_action`"),
    ]);
  });

  it("--host and --native are gone", () => {
    for (const flag of [["--host", "ui"], ["--native", "a,b"]]) {
      const r = petalCapture(["check", ...flag, "-e", "1"]);
      expect(r.code).not.toBe(0);
    }
  });
});

// Natives declare argument slots, and the ui prelude's un-annotated draw
// overloads are held to what their bodies do with each parameter, so a call
// whose argument count selects the wrong overload is caught by `check`
// instead of failing inside the prelude. See core/src/typecheck/param_reqs.rs.
describe("check: native arguments and prelude overloads", () => {
  it("flags a native argument of the wrong type", () => {
    expect(checkWith([], 'print(sqrt("x"), range({r: 1}))')).toEqual([
      "argument 1 to `sqrt`: expected a number, found `string`",
      "argument 1 to `range`: expected a number, found `record`",
    ]);
  });

  it("flags a prelude overload that cannot take its arguments", () => {
    const msgs = checkWith([], "import ui\ndraw_rect_outline(0, 0, 10, 10, 5)");
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toContain(
      "argument 5 to `draw_rect_outline`: the 5-argument `draw_rect_outline` reads field `r` from it, found `int`",
    );
    // An optional trailing argument is held to its use as well.
    expect(checkWith([], "import ui\ndraw_rect_outline({x: 0, y: 0, w: 1, h: 1}, {r: 1, g: 2, b: 3}, {r: 1})")[0]).toContain(
      "argument 3 to `draw_rect_outline`: the 2-4-argument `draw_rect_outline` uses it as a number, found `record`",
    );
  });

  it("holds a prelude native with several declared shapes to their types", () => {
    // `draw_rect` is one native declared in three shapes. A number
    // parameter is checked; the colour is not, since it may be a record or
    // a packed int.
    const msgs = checkWith([], "import ui\ndraw_rect(0, 0, {r: 1}, 4, 1, 2, 3)\ndraw_rect(0, 0, 10, 10, 5)\ndraw_rect({x: 0, y: 0, w: 1, h: 1}, {r: 1, g: 2, b: 3}, {r: 1})");
    expect(msgs).toEqual([
      "argument 3 to `draw_rect`: expected a number, found `record`",
      "argument 3 to `draw_rect`: expected a number, found `record`",
    ]);
  });

  it("accepts every shape the prelude really takes", () => {
    const r = checkStrict(`import ui
let C = {r: 255, g: 0, b: 0}
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

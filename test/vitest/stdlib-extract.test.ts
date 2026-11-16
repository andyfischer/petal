// Guards the stdlib doc extractor (tools/extract-stdlib.ts) against drift.
//
// The extractor reads the Rust registration tables (what exists, aliases, the
// implementation's line) and joins them with `petal doc --json`: each
// builtin's declared call forms, parameter names and types (the ones named
// arguments bind against) and its @doc. These tests don't pin the exact function count
// (which grows as the language does) — they assert the *invariants* that the
// docs site relies on, so a refactor that breaks extraction fails loudly here
// rather than silently producing an empty or wrong reference.

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { buildManifest } from "../../tools/extract-stdlib";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const manifest = buildManifest();
const byName = new Map(manifest.functions.map((f) => [f.name, f]));

/** The parameters every call must pass: `params` without the optional tail. */
const required = (name: string) => byName.get(name)!.params.filter((p) => !p.optional);

describe("stdlib extractor", () => {
  it("recovers every name registered in register_builtins", () => {
    const modRs = readFileSync(
      join(repoRoot, "core/src/builtins/mod.rs"),
      "utf8",
    );
    const block = modRs.slice(modRs.indexOf("pub fn register_builtins"));
    const registered = [...block.matchAll(/table\.register(?:_with)?\(\s*"([^"]+)"/g)].map(
      (m) => m[1],
    );
    // De-dupe: a name can't be registered twice, but be defensive.
    for (const name of new Set(registered)) {
      expect(byName.has(name), `missing builtin: ${name}`).toBe(true);
    }
    expect(registered.length).toBeGreaterThan(60);
  });

  it("recovers the canvas drawing + input builtins", () => {
    for (const name of [
      "draw_rect",
      "draw_circle",
      "clear",
      "mouse_x",
      "key_down",
      "dt",
      "screen_width",
    ]) {
      expect(byName.has(name), `missing canvas builtin: ${name}`).toBe(true);
    }
  });

  it("reads argument names + types from the declarations", () => {
    // draw_rect's declared parameters (`native fn draw_rect(x: num, …)` in
    // ui/_base.ptl), types as written.
    const drawRect = required("draw_rect");
    expect(drawRect.map((p) => p.name)).toEqual([
      "x",
      "y",
      "w",
      "h",
      "r",
      "g",
      "b",
    ]);
    expect(drawRect.every((p) => p.type === "num")).toBe(true);
    expect(byName.get("draw_rect")!.declaration).toMatchObject({
      module: "ui/_base",
      file: "core-libs/petal-ui/prelude/_base.ptl",
    });
    // …and the optional alpha it also takes is there, flagged.
    expect(byName.get("draw_rect")!.params.at(-1)).toMatchObject({
      name: "a",
      optional: true,
    });

    // The declared name (`value`), not the body's local (`v`).
    const mapRange = byName.get("map_range")!;
    expect(mapRange.arity).toBe(5);
    expect(mapRange.params.map((p) => p.name)).toEqual([
      "value",
      "in_lo",
      "in_hi",
      "out_lo",
      "out_hi",
    ]);
    expect(mapRange.params.every((p) => p.type === "num")).toBe(true);
  });

  it("lists every call form a builtin declares for named arguments", () => {
    const forms = (name: string) =>
      byName
        .get(name)!
        .signatures.map((s) => s.map((p) => p.name + (p.optional ? "?" : "")).join(", "));
    expect(forms("clamp")).toEqual(["value, lo, hi"]);
    expect(forms("slice")).toEqual(["collection, start, end?"]);
    expect(forms("random")).toEqual(["", "max", "min, max"]);
    expect(forms("distance")).toEqual(["x1, y1, x2, y2", "v1, v2"]);
    expect(forms("map")).toEqual(["list, f"]);
    expect(forms("mouse_down")).toEqual(["button"]);
    expect(forms("text_width")).toEqual(["text, style", "text, size, font?"]);
    // `params` is the longest form.
    expect(byName.get("random")!.params.map((p) => p.name)).toEqual(["min", "max"]);
    // A prelude `fn` takes names under its own parameters.
    expect(forms("take")).toEqual(["xs, n"]);
  });

  it("leaves only the variadic and internal builtins without a call form", () => {
    const undeclared = manifest.functions
      .filter((f) => f.signatures.length === 0)
      .map((f) => f.name)
      .sort();
    expect(undeclared).toEqual(["__pending", "__reject", "__resolve", "format", "print", "speculate", "speculate_value"]);
  });

  it("keeps every argument in order for the point-list and arc natives", () => {
    // Regression (from when parameters were read off the Rust bodies): a
    // point-list or float argument read through a helper went missing, so
    // `fill_arc` documented its colours as arguments 3-5 when they are really
    // 7-9, and `fill_poly` lost its point list entirely.
    expect(required("fill_arc").map((p) => p.name)).toEqual([
      "cx",
      "cy",
      "r_in",
      "r_out",
      "a0",
      "a1",
      "r",
      "g",
      "b",
    ]);
    expect(byName.get("fill_arc")!.params[2].type).toBe("num");

    for (const name of ["fill_poly", "fill_polygon"]) {
      const params = required(name);
      expect(params.length, `${name} lost its point list`).toBe(4);
      expect(params[0].name).toBe("points");
      expect(params.slice(1).map((p) => p.name)).toEqual(["r", "g", "b"]);
    }
    // `draw_polyline` takes its colour as one value or as three, so only the
    // points and the first colour argument are required.
    const polyline = byName.get("draw_polyline")!;
    expect(polyline.params.map((p: any) => p.name)).toEqual(["points", "r", "g", "b", "a", "width"]);
    expect(required("draw_polyline").map((p) => p.name)).toEqual(["points", "r"]);
  });

  it("documents a full colour triple for every solid-colour drawing call", () => {
    // A drawing native that takes r/g/b must say so. A body that reads its
    // colour through a loop index (`for i in 4..=6 { state.get_int(i) }`)
    // leaves the extractor with no name to report, which reads in the docs as
    // "this call takes no colour".
    for (const name of [
      "draw_rect",
      "draw_circle",
      "draw_circle_outline",
      "draw_ellipse",
      "draw_ellipse_outline",
      "fill_arc",
      "fill_fan",
      "fill_triangle",
      "fill_poly",
    ]) {
      const names = required(name).map((p) => p.name);
      expect(names.slice(-3), `${name} is missing r/g/b`).toEqual([
        "r",
        "g",
        "b",
      ]);
    }
  });

  it("flags arg-count-dispatching builtins as variadic", () => {
    expect(byName.get("noise")!.variadic).toBe(true);
    expect(byName.get("print")!.variadic).toBe(true);
    expect(byName.get("slice")!.variadic).toBe(true);
    expect(byName.get("abs")!.variadic).toBe(false);
  });

  it("detects aliases that share an implementation", () => {
    expect(byName.get("includes")!.aliasOf).toBe("contains");
  });

  it("recovers the Petal-source std prelude (core/prelude/std.ptl)", () => {
    // Every `pub fn` in the prelude should surface as a stdlib function.
    for (const name of [
      "first",
      "is_empty",
      "sum",
      "product",
      "mean",
      "minimum",
      "maximum",
      "count",
      "take",
      "drop",
      "clamp01",
    ]) {
      const fn = byName.get(name);
      expect(fn, `missing prelude fn: ${name}`).toBeDefined();
      expect(fn!.group).toBe("prelude");
      expect(fn!.category).toBe("std");
      expect(fn!.source.file).toBe("core/prelude/std.ptl");
      expect(fn!.source.line).toBeGreaterThan(0);
    }
  });

  it("lists the native predicate queries as core builtins", () => {
    for (const name of ["any", "all", "find"]) {
      expect(byName.get(name)!.group, name).toBe("core");
      expect(byName.get(name)!.source.file, name).toBe("core/src/builtins/mod.rs");
    }
  });

  it("reads prelude parameter names + arity from the Petal source", () => {
    const take = byName.get("take")!;
    expect(take.arity).toBe(2);
    expect(take.variadic).toBe(false);
    expect(take.params.map((p) => p.name)).toEqual(["xs", "n"]);
    // std.ptl declares no parameter types.
    expect(take.params.every((p) => p.type === "any")).toBe(true);

    expect(byName.get("clamp01")!.params.map((p) => p.name)).toEqual(["x"]);
    expect(byName.get("find")!.params.map((p) => p.name)).toEqual([
      "xs",
      "pred",
    ]);
    expect(byName.get("sum")!.arity).toBe(1);
  });

  it("points every function at a real source location", () => {
    for (const fn of manifest.functions) {
      // Native + canvas builtins live in Rust; the core prelude lives in Petal.
      expect(fn.source.file, fn.name).toMatch(/\.(rs|ptl)$/);
      expect(fn.source.line, fn.name).toBeGreaterThan(0);
    }
  });

  it("finds a Petal declaration for every registered function", () => {
    // A native registered in Rust with no `native fn` declaring it could not
    // be called by name from a script, and would have no doc.
    for (const fn of manifest.functions) {
      expect(fn.declaration, fn.name).toBeDefined();
      expect(fn.declaration!.file, fn.name).toMatch(/\.ptl$/);
      expect(fn.declaration!.line, fn.name).toBeGreaterThan(0);
    }
    expect(byName.get("clamp")!.declaration!.module).toBe("core/math");
    expect(byName.get("noise")!.declaration!.module).toBe("noise");
    expect(byName.get("noise")!.import).toBe("noise");
    expect(byName.get("clamp")!.import).toBeUndefined();
  });

  it("carries each declaration's @doc as petal doc prints it", () => {
    // `doc` is always present: the @doc object, or null when undocumented.
    const keys = ["body", "summary", "params", "returns", "options", "example", "see", "deprecated", "hidden", "extra"];
    for (const fn of manifest.functions) {
      expect(fn, fn.name).toHaveProperty("doc");
      if (fn.doc !== null) expect(Object.keys(fn.doc).sort(), fn.name).toEqual([...keys].sort());
    }
  });

  it("assigns every function to a declared category", () => {
    const ids = new Set(manifest.categories.map((c) => c.id));
    for (const fn of manifest.functions) {
      expect(ids.has(fn.category), `${fn.name} → ${fn.category}`).toBe(true);
    }
  });

  it("gives every category a friendly (non-id) title", () => {
    // The site sidebar shows these titles; a bare lowercase id means the
    // category was added in Rust without a CATEGORY_TITLES entry here.
    for (const cat of manifest.categories) {
      expect(cat.title, `category "${cat.id}" has no friendly title`).not.toBe(
        cat.id,
      );
    }
  });

  it("marks __-prefixed builtins internal so the public reference can hide them", () => {
    expect(byName.get("__pending")!.internal).toBe(true);
    expect(byName.get("__resolve")!.internal).toBe(true);
    // Public builtins carry no internal flag.
    expect(byName.get("abs")!.internal).toBeUndefined();
    for (const fn of manifest.functions) {
      if (fn.internal) expect(fn.name.startsWith("__")).toBe(true);
    }
  });

  it("keeps the committed docs/stdlib.json in sync with the extractor", () => {
    // The manifest is a generated artifact checked into the repo so the docs
    // site (and the CI drift gate) can consume it without re-parsing Rust.
    // If this fails, run `npm run stdlib:json` and commit the result.
    const committedPath = join(repoRoot, "docs/stdlib.json");
    const committed = JSON.parse(readFileSync(committedPath, "utf8"));
    expect(committed).toEqual(manifest);
  });
});

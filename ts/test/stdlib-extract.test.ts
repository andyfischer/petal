// Guards the stdlib doc extractor (tools/extract-stdlib.ts) against drift.
//
// The extractor reads the Rust registration tables and recovers each builtin's
// arity and argument names. These tests don't pin the exact function count
// (which grows as the language does) — they assert the *invariants* that the
// docs site relies on, so a refactor that breaks extraction fails loudly here
// rather than silently producing an empty or wrong reference.

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { buildManifest } from "../tools/extract-stdlib";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const manifest = buildManifest();
const byName = new Map(manifest.functions.map((f) => [f.name, f]));

describe("stdlib extractor", () => {
  it("recovers every name registered in register_builtins", () => {
    const modRs = readFileSync(
      join(repoRoot, "rust/src/builtins/mod.rs"),
      "utf8",
    );
    const block = modRs.slice(modRs.indexOf("pub fn register_builtins"));
    const registered = [...block.matchAll(/table\.register\(\s*"([^"]+)"/g)].map(
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

  it("reads argument names + types straight from the source", () => {
    // draw_rect's `let x = state.get_int(1)` … bindings give a full signature.
    const drawRect = byName.get("draw_rect")!;
    expect(drawRect.params.map((p) => p.name)).toEqual([
      "x",
      "y",
      "w",
      "h",
      "r",
      "g",
      "b",
    ]);
    expect(drawRect.params.every((p) => p.type === "int")).toBe(true);

    const mapRange = byName.get("map_range")!;
    expect(mapRange.arity).toBe(5);
    expect(mapRange.params.map((p) => p.name)).toEqual([
      "v",
      "in_lo",
      "in_hi",
      "out_lo",
      "out_hi",
    ]);
  });

  it("recovers arguments read through a helper, not just state.get_*", () => {
    // Regression: when draw.rs moved its point-list and float arguments behind
    // `point_list_arg(state, i, …)` / `get_num(state, i, …)`, the extractor saw
    // no binding at those indices and silently dropped them — so `fill_arc`
    // documented its colours as arguments 3-5 when they are really 7-9, and
    // `fill_poly` lost its point list entirely.
    expect(byName.get("fill_arc")!.params.map((p) => p.name)).toEqual([
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
    expect(byName.get("fill_arc")!.params[2].type).toBe("float");

    for (const name of ["fill_poly", "fill_polygon", "draw_polyline"]) {
      const fn = byName.get(name)!;
      expect(fn.params.length, `${name} lost its point list`).toBe(4);
      expect(fn.params[0].type).toBe("list");
      expect(fn.params.slice(1).map((p) => p.name)).toEqual(["r", "g", "b"]);
    }
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
      "draw_polyline",
    ]) {
      const names = byName.get(name)!.params.map((p) => p.name);
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

  it("recovers the Petal-source std prelude (rust/prelude/std.ptl)", () => {
    // Every `export fn` in the prelude should surface as a stdlib function.
    for (const name of [
      "first",
      "is_empty",
      "sum",
      "product",
      "mean",
      "minimum",
      "maximum",
      "count",
      "any",
      "all",
      "find",
      "take",
      "drop",
      "clamp01",
    ]) {
      const fn = byName.get(name);
      expect(fn, `missing prelude fn: ${name}`).toBeDefined();
      expect(fn!.group).toBe("prelude");
      expect(fn!.category).toBe("std");
      expect(fn!.source.file).toBe("rust/prelude/std.ptl");
      expect(fn!.source.line).toBeGreaterThan(0);
    }
  });

  it("reads prelude parameter names + arity from the Petal source", () => {
    const take = byName.get("take")!;
    expect(take.arity).toBe(2);
    expect(take.variadic).toBe(false);
    expect(take.params.map((p) => p.name)).toEqual(["xs", "n"]);
    // Petal source carries no static types, so params are untyped.
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

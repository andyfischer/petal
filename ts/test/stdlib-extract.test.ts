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

  it("flags arg-count-dispatching builtins as variadic", () => {
    expect(byName.get("noise")!.variadic).toBe(true);
    expect(byName.get("print")!.variadic).toBe(true);
    expect(byName.get("slice")!.variadic).toBe(true);
    expect(byName.get("abs")!.variadic).toBe(false);
  });

  it("detects aliases that share an implementation", () => {
    expect(byName.get("includes")!.aliasOf).toBe("contains");
  });

  it("points every function at a real source location", () => {
    for (const fn of manifest.functions) {
      expect(fn.source.file, fn.name).toMatch(/\.rs$/);
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

  it("keeps the committed docs/stdlib.json in sync with the extractor", () => {
    // The manifest is a generated artifact checked into the repo so the docs
    // site (and the CI drift gate) can consume it without re-parsing Rust.
    // If this fails, run `npm run stdlib:json` and commit the result.
    const committedPath = join(repoRoot, "docs/stdlib.json");
    const committed = JSON.parse(readFileSync(committedPath, "utf8"));
    expect(committed).toEqual(manifest);
  });
});

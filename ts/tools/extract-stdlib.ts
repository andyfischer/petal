#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
//
// extract-stdlib.ts — Generate a structured manifest of Petal's standard
// library directly from the Rust source of truth.
//
// The point of this tool is that documentation can never silently drift from
// the implementation: the *list* of functions, their arity, and their argument
// names are all read out of the Rust source rather than maintained by hand.
// Prose and examples live elsewhere (markdown), but the spine — what exists —
// comes from here.
//
// Two registration tables are parsed:
//
//   1. Core builtins — `rust/src/builtins/mod.rs`'s `register_builtins()`,
//      which is the canonical, append-only list of `table.register("name", …)`
//      calls. Each entry points at a `native_*` fn in a topic submodule
//      (math.rs, collections.rs, …); the submodule it lives in becomes the
//      function's category.
//
//   2. Canvas builtins — the shared `petal-ui` crate's `register_draw` +
//      `register_canvas` (drawing) and `register_input` (input/timing), the
//      interactivity API that hosts like petal-web-canvas and petal-sdl expose
//      to sketches.
//
// For each registered function we open its implementation and read:
//   • arity      — from `require_args(state, N, "name")`
//   • parameters — from `let <name> = state.get_<type>(<index>)` bindings,
//                  which give both the argument's name and its type, in order
//   • source     — file + line, so docs can point back at the implementation
//
// Functions that dispatch on `arg_count()` (overloaded arities like `noise`,
// `distance`, `mag`, `range`, `slice`) can't be summarised by a single
// signature; they're flagged `variadic` and their human-facing signature is
// expected to come from the markdown overlay instead.
//
// Usage:
//   tsx tools/extract-stdlib.ts            # write stdlib.json next to docs/
//   tsx tools/extract-stdlib.ts --stdout   # print JSON to stdout
//   tsx tools/extract-stdlib.ts -o path    # write to an explicit path

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const coreModRs = join(repoRoot, "rust/src/builtins/mod.rs");
const petalUiDrawRs = join(repoRoot, "petal-ui/src/draw.rs");
const petalUiInputRs = join(repoRoot, "petal-ui/src/input.rs");

// ── Types ──────────────────────────────────────────────────────────────────

export type ParamType = "int" | "float" | "string" | "list" | "any";

export interface Param {
  name: string;
  type: ParamType;
}

export interface StdlibFunction {
  /** Petal-facing name, e.g. "draw_rect". */
  name: string;
  /** Category id, e.g. "math" or "drawing". */
  category: string;
  /** Which runtime registers it. */
  group: "core" | "canvas";
  /** Fixed argument count, or null when the function dispatches on arg count. */
  arity: number | null;
  /** True when the function accepts a variable number of arguments. */
  variadic: boolean;
  /** Argument names + types recovered from the Rust source (best effort). */
  params: Param[];
  /** Source location of the implementation, repo-relative. */
  source: { file: string; line: number };
  /** When set, this name is an alias for another builtin. */
  aliasOf?: string;
}

export interface StdlibCategory {
  id: string;
  title: string;
  group: "core" | "canvas";
  /** First line of the module's `//!` doc comment, when available. */
  doc: string;
}

export interface StdlibManifest {
  /** Repo-relative paths the manifest was generated from. */
  generatedFrom: string[];
  categories: StdlibCategory[];
  functions: StdlibFunction[];
}

// ── Friendly category metadata ───────────────────────────────────────────────
// The id is the Rust submodule name (core) or a canvas sub-group; the title is
// what the docs sidebar shows. Order here is the order categories render in.

const CATEGORY_TITLES: Record<string, string> = {
  io: "I/O & Types",
  math: "Math",
  creative_coding: "Creative-Coding Math",
  noise: "Noise",
  color: "Color",
  vec2: "Vectors (2D)",
  collections: "Collections",
  "higher-order": "Higher-Order Functions",
  autodiff: "Automatic Differentiation",
  output: "Output & Symbols",
  handle: "Handles",
  pending: "Async & Query Values",
  drawing: "Drawing",
  input: "Input & Timing",
};

const CATEGORY_ORDER = Object.keys(CATEGORY_TITLES);

// ── Rust parsing helpers ─────────────────────────────────────────────────────

/** Extract the body of a named braced block, e.g. `register_builtins`. */
function extractBlock(source: string, signature: RegExp): string {
  const m = signature.exec(source);
  if (!m) throw new Error(`could not find block: ${signature}`);
  let depth = 0;
  let i = source.indexOf("{", m.index);
  const start = i + 1;
  for (; i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") {
      depth--;
      if (depth === 0) return source.slice(start, i);
    }
  }
  throw new Error(`unterminated block: ${signature}`);
}

/** First line of a module's `//!` doc comment, stripped. */
function moduleDoc(source: string): string {
  const lines = source.split("\n");
  const doc: string[] = [];
  for (const line of lines) {
    const t = line.trim();
    if (t.startsWith("//!")) doc.push(t.slice(3).trim());
    else if (doc.length) break;
    else if (t === "") continue;
    else break;
  }
  return doc.join(" ").trim();
}

const GET_TYPE: Record<string, ParamType> = {
  get_int: "int",
  get_float: "float",
  get_string: "string",
  get_list: "list",
  get_value: "any",
};

/**
 * Pull arity + parameters out of a single `fn native_*` body.
 *
 * Arity comes from `require_args(state, N, …)` when present. Parameters come
 * from `let <name> = state.get_<type>(<index>)?` bindings, keyed by the
 * stack index so we recover them in declared order even across `match` arms;
 * the first binding seen for a given index wins.
 */
function parseFnBody(body: string): {
  arity: number | null;
  variadic: boolean;
  params: Param[];
} {
  const requireArgs = /require_args\(\s*state\s*,\s*(\d+)\s*,/.exec(body);
  const dispatches = /\bstate\.arg_count\(\)/.test(body) && !requireArgs;
  const arity = requireArgs ? Number(requireArgs[1]) : null;

  const byIndex = new Map<number, Param>();
  const re =
    /let\s+(\w+)\s*=\s*(?:match\s+)?state\.(get_int|get_float|get_string|get_list|get_value)\(\s*(\d+)\s*\)/g;
  for (let m; (m = re.exec(body)); ) {
    const [, name, getter, idxStr] = m;
    const idx = Number(idxStr);
    if (idx === 0) continue; // index 0 is the callee slot, not an argument
    if (!byIndex.has(idx) && name !== "_") {
      byIndex.set(idx, { name, type: GET_TYPE[getter] });
    }
  }
  const params = [...byIndex.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([, p]) => p);

  // When arity is fixed but a `match` arm shadowed some bindings, trust the
  // recovered list only if it's consistent with the declared arity.
  const variadic = dispatches || (arity !== null && params.length > arity);
  return { arity, variadic, params };
}

/** Find a `fn <name>(` definition and return its body + 1-based line number. */
function findFn(
  source: string,
  fnName: string,
): { body: string; line: number } | null {
  const re = new RegExp(`fn\\s+${fnName}\\s*\\(`);
  const m = re.exec(source);
  if (!m) return null;
  const line = source.slice(0, m.index).split("\n").length;
  // Body: from the `{` after the signature to its matching `}`.
  let i = source.indexOf("{", m.index);
  let depth = 0;
  const start = i + 1;
  for (; i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") {
      depth--;
      if (depth === 0) return { body: source.slice(start, i), line };
    }
  }
  return { body: source.slice(start), line };
}

// ── Core builtins ────────────────────────────────────────────────────────────

interface Registration {
  name: string;
  module: string | null; // null for locally-defined fns (intrinsics)
  fnName: string;
  aliasComment: string | null;
}

/** Parse `table.register("name", module::native_fn);` lines, in order. */
function parseCoreRegistrations(modSource: string): Registration[] {
  const block = extractBlock(modSource, /pub fn register_builtins\s*\(/);
  const out: Registration[] = [];
  const re =
    /(?:let\s+\w+\s*=\s*)?table\.register\(\s*"([^"]+)"\s*,\s*(?:(\w+)::)?(\w+)\s*\)\s*;?\s*(?:\/\/\s*(.*))?/g;
  for (let m; (m = re.exec(block)); ) {
    const [, name, module, fnName, comment] = m;
    out.push({
      name,
      module: module ?? null,
      fnName,
      aliasComment: comment?.trim() ?? null,
    });
  }
  return out;
}

const moduleSourceCache = new Map<string, string>();
function moduleSource(module: string): string {
  if (!moduleSourceCache.has(module)) {
    const path = join(repoRoot, `rust/src/builtins/${module}.rs`);
    moduleSourceCache.set(module, readFileSync(path, "utf8"));
  }
  return moduleSourceCache.get(module)!;
}

function extractCore(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const modSource = readFileSync(coreModRs, "utf8");
  const regs = parseCoreRegistrations(modSource);

  // Map each impl fn name to the registered Petal name(s), so an alias whose
  // comment says "alias for contains" can be linked even without parsing prose:
  // two registrations sharing the same impl fn means the later one is an alias.
  const implFirstSeen = new Map<string, string>();

  const functions: StdlibFunction[] = [];
  const usedModules = new Set<string>();

  for (const reg of regs) {
    const isIntrinsic = reg.module === null;
    const category = isIntrinsic ? "higher-order" : reg.module!;
    let parsed = { arity: null as number | null, variadic: false, params: [] as Param[] };
    let source = { file: "rust/src/builtins/mod.rs", line: 0 };

    if (!isIntrinsic) {
      usedModules.add(reg.module!);
      const src = moduleSource(reg.module!);
      const fn = findFn(src, reg.fnName);
      if (fn) {
        parsed = parseFnBody(fn.body);
        source = { file: `rust/src/builtins/${reg.module}.rs`, line: fn.line };
      }
    } else {
      const fn = findFn(modSource, reg.fnName);
      if (fn) source = { file: "rust/src/builtins/mod.rs", line: fn.line };
      // Intrinsics (map/filter/reduce/forEach) take a list + a function; their
      // real shape is documented in the overlay.
      parsed.variadic = true;
    }

    const aliasOf =
      implFirstSeen.get(reg.fnName) && implFirstSeen.get(reg.fnName) !== reg.name
        ? implFirstSeen.get(reg.fnName)
        : undefined;
    if (!implFirstSeen.has(reg.fnName)) implFirstSeen.set(reg.fnName, reg.name);

    functions.push({
      name: reg.name,
      category,
      group: "core",
      arity: parsed.arity,
      variadic: parsed.variadic,
      params: parsed.params,
      source,
      ...(aliasOf ? { aliasOf } : {}),
    });
  }

  const categories: StdlibCategory[] = [];
  for (const id of usedModules) {
    categories.push({
      id,
      title: CATEGORY_TITLES[id] ?? id,
      group: "core",
      doc: moduleDoc(moduleSource(id)),
    });
  }
  if (functions.some((f) => f.category === "higher-order")) {
    categories.push({
      id: "higher-order",
      title: CATEGORY_TITLES["higher-order"],
      group: "core",
      doc: "List transforms that take a function: map, filter, reduce, forEach.",
    });
  }
  return { functions, categories };
}

// ── Canvas builtins ──────────────────────────────────────────────────────────

/**
 * The buffered draw builtins (`draw_rect`, `draw_line`, …) don't name their
 * arguments in the native fn: they forward a positional `int_args(state, N)`
 * list to `emit_draw(state, "<tag>", …)`, so the generic
 * `let <name> = state.get_int(…)` extraction finds nothing. The canonical
 * positional→name mapping lives on the decode side — `draw.rs`'s
 * `DrawCommand::from_value`, whose match arms turn each `{tag, data}` command
 * back into a named-field struct (`"rect" => DrawCommand::Rect { x: i32_at(0)?,
 * … }`). We read that mapping so the extracted signatures stay derived from
 * source rather than hand-maintained here.
 *
 * Only the *required* positional args (bound with `i32_at`/`u32_at`/`u8_at` or
 * `as_i64(arg(i))`) are collected; trailing optional args (`opt_u8`, `opt_u32`
 * for alpha/radius/width) are excluded, so the recovered arity matches the
 * native's `int_args(state, N)` count.
 *
 * Returns a map from draw-command tag (e.g. "rect") to argument names ordered by
 * their data index (e.g. ["x","y","w","h","r","g","b"]).
 */
function loadDrawArgNames(drawSource: string): Map<string, string[]> {
  const body = extractBlock(drawSource, /fn from_value\s*\(/);
  // Split the `match tag.as_str()` into arms: each starts with `"<tag>" =>`.
  // The catch-all `_ => DrawCommand::Host` isn't quoted, so it isn't a start.
  const armStarts: Array<{ tag: string; at: number }> = [];
  const armRe = /"(\w+)"\s*=>/g;
  for (let m; (m = armRe.exec(body)); ) {
    armStarts.push({ tag: m[1], at: m.index });
  }
  const out = new Map<string, string[]>();
  // Required positional accessors: `<name>: i32_at(<i>)`, `u32_at`, `u8_at`, or
  // `<name>: as_i64(arg(<i>)…` (text's `size`). `opt_u8`/`opt_u32` are excluded.
  const fieldRe =
    /(\w+)\s*:\s*(?:(?:i32_at|u32_at|u8_at)\(\s*(\d+)\s*\)|as_i64\(\s*arg\(\s*(\d+)\s*\))/g;
  for (let i = 0; i < armStarts.length; i++) {
    const { tag, at } = armStarts[i];
    const end = i + 1 < armStarts.length ? armStarts[i + 1].at : body.length;
    const chunk = body.slice(at, end);
    const byIndex: Array<[number, string]> = [];
    for (let f; (f = fieldRe.exec(chunk)); ) {
      byIndex.push([Number(f[2] ?? f[3]), f[1]]);
    }
    if (byIndex.length) {
      out.set(
        tag,
        byIndex.sort((a, b) => a[0] - b[0]).map(([, name]) => name),
      );
    }
  }
  return out;
}

/**
 * If a canvas fn forwards a positional `int_args(state, N)` list straight to
 * `emit_draw(state, "<tag>", …)`, recover its signature from the tag's argument
 * names (all `int`). Returns null when the fn isn't of that shape, leaving the
 * generic `let <name> = state.get_<type>(…)` extraction in charge.
 */
function bufferedDrawSignature(
  body: string,
  drawArgNames: Map<string, string[]>,
): { arity: number | null; variadic: boolean; params: Param[] } | null {
  const intArgs = /int_args\(\s*state\s*,\s*(\d+)\s*\)/.exec(body);
  const emit = /emit_draw\(\s*state\s*,\s*"([^"]+)"/.exec(body);
  if (!intArgs || !emit) return null;
  const names = drawArgNames.get(emit[1]);
  if (!names) return null;
  return {
    arity: Number(intArgs[1]),
    variadic: false,
    params: names.map((name) => ({ name, type: "int" as ParamType })),
  };
}

/** Parse `env.register_native("name", native_fn)` lines from a register block. */
function parseNativeRegistrations(block: string): Array<{ name: string; fnName: string }> {
  const re = /env\.register_native\(\s*"([^"]+)"\s*,\s*(\w+)\s*\)/g;
  const out: Array<{ name: string; fnName: string }> = [];
  for (let m; (m = re.exec(block)); ) out.push({ name: m[1], fnName: m[2] });
  return out;
}

/**
 * Extract the canvas builtins from the shared `petal-ui` crate. Drawing lives in
 * `draw.rs` (`register_draw` + the offscreen-canvas `register_canvas`); input +
 * timing lives in `input.rs` (`register_input`). Each register fn is a flat list
 * of `env.register_native(…)` calls, so the block a function is registered in —
 * not source order — decides its category.
 */
function extractCanvas(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const drawSource = readFileSync(petalUiDrawRs, "utf8");
  const inputSource = readFileSync(petalUiInputRs, "utf8");
  const drawArgNames = loadDrawArgNames(drawSource);

  const functions: StdlibFunction[] = [];

  const addCanvasFn = (
    name: string,
    fnName: string,
    category: "drawing" | "input",
    source: string,
    file: string,
  ) => {
    const fn = findFn(source, fnName);
    const parsed = fn
      ? (bufferedDrawSignature(fn.body, drawArgNames) ?? parseFnBody(fn.body))
      : { arity: null, variadic: false, params: [] };
    functions.push({
      name,
      category,
      group: "canvas",
      arity: parsed.arity,
      variadic: parsed.variadic,
      params: parsed.params,
      source: { file, line: fn?.line ?? 0 },
    });
  };

  // Drawing: register_draw + the offscreen-canvas register_canvas.
  for (const sig of [/pub fn register_draw\s*\(/, /pub fn register_canvas\s*\(/]) {
    for (const reg of parseNativeRegistrations(extractBlock(drawSource, sig))) {
      addCanvasFn(reg.name, reg.fnName, "drawing", drawSource, "petal-ui/src/draw.rs");
    }
  }
  // Input + timing: register_input.
  for (const reg of parseNativeRegistrations(
    extractBlock(inputSource, /pub fn register_input\s*\(/),
  )) {
    addCanvasFn(reg.name, reg.fnName, "input", inputSource, "petal-ui/src/input.rs");
  }

  const categories: StdlibCategory[] = [
    {
      id: "drawing",
      title: CATEGORY_TITLES.drawing,
      group: "canvas",
      doc: "Canvas drawing commands. Colors are r, g, b integer channels 0–255; the origin is the top-left.",
    },
    {
      id: "input",
      title: CATEGORY_TITLES.input,
      group: "canvas",
      doc: "Read the mouse, keyboard, clock, and canvas size each frame.",
    },
  ];
  return { functions, categories };
}

// ── Build + emit ─────────────────────────────────────────────────────────────

export function buildManifest(): StdlibManifest {
  const core = extractCore();
  const canvas = extractCanvas();
  const functions = [...core.functions, ...canvas.functions];
  const categories = [...core.categories, ...canvas.categories].sort(
    (a, b) => CATEGORY_ORDER.indexOf(a.id) - CATEGORY_ORDER.indexOf(b.id),
  );
  return {
    generatedFrom: [
      "rust/src/builtins/mod.rs",
      "rust/src/builtins/*.rs",
      "petal-ui/src/draw.rs",
      "petal-ui/src/input.rs",
    ],
    categories,
    functions,
  };
}

function main() {
  const args = process.argv.slice(2);
  const manifest = buildManifest();
  const json = JSON.stringify(manifest, null, 2) + "\n";

  if (args.includes("--stdout")) {
    process.stdout.write(json);
    return;
  }
  const oIdx = args.indexOf("-o");
  const outPath =
    oIdx >= 0 && args[oIdx + 1]
      ? resolve(args[oIdx + 1])
      : join(repoRoot, "docs", "stdlib.json");
  writeFileSync(outPath, json);
  process.stderr.write(
    `wrote ${manifest.functions.length} functions across ` +
      `${manifest.categories.length} categories to ${outPath}\n`,
  );
}

// Only run when invoked directly, not when imported by the test.
if (import.meta.url === `file://${process.argv[1]}`) {
  main();
}

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
// Three registration sources are parsed:
//
//   1. Core builtins — `rust/src/builtins/mod.rs`'s `register_builtins()`,
//      which is the canonical, append-only list of `table.register("name", …)`
//      calls. Each entry points at a `native_*` fn in a topic submodule
//      (math.rs, collections.rs, …); the submodule it lives in becomes the
//      function's category.
//
//   2. Core prelude — `rust/prelude/std.ptl` (module `std`), the slice of the
//      standard library written in Petal source rather than as Rust natives.
//      `Env::new` loads it as an implicit import; its `export fn` declarations
//      become the `prelude` group / `std` category.
//
//   3. Canvas builtins — the shared `petal-ui` crate's `register_draw` +
//      `register_canvas` (drawing) and `register_input` (input/timing), the
//      interactivity API that hosts like petal-web-canvas and petal-sdl expose
//      to sketches.
//
// For each registered function we read:
//   • parameters — the names the native *declares* for named arguments:
//                  `BUILTIN_PARAMS` (rust/src/builtins/params.rs) for the core
//                  builtins, `PETAL_UI_NATIVE_PARAMS`
//                  (rust/src/typecheck/globals.rs) for the canvas ones. These
//                  are call syntax, not just documentation — the registry
//                  binds `clamp(value: v, lo: 0, hi: 1)` against them — so
//                  they are the source of truth for every name in the
//                  manifest. A builtin that declares several call forms
//                  (`random`, `distance`) lists them all in `signatures`.
//   • arity      — from `require_args(state, N, "name")` in the implementation
//   • types      — from `let <name> = state.get_<type>(<index>)` bindings in
//                  the implementation, matched onto the declared parameters
//   • source     — file + line, so docs can point back at the implementation
//
// A native with no declaration (the variadic `print` / `format`, the `__`
// internals) refuses named arguments at runtime; for those the names fall back
// to what the body's bindings suggest, and `signatures` is empty.
//
// Functions that dispatch on `arg_count()` (overloaded arities like `noise`,
// `distance`, `mag`, `range`, `slice`) have no single arity; they're flagged
// `variadic`, and their call forms are the entries of `signatures`.
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
const coreParamsRs = join(repoRoot, "rust/src/builtins/params.rs");
const hostParamsRs = join(repoRoot, "rust/src/typecheck/globals.rs");
const petalUiDrawRs = join(repoRoot, "petal-ui/src/draw.rs");
const petalUiTextRs = join(repoRoot, "petal-ui/src/text.rs");
const petalUiInputRs = join(repoRoot, "petal-ui/src/input.rs");
const preludeStdPtl = join(repoRoot, "rust/prelude/std.ptl");

// ── Types ──────────────────────────────────────────────────────────────────

export type ParamType = "int" | "float" | "string" | "list" | "any";

export interface Param {
  name: string;
  type: ParamType;
  /** Set on a trailing parameter a call may leave off. */
  optional?: boolean;
}

export interface StdlibFunction {
  /** Petal-facing name, e.g. "draw_rect". */
  name: string;
  /** Category id, e.g. "math" or "drawing". */
  category: string;
  /** Which runtime registers it. `prelude` = Petal-source std, not a native. */
  group: "core" | "canvas" | "prelude";
  /** Fixed argument count, or null when the function dispatches on arg count. */
  arity: number | null;
  /** True when the function accepts a variable number of arguments. */
  variadic: boolean;
  /**
   * The parameters, by the names a call can pass them under. For a native
   * that declares its parameters this is its longest declared form (optional
   * ones flagged); otherwise it is what the implementation's bindings suggest
   * (best effort), and the function takes positional arguments only.
   */
  params: Param[];
  /**
   * Every call form the function declares for named arguments — one entry
   * for most, several for a builtin that reads its arguments differently by
   * count (`random()` / `random(max)` / `random(min, max)`). Empty when the
   * function refuses named arguments (variadic builtins, internals).
   */
  signatures: Param[][];
  /** Source location of the implementation, repo-relative. */
  source: { file: string; line: number };
  /** When set, this name is an alias for another builtin. */
  aliasOf?: string;
  /**
   * Internal builtins the public reference hides — `__`-prefixed names the
   * runtime uses for tests/plumbing (e.g. `__pending`), not part of the
   * user-facing standard library.
   */
  internal?: boolean;
}

export interface StdlibCategory {
  id: string;
  title: string;
  group: "core" | "canvas" | "prelude";
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
  format: "Formatting",
  math: "Math",
  creative_coding: "Creative-Coding Math",
  noise: "Noise",
  color: "Color",
  vec2: "Vectors (2D & 3D)",
  collections: "Collections",
  json: "JSON",
  classes: "Built-in Classes",
  "higher-order": "Higher-Order Functions",
  std: "Core Prelude",
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

// Argument readers that wrap `state.get_*` behind a helper taking the stack
// index as their second argument: `let <name> = <helper>(state, <index>, …)`.
// Without these, a native that validates an argument through a helper looks to
// the extractor like it has no parameter at that index at all — and the docs
// then renumber every later argument (`fill_arc`'s colours slid into slots
// 3-5). Optional-argument helpers (`opt_int`) stay out on purpose: no optional
// argument is documented anywhere else either.
const HELPER_TYPE: Record<string, ParamType> = {
  point_list_arg: "list",
  get_num: "float",
};

/**
 * Pull arity + parameters out of a single `fn native_*` body.
 *
 * Arity comes from `require_args(state, N, …)` when present. Parameters come
 * from `let <name> = state.get_<type>(<index>)?` bindings (or the `HELPER_TYPE`
 * accessors), keyed by the stack index so we recover them in declared order
 * even across `match` arms; the first binding seen for a given index wins.
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
  const helpers = Object.keys(HELPER_TYPE).join("|");
  const re = new RegExp(
    String.raw`let\s+(\w+)\s*=\s*(?:match\s+)?(?:state\.(get_int|get_float|get_string|get_list|get_value)\(\s*(\d+)\s*\)|(${helpers})\(\s*state\s*,\s*(\d+)\s*[,)])`,
    "g",
  );
  for (let m; (m = re.exec(body)); ) {
    const [, name, getter, getterIdx, helper, helperIdx] = m;
    const idx = Number(getter ? getterIdx : helperIdx);
    if (idx === 0) continue; // index 0 is the callee slot, not an argument
    if (!byIndex.has(idx) && name !== "_") {
      byIndex.set(idx, {
        name,
        type: getter ? GET_TYPE[getter] : HELPER_TYPE[helper],
      });
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

// ── Declared parameters ──────────────────────────────────────────────────────

/** One declared call form: names in argument order, optional ones flagged. */
type DeclaredForm = Array<{ name: string; optional: boolean }>;

/**
 * Parse a `pub const <NAME>: &[(&str, &[&str])] = &[ ("fn", &["a, b?"]), … ];`
 * table: each entry is a native's name and one spec per call form, a spec being
 * comma-separated parameter names with `?` on a trailing optional one (the
 * format `NativeSignature::parse` reads).
 */
function parseParamTable(source: string, constName: string): Map<string, DeclaredForm[]> {
  const start = source.indexOf(`pub const ${constName}:`);
  if (start < 0) throw new Error(`could not find ${constName}`);
  const end = source.indexOf("\n];", start);
  if (end < 0) throw new Error(`unterminated ${constName}`);
  const body = source.slice(source.indexOf("= &[", start) + 4, end);
  const out = new Map<string, DeclaredForm[]>();
  const entryRe = /\(\s*"([^"]+)"\s*,\s*&\[([^\]]*)\]\s*,?\s*\)/g;
  for (let m; (m = entryRe.exec(body)); ) {
    const forms: DeclaredForm[] = [];
    for (const spec of m[2].matchAll(/"([^"]*)"/g)) {
      forms.push(
        spec[1]
          .split(",")
          .map((p) => p.trim())
          .filter((p) => p.length > 0)
          .map((p) => ({ name: p.replace(/\?$/, ""), optional: p.endsWith("?") })),
      );
    }
    out.set(m[1], forms);
  }
  return out;
}

let declaredParamsCache: Map<string, DeclaredForm[]> | null = null;
/** The declared call forms of every native, core and canvas, by name. */
function declaredParams(): Map<string, DeclaredForm[]> {
  if (!declaredParamsCache) {
    declaredParamsCache = new Map([
      ...parseParamTable(readFileSync(coreParamsRs, "utf8"), "BUILTIN_PARAMS"),
      ...parseParamTable(readFileSync(hostParamsRs, "utf8"), "PETAL_UI_NATIVE_PARAMS"),
    ]);
  }
  return declaredParamsCache;
}

/**
 * The manifest's `params` + `signatures` for native `name`, given what its
 * body's bindings recovered. Declared names win; the recovered list only
 * supplies types — by name where a binding happens to share the declared
 * name, else by position when the body bound exactly the form's required
 * arguments (so positions line up). With no declaration the recovered list is
 * all there is.
 */
function withDeclaredParams(
  name: string,
  recovered: Param[],
): { params: Param[]; signatures: Param[][] } {
  const forms = declaredParams().get(name);
  if (!forms) return { params: recovered, signatures: [] };
  const byName = new Map(recovered.map((p) => [p.name, p.type]));
  const signatures = forms.map((form) => {
    const required = form.filter((p) => !p.optional).length;
    const dense = recovered.length === required;
    return form.map((p, i): Param => ({
      name: p.name,
      type: byName.get(p.name) ?? (dense && i < required ? recovered[i].type : "any"),
      ...(p.optional ? { optional: true } : {}),
    }));
  });
  // The longest form is the one that shows every parameter the function has.
  const params = signatures.reduce((a, b) => (b.length > a.length ? b : a), signatures[0] ?? []);
  return { params, signatures };
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

/**
 * Parse `table.register("name", module::native_fn, <effects>);` lines, in
 * order (the older two-argument `register_with` spelling is accepted too). The
 * effect row is skipped: it says what the native does at runtime, not what it
 * is for.
 */
function parseCoreRegistrations(modSource: string): Registration[] {
  const block = extractBlock(modSource, /pub fn register_builtins\s*\(/);
  const out: Registration[] = [];
  const re =
    /(?:let\s+\w+\s*=\s*)?table\.register(?:_with)?\(\s*"([^"]+)"\s*,\s*(?:(\w+)::)?(\w+)\s*(?:,[^;]*)?\)\s*;?\s*(?:\/\/\s*(.*))?/g;
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

/**
 * Built-in class constructors, registered by `builtins/classes.rs`'s own
 * `register()` rather than inline in `register_builtins()`. Only the bare
 * constructors are picked up: the class *methods* are registered under dotted
 * names in a loop and are not callable as bare functions, so they belong in
 * the language guide, not in the function manifest.
 */
function parseClassRegistrations(): Registration[] {
  const src = moduleSource("classes");
  const block = extractBlock(src, /pub\(super\) fn register\s*\(/);
  const out: Registration[] = [];
  const re = /table\.register(?:_with)?\(\s*"([^"]+)"\s*,\s*(\w+)\s*(?:,[^;]*)?\)/g;
  for (let m; (m = re.exec(block)); ) {
    out.push({ name: m[1], module: "classes", fnName: m[2], aliasComment: null });
  }
  return out;
}

function extractCore(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const modSource = readFileSync(coreModRs, "utf8");
  const regs = [...parseCoreRegistrations(modSource), ...parseClassRegistrations()];

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
      // Intrinsics (map/filter/reduce/forEach) take a list + a function. The
      // VM drives them, so there is no body to read an arity from; their
      // parameters come from the declaration alone.
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
      ...withDeclaredParams(reg.name, parsed.params),
      source,
      ...(aliasOf ? { aliasOf } : {}),
      ...(reg.name.startsWith("__") ? { internal: true } : {}),
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

// ── Prelude (Petal-source std) ───────────────────────────────────────────────

/**
 * The core prelude (`rust/prelude/std.ptl`, module `std`) is standard library
 * written in Petal source rather than as Rust natives — `Env::new` loads it as
 * a permanent implicit import, so every program calls its helpers bare. We parse
 * its `export fn` declarations so these functions appear in the reference next
 * to the native builtins instead of silently drifting out of the docs.
 *
 * Petal source carries no static types, so every parameter is reported as
 * `any`; arity is simply the declared parameter count (none are variadic).
 * Only `export`ed declarations are visible to importers, so private helpers
 * (were any added) are correctly skipped.
 */
function extractPrelude(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const source = readFileSync(preludeStdPtl, "utf8");
  const functions: StdlibFunction[] = [];
  const re = /^[ \t]*export\s+fn\s+(\w+)\s*\(([^)]*)\)/gm;
  for (let m; (m = re.exec(source)); ) {
    const [, name, rawParams] = m;
    const line = source.slice(0, m.index).split("\n").length;
    const params: Param[] = rawParams
      .split(",")
      .map((p) => p.trim())
      .filter((p) => p.length > 0)
      .map((p) => ({ name: p, type: "any" as ParamType }));
    functions.push({
      name,
      category: "std",
      group: "prelude",
      arity: params.length,
      variadic: false,
      params,
      // A Petal `fn` takes named arguments under its own parameter names.
      signatures: [params],
      source: { file: "rust/prelude/std.ptl", line },
    });
  }

  const categories: StdlibCategory[] = functions.length
    ? [
        {
          id: "std",
          title: CATEGORY_TITLES.std,
          group: "prelude",
          doc: "Standard-library helpers written in Petal source (module `std`), auto-imported into every program: list reductions, predicate queries, sublists, and number helpers.",
        },
      ]
    : [];
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

/**
 * Parse the registrations in a register block: `register(env, "name",
 * native_fn, <effects>)` — petal-ui's wrapper, which also declares the
 * native's parameters (`petal-ui/src/params.rs`) — or a bare
 * `env.register_native("name", native_fn, <effects>)`.
 */
function parseNativeRegistrations(block: string): Array<{ name: string; fnName: string }> {
  const re =
    /(?:env\.register_native\(|\bregister\(\s*env\s*,)\s*"([^"]+)"\s*,\s*(\w+)\s*(?:,[^;]*)?\)/g;
  const out: Array<{ name: string; fnName: string }> = [];
  for (let m; (m = re.exec(block)); ) out.push({ name: m[1], fnName: m[2] });
  return out;
}

/**
 * Extract the canvas builtins from the shared `petal-ui` crate. Drawing lives in
 * `draw.rs` (`register_draw` + the offscreen-canvas `register_canvas`), with the
 * text-measurement natives implemented next door in `text.rs`; input + timing
 * lives in `input.rs` (`register_input`). Each register fn is a flat list of
 * `env.register_native(…)` calls, so the block a function is registered in —
 * not source order — decides its category.
 */
function extractCanvas(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const drawSource = readFileSync(petalUiDrawRs, "utf8");
  const textSource = readFileSync(petalUiTextRs, "utf8");
  const inputSource = readFileSync(petalUiInputRs, "utf8");
  const drawArgNames = loadDrawArgNames(drawSource);

  const functions: StdlibFunction[] = [];

  // A native is *registered* in one file's `register_*` block but its body may
  // live in a sibling module — text measurement is registered by `register_draw`
  // and implemented in `text.rs`. So resolve the body across every candidate
  // file and record the one it was actually found in.
  const addCanvasFn = (
    name: string,
    fnName: string,
    category: "drawing" | "input",
    candidates: Array<{ file: string; source: string }>,
  ) => {
    const found = candidates
      .map(({ file, source }) => ({ file, fn: findFn(source, fnName) }))
      .find(({ fn }) => fn !== null);
    const fn = found?.fn ?? null;
    const parsed = fn
      ? (bufferedDrawSignature(fn.body, drawArgNames) ?? parseFnBody(fn.body))
      : { arity: null, variadic: false, params: [] };
    functions.push({
      name,
      category,
      group: "canvas",
      arity: parsed.arity,
      variadic: parsed.variadic,
      ...withDeclaredParams(name, parsed.params),
      source: { file: found?.file ?? candidates[0].file, line: fn?.line ?? 0 },
    });
  };

  const drawFiles = [
    { file: "petal-ui/src/draw.rs", source: drawSource },
    { file: "petal-ui/src/text.rs", source: textSource },
  ];

  // Drawing: register_draw + the offscreen-canvas register_canvas.
  for (const sig of [/pub fn register_draw\s*\(/, /pub fn register_canvas\s*\(/]) {
    for (const reg of parseNativeRegistrations(extractBlock(drawSource, sig))) {
      addCanvasFn(reg.name, reg.fnName, "drawing", drawFiles);
    }
  }
  // Input + timing: register_input.
  for (const reg of parseNativeRegistrations(
    extractBlock(inputSource, /pub fn register_input\s*\(/),
  )) {
    addCanvasFn(reg.name, reg.fnName, "input", [
      { file: "petal-ui/src/input.rs", source: inputSource },
    ]);
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
  const prelude = extractPrelude();
  const canvas = extractCanvas();
  const functions = [
    ...core.functions,
    ...prelude.functions,
    ...canvas.functions,
  ];
  const categories = [
    ...core.categories,
    ...prelude.categories,
    ...canvas.categories,
  ].sort((a, b) => CATEGORY_ORDER.indexOf(a.id) - CATEGORY_ORDER.indexOf(b.id));
  return {
    generatedFrom: [
      "rust/src/builtins/mod.rs",
      "rust/src/builtins/*.rs",
      "rust/src/typecheck/globals.rs",
      "rust/prelude/std.ptl",
      "petal-ui/src/draw.rs",
      "petal-ui/src/text.rs",
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

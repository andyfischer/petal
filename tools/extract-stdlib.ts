#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
//
// extract-stdlib.ts — Generate a structured manifest of Petal's standard
// library: what the runtime registers (read from the Rust source) joined with
// what the declarations say about it (read from `petal doc --json`).
//
// The point of this tool is that documentation can never silently drift from
// the implementation: the *list* of functions comes from the Rust registration
// tables, and everything a script sees of a function (its call forms,
// parameter names and types, and its `@doc`) comes from its Petal declaration
// through `petal doc`, the same reader the generated reference
// (`tools/gen-reference.ts`) and the website's API pages use.
//
// Only what the Petal declarations cannot say is read from Rust:
//
//   • which functions exist, and which runtime registers each (`group`):
//       1. core builtins: `core/src/builtins/mod.rs`'s `register_builtins()`
//          (the canonical, append-only list of `table.register("name", …)`
//          calls) and the class constructors of `builtins/classes.rs`. The
//          topic submodule a `native_*` fn lives in (math.rs, …) is the
//          function's category.
//       2. canvas builtins: the shared `petal-ui` crate's `register_draw` +
//          `register_canvas` (drawing) and `register_input_module` (input and
//          timing), the API hosts like petal-web-canvas and petal-sdl expose.
//   • aliases: two registrations sharing one implementation (`aliasOf`);
//   • the implementation's file and line (`source`);
//   • a category's blurb: the first line of the Rust module's `//!` doc.
//
// The core prelude (`core/prelude/std.ptl`, module `std`, group `prelude`) is
// Petal source, so everything about it comes from `petal doc`.
//
// From `petal doc --json` (docs/schemas/petal-doc.schema.json), for the
// declaring `native fn`: in the bundled `core` package
// (core/packages/core/src/*.ptl), a domain package behind an import
// (core/packages/<name>/, which sets `import`), or the `ui` package
// (core-stdlib/ui/src/) for the canvas natives:
//   • signatures: every call form that binds named arguments. A form with a
//                 `...rest` parameter binds none and is left out, which is
//                 what makes the native refuse named arguments.
//   • params:     the longest form, optional parameters flagged.
//   • types:      as declared (`num`, `string`, …); `any` where none is written.
//   • arity:      the parameter count when there is exactly one form and
//                 every parameter is required; else null, and `variadic`.
//   • doc:        the item's `@doc`, every field, cleaned as `petal doc`
//                 prints it (null when undocumented).
//   • declaration: the declaring module, file and line.
//
// `petal doc` runs through `tools/run-petal.ts` (or `$PETAL_BIN`); see
// tools/petal-doc.ts.
//
// Usage:
//   node tools/extract-stdlib.ts            # write docs/stdlib.json
//   node tools/extract-stdlib.ts --stdout   # print JSON to stdout
//   node tools/extract-stdlib.ts -o path    # write to an explicit path

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { petalDoc, moduleFile, repoRoot, type Doc, type DocForm, type DocItem } from "./petal-doc.ts";

const coreModRs = join(repoRoot, "core/src/builtins/mod.rs");
const petalUiDrawRs = join(repoRoot, "core-stdlib/ui/native/src/draw.rs");
const petalUiTextRs = join(repoRoot, "core-stdlib/ui/native/src/text.rs");
const petalUiInputRs = join(repoRoot, "core-stdlib/ui/native/src/input.rs");

/** `petal doc` targets, repo-relative. */
const CORE_PACKAGE_DIR = "core/packages/core";
const UI_PACKAGE_DIR = "core-stdlib/ui";
const PRELUDE_STD = "core/prelude/std.ptl";

// ── Types ──────────────────────────────────────────────────────────────────

/**
 * A parameter's declared type (`num`, `int`, `string`, `list`, `record`, a
 * class name, …), or `any` where the declaration writes none.
 */
export type ParamType = string;

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
  /**
   * Fixed argument count: set when the function has one call form and every
   * parameter is required. Null otherwise (several forms, optional or rest
   * parameters, or no declaration).
   */
  arity: number | null;
  /** True when the declaration accepts more than one argument count. */
  variadic: boolean;
  /**
   * The parameters, by the names a call can pass them under: the longest
   * declared call form, optional ones flagged. Empty when nothing declares
   * names (a variadic native, an undeclared internal).
   */
  params: Param[];
  /**
   * Every call form the function declares for named arguments: one entry for
   * most, several for a builtin that reads its arguments differently by count
   * (`random()` / `random(max)` / `random(min, max)`). Empty when the function
   * refuses named arguments (variadic builtins, internals).
   */
  signatures: Param[][];
  /** Source location of the implementation, repo-relative. */
  source: { file: string; line: number };
  /** The Petal declaration (`native fn`, or `fn` in the prelude), repo-relative. */
  declaration?: { module: string; file: string; line: number };
  /** The declaration's `@doc` as `petal doc --json` prints it; null when undocumented. */
  doc: Doc | null;
  /** When set, this name is an alias for another builtin. */
  aliasOf?: string;
  /**
   * The package a script imports before it can use this name (`import
   * noise`): set for the domain builtins, absent for what is global.
   */
  import?: string;
  /**
   * Internal builtins the public reference hides: `__`-prefixed names the
   * runtime uses for tests/plumbing (e.g. `__pending`), not part of the
   * user-facing standard library.
   */
  internal?: boolean;
}

export interface StdlibCategory {
  id: string;
  title: string;
  group: "core" | "canvas" | "prelude";
  /** First line of the Rust module's `//!` doc comment; for `std`, its `@module_doc` summary. */
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
  hints: "Direct-Manipulation Hints",
  paths: "Nested Paths",
  drag_math: "Drag Math",
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

/** The 1-based line of a `fn <name>(` definition, or null. */
function findFnLine(source: string, fnName: string): number | null {
  const m = new RegExp(`fn\\s+${fnName}\\s*\\(`).exec(source);
  return m ? source.slice(0, m.index).split("\n").length : null;
}

// ── Declarations (`petal doc --json`) ───────────────────────────────────────

/**
 * The bundled packages that declare the domain builtins: natives the runtime
 * registers everywhere but that are in scope only where the package is
 * imported (`import noise`). Mirrors `petal::bundled::BUILTIN_PACKAGES`.
 */
export const BUILTIN_PACKAGES = ["noise", "color", "vec2", "autodiff", "json", "io"];

interface Declaration {
  item: DocItem;
  module: string;
  file: string;
  /** The domain package that declares it, when it needs an import. */
  import?: string;
}

let declarationsCache: Map<string, Declaration[]> | null = null;
/**
 * Every `native fn` of `core`, the domain packages and `ui`, by the host
 * native it binds: its own name, or the one a `= "host_name"` suffix names.
 * Non-`pub` declarations count (`--private`): they bind the native too.
 */
function declarations(): Map<string, Declaration[]> {
  if (declarationsCache) return declarationsCache;
  const out = new Map<string, Declaration[]>();
  const targets: Array<{ dir: string; import?: string }> = [
    { dir: CORE_PACKAGE_DIR },
    ...BUILTIN_PACKAGES.map((name) => ({ dir: `core/packages/${name}`, import: name })),
    { dir: UI_PACKAGE_DIR },
  ];
  for (const { dir, import: imp } of targets) {
    const report = petalDoc(dir, { private: true });
    for (const mod of report.modules) {
      for (const item of mod.items) {
        const natives = (item.forms ?? []).filter((f) => f.native);
        for (const host of new Set(natives.map((f) => f.host ?? item.name))) {
          const decl: Declaration = { item, module: mod.name, file: moduleFile(report, mod) };
          if (imp) decl.import = imp;
          out.set(host, [...(out.get(host) ?? []), decl]);
        }
      }
    }
  }
  declarationsCache = out;
  return out;
}

/**
 * The declaration of host native `name`: the item named like it (`draw_rect`)
 * over one that renames it (`_native_rect = "draw_rect"`).
 */
function declarationOf(name: string): Declaration | undefined {
  const decls = declarations().get(name) ?? [];
  return decls.find((d) => d.item.name === name) ?? decls[0];
}

/** One declared call form as manifest params; null for a form with a rest parameter. */
function formParams(form: DocForm): Param[] | null {
  if (form.params.some((p) => p.rest)) return null;
  return form.params.map((p) => ({
    name: p.name,
    type: p.type ?? "any",
    ...(p.optional ? { optional: true } : {}),
  }));
}

type Declared = Pick<
  StdlibFunction,
  "arity" | "variadic" | "params" | "signatures" | "declaration" | "doc"
>;

/**
 * What the manifest says about a declared function: its call forms, the
 * longest of them as `params`, the arity they imply, where it is declared and
 * its doc. With no declaration there is nothing to say.
 */
function declared(decl: Declaration | undefined): Declared {
  if (!decl) return { arity: null, variadic: false, params: [], signatures: [], doc: null };
  const forms = decl.item.forms ?? [];
  const signatures = forms.map(formParams).filter((s): s is Param[] => s !== null);
  const params = signatures.reduce((a, b) => (b.length > a.length ? b : a), signatures[0] ?? []);
  const fixed =
    forms.length === 1 && signatures.length === 1 && signatures[0].every((p) => !p.optional);
  return {
    arity: fixed ? signatures[0].length : null,
    variadic: !fixed && forms.length > 0,
    params,
    signatures,
    declaration: { module: decl.module, file: decl.file, line: decl.item.line },
    doc: decl.item.doc,
  };
}

// ── Core builtins ────────────────────────────────────────────────────────────

interface Registration {
  name: string;
  module: string | null; // null for locally-defined fns (intrinsics)
  fnName: string;
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
    /(?:let\s+\w+\s*=\s*)?table\.register(?:_with)?\(\s*"([^"]+)"\s*,\s*(?:(\w+)::)?(\w+)\s*(?:,[^;]*)?\)/g;
  for (let m; (m = re.exec(block)); ) {
    const [, name, module, fnName] = m;
    out.push({ name, module: module ?? null, fnName });
  }
  return out;
}

const moduleSourceCache = new Map<string, string>();
function moduleSource(module: string): string {
  if (!moduleSourceCache.has(module)) {
    const path = join(repoRoot, `core/src/builtins/${module}.rs`);
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
    out.push({ name: m[1], module: "classes", fnName: m[2] });
  }
  return out;
}

function extractCore(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const modSource = readFileSync(coreModRs, "utf8");
  const regs = [...parseCoreRegistrations(modSource), ...parseClassRegistrations()];

  // Two registrations sharing the same impl fn means the later one is an
  // alias of the first.
  const implFirstSeen = new Map<string, string>();

  const functions: StdlibFunction[] = [];
  const usedModules = new Set<string>();

  for (const reg of regs) {
    // Intrinsics (map/filter/reduce/forEach) are defined in mod.rs itself:
    // the VM drives them.
    const isIntrinsic = reg.module === null;
    const category = isIntrinsic ? "higher-order" : reg.module!;
    const file = isIntrinsic ? "core/src/builtins/mod.rs" : `core/src/builtins/${reg.module}.rs`;
    if (!isIntrinsic) usedModules.add(reg.module!);
    const line = findFnLine(isIntrinsic ? modSource : moduleSource(reg.module!), reg.fnName) ?? 0;

    const aliasOf =
      implFirstSeen.get(reg.fnName) && implFirstSeen.get(reg.fnName) !== reg.name
        ? implFirstSeen.get(reg.fnName)
        : undefined;
    if (!implFirstSeen.has(reg.fnName)) implFirstSeen.set(reg.fnName, reg.name);

    const decl = declarationOf(reg.name);
    functions.push({
      name: reg.name,
      category,
      group: "core",
      ...declared(decl),
      source: { file, line },
      ...(aliasOf ? { aliasOf } : {}),
      ...(decl?.import ? { import: decl.import } : {}),
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
 * The core prelude (`core/prelude/std.ptl`, module `std`) is standard library
 * written in Petal source rather than as Rust natives: `Env::new` loads it as
 * a permanent implicit import, so every program calls its helpers bare. Its
 * `pub fn`s (as `petal doc` lists them) appear in the manifest next to the
 * native builtins; its implementation *is* its declaration.
 */
function extractPrelude(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const report = petalDoc(PRELUDE_STD);
  const mod = report.modules[0];
  const functions: StdlibFunction[] = [];
  for (const item of mod.items) {
    if (item.kind !== "fn" || !item.exported) continue;
    const decl: Declaration = { item, module: "std", file: PRELUDE_STD };
    functions.push({
      name: item.name,
      category: "std",
      group: "prelude",
      ...declared(decl),
      source: { file: PRELUDE_STD, line: item.line },
    });
  }

  const categories: StdlibCategory[] = functions.length
    ? [
        {
          id: "std",
          title: CATEGORY_TITLES.std,
          group: "prelude",
          doc:
            mod.doc?.summary ??
            "Standard-library helpers written in Petal source (module `std`), auto-imported into every program: list reductions, predicate queries, sublists, and number helpers.",
        },
      ]
    : [];
  return { functions, categories };
}

// ── Canvas builtins ──────────────────────────────────────────────────────────

/**
 * Parse the registrations in a register block: `register(env, "name",
 * native_fn, <effects>)`, petal-ui's wrapper, or a bare
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
 * lives in `input.rs` (`register_input_module`). Each register fn is a flat
 * list of registrations, so the block a function is registered in (not source
 * order) decides its category.
 */
function extractCanvas(): {
  functions: StdlibFunction[];
  categories: StdlibCategory[];
} {
  const drawSource = readFileSync(petalUiDrawRs, "utf8");
  const textSource = readFileSync(petalUiTextRs, "utf8");
  const inputSource = readFileSync(petalUiInputRs, "utf8");

  const functions: StdlibFunction[] = [];

  // A native is *registered* in one file's `register_*` block but its body may
  // live in a sibling module (text measurement is registered by `register_draw`
  // and implemented in `text.rs`), so look for the body in every candidate
  // file and record the one it was found in.
  const addCanvasFn = (
    name: string,
    fnName: string,
    category: "drawing" | "input",
    candidates: Array<{ file: string; source: string }>,
  ) => {
    const found = candidates
      .map(({ file, source }) => ({ file, line: findFnLine(source, fnName) }))
      .find(({ line }) => line !== null);
    functions.push({
      name,
      category,
      group: "canvas",
      ...declared(declarationOf(name)),
      source: { file: found?.file ?? candidates[0].file, line: found?.line ?? 0 },
    });
  };

  const drawFiles = [
    { file: "core-stdlib/ui/native/src/draw.rs", source: drawSource },
    { file: "core-stdlib/ui/native/src/text.rs", source: textSource },
  ];

  // Drawing: register_draw + the offscreen-canvas register_canvas.
  for (const sig of [/pub fn register_draw\s*\(/, /pub fn register_canvas\s*\(/]) {
    for (const reg of parseNativeRegistrations(extractBlock(drawSource, sig))) {
      addCanvasFn(reg.name, reg.fnName, "drawing", drawFiles);
    }
  }
  // Input + timing: register_input_module (register_input wraps it).
  for (const reg of parseNativeRegistrations(
    extractBlock(inputSource, /pub fn register_input_module\s*\(/),
  )) {
    addCanvasFn(reg.name, reg.fnName, "input", [
      { file: "core-stdlib/ui/native/src/input.rs", source: inputSource },
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
      "core/src/builtins/mod.rs",
      "core/src/builtins/*.rs",
      "core-stdlib/ui/native/src/draw.rs",
      "core-stdlib/ui/native/src/text.rs",
      "core-stdlib/ui/native/src/input.rs",
      "petal doc --json --private core/packages/core",
      ...BUILTIN_PACKAGES.map((name) => `petal doc --json --private core/packages/${name}`),
      "petal doc --json --private core-stdlib/ui",
      "petal doc --json core/prelude/std.ptl",
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

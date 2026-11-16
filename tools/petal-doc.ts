// petal-doc.ts — run `petal doc --json` and type what it prints.
//
// The shared reader behind the generated references: `extract-stdlib.ts`
// (docs/stdlib.json) and `gen-reference.ts` (docs/Builtins.md, the `ui` and
// `bloom` references). The report's shape is docs/schemas/petal-doc.schema.json
// (`petal-doc/1`); the types below mirror it.
//
// The binary is `$PETAL_BIN` when set, else `tools/run-petal.ts` (which
// rebuilds `cli/target/debug/petal` when a Rust source is newer). Targets are
// package *directories* and files rather than package names: a bundled package
// named by name is the copy embedded in the binary, which a `.ptl` edit does
// not rebuild, so its docs could be stale.

import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export interface DocParam {
  name: string;
  type: string | null;
  optional: boolean;
  default?: string;
  rest?: true;
}

export interface Doc {
  body: string | null;
  summary: string | null;
  params: Record<string, string>;
  returns: string | null;
  options: Record<string, string>;
  example: string[];
  see: string[];
  deprecated: string | null;
  hidden: boolean;
  extra: Record<string, unknown>;
}

export interface DocForm {
  params: DocParam[];
  returns: string | null;
  native: boolean;
  host?: string;
  line: number;
  doc: Doc | null;
}

export type ItemKind =
  | "fn"
  | "native_fn"
  | "class"
  | "enum"
  | "let"
  | "const"
  | "var"
  | "state"
  | "state_var"
  | "memo";

export interface DocItem {
  kind: ItemKind;
  name: string;
  exported: boolean;
  hidden: boolean;
  line: number;
  doc: Doc | null;
  receiver?: string;
  forms?: DocForm[];
  fields?: Array<{ name: string; type: string | null }>;
  variants?: Array<{ name: string; fields: string[]; data: string[] }>;
  type?: string;
  value?: string;
}

export interface DocReexport {
  module: string;
  canonical: string | null;
  names: string[] | null;
}

export interface DocModule {
  name: string;
  file: string;
  doc: Doc | null;
  reexports: DocReexport[];
  items: DocItem[];
  error?: string;
}

export interface DocPackage {
  name: string;
  version: string | null;
  description: string | null;
  root: string;
  root_module: string | null;
  doc: Doc | null;
  modules: string[];
}

export interface DocReport {
  schema: "petal-doc/1";
  target: string;
  package: DocPackage | null;
  modules: DocModule[];
}

const cache = new Map<string, DocReport>();

/**
 * `petal doc --json [--private] <target>`, parsed. `target` is relative to the
 * repo root (a package directory or a `.ptl` file). Throws when the command
 * fails, which it does when a module does not parse.
 */
export function petalDoc(target: string, opts: { private?: boolean } = {}): DocReport {
  const key = `${opts.private ? "private:" : ""}${target}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const args = ["doc", "--json", ...(opts.private ? ["--private"] : []), target];
  const bin = process.env.PETAL_BIN;
  const out = bin
    ? execFileSync(bin, args, { cwd: repoRoot, encoding: "utf8", maxBuffer: 256 << 20 })
    : execFileSync(process.execPath, ["--disable-warning=MODULE_TYPELESS_PACKAGE_JSON", join(repoRoot, "tools/run-petal.ts"), ...args], {
        cwd: repoRoot,
        encoding: "utf8",
        maxBuffer: 256 << 20,
        stdio: ["ignore", "pipe", "inherit"],
      });
  const report = JSON.parse(out) as DocReport;
  if (report.schema !== "petal-doc/1") {
    throw new Error(`petal doc ${target}: unknown report schema ${JSON.stringify(report.schema)}`);
  }
  cache.set(key, report);
  return report;
}

/**
 * Repo-relative path of a module's file. A package module's `file` is relative
 * to the package root (which is the target as given, so repo-relative here);
 * a loose file's is the path as given.
 */
export function moduleFile(report: DocReport, mod: DocModule): string {
  if (!report.package || report.package.root.startsWith("<")) return mod.file;
  return join(report.package.root, mod.file);
}

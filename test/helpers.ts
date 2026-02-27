import { execSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../rust/target/debug/petal");
/**
 * No-op. Build now happens once in globalSetup (test/global-setup.ts)
 * before any test workers start, eliminating the race condition where
 * parallel workers would all run cargo build concurrently.
 */
export function ensureBuild() {}

function run(args: string[]): string {
  return execSync([PETAL, ...args].join(" "), {
    encoding: "utf-8",
    timeout: 10000,
  }).trim();
}

export function showIrJson(code: string): any {
  return JSON.parse(run(["show-ir", "--json", "-e", shellEscape(code)]));
}

export function showAstJson(code: string): any {
  return JSON.parse(run(["show-ast", "--json", "-e", shellEscape(code)]));
}

export function showTokensJson(code: string): any {
  return JSON.parse(run(["show-tokens", "--json", "-e", shellEscape(code)]));
}

export function runPetal(code: string): string {
  return run(["run", "-e", shellEscape(code)]);
}

/** Run petal code that's expected to fail, return stderr */
export function runPetalError(code: string): string {
  try {
    execSync([PETAL, "run", "-e", shellEscape(code)].join(" "), {
      encoding: "utf-8",
      timeout: 10000,
    });
    throw new Error("Expected petal to fail but it succeeded");
  } catch (e: any) {
    return (e.stderr || "").trim();
  }
}

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

/** Number of builtin phantom terms (t0..t{N-1}) in the root block. */
export const BUILTIN_COUNT = 35;

/** Get only the "user" terms (after builtins) from IR JSON */
export function userTerms(ir: any): any[] {
  return ir.terms.filter((t: any) => t.id >= BUILTIN_COUNT);
}

/** Find a term by name */
export function termByName(ir: any, name: string): any {
  return ir.terms.find((t: any) => t.name === name);
}

/** Find terms by op (string match for simple ops, or object key for complex) */
export function termsByOp(ir: any, op: string): any[] {
  return ir.terms.filter(
    (t: any) => t.op === op || (typeof t.op === "object" && op in t.op)
  );
}

/** Get a term by its id */
export function termById(ir: any, id: number): any {
  return ir.terms.find((t: any) => t.id === id);
}

import { execSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../rust/target/debug/petal");
let built = false;

export function ensureBuild() {
  if (built) return;
  execSync("cargo build --manifest-path rust/Cargo.toml", {
    cwd: resolve(__dirname, ".."),
    stdio: "pipe",
  });
  built = true;
}

function run(args: string[]): string {
  return execSync([PETAL, ...args].join(" "), {
    encoding: "utf-8",
    timeout: 10000,
  }).trim();
}

export function showIrJson(code: string): any {
  ensureBuild();
  return JSON.parse(run(["show-ir", "--json", "-e", shellEscape(code)]));
}

export function showAstJson(code: string): any {
  ensureBuild();
  return JSON.parse(run(["show-ast", "--json", "-e", shellEscape(code)]));
}

export function showTokensJson(code: string): any {
  ensureBuild();
  return JSON.parse(run(["show-tokens", "--json", "-e", shellEscape(code)]));
}

export function runPetal(code: string): string {
  ensureBuild();
  return run(["run", "-e", shellEscape(code)]);
}

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

/** Number of builtin phantom terms (t0..t{N-1}) in the root block. */
export const BUILTIN_COUNT = 21;

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

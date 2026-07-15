import { execSync, spawnSync } from "child_process";
import { resolve } from "path";

const PETAL = resolve(__dirname, "../../rust/target/debug/petal");
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

/** `petal check --json -e <code>`, parsed. */
export function checkJson(code: string): any {
  return JSON.parse(run(["check", "--json", "-e", shellEscape(code)]));
}

/** Run the built binary capturing stdout, stderr, and exit code separately. */
function capture(args: string[]): {
  stdout: string;
  stderr: string;
  code: number;
} {
  const r = spawnSync(PETAL, args, { encoding: "utf-8", timeout: 10000 });
  return {
    stdout: (r.stdout || "").toString(),
    stderr: (r.stderr || "").toString(),
    code: typeof r.status === "number" ? r.status : 1,
  };
}

/** `petal check -e <code>` (text mode): capture stdout/stderr/exit code. */
export function checkText(code: string): {
  stdout: string;
  stderr: string;
  code: number;
} {
  return capture(["check", "-e", code]);
}

/** `petal check --strict -e <code>`: capture stdout/stderr/exit code. */
export function checkStrict(code: string): {
  stdout: string;
  stderr: string;
  code: number;
} {
  return capture(["check", "--strict", "-e", code]);
}

/** `petal run -e <code>`: capture both stdout and stderr (expects success). */
export function runWithStderr(code: string): {
  stdout: string;
  stderr: string;
} {
  const { stdout, stderr } = capture(["run", "-e", code]);
  return { stdout, stderr };
}

/** Raw `show-ir --json` output as a JSON string (not parsed). */
export function showIrJsonRaw(code: string): string {
  return run(["show-ir", "--json", "-e", shellEscape(code)]);
}

/** Run a JSON IR string through `petal run --ir -` (IR read from stdin). */
export function runIr(irJson: string): string {
  return execSync([PETAL, "run", "--ir", "-"].join(" "), {
    encoding: "utf-8",
    timeout: 10000,
    input: irJson,
  }).trim();
}

/** Run a JSON IR file through `petal run --ir <path>`. */
export function runIrFile(path: string): string {
  return run(["run", "--ir", path]);
}

/** Expect `petal run --ir -` to fail; return its stderr. */
export function runIrError(irJson: string): string {
  try {
    execSync([PETAL, "run", "--ir", "-"].join(" "), {
      encoding: "utf-8",
      timeout: 10000,
      input: irJson,
      stdio: ["pipe", "pipe", "pipe"],
    });
    throw new Error("Expected petal to fail but it succeeded");
  } catch (e: any) {
    return (e.stderr || "").trim();
  }
}

/** Run petal code that's expected to fail, return stderr.
 *  Uses pipe stdio to prevent error messages from leaking into test output. */
export function runPetalError(code: string): string {
  try {
    execSync([PETAL, "run", "-e", shellEscape(code)].join(" "), {
      encoding: "utf-8",
      timeout: 10000,
      stdio: ["pipe", "pipe", "pipe"],
    });
    throw new Error("Expected petal to fail but it succeeded");
  } catch (e: any) {
    return (e.stderr || "").trim();
  }
}

function shellEscape(s: string): string {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

/** Get only the "user" terms (after builtins) from IR JSON.
 *  Builtin phantom terms are Copy ops with no inputs and a name. */
export function userTerms(ir: any): any[] {
  return ir.terms.filter(
    (t: any) =>
      !(t.op === "Copy" && t.inputs.length === 0 && t.name != null)
  );
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

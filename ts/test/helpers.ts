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

/** `petal show-tokens -e <code>` — the text form, one token per line. */
export function showTokensText(code: string): string {
  return run(["show-tokens", "-e", shellEscape(code)]);
}

/** Map show-tokens JSON rows to their kind names. */
export function tokenKinds(tokens: any[]): string[] {
  return tokens.map((t: any) => t.kind);
}

export function showBytecodeJson(code: string): any {
  return JSON.parse(run(["show-bytecode", "--json", "-e", shellEscape(code)]));
}

/** `petal show-bytecode -e <code>` (text form), raw stdout. */
export function showBytecodeText(code: string): string {
  return run(["show-bytecode", "-e", shellEscape(code)]);
}

export function runPetal(code: string): string {
  return run(["run", "-e", shellEscape(code)]);
}

/** `petal check --json -e <code>`, parsed. */
export function checkJson(code: string): any {
  return JSON.parse(run(["check", "--json", "-e", shellEscape(code)]));
}

// ---------------------------------------------------------------------------
// Dataflow-query helpers. Shared rather than redefined per suite so the cell
// frontier (docs/dev/var-next-steps.md, Provenance) is asserted the same way
// everywhere it surfaces.
// ---------------------------------------------------------------------------

/** `petal show-provenance --json --term <term>`, parsed. */
export function showProvenanceJson(code: string, term: string): any {
  return JSON.parse(
    run(["show-provenance", "--json", "--term", term, "-e", shellEscape(code)])
  );
}

/** `petal show-dependents --json --term <term>`, parsed. */
export function showDependentsJson(code: string, term: string): any {
  return JSON.parse(
    run(["show-dependents", "--json", "--term", term, "-e", shellEscape(code)])
  );
}

/** `petal show-slice --json --term <t>...`, parsed. */
export function showSliceJson(code: string, terms: string[]): any {
  const termArgs = terms.flatMap((t) => ["--term", t]);
  return JSON.parse(run(["show-slice", "--json", ...termArgs, "-e", shellEscape(code)]));
}

/** `petal explain --json --term <term>`, parsed. Runs the program. */
export function explainJson(code: string, term: string): any {
  return JSON.parse(
    run(["explain", "--json", "--term", term, "-e", shellEscape(code)])
  );
}

/** `petal <cmd> --term <term>` in text mode, raw stdout. */
export function dataflowText(cmd: string, code: string, terms: string[]): string {
  const termArgs = terms.flatMap((t) => ["--term", t]);
  return run([cmd, ...termArgs, "-e", shellEscape(code)]);
}

/** Run the built binary capturing stdout, stderr, and exit code separately.
 *  `input`, when given, is written to the child's stdin (for `--ir -`). */
function capture(
  args: string[],
  input?: string
): {
  stdout: string;
  stderr: string;
  code: number;
} {
  const r = spawnSync(PETAL, args, { encoding: "utf-8", timeout: 10000, input });
  return {
    stdout: (r.stdout || "").toString(),
    stderr: (r.stderr || "").toString(),
    code: typeof r.status === "number" ? r.status : 1,
  };
}

/**
 * `petal check --json -e <code>`, parsed, tolerating a non-zero exit — unlike
 * [`checkJson`], which throws. Use when the failure object *is* the assertion.
 */
export function checkJsonAllowFail(code: string): any {
  return JSON.parse(capture(["check", "--json", "-e", code]).stdout);
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

/**
 * `petal check --json --ir -` on a JSON IR string (IR read from stdin),
 * parsed, tolerating a non-zero exit — the failure object *is* the assertion.
 */
export function checkIrJsonAllowFail(irJson: string): any {
  return JSON.parse(capture(["check", "--json", "--ir", "-"], irJson).stdout);
}

/** `petal check --ir -` (text mode) on a JSON IR string read from stdin. */
export function checkIrText(irJson: string): {
  stdout: string;
  stderr: string;
  code: number;
} {
  return capture(["check", "--ir", "-"], irJson);
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
 *  Builtin phantom terms are Copy ops with no inputs and a name.
 *  Note: since schema 0.2 empty `inputs` is omitted on the wire. */
export function userTerms(ir: any): any[] {
  return ir.terms.filter(
    (t: any) =>
      !(t.op === "Copy" && (t.inputs ?? []).length === 0 && t.name != null)
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

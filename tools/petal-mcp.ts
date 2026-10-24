#!/usr/bin/env node --experimental-strip-types
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFile } from "node:child_process";
import { writeFile, unlink, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { proposeEditArgs, type ProposeInput } from "./petal-mcp-propose.ts";

const projectRoot = resolve(import.meta.dirname, "..");
const petalBin = join(projectRoot, "core/target/debug/petal");

type ToolResult = { content: { type: "text"; text: string }[]; isError?: boolean };

function runCommand(cmd: string, args: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  return new Promise((resolve) => {
    execFile(cmd, args, { timeout: 10_000 }, (error, stdout, stderr) => {
      const exitCode = error ? (error as any).code ?? 1 : 0;
      resolve({ stdout, stderr, exitCode });
    });
  });
}

async function ensureBuild(): Promise<ToolResult | null> {
  const build = await runCommand("cargo", [
    "build", "--quiet", "--manifest-path", join(projectRoot, "core/Cargo.toml"),
  ]);
  if (build.exitCode !== 0) {
    return { content: [{ type: "text", text: `Build failed:\n${build.stderr}` }], isError: true };
  }
  return null;
}

type PetalRun = { stdout: string; stderr: string; exitCode: number };

// Build the binary, write `code` to a temp .ptl file, run `petal <args...> <file>`,
// and clean up. Returns the build failure as a ToolResult, or the raw run.
async function runPetalCommand(args: string[], code: string): Promise<ToolResult | PetalRun> {
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;
  const tmpFile = join(tmpdir(), `petal-${randomBytes(8).toString("hex")}.ptl`);
  await writeFile(tmpFile, code);
  try {
    return await runCommand(petalBin, [...args, tmpFile]);
  } finally {
    await unlink(tmpFile).catch(() => {});
  }
}

// The common tool shape: stdout (or stderr, or `fallback`) as text, error on non-zero exit.
async function petalTool(args: string[], code: string, fallback = ""): Promise<ToolResult> {
  const result = await runPetalCommand(args, code);
  if ("content" in result) return result;
  return {
    content: [{ type: "text", text: result.stdout || result.stderr || fallback }],
    isError: result.exitCode !== 0,
  };
}

const server = new McpServer({
  name: "petal-tools",
  version: "1.0.0",
});

server.registerTool("TestSnippet", {
  title: "Test Petal Snippet",
  description:
    "Compiles and runs a snippet of Petal code, returning stdout, stderr, and exit code. " +
    "Non-fatal type-checker warnings (if any) are printed to stderr before the " +
    "program's own output; they never change the exit code or runtime behavior. " +
    "Set `trace: true` to also record a structured per-term execution trace " +
    "(returned as parsed JSON in the tool result). Use this when debugging " +
    "wrong values or off-by-one bugs — the trace shows every term's inputs " +
    "and result with source line/column.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    trace: z
      .boolean()
      .optional()
      .describe(
        "If true, record a per-term execution trace and include it in the result.",
      ),
  },
}, async ({ code, trace }) => {
  const traceFile = trace
    ? join(tmpdir(), `petal-${randomBytes(8).toString("hex")}-trace.json`)
    : null;
  const args = ["run"];
  if (traceFile) args.push("--record-trace", traceFile);

  const result = await runPetalCommand(args, code);
  if ("content" in result) return result;

  let traceJson: string | null = null;
  if (traceFile) {
    try {
      traceJson = await readFile(traceFile, "utf8");
    } catch {
      // trace file may not exist if the program failed before any term ran
    } finally {
      await unlink(traceFile).catch(() => {});
    }
  }

  const sections = [
    result.stdout ? `stdout:\n${result.stdout}` : "stdout: (empty)",
    result.stderr ? `stderr:\n${result.stderr}` : "",
    `Exit code: ${result.exitCode}`,
  ].filter(Boolean);
  if (traceJson) {
    sections.push(`trace:\n${traceJson}`);
  }

  return {
    content: [{ type: "text", text: sections.join("\n\n") }],
    isError: result.exitCode !== 0,
  };
});

server.registerTool("ExplainTerm", {
  title: "Explain a Petal term",
  description:
    "Runs Petal code with execution tracing enabled, then walks the dataflow " +
    "graph backward from `term` and reports every recorded value (the target " +
    "and its ancestors). At a `var` read the walk names the `set` that " +
    "actually supplied the value, lists every write to that cell in order, " +
    "and continues the chain through it. Use this to answer 'why does X have " +
    "value Y?'.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    term: z
      .string()
      .describe("Variable name (e.g. 'total'), term id (e.g. '72' or 't72')"),
  },
}, ({ code, term }) => petalTool(["explain", "--json", "--term", term], code));

server.registerTool("CheckSnippet", {
  title: "Check Petal Snippet",
  description:
    "Lex+parse+compile a Petal snippet without running it. Returns " +
    "{ok, warnings: [...]} where each entry is a checker diagnostic " +
    "{message, severity, line, column, file}. severity \"warning\" is advice " +
    "(a declared/inferred type mismatch, an unknown type name) and never fails " +
    "the check; severity \"error\" is a line that fails whenever it runs (an " +
    "unknown function or variable, a call no overload's arity accepts) and makes " +
    "ok false. The names known are core builtins plus petal-ui. A program that " +
    "does not compile returns a structured error with phase/line/column instead. " +
    "Cheaper than TestSnippet for validating syntax and type annotations.",
  inputSchema: {
    code: z.string().describe("The Petal source code to validate"),
  },
}, ({ code }) => petalTool(["check", "--json"], code, '{"ok": true, "warnings": []}'));

const stageCommands = {
  tokens: "show-tokens",
  ast: "show-ast",
  ir: "show-ir",
  bytecode: "show-bytecode",
} as const;

server.registerTool("ShowStage", {
  title: "Show a Petal compilation stage",
  description:
    "Runs Petal code through the compiler up to one stage and returns that stage's " +
    "dump — the same output as the CLI's `show-tokens` / `show-ast` / `show-ir` / " +
    "`show-bytecode`. `stage: \"tokens\"` is the lexer's token list; `\"ast\"` the " +
    "parsed syntax tree; `\"ir\"` the intermediate representation; `\"bytecode\"` the " +
    "bytecode lowering (one function per entry, with disassembled instructions and " +
    "register metadata). JSON by default; `json: false` returns the human-readable " +
    "text form. For `ir`, the default is the user-only view (`show-ir --user-only`): " +
    "builtin phantom terms, the auto-loaded std prelude, and imported-module internals " +
    "are filtered out, and in JSON `constants.values` is an id-keyed object. Ids are " +
    "preserved, but the view is not loadable by `run --ir`. Pass `all: true` for the " +
    "complete Program object (the `run --ir` interchange format).",
  inputSchema: {
    code: z.string().describe("The Petal source code to compile"),
    stage: z.enum(["tokens", "ast", "ir", "bytecode"]).describe("Which compilation stage to dump"),
    json: z.boolean().optional().describe("Return JSON (default true); false returns the text dump"),
    all: z.boolean().optional().describe(
      "ir only: return the complete program, including builtin phantom terms and prelude/module internals"
    ),
  },
}, ({ code, stage, json, all }) => {
  const args: string[] = [stageCommands[stage]];
  const asJson = json !== false;
  if (asJson) args.push("--json");
  // show-ir's text form is already user-only; its JSON form needs the flag.
  if (stage === "ir" && all) args.push("--all");
  else if (stage === "ir" && asJson) args.push("--user-only");
  return petalTool(args, code);
});

server.registerTool("TraceEmits", {
  title: "Trace Emitted Values",
  description:
    "Runs Petal code with emit tracing and returns, per output channel " +
    "(anything pushed with push_output / draw commands), every emitted value " +
    "with its attribution: the call that produced it (callee, span, term id), " +
    "per-argument edit info (kind literal|binding|computed, resolved " +
    "constant value, span, editable span), the emitting native's raw " +
    "arguments (native_args) and the emit's stable key. This is the " +
    "observation half of direct manipulation " +
    "(docs/features/direct-manipulation.md): use it to see what a script " +
    "emitted and which source text each value traces to, then use " +
    "ProposeEdit to turn a change request into concrete source edits. Pass " +
    "the same `seed` to both when the script draws random numbers.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    seed: z.number().int().min(0).optional()
      .describe("Seed the random stream (ProposeEdit's v2 engine defaults to 0)"),
  },
}, ({ code, seed }) =>
  petalTool(["run", "--trace-emits", "--json", ...(seed !== undefined ? ["--seed", String(seed)] : [])], code));

const goalShape = {
  arg: z.number().int().min(0).optional()
    .describe("Argument k (0-based) of the user-file call that produced the emit"),
  nativeArg: z.number().int().min(0).optional()
    .describe("v2: raw argument k of the native that produced the emit (before any int truncation)"),
  value: z.boolean().optional().describe("v2: address the emitted value itself"),
  field: z.string().optional().describe("v2: a path into the addressed value, e.g. 'center.0' or 'x'"),
  to: z.union([z.string(), z.number()]).optional()
    .describe("Goal value: 55, 2.5 (v1 also takes true / hello; v2 hands non-numbers to v1)"),
  by: z.number().optional().describe("v2: goal is the current value plus this delta"),
};

server.registerTool("ProposeEdit", {
  title: "Propose Goal-Based Source Edit",
  description:
    "The goal-based direct-manipulation query (docs/features/direct-manipulation.md): " +
    "say where an emitted value should be, get the source edits that put it there. " +
    "Nothing is written; applying is the caller's move. Use TraceEmits first to find " +
    "channel / emit / argument indices.\n\n" +
    "engine 'v2' (chosen automatically when any v2-only field is given) re-runs the " +
    "program speculatively with each candidate literal nudged (effects suppressed, " +
    "random draws replayed) and solves THROUGH function calls, loops, records and " +
    "math. It returns JSON: `alternatives` (ranked; each has `verified` — re-run and " +
    "confirmed —, `description`, `changes` (old_text → new_text per literal, with " +
    "binding / function / line), `edits` (file + span + new_text), `achieved`, " +
    "`misses`, `max_error`, `collateral` (other emitted values it moves) and " +
    "`score`), `candidates` (every literal considered, with why it was excluded and its " +
    "measured sensitivity `column`) and `diagnostics`. Workflow: ask once (with " +
    "`explain: true` to see the candidate table), then re-ask with `allow` naming the " +
    "literals you want moved (a binding name, 'line:N', 'fn:NAME', 'config') or `deny` " +
    "for ones to keep — the engine works out the values. In source, `config let` and " +
    "`tunable(x, min, max)` mark preferred knobs and `rigid(x)` freezes a value.\n\n" +
    "engine 'v1' (the default for arg+to goals) inverts the emitting call's own " +
    "arguments: a literal, a binding, or + - * / arithmetic; it refuses values that " +
    "come out of a function call. Narrow v1 with `configurable` / `static`.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    channel: z.string().describe("Output channel name (e.g. 'draw_commands')"),
    emit: z.number().int().min(0).describe("0-based emit index within the channel"),
    engine: z.enum(["v1", "v2"]).optional()
      .describe("Which solver; default v2 when a v2-only field is given, else v1"),
    ...goalShape,
    goals: z.array(z.object({
      channel: z.string().optional().describe("Defaults to the top-level channel"),
      emit: z.number().int().min(0).optional().describe("Defaults to the top-level emit (v2 may use several)"),
      ...goalShape,
    })).optional()
      .describe("Several goals changed together (e.g. x and y of one drag)"),
    configurable: z.array(z.string()).optional()
      .describe("Variables to prefer editing (v1); under v2 the same as allow"),
    static: z.array(z.string()).optional()
      .describe("Variables that must not be edited (v1); under v2 the same as deny"),
    allow: z.array(z.string()).optional()
      .describe("v2: only these may move: binding names, 'line:N', 'fn:NAME', 'config', 'user'"),
    deny: z.array(z.string()).optional().describe("v2: these may not move (same selectors)"),
    configStrict: z.boolean().optional().describe("v2: v1's rule that config lets win outright"),
    alternatives: z.number().int().min(1).optional().describe("v2: how many alternatives (default 3)"),
    explain: z.boolean().optional().describe("v2: include the pipeline log and candidate table"),
    tol: z.number().optional().describe("v2: goal tolerance"),
    maxRuns: z.number().int().min(1).optional().describe("v2: speculative-run budget (default 200)"),
    seed: z.number().int().min(0).optional().describe("v2: random seed (default 0)"),
  },
}, async (input) => {
  const built = proposeEditArgs(input as ProposeInput);
  if ("error" in built) {
    return { content: [{ type: "text" as const, text: built.error }], isError: true };
  }
  return petalTool(built.args, input.code);
});

server.registerTool("PendingReport", {
  title: "Pending Report",
  description:
    "Runs Petal code and returns the frame pending report as JSON: an array of " +
    "every live pending/unresolved resource with its id, key, state " +
    "(loading|errored|ready), age in frames, origin call site, and how many " +
    "operations absorbed it this frame. Use it to debug why a region is blank — " +
    "which resources stayed unresolved and where they came from.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
  },
}, ({ code }) => petalTool(["pending-report", "--json"], code));

const transport = new StdioServerTransport();
await server.connect(transport);

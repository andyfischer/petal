#!/usr/bin/env node --experimental-strip-types
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFile } from "node:child_process";
import { writeFile, unlink, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { randomBytes } from "node:crypto";

const projectRoot = resolve(import.meta.dirname, "..", "..");
const petalBin = join(projectRoot, "rust/target/debug/petal");

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
    "build", "--quiet", "--manifest-path", join(projectRoot, "rust/Cargo.toml"),
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
    "Lex+parse+compile a Petal snippet without running it. On success returns " +
    "{ok: true, warnings: [...]} where each warning is a non-fatal type-checker " +
    "diagnostic {message, line, column, file} (e.g. a declared/inferred type " +
    "mismatch or unknown type name); on failure a structured error with " +
    "phase/line/column. Warnings never fail the check. Cheaper than TestSnippet " +
    "for validating syntax and type annotations.",
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
    "with its attribution: the call that produced it (callee, span, term id) " +
    "and per-argument edit info (kind literal|binding|computed, resolved " +
    "constant value, span, editable span). This is the observation half of " +
    "direct manipulation (docs/direct-manipulation.md): use it to see what a " +
    "live script emitted and which source text each value traces to, then " +
    "use ProposeEdit to turn a change request into concrete source edits.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
  },
}, ({ code }) => petalTool(["run", "--trace-emits", "--json"], code));

server.registerTool("ProposeEdit", {
  title: "Propose Goal-Based Source Edit",
  description:
    "The goal-based direct-manipulation query (docs/direct-manipulation.md). " +
    "Runs Petal code with emit tracing, finds the call that produced emit " +
    "number `emit` on `channel`, and returns source-edit proposals that make " +
    "argument `arg` of that call evaluate to `to`. A literal argument yields " +
    "one direct edit; a computed argument (e.g. `x + offset`) yields one " +
    "proposal per contributing variable, solved with the values the run " +
    "actually saw. Narrow multiple proposals by naming variables in " +
    "`configurable` (prefer editing these) or `static` (never edit these) — " +
    "or declare knobs in the source itself with `config let x = …`, which " +
    "makes config bindings the default edit targets and pins the rest. " +
    "Pass `goals` instead of `arg`/`to` to state a multi-goal batch (one " +
    "gesture changing several arguments of the same call), resolved so all " +
    "goals can hold together. Each proposal carries the span " +
    "(line/column/offset) and replacement text; nothing is written — " +
    "applying is the caller's move. Use TraceEmits first to find channel / " +
    "emit / arg indices.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    channel: z.string().describe("Output channel name (e.g. 'draw_commands')"),
    emit: z.number().int().min(0).describe("0-based emit index within the channel"),
    arg: z.number().int().min(0).optional()
      .describe("0-based argument position in the emitting call (single-goal form)"),
    to: z.string().optional()
      .describe("Goal value as source-ish text: 55, 2.5, true, hello (single-goal form)"),
    goals: z.array(z.object({
      arg: z.number().int().min(0).describe("0-based argument position"),
      to: z.string().describe("Goal value as source-ish text"),
    })).optional()
      .describe("Multi-goal batch: several arguments of the same emit that must change together"),
    configurable: z.array(z.string()).optional()
      .describe("Variables to prefer editing"),
    static: z.array(z.string()).optional()
      .describe("Variables that must not be edited"),
  },
}, async ({ code, channel, emit, arg, to, goals, configurable, static: pinned }) => {
  const pairs = goals ?? (arg !== undefined && to !== undefined ? [{ arg, to }] : []);
  if (pairs.length === 0) {
    return {
      content: [{ type: "text" as const, text: "Provide either arg+to or a non-empty goals array." }],
      isError: true,
    };
  }
  const args = [
    "propose-edit", "--json",
    "--channel", channel,
    "--emit", String(emit),
  ];
  for (const g of pairs) args.push("--arg", String(g.arg), "--to", g.to);
  for (const name of configurable ?? []) args.push("--configurable", name);
  for (const name of pinned ?? []) args.push("--static", name);
  return petalTool(args, code);
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

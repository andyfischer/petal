#!/usr/bin/env node --experimental-strip-types
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFile } from "node:child_process";
import { writeFile, unlink, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { randomBytes } from "node:crypto";

const projectRoot = resolve(import.meta.dirname, "..");
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

async function runPetalCommand(args: string[]): Promise<ToolResult> {
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;

  const result = await runCommand(petalBin, args);
  if (result.exitCode !== 0) {
    return { content: [{ type: "text", text: `Error:\n${result.stderr}` }], isError: true };
  }
  return { content: [{ type: "text", text: result.stdout }] };
}

const server = new McpServer({
  name: "petal-tools",
  version: "1.0.0",
});

server.registerTool("TestSnippet", {
  title: "Test Petal Snippet",
  description:
    "Compiles and runs a snippet of Petal code, returning stdout, stderr, and exit code. " +
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
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;

  const tmpFile = join(tmpdir(), `petal-${randomBytes(8).toString("hex")}.ptl`);
  const traceFile = trace
    ? join(tmpdir(), `petal-${randomBytes(8).toString("hex")}-trace.json`)
    : null;
  await writeFile(tmpFile, code);

  try {
    const args = ["run"];
    if (traceFile) args.push("--record-trace", traceFile);
    args.push(tmpFile);

    const result = await runCommand(petalBin, args);

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
  } finally {
    await unlink(tmpFile).catch(() => {});
  }
});

server.registerTool("ExplainTerm", {
  title: "Explain a Petal term",
  description:
    "Runs Petal code with execution tracing enabled, then walks the dataflow " +
    "graph backward from `term` and reports every recorded value (the target " +
    "and its ancestors). For variables that get reassigned in a loop, also " +
    "lists every write in order. Use this to answer 'why does X have value Y?'.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
    term: z
      .string()
      .describe("Variable name (e.g. 'total'), term id (e.g. '72' or 't72')"),
  },
}, async ({ code, term }) => {
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;
  const tmpFile = join(tmpdir(), `petal-${randomBytes(8).toString("hex")}.ptl`);
  await writeFile(tmpFile, code);
  try {
    const result = await runCommand(petalBin, ["explain", "--json", "--term", term, tmpFile]);
    return {
      content: [{ type: "text", text: result.stdout || result.stderr }],
      isError: result.exitCode !== 0,
    };
  } finally {
    await unlink(tmpFile).catch(() => {});
  }
});

server.registerTool("CheckSnippet", {
  title: "Check Petal Snippet",
  description:
    "Lex+parse+compile a Petal snippet without running it. Returns either " +
    "{ok: true} or a structured error with phase/line/column. Cheaper than " +
    "TestSnippet for validating syntax.",
  inputSchema: {
    code: z.string().describe("The Petal source code to validate"),
  },
}, async ({ code }) => {
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;
  const tmpFile = join(tmpdir(), `petal-${randomBytes(8).toString("hex")}.ptl`);
  await writeFile(tmpFile, code);
  try {
    const result = await runCommand(petalBin, ["check", "--json", tmpFile]);
    return {
      content: [{ type: "text", text: result.stdout || result.stderr || '{"ok": true}' }],
      isError: result.exitCode !== 0,
    };
  } finally {
    await unlink(tmpFile).catch(() => {});
  }
});

server.registerTool("ShowIR", {
  title: "Show Petal IR",
  description: "Compiles Petal code and returns the intermediate representation (IR) as JSON.",
  inputSchema: {
    code: z.string().describe("The Petal source code to compile"),
  },
}, ({ code }) => runPetalCommand(["show-ir", "--json", "-e", code]));

server.registerTool("ShowAST", {
  title: "Show Petal AST",
  description: "Parses Petal code and returns the abstract syntax tree (AST) as JSON.",
  inputSchema: {
    code: z.string().describe("The Petal source code to parse"),
  },
}, ({ code }) => runPetalCommand(["show-ast", "--json", "-e", code]));

server.registerTool("ShowTokens", {
  title: "Show Petal Tokens",
  description: "Lexes Petal code and returns the token list as JSON.",
  inputSchema: {
    code: z.string().describe("The Petal source code to tokenize"),
  },
}, ({ code }) => runPetalCommand(["show-tokens", "--json", "-e", code]));

const transport = new StdioServerTransport();
await server.connect(transport);

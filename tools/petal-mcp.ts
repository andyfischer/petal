#!/usr/bin/env node --experimental-strip-types
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFile } from "node:child_process";
import { writeFile, unlink } from "node:fs/promises";
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
  description: "Compiles and runs a snippet of Petal code, returning stdout, stderr, and exit code.",
  inputSchema: {
    code: z.string().describe("The Petal source code to run"),
  },
}, async ({ code }) => {
  const buildErr = await ensureBuild();
  if (buildErr) return buildErr;

  const tmpFile = join(tmpdir(), `petal-${randomBytes(8).toString("hex")}.ptl`);
  await writeFile(tmpFile, code);

  try {
    const result = await runCommand(petalBin, [tmpFile]);
    const output = [
      result.stdout ? `stdout:\n${result.stdout}` : "stdout: (empty)",
      result.stderr ? `stderr:\n${result.stderr}` : "",
      `Exit code: ${result.exitCode}`,
    ].filter(Boolean).join("\n\n");

    return {
      content: [{ type: "text", text: output }],
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

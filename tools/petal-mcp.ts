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

function runCommand(cmd: string, args: string[]): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  return new Promise((resolve) => {
    execFile(cmd, args, { timeout: 10_000 }, (error, stdout, stderr) => {
      const exitCode = error ? (error as any).code ?? 1 : 0;
      resolve({ stdout, stderr, exitCode });
    });
  });
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
  // Build first (like test-snippet.sh does)
  const build = await runCommand("cargo", [
    "build", "--quiet", "--manifest-path", join(projectRoot, "rust/Cargo.toml"),
  ]);

  if (build.exitCode !== 0) {
    return {
      content: [{ type: "text", text: `Build failed:\n${build.stderr}` }],
      isError: true,
    };
  }

  // Write snippet to a temp file
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

const transport = new StdioServerTransport();
await server.connect(transport);

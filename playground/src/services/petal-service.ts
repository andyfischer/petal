import { createEndpoint } from '@facetlayer/prism-framework';
import type { ServiceDefinition } from '@facetlayer/prism-framework';
import { z } from 'zod';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';

const execFileAsync = promisify(execFile);

const PROJECT_ROOT = path.resolve(new URL('../../..', import.meta.url).pathname);

const PETAL_BIN = path.resolve(PROJECT_ROOT, 'rust/target/debug/petal');
const EXAMPLES_DIR = path.resolve(PROJECT_ROOT, 'examples');

async function runPetal(
  command: string,
  code: string,
  json: boolean = false,
): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  const args = [command];
  if (json) args.push('--json');
  args.push('-e', code);

  try {
    const { stdout, stderr } = await execFileAsync(PETAL_BIN, args, {
      timeout: 5000,
      maxBuffer: 1024 * 1024,
    });
    return { stdout, stderr, exitCode: 0 };
  } catch (err: any) {
    return {
      stdout: err.stdout || '',
      stderr: err.stderr || err.message || 'Unknown error',
      exitCode: err.code === 'ETIMEDOUT' ? 124 : (err.status || 1),
    };
  }
}

const analyze = createEndpoint({
  method: 'POST',
  path: '/analyze',
  description: 'Analyze Petal code: returns tokens, AST, IR, and run output',
  requestSchema: z.object({
    code: z.string(),
  }),
  handler: async (input: { code: string }) => {
    const code = input.code;

    const [tokens, ast, ir, run] = await Promise.all([
      runPetal('show-tokens', code, true),
      runPetal('show-ast', code, true),
      runPetal('show-ir', code, true),
      runPetal('run', code),
    ]);

    return {
      tokens: {
        json: tokens.exitCode === 0 ? tokens.stdout : null,
        error: tokens.exitCode !== 0 ? tokens.stderr : null,
      },
      ast: {
        json: ast.exitCode === 0 ? ast.stdout : null,
        error: ast.exitCode !== 0 ? ast.stderr : null,
      },
      ir: {
        json: ir.exitCode === 0 ? ir.stdout : null,
        error: ir.exitCode !== 0 ? ir.stderr : null,
      },
      run: {
        output: run.stdout,
        error: run.stderr || null,
        exitCode: run.exitCode,
      },
    };
  },
});

const analyzeText = createEndpoint({
  method: 'POST',
  path: '/analyze-text',
  description: 'Analyze Petal code: returns human-readable text representations',
  requestSchema: z.object({
    code: z.string(),
  }),
  handler: async (input: { code: string }) => {
    const code = input.code;

    const [tokens, ast, ir] = await Promise.all([
      runPetal('show-tokens', code, false),
      runPetal('show-ast', code, false),
      runPetal('show-ir', code, false),
    ]);

    return {
      tokens: tokens.exitCode === 0 ? tokens.stdout : tokens.stderr,
      ast: ast.exitCode === 0 ? ast.stdout : ast.stderr,
      ir: ir.exitCode === 0 ? ir.stdout : ir.stderr,
    };
  },
});

const listExamples = createEndpoint({
  method: 'GET',
  path: '/examples',
  description: 'List available Petal example files',
  handler: async () => {
    const files = await readdir(EXAMPLES_DIR);
    const ptlFiles = files.filter((f) => f.endsWith('.ptl')).sort();
    return Promise.all(
      ptlFiles.map(async (f) => ({
        filename: f,
        name: f.replace('.ptl', '').replace(/_/g, ' '),
        content: await readFile(path.join(EXAMPLES_DIR, f), 'utf-8'),
      })),
    );
  },
});

export const petalService: ServiceDefinition = {
  name: 'petal',
  endpoints: [analyze, analyzeText, listExamples],
};

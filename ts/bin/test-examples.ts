#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Run every examples/*.ptl file as a differential test between the two
// execution backends (graph and bytecode): each example must exit 0 under
// both AND produce byte-identical stdout/stderr.
// Usage:
//   ./bin/test-examples.ts                    # differential sweep, 8-line preview
//   ./bin/test-examples.ts --full             # differential sweep, full output
//   ./bin/test-examples.ts --backend=graph    # single backend, no diff
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const examplesDir = join(repoRoot, 'examples');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'debug', 'petal');

const ALL_BACKENDS = ['graph', 'bytecode'];
const full = process.argv.includes('--full');
const backendArg = process.argv
    .map(a => /^--backend=(.+)$/.exec(a)?.[1])
    .find(Boolean);
const backends = backendArg ? [backendArg] : ALL_BACKENDS;

interface RunResult {
    status: number | null;
    stdout: string;
    stderr: string;
}

function runExample(filePath: string, backend: string): RunResult {
    const result = spawnSync(petal, [filePath, `--backend=${backend}`], {
        encoding: 'utf-8',
    });
    return {
        status: result.status,
        stdout: result.stdout ?? '',
        stderr: result.stderr ?? '',
    };
}

function printPreview(output: string) {
    if (full) {
        process.stdout.write(output);
        return;
    }
    const lines = output.split('\n');
    const head = lines.slice(0, 8).join('\n');
    process.stdout.write(head);
    if (!head.endsWith('\n')) process.stdout.write('\n');
    if (lines.length > 8) console.log(`  ... (${lines.length} lines total)`);
}

// Report the first line where two outputs disagree, for quick triage.
function firstDivergence(a: string, b: string): string {
    const aLines = a.split('\n');
    const bLines = b.split('\n');
    const n = Math.max(aLines.length, bLines.length);
    for (let i = 0; i < n; i++) {
        if (aLines[i] !== bLines[i]) {
            return [
                `  first divergence at line ${i + 1}:`,
                `    ${backends[0]}:    ${aLines[i] ?? '<missing>'}`,
                `    ${backends[1]}: ${bLines[i] ?? '<missing>'}`,
            ].join('\n');
        }
    }
    return '  outputs differ (identical lines, differing whitespace?)';
}

const build = spawnSync(
    'cargo',
    ['build', '--quiet', '--manifest-path', cargoToml],
    { stdio: 'inherit' },
);
if (build.status !== 0) process.exit(build.status ?? 1);

const files = readdirSync(examplesDir).filter(f => f.endsWith('.ptl')).sort();
let pass = 0;
let fail = 0;

for (const name of files) {
    const filePath = join(examplesDir, name);
    console.log(`=== ${name} ===`);
    const runs = backends.map(b => runExample(filePath, b));

    const failed = runs.find(r => r.status !== 0);
    if (failed) {
        const which = backends[runs.indexOf(failed)];
        const head = (failed.stdout + failed.stderr).split('\n').slice(0, 5).join('\n');
        console.log(`FAILED (${which}): ${head}`);
        fail++;
    } else if (
        runs.length === 2 &&
        (runs[0].stdout !== runs[1].stdout || runs[0].stderr !== runs[1].stderr)
    ) {
        const onStdout = runs[0].stdout !== runs[1].stdout;
        console.log(`BACKEND DIVERGENCE (${backends.join(' vs ')}, ${onStdout ? 'stdout' : 'stderr'}):`);
        console.log(onStdout
            ? firstDivergence(runs[0].stdout, runs[1].stdout)
            : firstDivergence(runs[0].stderr, runs[1].stderr));
        fail++;
    } else {
        printPreview(runs[0].stdout + runs[0].stderr);
        pass++;
    }
    console.log();
}

const mode = backends.length === 2 ? `differential (${backends.join(' vs ')})` : backends[0];
console.log(`Results [${mode}]: ${pass} passed, ${fail} failed`);
process.exit(fail > 0 ? 1 : 0);

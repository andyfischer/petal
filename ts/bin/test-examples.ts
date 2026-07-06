#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Run every examples/*.ptl file as a differential test on the bytecode VM: each
// example must exit 0 under both optimization levels (clone-and-alloc baseline
// via --no-opt, and all in-place opts on by default), produce byte-identical
// stdout/stderr across the two, AND match the frozen golden in
// test/example-golden/. The golden corpus is the absolute-correctness anchor;
// regenerate it deliberately with ts/bin/gen-example-golden.ts.
// Usage:
//   ./bin/test-examples.ts            # differential + golden sweep, 8-line preview
//   ./bin/test-examples.ts --full     # same, full output
import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const examplesDir = join(repoRoot, 'examples');
const goldenDir = join(repoRoot, 'test', 'example-golden');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'debug', 'petal');

const full = process.argv.includes('--full');

interface RunResult {
    status: number | null;
    stdout: string;
    stderr: string;
}

// The two bytecode optimization levels the sweep diffs against each other.
const OPT_LEVELS = [
    { label: 'opts', args: [] as string[] },
    { label: 'no-opt', args: ['--no-opt'] },
];

function runExample(filePath: string, optArgs: string[]): RunResult {
    const result = spawnSync(petal, [filePath, ...optArgs], {
        encoding: 'utf-8',
    });
    return {
        status: result.status,
        stdout: result.stdout ?? '',
        stderr: result.stderr ?? '',
    };
}

function loadGolden(name: string): RunResult | null {
    const path = join(goldenDir, name.replace(/\.ptl$/, '.json'));
    if (!existsSync(path)) return null;
    const g = JSON.parse(readFileSync(path, 'utf-8'));
    return { status: g.status, stdout: g.stdout, stderr: g.stderr };
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
function firstDivergence(labelA: string, a: string, labelB: string, b: string): string {
    const aLines = a.split('\n');
    const bLines = b.split('\n');
    const n = Math.max(aLines.length, bLines.length);
    for (let i = 0; i < n; i++) {
        if (aLines[i] !== bLines[i]) {
            return [
                `  first divergence at line ${i + 1}:`,
                `    ${labelA}: ${aLines[i] ?? '<missing>'}`,
                `    ${labelB}: ${bLines[i] ?? '<missing>'}`,
            ].join('\n');
        }
    }
    return '  outputs differ (identical lines, differing whitespace?)';
}

// Compare two runs on stdout+stderr; return a divergence report or null.
function diff(labelA: string, a: RunResult, labelB: string, b: RunResult): string | null {
    if (a.stdout !== b.stdout) {
        return firstDivergence(labelA, a.stdout, labelB, b.stdout);
    }
    if (a.stderr !== b.stderr) {
        return firstDivergence(`${labelA}(stderr)`, a.stderr, `${labelB}(stderr)`, b.stderr);
    }
    return null;
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
let missingGolden = 0;

for (const name of files) {
    const filePath = join(examplesDir, name);
    console.log(`=== ${name} ===`);
    const runs = OPT_LEVELS.map(o => ({ label: o.label, result: runExample(filePath, o.args) }));

    const failed = runs.find(r => r.result.status !== 0);
    if (failed) {
        const head = (failed.result.stdout + failed.result.stderr).split('\n').slice(0, 5).join('\n');
        console.log(`FAILED (${failed.label}): ${head}`);
        fail++;
        console.log();
        continue;
    }

    // Differential: the two optimization levels must agree exactly.
    const optDiff = diff(runs[0].label, runs[0].result, runs[1].label, runs[1].result);
    if (optDiff) {
        console.log(`OPT-LEVEL DIVERGENCE (${runs[0].label} vs ${runs[1].label}):`);
        console.log(optDiff);
        fail++;
        console.log();
        continue;
    }

    // Absolute: both must match the frozen graph-captured golden.
    const golden = loadGolden(name);
    if (!golden) {
        console.log(`NO GOLDEN (run ts/bin/gen-example-golden.ts): ${name}`);
        missingGolden++;
        fail++;
        console.log();
        continue;
    }
    if (golden.status !== runs[0].result.status) {
        console.log(`GOLDEN STATUS MISMATCH: golden=${golden.status} actual=${runs[0].result.status}`);
        fail++;
        console.log();
        continue;
    }
    const goldenDiff = diff('golden', golden, 'actual', runs[0].result);
    if (goldenDiff) {
        console.log('GOLDEN DIVERGENCE (frozen graph output vs bytecode):');
        console.log(goldenDiff);
        fail++;
        console.log();
        continue;
    }

    printPreview(runs[0].result.stdout + runs[0].result.stderr);
    pass++;
    console.log();
}

console.log(
    `Results [differential (opts vs no-opt) + golden corpus]: ${pass} passed, ${fail} failed` +
    (missingGolden > 0 ? ` (${missingGolden} missing golden)` : ''),
);
process.exit(fail > 0 ? 1 : 0);

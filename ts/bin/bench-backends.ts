#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Time every benchmarks/*.ptl under both backends (release build) and report
// per-file medians and the graph/bytecode speed ratio. Outputs must also be
// byte-identical between backends — a divergence fails the run.
// Usage:
//   ./bin/bench-backends.ts             # 5 timed runs per file per backend
//   ./bin/bench-backends.ts --runs=10   # more repetitions
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const benchDir = join(repoRoot, 'benchmarks');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'release', 'petal');

const BACKENDS = ['graph', 'bytecode'];
const runsArg = process.argv
    .map(a => /^--runs=(\d+)$/.exec(a)?.[1])
    .find(Boolean);
const runs = runsArg ? parseInt(runsArg, 10) : 5;

function timeOnce(filePath: string, backend: string): { ms: number; output: string } {
    const start = process.hrtime.bigint();
    const result = spawnSync(petal, [filePath, `--backend=${backend}`], {
        encoding: 'utf-8',
    });
    const ms = Number(process.hrtime.bigint() - start) / 1e6;
    if (result.status !== 0) {
        console.error(`FAILED: ${filePath} (${backend}):\n${result.stdout}${result.stderr}`);
        process.exit(1);
    }
    return { ms, output: result.stdout };
}

function median(xs: number[]): number {
    const sorted = [...xs].sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
}

console.log('Building release binary...');
const build = spawnSync(
    'cargo',
    ['build', '--release', '--quiet', '--manifest-path', cargoToml],
    { stdio: 'inherit' },
);
if (build.status !== 0) process.exit(build.status ?? 1);

const files = readdirSync(benchDir).filter(f => f.endsWith('.ptl')).sort();
console.log(`\n${runs} runs per backend, median reported (includes ~process startup)\n`);
console.log('benchmark        graph (ms)  bytecode (ms)   speedup');
console.log('---------        ----------  -------------   -------');

let diverged = false;
for (const name of files) {
    const filePath = join(benchDir, name);
    const medians: Record<string, number> = {};
    const outputs: Record<string, string> = {};
    for (const backend of BACKENDS) {
        const times: number[] = [];
        for (let i = 0; i < runs; i++) {
            const { ms, output } = timeOnce(filePath, backend);
            times.push(ms);
            outputs[backend] = output;
        }
        medians[backend] = median(times);
    }
    const ratio = medians.graph / medians.bytecode;
    console.log(
        `${name.padEnd(18)}${medians.graph.toFixed(1).padStart(8)}${medians.bytecode
            .toFixed(1)
            .padStart(15)}${(ratio.toFixed(2) + 'x').padStart(10)}`,
    );
    if (outputs.graph !== outputs.bytecode) {
        console.log(`  BACKEND DIVERGENCE in ${name}!`);
        diverged = true;
    }
}
process.exit(diverged ? 1 : 0);

#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Time every test/benchmarks/*.ptl on the bytecode VM at both optimization
// levels (release build) and report per-file medians plus the no-opt/opts
// speedup — i.e. how much the in-place mutation optimization buys. Outputs must
// be byte-identical between the two levels; a divergence fails the run.
// Usage:
//   ./bin/bench-opts.ts             # 5 timed runs per file per opt level
//   ./bin/bench-opts.ts --runs=10   # more repetitions
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const benchDir = join(repoRoot, 'test', 'benchmarks');
const cargoToml = join(repoRoot, 'rust', 'Cargo.toml');
const petal = join(repoRoot, 'rust', 'target', 'release', 'petal');

// The two optimization levels: clone-and-alloc baseline vs all opts on.
const LEVELS = [
    { key: 'no-opt', args: ['--no-opt'] },
    { key: 'opts', args: [] as string[] },
];
const runsArg = process.argv
    .map(a => /^--runs=(\d+)$/.exec(a)?.[1])
    .find(Boolean);
const runs = runsArg ? parseInt(runsArg, 10) : 5;

function timeOnce(filePath: string, args: string[]): { ms: number; output: string } {
    const start = process.hrtime.bigint();
    const result = spawnSync(petal, [filePath, ...args], {
        encoding: 'utf-8',
    });
    const ms = Number(process.hrtime.bigint() - start) / 1e6;
    if (result.status !== 0) {
        console.error(`FAILED: ${filePath} (${args.join(' ') || 'opts'}):\n${result.stdout}${result.stderr}`);
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
console.log(`\n${runs} runs per level, median reported (includes ~process startup)\n`);
console.log('benchmark        no-opt (ms)   opts (ms)   speedup');
console.log('---------        -----------   ---------   -------');

let diverged = false;
for (const name of files) {
    const filePath = join(benchDir, name);
    const medians: Record<string, number> = {};
    const outputs: Record<string, string> = {};
    for (const level of LEVELS) {
        const times: number[] = [];
        for (let i = 0; i < runs; i++) {
            const { ms, output } = timeOnce(filePath, level.args);
            times.push(ms);
            outputs[level.key] = output;
        }
        medians[level.key] = median(times);
    }
    const ratio = medians['no-opt'] / medians['opts'];
    console.log(
        `${name.padEnd(18)}${medians['no-opt'].toFixed(1).padStart(9)}${medians['opts']
            .toFixed(1)
            .padStart(12)}${(ratio.toFixed(2) + 'x').padStart(10)}`,
    );
    if (outputs['no-opt'] !== outputs['opts']) {
        console.log(`  OPT-LEVEL DIVERGENCE in ${name}!`);
        diverged = true;
    }
}
process.exit(diverged ? 1 : 0);

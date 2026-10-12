#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// The garbage-collection pause a big rooted table adds to every collection,
// with the table frozen (`freeze`) and not: runs
// test/benchmarks/micro/gc_rooted_table.ptl on the release build for each
// table size, reads `petal run --profile`'s `collections N (T ms)` line, and
// reports the mean pause of the churn phase alone, i.e. with a CHURN = 0 run
// of the same configuration subtracted, so the collections the build itself
// triggers are not counted.
// Usage:
//   ./tools/bench-gc-pause.ts                 # N = 10,000 and 100,000, 5 runs each
//   ./tools/bench-gc-pause.ts --runs=9 --n=10000,100000,400000
//   ./tools/bench-gc-pause.ts --petal=/path/to/another/petal   # e.g. an older build
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const script = join(repoRoot, 'test', 'benchmarks', 'micro', 'gc_rooted_table.ptl');

function flag(name: string): string | undefined {
    const prefix = `--${name}=`;
    return process.argv.find(a => a.startsWith(prefix))?.slice(prefix.length);
}
const runs = parseInt(flag('runs') ?? '5', 10);
const sizes = (flag('n') ?? '10000,100000').split(',').map(s => parseInt(s, 10));
const petalFlag = flag('petal');
const petal = petalFlag ?? join(repoRoot, 'core', 'target', 'release', 'petal');

if (!petalFlag) {
    const build = spawnSync(
        'cargo',
        ['build', '--release', '--quiet', '--manifest-path', join(repoRoot, 'core', 'Cargo.toml')],
        { stdio: 'inherit' },
    );
    if (build.status !== 0) process.exit(build.status ?? 1);
}

const source = readFileSync(script, 'utf-8');

function configure(n: number, freeze: boolean, churn: number): string {
    const set = (src: string, name: string, value: string) => {
        const re = new RegExp(`^let ${name} = .*$`, 'm');
        if (!re.test(src)) throw new Error(`no 'let ${name} = ...' line in ${script}`);
        return src.replace(re, `let ${name} = ${value}`);
    };
    return set(set(set(source, 'N', String(n)), 'FREEZE', String(freeze)), 'CHURN', String(churn));
}

interface Gc { collections: number; ms: number }

function gcOf(code: string): Gc {
    const r = spawnSync(petal, ['run', '--profile', '-e', code], { encoding: 'utf-8' });
    if (r.status !== 0) {
        console.error(`FAILED:\n${r.stdout}${r.stderr}`);
        process.exit(1);
    }
    const m = /collections\s+([\d,]+)\s+\(([\d.]+) ms\)/.exec(r.stderr);
    if (!m) {
        console.error(`no 'collections N (T ms)' line in the profile:\n${r.stderr}`);
        process.exit(1);
    }
    return { collections: parseInt(m[1].replace(/,/g, ''), 10), ms: parseFloat(m[2]) };
}

function median(xs: number[]): number {
    const sorted = [...xs].sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
}

/** Median over `runs` of one configuration; the collection count is deterministic. */
function measure(n: number, freeze: boolean, churn: number): Gc {
    const samples = Array.from({ length: runs }, () => gcOf(configure(n, freeze, churn)));
    return { collections: samples[0].collections, ms: median(samples.map(s => s.ms)) };
}

const CHURN = parseInt(/^let CHURN = (\d+)$/m.exec(source)?.[1] ?? '2000000', 10);

console.log(`${petal}`);
console.log(`churn phase: ${CHURN.toLocaleString('en-US')} short-lived records; median of ${runs} runs\n`);
console.log('records   table     collections   gc total (ms)   pause per collection (ms)');
console.log('-------   -----     -----------   -------------   -------------------------');
for (const n of sizes) {
    for (const freeze of [false, true]) {
        const base = measure(n, freeze, 0);
        const full = measure(n, freeze, CHURN);
        const collections = full.collections - base.collections;
        const ms = full.ms - base.ms;
        const pause = collections > 0 ? (ms / collections).toFixed(3) : 'n/a';
        console.log(
            `${n.toLocaleString('en-US').padStart(7)}   ${(freeze ? 'frozen' : 'rooted').padEnd(6)}    ` +
            `${String(collections).padStart(11)}   ${ms.toFixed(1).padStart(13)}   ${pause.padStart(25)}`,
        );
    }
}

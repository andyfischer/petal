// Shared plumbing for the example-golden corpus scripts (test-examples.ts and
// gen-example-golden.ts): repo paths, the debug-binary build, the example
// listing, and the golden JSON shape — one definition, so the writer and the
// reader of the corpus cannot drift apart.
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const examplesDir = join(repoRoot, 'examples', 'console');
export const goldenDir = join(repoRoot, 'test', 'example-golden');
export const petal = join(repoRoot, 'rust', 'target', 'debug', 'petal');

/** One captured run; also the golden JSON shape (minus the `example` tag). */
export interface RunResult {
    status: number | null;
    stdout: string;
    stderr: string;
}

/** Build the debug `petal` binary, exiting the process on failure. */
export function buildPetal(): void {
    const build = spawnSync(
        'cargo',
        ['build', '--quiet', '--manifest-path', join(repoRoot, 'rust', 'Cargo.toml')],
        { stdio: 'inherit' },
    );
    if (build.status !== 0) process.exit(build.status ?? 1);
}

/** Every examples/console/*.ptl file name, sorted. */
export function listExamples(): string[] {
    return readdirSync(examplesDir).filter(f => f.endsWith('.ptl')).sort();
}

/** Run one example through the built binary. */
export function runExample(filePath: string, extraArgs: string[] = []): RunResult {
    const result = spawnSync(petal, [filePath, ...extraArgs], { encoding: 'utf-8' });
    return { status: result.status, stdout: result.stdout ?? '', stderr: result.stderr ?? '' };
}

/** Where `name`'s golden capture lives (`foo.ptl` → `<goldenDir>/foo.json`). */
export function goldenPath(name: string): string {
    return join(goldenDir, name.replace(/\.ptl$/, '.json'));
}

/** Load a frozen golden capture, or null when none exists yet. */
export function loadGolden(name: string): RunResult | null {
    const path = goldenPath(name);
    if (!existsSync(path)) return null;
    const g = JSON.parse(readFileSync(path, 'utf-8'));
    return { status: g.status, stdout: g.stdout, stderr: g.stderr };
}

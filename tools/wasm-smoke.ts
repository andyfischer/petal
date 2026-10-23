#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// Runs scripts through the built web runtimes, in Node, a few frames each.
//
// A wasm build compiles cleanly and still traps on every script when core
// reaches something wasm32-unknown-unknown lacks (a clock read, say), and
// nothing short of running a script catches that. This instantiates each
// built pkg and runs:
//
//   petal-web-canvas: every examples/*.ptl, plus an inline script with a
//                     `const` table (a static build), 3 frames each
//   petal-web-html:   an inline script with a `const` table, once
//
// A pkg that has not been built is skipped, unless --require names it.
// Exits non-zero if any script throws.
//
// Usage:
//   ./tools/wasm-smoke.ts                       # after ./integrations/*/build-wasm.sh
//   ./tools/wasm-smoke.ts --require canvas,html # CI: a missing pkg fails

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const requireArg = process.argv.find((a) => a.startsWith('--require'));
const required = new Set(
    (requireArg?.includes('=') ? requireArg.split('=')[1] : process.argv[process.argv.indexOf('--require') + 1] ?? '')
        .split(',')
        .filter(Boolean),
);

// A static build on the first frame: the path that trapped in the
// 2026-10-15 report (docs/wasm-use-case-feedback-10-15.md).
const STATIC_SCRIPT = `
const table = for i in range(0, 8) do [i, i * i] end
let total = 0
for row in table do
  total += row[1]
end
print(total)
`;

let failures = 0;

async function loadPkg(dir: string, name: string): Promise<any | null> {
    const js = join(dir, `${name}.js`);
    const wasm = join(dir, `${name}_bg.wasm`);
    if (!existsSync(js) || !existsSync(wasm)) return null;
    const mod = await import(pathToFileURL(js).href);
    await mod.default({ module_or_path: readFileSync(wasm) });
    const info = join(dir, 'BUILD_INFO');
    if (existsSync(info)) console.log(`  ${readFileSync(info, 'utf8').trim().split('\n').join(', ')}`);
    return mod;
}

function check(label: string, run: () => string): void {
    try {
        console.log(`  ok    ${label} ${run()}`);
    } catch (err) {
        // wasm-bindgen throws a plain string for a returned Err, an Error for a trap.
        console.log(`  FAIL  ${label}: ${err instanceof Error ? err.stack : String(err)}`);
        failures++;
    }
}

function missing(key: string, dir: string): void {
    if (required.has(key)) {
        console.log(`  FAIL  no built pkg in ${dir}`);
        failures++;
    } else {
        console.log(`  skip  no built pkg in ${dir}`);
    }
}

// ----- petal-web-canvas -----
{
    const root = join(repoRoot, 'integrations/petal-web-canvas');
    console.log('petal-web-canvas');
    const mod = await loadPkg(join(root, 'pkg'), 'petal_web_canvas');
    if (!mod) {
        missing('canvas', join(root, 'pkg'));
    } else {
        const examples = join(root, 'examples');
        const scripts: [string, string][] = readdirSync(examples)
            .filter((f) => f.endsWith('.ptl'))
            .sort()
            .map((f) => [f, readFileSync(join(examples, f), 'utf8')]);
        scripts.push(['(inline const table)', STATIC_SCRIPT]);
        for (const [label, source] of scripts) {
            // A trap leaves the runtime unusable, so each script gets its own.
            const rt = new mod.PetalRuntime();
            check(label, () => {
                const stack = rt.create_stack(rt.load_program(source));
                let cmds = 0;
                for (let i = 0; i < 3; i++) {
                    rt.set_frame_info(0.016, 1 + i * 0.016, i, 1440, 900);
                    rt.begin_frame();
                    rt.frame_needed(stack);
                    rt.reset_and_run(stack);
                    cmds += JSON.parse(rt.take_draw_commands()).length;
                }
                return `(${cmds} draw commands over 3 frames)`;
            });
        }
    }
}

// ----- petal-web-html (core's wasm feature) -----
{
    const root = join(repoRoot, 'integrations/petal-web-html');
    console.log('petal-web-html');
    const mod = await loadPkg(join(root, 'pkg'), 'petal');
    if (!mod) {
        missing('html', join(root, 'pkg'));
    } else {
        const rt = new mod.PetalRuntime();
        check('(inline const table)', () => {
            const stack = rt.create_stack(rt.load_program(STATIC_SCRIPT));
            rt.reset_and_run(stack);
            return `(output: ${JSON.stringify(rt.take_output().trim())})`;
        });
    }
}

if (failures > 0) {
    console.log(`\n${failures} failure(s)`);
    process.exit(1);
}

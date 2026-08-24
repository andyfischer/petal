#!/usr/bin/env -S node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON
// (Re)baseline the golden corpus (test/example-golden/<name>.json) that
// test-examples.ts checks every examples/*.ptl against. The corpus was
// originally frozen from the graph engine (the correctness reference during the
// bytecode VM's bring-up); with the VM now the only engine, this captures its
// current output.
//
// Regenerate ONLY deliberately — a golden update is a claim that the intended
// behavior changed. Never run it to "make the sweep pass" after an unexpected
// bytecode diff; investigate the diff first.
//
// Usage:  ./ts/bin/gen-example-golden.ts
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import {
    buildPetal, examplesDir, goldenDir, goldenPath, listExamples, runExample,
} from './example-corpus.ts';

buildPetal();

mkdirSync(goldenDir, { recursive: true });

const files = listExamples();
let count = 0;
for (const file of files) {
    const result = runExample(join(examplesDir, file));
    if (result.status === null) {
        // Killed by a signal (e.g. a timeout): freezing `null` would make every
        // later signal death "match". Fail the regen instead.
        console.error(`petal died on a signal for ${file}; refusing to freeze a null status`);
        process.exit(1);
    }
    const golden = {
        example: file,
        status: result.status,
        stdout: result.stdout,
        stderr: result.stderr,
    };
    writeFileSync(goldenPath(file), JSON.stringify(golden, null, 2) + '\n');
    count++;
}
console.log(`Wrote ${count} golden captures to ${goldenDir}`);

import { afterEach } from "vitest";

// Give the worker's event loop a full turn after every test.
//
// Most tests here call the CLI with `spawnSync`, which blocks the worker. With
// nothing but microtasks between tests, a file whose synchronous tests add up
// to more than 60 seconds never reads the main process's reply to its
// `onTaskUpdate` RPC before that call's 60 second timer fires, and vitest
// reports "Timeout calling onTaskUpdate" as an unhandled error (exit 1) though
// every test passed. `petal check` compiles and lowers the `ui` prelude, close
// to a second per call in a debug build, so a check-heavy file gets there
// easily when the machine is busy. `setImmediate` runs after the poll phase,
// so the reply is read between tests and only a single test that blocks for
// 60 seconds on its own could still starve the worker.
afterEach(() => new Promise<void>((resolve) => setImmediate(resolve)));

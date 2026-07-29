// The `phase` field of `petal check --json` says which stage of the front end
// rejected the program. It is a typed channel, not a guess made by sniffing the
// message text — so a module-resolution failure reports "module" and a Compiler
// diagnostic reports "compile", however their messages happen to be worded.
//
// Each case also pins the `message`, so a refactor of the phase channel is
// visible here if it disturbs the wording users actually read.

import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, checkJsonAllowFail } from "./helpers";

beforeAll(() => ensureBuild());

/** Assert the phase and message of a program that must fail `check`. */
const expectPhase = (code: string, phase: string, message: string) => {
  const out = checkJsonAllowFail(code);
  expect(out.error).toBe(true);
  expect(out.message).toContain(message);
  expect(out.phase).toBe(phase);
};

describe("check --json reports the phase that rejected the program", () => {
  it('reports "lex" for an unterminated string', () => {
    expectPhase(`let s = "abc\nprint(1)`, "lex", "Unterminated string");
  });

  it('reports "lex" for an unexpected character', () => {
    expectPhase(`let x = 1 § 2`, "lex", "Unexpected character '§'");
  });

  it('reports "parse" for a `let` with no initializer', () => {
    expectPhase(`let x\nprint(1)`, "parse", "Expected Assign");
  });

  it('reports "parse" for a malformed `fn` header', () => {
    expectPhase(`fn (`, "parse", "Expected identifier");
  });

  it('reports "module" when an import cannot be resolved', () => {
    // Module resolution runs after a clean lex+parse, so its failures are their
    // own phase. (An import cycle is the other "module" failure; it needs
    // fixture files on disk rather than `-e`, so it lives in modules.test.ts.)
    expectPhase(`import nope\nprint(1)`, "module", "cannot find module 'nope'");
  });

  it('reports "compile" for a `var` written with `=`', () => {
    expectPhase(
      `var x = 1\nx = 2`,
      "compile",
      "`x` is a `var`; use `set x = ...` to write it"
    );
  });

  it('reports "compile" for a cross-function assignment', () => {
    expectPhase(
      `let i = 10\nfn f()\n  i = i + 1\n  i\nend\nprint(f())`,
      "compile",
      "`i` is bound outside this function"
    );
  });

  // No "lower" case yet: bytecode lowering is not reachable from `check`, so
  // there is no program here that can fail in that phase. A later chunk adds
  // `check --ir`, which is the only way to reach it.
});

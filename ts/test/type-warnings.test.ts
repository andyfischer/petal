import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  checkJson,
  checkText,
  checkStrict,
  runWithStderr,
} from "./helpers";

beforeAll(() => ensureBuild());

// Chunk E: type-checker warnings surfaced by `petal check` and `petal run`.
// Warnings are non-fatal: `check` still exits 0 and `run` still executes the
// program (annotations are runtime-inert). `--json` check emits a `warnings`
// array; text mode prints `warning:` lines to stderr.

describe("type-checker warnings via `petal check --json`", () => {
  it("reports a let type mismatch as a single warning, ok stays true", () => {
    const out = checkJson('let x: int = "hi"');
    expect(out.ok).toBe(true);
    expect(Array.isArray(out.warnings)).toBe(true);
    expect(out.warnings).toHaveLength(1);
    const w = out.warnings[0];
    expect(w.message).toMatch(/mismatch/i);
    expect(typeof w.line).toBe("number");
    expect(typeof w.column).toBe("number");
    expect(w.line).toBeGreaterThan(0);
    expect(w.column).toBeGreaterThan(0);
  });

  it("emits an empty warnings array for a clean program", () => {
    const out = checkJson("let x: int = 5");
    expect(out.ok).toBe(true);
    expect(out.warnings).toEqual([]);
  });

  it("reports a call-argument mismatch end-to-end", () => {
    const out = checkJson('fn area(r: float) -> float\n  r\nend\nprint(area("x"))');
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/argument 1/);
  });
});

describe("type-checker warnings via `petal check` (text)", () => {
  it("prints a warning to stderr, empty stdout, exit 0", () => {
    const { stdout, stderr, code } = checkText('let x: int = "hi"');
    expect(code).toBe(0);
    expect(stdout).toBe("");
    expect(stderr).toContain("warning:");
    expect(stderr).toMatch(/mismatch/i);
  });
});

describe("`petal check --strict`", () => {
  it("exits non-zero when warnings exist", () => {
    const { code, stderr } = checkStrict('let x: int = "hi"');
    expect(code).toBe(1);
    expect(stderr).toContain("warning:");
  });

  it("exits 0 for a clean program", () => {
    const { code } = checkStrict("let x: int = 5");
    expect(code).toBe(0);
  });
});

describe("type-checker warnings via `petal run`", () => {
  it("still runs the program (runtime-inert) and warns on stderr", () => {
    const { stdout, stderr } = runWithStderr('let x: int = "hi"\nprint(x)');
    expect(stdout.trim()).toBe("hi");
    expect(stderr).toContain("warning:");
  });
});

// Chunk F: discarded-result lint. A side-effect-free builtin call whose value
// is thrown away does nothing — the value-semantics migration footgun where
// statement-form `push(xs, x)` / `append(xs, x)` silently accumulate nothing.
describe("discarded pure-builtin result lint", () => {
  it("warns on statement-form push() with a capture hint", () => {
    const out = checkJson("state xs = []\nfor i in range(0, 3) do\n  push(xs, i)\nend");
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`push`.*discarded/);
    expect(out.warnings[0].message).toMatch(/xs = push/);
  });

  it("warns on statement-form append()", () => {
    const out = checkJson("let a = [1]\nappend(a, 2)\nprint(len(a))");
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`append`.*discarded/);
  });

  it("stays silent when the result is captured", () => {
    const out = checkJson("let a = [1]\na = append(a, 2)\nprint(len(a))");
    expect(out.warnings).toEqual([]);
  });

  it("does not warn on effectful calls (print, random)", () => {
    const out = checkJson('print("hi")\nlet r = random(0.0, 1.0)\nr');
    expect(out.warnings).toEqual([]);
  });

  it("does not warn when a user fn shadows a builtin name", () => {
    const out = checkJson('fn push(a, b)\n  print("fx")\n  a\nend\npush([1], 2)');
    expect(out.warnings).toEqual([]);
  });

  it("does not warn on a pure builtin as the program's final value", () => {
    const out = checkJson("let a = [1]\nappend(a, 3)");
    expect(out.warnings).toEqual([]);
  });

  it("does not warn inside a value-position for-loop that collects results", () => {
    const out = checkJson("let ys = for i in range(0, 3) do\n  append([], i)\nend\nprint(len(ys))");
    expect(out.warnings).toEqual([]);
  });
});

// (docs/dev/var-next-steps.md, Cells): a `var` is a cell, so its
// *writes* must stay assignable to its declared type — and its *reads* must not
// be typed from the initializer, because a `set` can retype the cell from inside
// any function or closure that captured it.
describe("`var` cells and the type checker", () => {
  it("warns on a `set` that conflicts with the var's declared type", () => {
    const out = checkJson('var n: int = 0\nset n = "hello"\nprint(n)');
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    // The same diagnostic shape a conflicting `=` reassignment produces.
    expect(out.warnings[0].message).toBe("type mismatch: `n` declared `int` but assigned `string`");
    expect(out.warnings[0].line).toBe(2);
  });

  it("stays silent when the `set` value matches (int still promotes to float)", () => {
    expect(checkJson("var n: int = 0\nset n = 5\nprint(n)").warnings).toEqual([]);
    expect(checkJson("var n: float = 0.0\nset n = 5\nprint(n)").warnings).toEqual([]);
  });

  it("checks a `set` written inside a closure, under control flow", () => {
    // The point of a cell: the write is nowhere near the declaration, and is
    // somewhere plain `=` could never have reached.
    const body = 'let g = fn(b)\n  if b then set n = "s" end\nend\ng(true)\nprint(n)';
    const out = checkJson(`var n: int = 0\n${body}`);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`n` declared `int`/);
    expect(out.warnings[0].line).toBe(3);
  });

  it("does not type an un-annotated var's reads from its initializer", () => {
    // All three are correct programs — the cell really does hold a string by
    // the time it is read. Trusting `var n = 0` would warn on every one.
    const src = 'var n = 0\nset n = "hi"\n';
    expect(checkJson(`${src}let s: string = n\nprint(s)`).warnings).toEqual([]);
    expect(checkJson(`fn g(s: string)\n  s\nend\n${src}print(g(n))`).warnings).toEqual([]);
    expect(checkJson(`${src}fn f() -> string\n  n\nend\nprint(f())`).warnings).toEqual([]);
  });

  it("does type an annotated var's reads from its annotation", () => {
    const out = checkJson("var n: int = 0\nlet s: string = n\nprint(s)");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`s` declared `string` but assigned `int`/);
  });

  it("leaves an un-annotated `state var` unconstrained in both directions", () => {
    expect(checkJson('state var n = 0\nset n = "hi"\nprint(n)').warnings).toEqual([]);
    const read = 'state var n = 0\nset n = "hi"\nlet s: string = n\nprint(s)';
    expect(checkJson(read).warnings).toEqual([]);
  });

  it("does not check the value of a field or index `set`, but walks its parts", () => {
    // `record`/`list` are opaque, so there is no field or element type for the
    // written value to conflict with; nested expressions are still checked.
    expect(checkJson('var r: record = {a: 1}\nset r.a = "s"\nprint(r)').warnings).toEqual([]);
    expect(checkJson('var xs: list = [1]\nset xs[0] = "s"\nprint(xs)').warnings).toEqual([]);
    const out = checkJson("fn g(s: string)\n  s\nend\nvar r: record = {a: 1}\nset r.a = g(1)\nr");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/argument 1 to `g`/);
  });

  it("is warning-only: the program still compiles and runs", () => {
    const { stdout, stderr } = runWithStderr('var n: int = 0\nset n = "hello"\nprint(n)');
    expect(stdout.trim()).toBe("hello");
    expect(stderr).toContain("warning:");
    expect(stderr).toMatch(/declared `int`/);
  });
});

// `state` annotations. A reactive binding has no useful inferred type — a
// re-render or a `set` from anywhere can replace it — so the *annotation* is the
// only thing that lets the checker say anything at all about a state name.
describe("`state` annotations and the type checker", () => {
  it("warns when the initializer conflicts with the declared type", () => {
    const out = checkJson('state n: int = "hi"\nprint(n)');
    expect(out.ok).toBe(true);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("type mismatch: `n` declared `int` but assigned `string`");
    expect(out.warnings[0].line).toBe(1);
  });

  it("stays silent when the initializer matches (int promotes to float)", () => {
    expect(checkJson("state n: int = 0\nprint(n)").warnings).toEqual([]);
    expect(checkJson("state n: float = 0\nprint(n)").warnings).toEqual([]);
    expect(checkJson('state var s: string = "a"\nprint(s)').warnings).toEqual([]);
  });

  it("warns on an unknown type name in a state annotation", () => {
    const out = checkJson("state n: banana = 0\nprint(n)");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("unknown type name `banana`");
  });

  it("checks a `set` against an annotated `state var`, wherever it is written", () => {
    const out = checkJson('state var n: int = 0\nset n = "hi"\nprint(n)');
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`n` declared `int`/);
    expect(out.warnings[0].line).toBe(2);
    const closure = 'state var n: int = 0\nlet g = fn(b)\n  if b then set n = "s" end\nend\ng(true)';
    expect(checkJson(closure).warnings).toHaveLength(1);
  });

  it("types an annotated state's reads", () => {
    const out = checkJson("state n: int = 0\nlet s: string = n\nprint(s)");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`s` declared `string` but assigned `int`/);
  });

  it("checks a keyed state and still walks the key expression", () => {
    expect(checkJson("state(1) n: int = 0\nprint(n)").warnings).toEqual([]);
    const out = checkJson("fn g(s: string)\n  s\nend\nstate(g(1)) n: int = 0\nprint(n)");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/argument 1 to `g`/);
  });

  it("is warning-only: an annotated state still runs", () => {
    const { stdout, stderr } = runWithStderr('state n: int = 0\nn = "hello"\nprint(n)');
    expect(stdout.trim()).toBe("hello");
    expect(stderr).toMatch(/declared `int`/);
  });
});

import { describe, it, expect } from "vitest";
import {
  checkJson,
  checkJsonAllowFail,
  checkText,
  checkStrict,
  runWithStderr,
} from "./helpers";


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

// (docs/var.md, Cells): a `var` is a cell, so its
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
    expect(checkJson(`${src}fn f() -> string\n  get n\nend\nprint(f())`).warnings).toEqual([]);
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

// A `function` type carries no arrow, so a call through a binding is only
// checkable if the binding remembers the signature it was given. These pin
// that a lambda's parameter annotations, and a named fn's, survive the
// binding — the checker treated only direct calls to named fns before.
describe("calls through a function-valued binding", () => {
  it("checks a lambda's parameter annotations at the call site", () => {
    const out = checkJson('let f = fn(n: int) -> n\nprint(f("hi"))');
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("argument 1 to `f`: expected `int`, found `string`");
    expect(checkJson("let f = fn(n: int) -> n\nprint(f(5))").warnings).toEqual([]);
  });

  it("checks a named function called through an alias", () => {
    const out = checkJson("fn g(s: string)\n  s\nend\nlet h = g\nprint(h(1))");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/argument 1 to `h`/);
  });

  it("carries the aliased function's return type", () => {
    const out = checkJson(
      "fn g(n: int) -> string\n  str(n)\nend\nlet h = g\nlet x: int = h(1)\nprint(x)"
    );
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`x` declared `int` but assigned `string`/);
  });

  it("forgets the signature once the binding is re-assigned", () => {
    expect(
      checkJson('let f = fn(n: int) -> n\nf = fn(s) -> s\nprint(f("hi"))').warnings
    ).toEqual([]);
  });
});

// `check` is "lex+parse+compile+lower without executing", so a call that can
// never resolve should not have to wait for the runtime to say so. Petal
// overloads by arity (docs/function-overloading.md): the call is wrong only
// when *no* declared arity matches.
describe("statically-known arity errors", () => {
  it("errors on a call no overload can take, which `run` rejects outright", () => {
    const out = checkJsonAllowFail("fn f(a, b)\n  a\nend\nprint(f(1))");
    // An error, not a warning: the call fails whenever it runs, so `check`
    // fails without `--strict`.
    expect(out.ok).toBe(false);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].severity).toBe("error");
    expect(out.warnings[0].message).toBe("`f` expects 2 arguments, got 1");
    const { stderr } = runWithStderr("fn f(a, b)\n  a\nend\nprint(f(1))");
    expect(stderr).toMatch(/f\(\) expects 2 arguments, got 1/);
  });

  it("accepts any declared arity and names them all when none matches", () => {
    const overloads = "fn f(a)\n  a\nend\nfn f(a, b)\n  b\nend\n";
    expect(checkJson(`${overloads}print(f(1))\nprint(f(1, 2))`).warnings).toEqual([]);
    const out = checkJsonAllowFail(`${overloads}print(f(1, 2, 3))`);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("`f` expects 1 or 2 arguments, got 3");
  });

  it("counts a lambda binding's parameters too", () => {
    const out = checkJsonAllowFail("let f = fn(a) -> a\nprint(f(1, 2))");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toBe("`f` expects 1 argument, got 2");
  });

  it("says nothing about builtins or undeclared names", () => {
    expect(checkJson("print(1, 2, 3)").warnings).toEqual([]);
    expect(checkJson("print(len([1, 2]))").warnings).toEqual([]);
  });

  it("`check --strict` now fails on one", () => {
    const { code, stderr } = checkStrict("fn f(a, b)\n  a\nend\nprint(f(1))");
    expect(code).toBe(1);
    expect(stderr).toMatch(/expects 2 arguments/);
  });
});

// Programs that compile and run but compute the wrong thing: each must fail
// `check --strict` with a warning that names the mistake.
describe("silently wrong programs warn", () => {
  it("a leading `-` under a line it was meant to continue", () => {
    const sum = "fn score(a, b, c)\n  a * 2\n    + b * 3\n    - c * 4\nend\nprint(score(1, 2, 3))";
    const out = checkJson(sum);
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/starts with `-` is a new statement/);
    expect(out.warnings[0].line).toBe(4);
    expect(checkStrict(sum).code).toBe(1);
    // The fix the guide gives: end the line with the operator.
    const fixed = "fn score(a, b, c)\n  a * 2 +\n    b * 3 -\n    c * 4\nend\nprint(score(1, 2, 3))";
    expect(checkJson(fixed).warnings).toEqual([]);
    // A negated tail after a `let` is an ordinary function body.
    expect(checkJson("fn f(a, b)\n  let d = a - b\n  -d\nend\nprint(f(1, 2))").warnings).toEqual([]);
  });

  it("a bare computation whose value is discarded", () => {
    const out = checkJson("let n = 1\nn + 1\nprint(n)");
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/value of this expression is discarded/);
  });

  it("a variant two enums declare, and a variant named like a `let`", () => {
    const two = checkJson("enum A\n  None,\nend\nenum B\n  None,\nend\nprint(None)");
    expect(two.warnings).toHaveLength(1);
    expect(two.warnings[0].message).toMatch(/declared by both `enum A` and `enum B`/);
    const over = checkJson("let Red = 5\nenum Color\n  Red,\nend\nprint(Red)");
    expect(over.warnings).toHaveLength(1);
    expect(over.warnings[0].message).toMatch(/same name as the `let Red` on line 1/);
  });

  it("a `state` that shadows a builtin called in the same file", () => {
    const out = checkJson('state split = 396\nfn parts(text)\n  split(text, ",")\nend\nprint(split)');
    expect(out.warnings).toHaveLength(1);
    expect(out.warnings[0].message).toMatch(/`state split` declared on line 1, which shadows the builtin `split`/);
    expect(out.warnings[0].line).toBe(3);
    // Not called, or declared below the caller: the builtin is still reached.
    expect(checkJson("let len = 3\nprint(len)").warnings).toEqual([]);
    expect(checkJson('fn parts(t)\n  split(t, ",")\nend\nlet split = 1\nprint(parts("a,b"), split)').warnings).toEqual([]);
  });

  it("an enum name is a type name", () => {
    const src = "enum Shape\n  Circle(r),\n  Dot,\nend\nfn f(s: Shape) -> Shape\n  s\nend\nprint(f(Dot))";
    expect(checkStrict(src).code).toBe(0);
    expect(checkJson("fn f() -> Shapee\n  1\nend").warnings[0].message).toMatch(/unknown type name `Shapee`/);
  });
});

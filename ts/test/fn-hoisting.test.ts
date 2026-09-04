// Top-level `fn` declarations are hoisted, so a function can call any other
// function declared in the same file regardless of order.
//
// Before this, a `fn` was bound only when its declaration statement ran, so a
// call to a name declared further down reached `nil` and died at run time with
// `Cannot call nil` — pointing at the call site with no mention of declaration
// order. That made mutual recursion impossible, which is table stakes for
// parsers, tree-walkers and state machines: a recursive-descent parser is
// mutually recursive by construction. Two apps in examples/ (the panel apps) worked
// around it (a restructured shunting-yard expression parser, and a
// `var _expr = nil … set _expr = parse_cmp` trampoline).
//
// The hoist is conservative on purpose. A `fn` whose body mentions something
// the file *computes* at run time (a top-level `let`/`var`/`state`, an enum
// variant) stays exactly where it was written, because moving its closure
// would change what the closure captured. For that residue — a *call* from a
// top-level statement to a not-hoistable declaration below it — the compiler
// reports a diagnostic naming declaration order instead of leaving a runtime
// `Cannot call nil`.

import { describe, it, expect } from "vitest";
import { resolve } from "path";
import { runPetal, runPetalFile, checkText, checkJsonAllowFail } from "./helpers";

const FIXTURES = resolve(__dirname, "fixtures/hoisting");

function runFile(name: string): string {
  return runPetalFile(resolve(FIXTURES, name));
}

describe("forward references between top-level functions", () => {
  it("mutual recursion between two functions", () => {
    // The report's exact reproduction.
    expect(
      runPetal(`
fn a(n) if n <= 0 then 0 else b(n-1) end end
fn b(n) if n <= 0 then 1 else a(n-1)+1 end end
print(a(5))
`)
    ).toBe("3");
  });

  it("a cycle of three functions", () => {
    expect(
      runPetal(`
fn even(n) if n == 0 then true else odd(n - 1) end end
fn odd(n) if n == 0 then false else even(n - 1) end end
print([even(10), odd(10), even(7)])
`)
    ).toBe("[true, false, false]");
  });

  it("a mutually recursive recursive-descent parser", () => {
    // The shape the workarounds existed for: expr -> term -> expr.
    expect(
      runPetal(`
fn parse_expr(toks, i)
  let l = parse_term(toks, i)
  if l.i < len(toks) && toks[l.i] == "+" then
    let r = parse_expr(toks, l.i + 1)
    {v: l.v + r.v, i: r.i}
  else
    l
  end
end

fn parse_term(toks, i)
  if toks[i] == "(" then
    let inner = parse_expr(toks, i + 1)
    {v: inner.v, i: inner.i + 1}
  else
    {v: parse_int(toks[i]), i: i + 1}
  end
end

print(parse_expr(["(", "1", "+", "2", ")", "+", "4"], 0).v)
`)
    ).toBe("7");
  });

  it("a top-level call above the declaration", () => {
    expect(runPetal(`print(f(3))\nfn f(n) n * 2 end`)).toBe("6");
  });

  it("`main()` written above `fn main`", () => {
    expect(runPetal(`main()\nfn main() print("hello") end`)).toBe("hello");
  });

  it("a forward reference in value position, not just call position", () => {
    expect(
      runPetal(`
fn apply_all(xs) map(xs, double) end
fn double(n) n * 2 end
print(apply_all([1, 2, 3]))
`)
    ).toBe("[2, 4, 6]");
  });

  it("a method body calling a plain function declared below it", () => {
    expect(
      runPetal(`
class P
  x
end
fn P.show(self) fmt(self.x) end
fn fmt(v) "<{v}>" end
print(P(7).show())
`)
    ).toBe("<7>");
  });

  it("a function calling a method pinned to a class declared below", () => {
    expect(
      runPetal(`
fn describe(b) "area={b.area()}" end
class Box
  w, h
end
fn Box.area(self) self.w * self.h end
print(describe(Box(2, 5)))
`)
    ).toBe("area=10");
  });

  it("an overloaded name is reachable at every arity from above", () => {
    expect(
      runPetal(`
fn caller() [area(3), area(3, 4)] end
fn area(s) s * s end
fn area(w, h) w * h end
print(caller())
`)
    ).toBe("[9, 12]");
  });

  it("hoisting does not disturb ordinary declare-then-call", () => {
    expect(
      runPetal(`
fn twice(n) n + n end
fn quad(n) twice(twice(n)) end
print(quad(3))
`)
    ).toBe("12");
  });
});

describe("hoisted functions across module boundaries", () => {
  // A hoisted `fn` binds a cell rather than the closure directly, so the
  // export and every import form has to dereference it — otherwise the
  // importer calls the cell id instead of the function.
  it("a mutually recursive pair survives a qualified import", () => {
    expect(runFile("qualified.ptl")).toBe("done");
  });

  it("...and a selective import, which binds the bare name", () => {
    expect(runFile("selective.ptl")).toBe("done");
  });
});

describe("what hoisting deliberately leaves alone", () => {
  it("a fn capturing a top-level `let` still captures it where written", () => {
    expect(
      runPetal(`
let x = 1
fn g() x + 1 end
print(g())
`)
    ).toBe("2");
  });

  it("a fn shadowing an existing name keeps the pre-shadow read", () => {
    // `let old = len` above `fn len` must still see the builtin: hoisting the
    // declaration, or binding the name to a cell, would turn that read to nil.
    expect(
      runPetal(`
let old_max = max
fn max(a, b) "shadowed" end
print([old_max(1, 2), max(1, 2)])
`)
    ).toBe(`[2, "shadowed"]`);
  });

  it("...but only when something actually reads the old meaning", () => {
    // The counterpart to the test above. Nothing here captures the builtin
    // `max`, so the declaration hoists and `widget`, written above it, binds
    // to this file's `max` rather than to the 2-argument builtin. That is the
    // library-module case: a host prelude puts hundreds of names in scope in
    // every file, and a collision alone must not un-hoist a declaration.
    expect(
      runPetal(`
fn widget()
  max({a: 1, b: 2})
end

fn max(r)
  r.a + r.b
end

print(widget())
`)
    ).toBe("3");
  });

  it("a caller of a shadowing overload is held back with it", () => {
    // The prelude shape: a file grabs the native (`let _native = max`),
    // redeclares the name with a different arity, and a widget below calls the
    // record form. The widget mentions no top-level `let`, so it used to be
    // hoisted above the declarations — where `max` still named the 2-argument
    // native, so `widget()` compiled against the wrong overload and died with
    // an arity error. Blocking is transitive now: a `fn` that references a
    // non-hoistable `fn` cannot be hoisted either.
    expect(
      runPetal(`
let _native_max = max

fn max(r)
  _native_max(r.a, r.b)
end

fn widget()
  max({a: 1, b: 2})
end

print(widget())
`)
    ).toBe("2");
  });

  it("blocking is transitive through a chain of callers", () => {
    // `mid` is blocked because it calls the shadowing `max`; `outer` is
    // blocked because it calls `mid`.
    expect(
      runPetal(`
let _native_max = max

fn max(r)
  _native_max(r.a, r.b)
end

fn mid(r) max(r) end
fn outer() mid({a: 3, b: 9}) end

print(outer())
`)
    ).toBe("9");
  });

  it("a call to a not-hoistable declaration below is a compile diagnostic", () => {
    // The residue hoisting cannot fix. Before, this was a bare runtime
    // `Cannot call nil` and `petal check` passed clean.
    const code = `let base = 10\nprint(h(1))\nfn h(n) n + base end`;
    const { stdout, stderr } = checkText(code);
    const out = stdout + stderr;
    expect(out).toContain("call to `h` before its declaration");
    expect(out).toContain("still nil");
  });

  it("that diagnostic reaches `check --json`", () => {
    const code = `let base = 10\nprint(h(1))\nfn h(n) n + base end`;
    const report = checkJsonAllowFail(code);
    expect(JSON.stringify(report)).toContain(
      "call to `h` before its declaration"
    );
  });

  it("a call above a shadowed, not-hoistable declaration is warned about", () => {
    // `max` is a builtin, so this used to be filtered out of the warnings and
    // silently bound to the builtin. The declaration below owns the name now,
    // which makes the early call a plain too-early call — and it says so.
    const code = `let base = 10\nprint(max(1))\nfn max(n) n + base end`;
    const { stdout, stderr } = checkText(code);
    expect(stdout + stderr).toContain("call to `max` before its declaration");
  });

  it("a reference from inside a body is never reported as too early", () => {
    // `k` runs after the whole file has, so reaching a declaration below it is
    // exactly what the hoist is for — no diagnostic, even un-hoistable.
    const code = `let base = 10\nfn k() h(1) end\nfn h(n) n + base end\nprint(k())`;
    const { stdout, stderr } = checkText(code);
    expect(stdout + stderr).not.toContain("before its declaration");
    expect(runPetal(code)).toBe("11");
  });
});

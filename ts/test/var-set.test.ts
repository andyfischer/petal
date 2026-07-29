// `var x = …` declares a mutable cell and `set x = …` writes it. The two write
// keywords are disjoint in both directions: `=` rejects a var, `set` rejects
// everything else. See docs/dev/var-next-steps.md (Two write keywords).
//
// A `var` binds a *cell*: a one-value mutable box on the heap. Reads
// dereference it (`CellRead`), `set` writes through it (`CellWrite`), and a
// closure that captures the name captures the box — so mutation crosses
// function and control-flow boundaries, which plain `=` never could. See
// sections 6c and 6d for cells and the containment invariant.

import { describe, it, expect, beforeAll } from "vitest";
import {
  checkJson,
  ensureBuild,
  runPetal,
  runPetalError,
  showAstJson,
  showIrJson,
  showTokensJson,
  termsByOp,
  explainJson,
  showProvenanceJson,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("var / set syntax", () => {
  it("declares and writes a cell", () => {
    expect(
      runPetal(`var x = 0
set x = x + 1
set x = x + 1
print(x)`),
    ).toBe("2");
  });

  it("accepts compound writes", () => {
    expect(
      runPetal(`var n = 1
set n += 4
print(n)`),
    ).toBe("5");
  });

  it("accepts field and index targets", () => {
    expect(
      runPetal(`var r = { a: 1 }
var xs = [1, 2]
set r.a = 9
set xs[0] = 7
print(r, xs)`),
    ).toBe("{ a: 9 } [7, 2]");
  });

  it("carries through control flow like any other binding", () => {
    expect(
      runPetal(`var total = 0
for i in [1, 2, 3] do
  set total = total + i
end
print(total)`),
    ).toBe("6");
  });

  it("accepts a type annotation, `export`, and the `state` forms", () => {
    expect(
      runPetal(`export var counter: int = 0
state var hits = 0
state(1) var keyed = 0
set counter += 2
if true then set hits = hits + 1 end
print(counter, hits, keyed)`),
    ).toBe("2 1 0");
  });

  it("lexes `var` and `set` as keywords", () => {
    const kinds = showTokensJson(`var x = 0
set x = 1`).map((t: any) => t.kind ?? t.type ?? t);
    expect(JSON.stringify(kinds)).toContain("Var");
    expect(JSON.stringify(kinds)).toContain("Set");
  });

  it("records is_var on the declaration, not on `let`", () => {
    const ast = showAstJson(`var a = 1
let b = 2
state var c = 3`);
    const flags = [...JSON.stringify(ast).matchAll(/"is_var":\s*(true|false)/g)].map(
      (m) => m[1],
    );
    expect(flags).toEqual(["true", "false", "true"]);
  });
});

describe("var / set disjointness", () => {
  it("rejects `=` on a var", () => {
    const err = runPetalError(`var x = 0
x = 1`);
    expect(err).toContain("`x` is a `var`; use `set x = ...`");
    expect(err).toContain("line 2");
  });

  it("rejects `set` on a let", () => {
    const err = runPetalError(`let x = 0
set x = 1`);
    expect(err).toContain("`x` is not a `var`");
  });

  it("rejects the `@` rebind sugar on a var", () => {
    // `f(@x)` desugars to `x = f(x)`, which is an `=` form.
    const err = runPetalError(`fn f(n) n + 1 end
var x = 0
f(@x)`);
    expect(err).toContain("`x` is a `var`; use `set x = ...`");
  });

  it("does not let `set` declare a binding", () => {
    expect(runPetalError(`set nope = 1`)).toContain("`nope` is not defined");
  });

  it("tracks the binding kind through shadowing, both ways", () => {
    // An inner `let` shadowing an outer `var` takes `=`...
    expect(
      runPetal(`var x = 1
fn f()
  let x = 5
  x = 6
  x
end
print(f())`),
    ).toBe("6");
    // ...and an inner `var` shadowing an outer `let` takes `set`.
    expect(
      runPetal(`let x = 1
fn f()
  var x = 5
  set x = 6
  x
end
print(f())`),
    ).toBe("6");
  });

  it("keeps the kind across repeated writes and through control flow", () => {
    // Every rebind mints a fresh term (a Copy, a phi, a loop-entry seed); the
    // kind has to ride along or the *second* write would be rejected.
    expect(
      runPetal(`var x = 0
set x = 1
set x = 2
if true then
  set x = 3
  set x = 4
end
for i in [1] do
  set x = x + 1
end
print(x)`),
    ).toBe("5");
  });
});

describe("cells", () => {
  it("mutates a binding from inside a function", () => {
    // The §1a shape: with `let` this silently shadows and prints 11, 11, 10.
    expect(
      runPetal(`var i = 10
fn f()
  set i = i + 1
  i
end
print(f())
print(f())
print(i)`),
    ).toBe("11\n12\n12");
  });

  it("writes from inside a conditional inside a lambda", () => {
    // The §1b shape — three SDL examples die at lowering on this with `=`,
    // because the phi's init names a term in another function. A `var` needs
    // no phi at all.
    expect(
      runPetal(`var score = 0
var hit = false
let doubled = map([1, 2, 3], fn(a)
  if a > 1 then
    set hit = true
    set score += 10
  end
  a * 2
end)
print(doubled, score, hit)`),
    ).toBe("[2, 4, 6] 20 true");
  });

  it("shares one cell between two closures", () => {
    expect(
      runPetal(`var n = 0
let inc = fn() set n = n + 1 end
let dec = fn() set n = n - 1 end
inc()
inc()
inc()
dec()
print(n)`),
    ).toBe("2");
  });

  it("gives each call of a factory its own cell", () => {
    // The declaration allocates; two calls of `counter` are two boxes.
    expect(
      runPetal(`fn counter()
  var c = 0
  let bump = fn()
    set c = c + 1
    c
  end
  bump
end
let a = counter()
let b = counter()
print(a(), a(), a(), b())`),
    ).toBe("1 2 3 1");
  });

  it("accumulates a list from inside a callback", () => {
    // `invaders.ptl`'s shape, reduced: the reason the escape hatch exists.
    expect(
      runPetal(`var acc = []
let _ = map([1, 2, 3], fn(x)
  set acc = append(acc, x * x)
  x
end)
print(acc)`),
    ).toBe("[1, 4, 9]");
  });

  it("writes a field or index of an outer var from inside a function", () => {
    expect(
      runPetal(`var st = { items: [], n: 0 }
fn add(v)
  set st.items = append(st.items, v)
  set st.n = st.n + 1
end
add(1)
add(2)
print(st)`),
    ).toBe("{ items: [1, 2], n: 2 }");
  });

  it("does not reject writing a var bound outside the function", () => {
    // The cross-function error is about `=`'s silent shadow. A `set` really
    // does modify the outer binding — that is the whole point of the escape
    // hatch — so it must pass cleanly, not merely avoid the message.
    const out = checkJson(`var i = 0
fn f() set i = 1 end`);
    expect(out.ok).toBe(true);
    expect(JSON.stringify(out)).not.toContain("bound outside");
  });
});

describe("cell containment", () => {
  // §6d: no expression evaluates to a cell. Reads dereference, so storing or
  // passing a `var` moves its *contents* — the box is never aliased except by
  // closure capture.
  it("stores contents, not the cell, into a record or list", () => {
    expect(
      runPetal(`var x = 1
let r = { a: x }
let xs = [x, x]
set x = 2
print(r, xs, x)`),
    ).toBe("{ a: 1 } [1, 1] 2");
  });

  it("passes contents to a function", () => {
    expect(
      runPetal(`fn id(v) v end
var x = 1
let before = id(x)
set x = 2
print(before, id(x), type(x))`),
    ).toBe("1 2 int");
  });

  it("keeps value semantics for a collection that a var also holds", () => {
    // `box` takes the list's id at the time of the write; the later `xs[2] = 3`
    // rebuilds rather than mutating, so `box` is unaffected. This is also the
    // regression test for the in-place rewrite: a container stored into a cell
    // is retained, so route A must decline to mutate it.
    expect(
      runPetal(`var box = []
fn build()
  let xs = [0, 0, 0]
  xs[0] = 1
  xs[1] = 2
  set box = xs
  xs[2] = 3
  xs
end
print(build(), box)`),
    ).toBe("[1, 2, 3] [1, 2, 0]");
  });
});

describe("cell IR", () => {
  it("compiles a var to CellNew / CellRead / CellWrite", () => {
    const ir = showIrJson(`var x = 0
set x = x + 1
print(x)`);
    expect(termsByOp(ir, "CellNew")).toHaveLength(1);
    expect(termsByOp(ir, "CellWrite")).toHaveLength(1);
    // One read for the `x + 1` operand, one for `print(x)`.
    expect(termsByOp(ir, "CellRead")).toHaveLength(2);
  });

  it("emits no phi for a var written inside a conditional", () => {
    // A cell's identity never changes, so there is nothing for a join to
    // reconcile — which is exactly why a `set` works across a function
    // boundary where an `=` cannot.
    const ir = showIrJson(`var x = 0
if true then set x = 1 end
print(x)`);
    expect(termsByOp(ir, "Phi")).toHaveLength(0);
  });
});

// The cost of the escape hatch, and the rule that keeps it honest: a backward
// walk stops at every cell read AND SAYS SO (§6e). The full frontier contract
// is exercised in provenance.test.ts and slicing.test.ts; these two pin that
// `var`'s own suite fails if the tooling ever goes quiet about it.
describe("var / set and provenance", () => {
  it("a var read is a dead end for provenance unless it is announced", () => {
    const prov = showProvenanceJson("var x = 0\nset x = x + 1\nlet y = x * 2\n", "y");
    expect(prov.complete).toBe(false);
    expect(prov.frontier[0].var).toBe("x");
  });

  it("the trace turns the dead end back into a chain", () => {
    const out = explainJson("var x = 0\nset x = x + 1\nlet y = x * 2\n", "y");
    expect(out.complete).toBe(true);
    const boundary = out.chain.map((e: any) => e.boundary).filter(Boolean)[0];
    expect(boundary.resolution).toBe("resolved");
    expect(boundary.last_write.line).toBe(2);
  });
});

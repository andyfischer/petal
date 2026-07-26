// `var x = …` declares a mutable cell and `set x = …` writes it. The two write
// keywords are disjoint in both directions: `=` rejects a var, `set` rejects
// everything else. See docs/lowering-confusion-20260726.md sections 5c and 6b.
//
// This is chunk 1 — syntax, binding kinds, and the errors. Cells do not exist
// yet, so a `var` still compiles exactly like a `let`; what a `var` buys today
// is the keyword discipline, not new semantics.

import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  runPetal,
  runPetalError,
  showAstJson,
  showTokensJson,
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

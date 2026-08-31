# `var`: mutable cells

A technical outline of how `var`, `set` and `get` are implemented and what
rules they are built on. For how to *use* them, see
[`var` and `set`](language-guide.md#var-and-set) in the Language Guide, the
[syntax overview](syntax/overview.md#var-set-and-get), the exported-`var` rule in
the [module system](module-system.md#an-exported-var-is-read-only-to-importers),
and [Cells and the frontier](CLI.md#cells-and-the-frontier) for the effect on
dataflow queries.

Petal's default binding is a *dataflow* binding: `x = e` rebinds a name to a
new value rather than overwriting a slot, which is what lets the compiler trace
every read back to the value it came from. `var` is the escape hatch for code
that genuinely wants a mutable slot. One keyword per operation, none implicit:

| | |
|---|---|
| `var x = …` | declare the box |
| `set x = …` | write it |
| `get x` | read it across a function boundary |

---

## Cells

`var x = e` allocates a `Value::Cell` (`rust/src/value.rs`) — a one-value box in
a heap slab — via a `CellNew` term. Every source-level *read* of the name
lowers to `CellRead`, every `set` to `CellWrite`.

The binding itself is never rebound. That is the whole mechanism: a `var` never
enters the SSA/phi machinery, which is what makes writes from inside a
conditional, a loop, a function or a closure work at all. Closure capture needed
no special case either — captures are by value, and the captured value *is* the
cell id, which gives Lua/JS upvalue semantics for free.

`state var` puts the `CellNew` inside the `StateInit` block, so the persisted
slot holds the cell and persistence is automatic. A slot is a declaration plus
the call path that reached it, so that is one cell per slot:

- one per explicit key for `state(key) var`,
- one per callsite/iteration for a `state var` declared inside a function,
- exactly one for a top-level `state var` — the idiom for shared cross-function
  state, since module scope is a single call path.

## Containment

**No expression ever evaluates to a cell.** Reads dereference; there is no
syntax that yields the box. Storing a `var` in a record, passing it to a
function, or printing it all move the *contents* as of that moment. The only
way to share a box is closure capture, which is lexically visible.

This invariant is what keeps the feature small: equality, hashing, `print`,
`value_to_json`, `get_state_json`, the type checker's value domain and
`HostData` all need no cell case.

It is also the one thing that has been breached in practice. An exported `var`
reached through a selective import forwarded the raw cell (`print(x)` printing
`<cell 0>`) because binding kind rode only on the qualified name; the fix lives
in `bind_imports`. Treat any new name-binding path as a place to re-check the
invariant.

## Two write keywords

`=` writes a `let` and errors on a `var`; `set` writes a `var` and errors on
anything else. The disjointness runs in both directions on purpose — erroring
in only one would leave `=` meaning two opposite things depending on a
declaration that may be far away, which is the ambiguity `set` exists to
remove.

`set` never declares: an unknown name is an error. It takes field, index and
compound targets (`set r.a = 1`, `set xs[0] = 1`, `set n += 1`). `@` stays a
`let`-only rebind because it desugars to `x = f(x)`.

An exported `var` is readable by importers under every import form and writable
only by the module that declared it, so a cross-module `set` names the owner.
There is deliberately **no cross-module write syntax**: `set m.x = 1` would be
rooted at a module alias, which is not a binding, so the owning module exports a
function instead. A first-class *shared* cell — an OCaml-style `ref` escaping
into records and arguments — would give up the containment invariant above and
has to be argued as its own feature.

Enforcement is `Compiler::check_write_keyword` (`rust/src/compiler/stmt.rs`).

## Reading a cell: `get`

The same argument applied to reads. A bare name is a *captured snapshot* if it
names a `let`/`state` and a *live cell read* if it names a `var` — two different
questions told apart only by a distant declaration. A function captures by value
at its own textual position (`MakeClosure` takes the term carrying the value on
that line), so in a script that re-runs per frame the two answers differ by
exactly one frame, every frame: a defect that presents as input lag rather than
as a bug.

- **Required only across a function boundary.** Inside the declaring scope no
  snapshot exists to confuse the read with, so a bare read stays legal — which
  also keeps the loop-accumulator idiom (`set out = append(out, x)`) free of
  ceremony it gains nothing from.
- **An error on a non-`var`,** so `get` in a body always means a cell.
- **Primary position, not a prefix operator,** so postfix applies to the
  contents: `get cfg.w` is `(get cfg).w`.
- **A compound `set x += 1` synthesizes its own `get`**
  (`parse::cell_get_at_root`), because that read has no source text an author
  could have annotated.

`get` is a keyword, so it can no longer name a field, a method or an FFI host
method.

## Cross-function assignment

Assignment to a name bound outside the current function is a **compile error at
the assignment site**, uniformly across all four declaration sites (module
`let`, module `state`, lambda capture, enclosing fn local) and all four
syntactic forms (`x =`, `xs[i] =`, `r.f =`, and `@x`, which desugars to `=` and
so is caught by the same check).

Such an assignment would create a function-local shadow and silently leave the
outer binding alone; one control-flow step further (inside an `if`, `while` or
`for`) it did not even compile, because the phi would have had to initialize
from a term in another function. `var`/`set` is exempt — a `set` really does
modify the outer binding, which is the point of the escape hatch.

`Compiler::check_assign_to_outer_function_binding` returns false and the
statement is abandoned, mirroring `check_write_keyword`. That matters beyond
tidiness: a rejected assignment no longer emits the phi that would fail to
lower, so the compiles-but-does-not-lower state is gone for this shape.

Migration is per-site judgment, and code that was silently shadowing is **not**
correctly migrated to `var` — it would start actually mutating and change
behavior. That case becomes a `let` local; only genuine intended mutation
becomes `var` + `set`.

To measure a source tree, use the compiler rather than a regex (a text sweep
produces hundreds of false positives from top-level assignments inside top-level
`if`s, which are legal):

```sh
find <roots> -name '*.ptl' -not -path '*/node_modules/*' -not -path '*/target/*' \
  | xargs -n1 petal check --json \
  | grep -c 'bound outside this function'
```

## Capture lag

`compiler::capture_lag` **warns** on a *named* function whose body reads a
module binding that is rebound after the declaration. The diagnostic is reported
at the read and names the rebinding's line; the fix is a parameter. It is scoped
to *reactive* bindings only:

- **`let` is exempt.** Capturing at the definition is the defined behaviour for
  a `let` — the later `let` is a new binding, and the function above it is meant
  to read the earlier one.
- **Module-level `state` warns.** `x = e` on a `state` does not create a new
  binding; it emits a `StateWrite` into the persisted slot, and the next run
  initialises the name from that slot, so the read really is one run behind
  every run. This stops at module scope because a module-level declaration is
  the one that runs on the empty call path — a single slot every function in the
  module reads. A `state` declared *inside* a function is one slot per call
  path, is not a module binding, and is not scanned.
- **`var`/`state var` are exempt.** A bare outer-cell read is already a hard
  error, and the `get` it demands is a live read that cannot lag.

Two deliberate under-approximations: **inline lambdas are exempt** (a
`map(xs, fn(a) … end)` callback cannot outlive the statement that made it, and
the author does not control a callback's parameter list, so a flag would be
unfixable — the cost is missing a lambda genuinely stored and called later); and
**only module bindings are scanned**, so the same staleness one scope in is not
checked.

Coverage: `rust/tests/capture_lag.rs`.

## Lexical shadowing

A `let`/`state` shadows from its own line onward. An assignment that lexically
*precedes* the declaration targets the outer binding and carries out; one after
it is block-local.

```petal
let x = 1
for i in [1, 2, 3] do
  x = 5              // targets the outer x, and reaches it
  let x = i * 10
  x = x + 1          // body-local
end
print(x)             // 5
```

Two halves make this work and both are required: the phi pre-scan
(`AssignedNames` in `rust/src/compiler/phi.rs`) is scope-aware, and
`Compiler::note_shadow` freezes the value the block carries out at the
declaration. Making the pre-scan lexical *alone* is worse than no fix at all —
`wire_phi_outs` reads the block's final binding, so the shadowed local's value
carries out to the outer name.

What this prevents: a phi hoisted past the block that owns a `let` resolves the
name at the outer level, where it can hit a prelude function of the same name
(`std::take`). That broke correct, mutation-free code in the shipped `petal-ui`
prelude, with the local's *name* as the only difference — which made every
addition to `std` a potential break of user code using that name as a local.

Regression coverage: `ts/test/loop-carry-limitations.test.ts`, the walker tests
in `rust/src/compiler/phi.rs`, and the `_wrap_segment` shape in
`ts/test/check-lowers.test.ts`.

## Provenance

A cell operand — of a `CellRead`, a `CellWrite`, or a `MakeClosure` capture —
names *which box*, not which value. The backward walk is defined over value
edges only, so it terminates at every `CellRead` and reports a first-class
`CellFrontier` (`rust/src/program_analysis.rs`) carrying the var name, the
declaration, the complete static write set, and `host_writable`. A result with a
non-empty frontier is by definition incomplete: incompleteness lives in the
return type, not in a convention.

- Backward is a *must* question, so may-writes are inadmissible as edges and go
  in the frontier as possibilities. Forward is already a *may* question, so
  `EdgeKind::CellMay` edges (decl → writes, decl → reads, write → reads) belong
  in it.
- Four consumers share the walk: `trace_provenance`/`slice`,
  `trace_dependents`, `TraceBuffer::explain`, and
  `backend::errors::format_provenance` (the "Caused by:" block).
- `slice` exposes `minimal()` (fallible, byte-identical to cell-free behaviour)
  and `conservative()` (closes over cells to a fixed point). Conservative is
  sufficient in *terms*, not faithful in *order* — neither accessor yields an
  extractable program.
- Dynamic resolution matches on `CellId`, not on the declaration term, since one
  declaration mints a fresh cell per execution (per key, per loop entry, per
  call). With the trace on, `explain` re-roots across the boundary and the chain
  is complete; the escape hatch's cost is paid only when the trace is off.

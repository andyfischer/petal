# `var`: mutable cells

How `var`, `set` and `get` are implemented, and the rules they are built on.
For how to *use* them, see [`var` and `set`](language-guide.md#var-and-set)
in the Language Guide, the [syntax overview](syntax/overview.md#var-set-and-get),
the exported-`var` rule in the
[module system](module-system.md#an-exported-var-is-read-only-to-importers),
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

## Cells

`var x = e` allocates a `Value::Cell` (`core/src/value.rs`), a one-value box in
a heap slab, via a `CellNew` term. Every source-level read of the name lowers
to `CellRead`, every `set` to `CellWrite`.

The binding itself is never rebound. That is the whole mechanism: a `var` never
enters the SSA/phi machinery, which is what makes writes from inside a
conditional, a loop, a function or a closure work. Closure capture needs no
special case either: captures are by value, and the captured value *is* the
cell id, which gives Lua/JS upvalue semantics for free.

`state var` puts the `CellNew` inside the `StateInit` block, so the persisted
slot holds the cell and persistence is automatic. A slot is a declaration plus
the call path that reached it, so there is one cell per slot:

- one per explicit key for `state(key) var`,
- one per callsite/iteration for a `state var` declared inside a function,
- exactly one for a top-level `state var`, the idiom for shared cross-function
  state, since module scope is a single call path.

## Containment

**No expression ever evaluates to a cell.** Reads dereference; there is no
syntax that yields the box. Storing a `var` in a record, passing it to a
function, or printing it all move the *contents* as of that moment. The only
way to share a box is closure capture, which is lexically visible.

This invariant keeps the feature small: equality, hashing, `print`,
`value_to_json`, `get_state_json`, the type checker's value domain and
`HostData` need no cell case. Any new name-binding path (imports, for example)
is a place to re-check it; binding kind must travel with the name.

## Two write keywords

`=` writes a `let` and errors on a `var`; `set` writes a `var` and errors on
anything else. The rule runs in both directions on purpose, so that `=` never
means two opposite things depending on a declaration that may be far away.

`set` never declares: an unknown name is an error. It takes field, index and
compound targets (`set r.a = 1`, `set xs[0] = 1`, `set n += 1`). `@` stays a
`let`-only rebind because it desugars to `x = f(x)`.

An exported `var` is readable by importers under every import form and writable
only by the module that declared it. There is deliberately no cross-module
write syntax: `set m.x = 1` would be rooted at a module alias, which is not a
binding, so the owning module exports a function instead. A first-class shared
cell (an OCaml-style `ref` escaping into records and arguments) would give up
the containment invariant and would be its own feature.

Enforcement is `Compiler::check_write_keyword` (`core/src/compiler/stmt.rs`).

## Reading a cell: `get`

A bare name is a *captured snapshot* if it names a `let`/`state` and a *live
cell read* if it names a `var`. A function captures by value at its own textual
position (`MakeClosure` takes the term carrying the value on that line), so in
a script that re-runs per frame the two answers differ by exactly one frame, a
defect that presents as input lag. `get` makes the live read explicit.

- **Required only across a function boundary.** Inside the declaring scope
  there is no snapshot to confuse the read with, so a bare read stays legal,
  which keeps the loop-accumulator idiom (`set out = append(out, x)`) free of
  ceremony.
- **An error on a non-`var`,** so `get` in a body always means a cell.
- **Primary position, not a prefix operator,** so postfix applies to the
  contents: `get cfg.w` is `(get cfg).w`.
- **A compound `set x += 1` synthesizes its own `get`**
  (`parse::cell_get_at_root`), because that read has no source text to
  annotate.

`get` is a keyword, so it cannot name a field, a method or an FFI host method.

## Writing a container in place

`set xs[i] = v` lowers to a `CellRead`, a `SetIndex` on what it read, and a
`CellWrite` of the result. Left at that, every such write copies its whole
container: the read hands the list to a register, so the mutation can never be
its only holder. The two in-place routes of the optimizer do not help, because
both prove a container unique from where it was allocated, and a cell's
contents were allocated in some other function, on some other frame, or in an
earlier run.

So uniqueness is tracked per cell, at run time. The heap keeps an *owned* bit
beside each cell's value, set while the cell is the only holder of its
container, and `backend::bytecode::cells` picks the accesses that can keep it
set:

| Lowered as | Source | What it does |
|---|---|---|
| take, in-place mutation, put | `set xs[i] = v`, `set r.f = v`, `set xs = append(xs, v)` | mutates the cell's own container if the cell owns it, a fresh copy otherwise; either way the cell owns the result |
| peek | `get xs[i]`, `get r.f`, `len(get xs)` | looks into the contents and lets them go; ownership is untouched |
| shared read / write | everything else | hands the contents out, or stores a value something else may hold; clears the bit |

The first write after anything else got hold of the contents pays one copy and
the writes after it pay none, from any function that can see the cell: a
top-level function writing a module-level `var` or `state var`, or a closure
writing an enclosing function's local. Value semantics needs no special case.
`let snap = get xs` is a shared read, so the write after it copies and `snap`
keeps what it saw.

Three limits, all of which fall back to copying rather than to a wrong answer:

- **A call between the read and the write.** `set xs = append(xs, f(x))` reads
  `xs`, then runs `f`, which might read or write `xs` itself. Bind the argument
  first: `let v = f(x)` then `set xs = append(xs, v)`.
- **Inner containers.** `set g[i][j] = v` rewrites `g` in place and copies row
  `i`; `set ps[i].x = v` copies one record. The bit covers the cell's top-level
  container only, since `get g[i]` hands a row out.
- **A write whose value is read.** A `set` is an expression, and `fn put(i, v)
  set xs[i] = v end` returns the whole list. Called as a statement, that result
  is dropped and the write stays in place; `let ys = put(i, v)` reads it, so
  that write hands the list out and the next one copies. The caller's answer
  reaches the callee on its frame (`isa::ResultUse`), which is also why a
  script whose *last* statement is such a call copies once per run: its value
  is the program's result.

Reading or writing a cell is not a slow path in the VM either. The
straight-line loop (`vm/fast.rs`) retires `CellRead`, `CellWrite` and
`SetIndexInPlace` itself, and hands one to the general executor only when
something outside the script has to hear of it: an open memo scope that did
not create the cell, or the frame gate's fingerprint before a `state var` is
first mutated in a run.

The pass is `OptFlags::in_place_cells`, on by default and off under the
`baseline` policy, and it stands down while the `explain` trace is recording,
because the trace is a history of values that an in-place write would rewrite.
The soundness argument and the three things outside the script that read a
cell across a write (memo records, the frame gate, the observation buffer) are
in the module docs of `core/src/backend/bytecode/cells.rs`. Coverage:
`core/tests/cell_in_place.rs` and the differential fuzzer in
`core/src/backend/bytecode/cell_fuzz.rs`.

## Cross-function assignment

Assignment to a name bound outside the current function is a compile error at
the assignment site. It applies to every declaration site (module `let`,
module `state`, lambda capture, enclosing fn local) and every syntactic form
(`x =`, `xs[i] =`, `r.f =`, and `@x`, which desugars to `=`).

Such an assignment would create a function-local shadow and silently leave the
outer binding alone. `var`/`set` is exempt: a `set` really does modify the
outer binding, which is the point of the escape hatch.

`Compiler::check_assign_to_outer_function_binding` returns false and the
statement is abandoned, so a rejected assignment never emits a phi that would
fail to lower.

When fixing such an error, code that was silently shadowing should become a
`let` local, not a `var`; converting it to `var` would start actually mutating
and change behavior. Only intended mutation becomes `var` + `set`.

When the mutation is intended, `petal apply-change convert-to-var` makes the
conversion mechanically: the declaration, every `=` (to `set`) and every read
across a function boundary (to `get`), following a `pub` binding into its
importers. It resolves mentions lexically, by the shadowing rules below. See
[CLI.md](CLI.md#apply-change--refactors-that-change-behaviour) and
`core/src/apply_change/`.

## Capture lag

`compiler::capture_lag` warns on a *named* function whose body reads a module
binding that is rebound after the declaration. The diagnostic is reported at
the read and names the rebinding's line; the fix is a parameter. It covers
reactive bindings only:

- **`let` is exempt.** Capturing at the definition is the defined behaviour
  for a `let`: the later `let` is a new binding, and the function above it is
  meant to read the earlier one.
- **Module-level `state` warns.** `x = e` on a `state` does not create a new
  binding; it emits a `StateWrite` into the persisted slot, and the next run
  initialises the name from that slot, so the read really is one run behind.
  A `state` declared inside a function is one slot per call path, is not a
  module binding, and is not scanned.
- **`var`/`state var` are exempt.** A bare outer-cell read is already a hard
  error, and the `get` it demands is a live read that cannot lag.

Two deliberate under-approximations: inline lambdas are exempt (a
`map(xs, fn(a) … end)` callback cannot outlive the statement that made it, and
the author does not control a callback's parameter list, so a warning would be
unfixable), and only module bindings are scanned.

Coverage: `core/tests/capture_lag.rs`.

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
(`AssignedNames` in `core/src/compiler/phi.rs`) is scope-aware, and
`Compiler::note_shadow` freezes the value the block carries out at the
declaration. Making the pre-scan lexical alone is worse than no fix at all,
because `wire_phi_outs` reads the block's final binding, so the shadowed
local's value would carry out to the outer name.

Without this, a phi hoisted past the block that owns a `let` resolves the name
at the outer level, where it can hit a prelude function of the same name
(`std::take`), so every addition to `std` could break user code using that
name as a local.

Regression coverage: `test/vitest/loop-carry-limitations.test.ts`, the walker tests
in `core/src/compiler/phi.rs`, and the `_wrap_segment` shape in
`test/vitest/check-lowers.test.ts`.

## Provenance

A cell operand (of a `CellRead`, a `CellWrite`, or a `MakeClosure` capture)
names *which box*, not which value. The backward walk is defined over value
edges only, so it stops at every `CellRead` and reports a `CellFrontier`
(`core/src/program_analysis.rs`) carrying the var name, the declaration, the
complete static write set, and `host_writable`. A result with a non-empty
frontier is by definition incomplete, and the return type says so.

- Backward is a *must* question, so may-writes go in the frontier as
  possibilities rather than as edges. Forward is already a *may* question, so
  `EdgeKind::CellMay` edges (decl → writes, decl → reads, write → reads) belong
  in it.
- Four consumers share the walk: `trace_provenance`/`slice`,
  `trace_dependents`, `TraceBuffer::explain`, and
  `backend::errors::format_provenance` (the "Caused by:" block).
- `slice` exposes `minimal()` (fallible, byte-identical to cell-free behaviour)
  and `conservative()` (closes over cells to a fixed point). Conservative is
  sufficient in *terms*, not faithful in *order*; neither yields an
  extractable program.
- Dynamic resolution matches on `CellId`, not on the declaration term, since
  one declaration mints a fresh cell per execution (per key, per loop entry,
  per call). With the trace on, `explain` re-roots across the boundary and the
  chain is complete.

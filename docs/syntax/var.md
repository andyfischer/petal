# `var`: mutable cells

`var`, `set` and `get`: how to use them, the rules they are built on, and how
they are implemented. Related pages: the exported-`var` rule in the
[module system](../module-system.md#an-exported-var-is-read-only-to-importers),
[Cells and the frontier](../CLI.md#cells-and-the-frontier) for the effect on
dataflow queries, and [Inline state](../features/inline-state.md) for
`state var`.

Petal's default binding is a *dataflow* binding: `x = e` rebinds a name to a
new value rather than overwriting a slot, which is what lets the compiler trace
every read back to the value it came from. `var` is the escape hatch for code
that genuinely wants a mutable slot, most often an accumulator written from
inside a callback. One keyword per operation, none implicit:

| | |
|---|---|
| `var x = …` | declare the box |
| `set x = …` | write it |
| `get x` | read it across a function boundary |

Prefer `let`. Reaching for `var` costs the provenance queries (`petal graph`)
that can otherwise answer "what produced this value?" (see
[Provenance](#provenance)).

## Declaring and writing a cell

```petal
var count = 0
for i in [1, 2, 3] do
  set count = count + i
end
print(count)   // 6
```

A `var` binds a *box*, not a value. A function or a closure that mentions the
name shares the same box, so a write from inside one is visible outside. This
is the case `=` cannot express: an `=` inside a function targeting a name
bound outside it is an error ([Cross-function assignment](#cross-function-assignment)).
Reach for `var`/`set` when you meant the write to land, and for `let` when you
meant a new local.

```petal
var score = 0
let doubled = map([1, 2, 3], fn(a)
  if a > 1 then set score += 10 end
  a * 2
end)
print(doubled, score)   // [2, 4, 6] 20
```

Each *evaluation* of the declaration makes a new box, so a factory hands out
independent counters:

```petal
fn counter()
  var c = 0
  let bump = fn()
    set c = get c + 1
    get c
  end
  bump
end
let a = counter()
let b = counter()
print(a(), a(), b())   // 1 2 1
```

A `var` takes a type annotation like any binding (`var total: int = 0`), and
there it constrains every `set` as well as typing every read; see
[Type Annotations](../language-guide.md#type-annotations).

## Two write keywords

`=` writes a `let` and errors on a `var`; `set` writes a `var` and errors on
anything else. The rule runs in both directions on purpose, so that `=` never
means two opposite things depending on a declaration that may be far away, and
every write to a cell is visible where it happens.

```petal ignore
let a = 1
var b = 1

a = 2          // fine
set b = 2      // fine

set a = 2      // error: `a` is not a `var`; use `a = ...`
b = 2          // error: `b` is a `var`; use `set b = ...` to write it
```

`set` never declares: an unknown name is an error. It takes field, index and
compound targets (`set r.a = 1`, `set xs[0] = 1`, `set n += 1`). `@` stays a
`let`-only [rebind](rebind-operator.md) because it desugars to `x = f(x)`.

An exported `var` is readable by importers under every import form and writable
only by the module that declared it. There is deliberately no cross-module
write syntax: `set m.x = 1` would be rooted at a module alias, which is not a
binding, so the owning module exports a function instead. A first-class shared
cell (an OCaml-style `ref` escaping into records and arguments) would give up
the containment invariant and would be its own feature.

Enforcement is `Compiler::check_write_keyword` (`core/src/compiler/stmt.rs`).

## Reading a cell: `get`

Reading a cell from inside a function is written `get`:

```petal
var hits = 0

fn describe()
  "hits so far: {get hits}"
end

set hits = 3
print(describe())   // hits so far: 3
```

The reason is timing. A bare name is a *captured snapshot* if it names a
`let`/`state` and a *live cell read* if it names a `var`. A function captures
the ordinary bindings it mentions by value at its own textual position
(`MakeClosure` takes the term carrying the value on that line), so in a script
that re-runs per frame the two answers differ by exactly one frame, a defect
that presents as input lag. `get` makes the live read explicit: a bare name
inside a function body is always a value that cannot change under you, and
anything live says `get`.

- **Required only across a function boundary.** Inside the declaring scope
  there is no snapshot to confuse the read with, so a bare read stays legal,
  which keeps the loop-accumulator idiom (`set out = append(out, x)`) free of
  ceremony.
- **A bare read of an outer `var` inside a function is an error** that asks
  for `get`.
- **An error on a non-`var`,** so `get` in a body always means a cell.
- **Primary position, not a prefix operator,** so postfix applies to the
  contents: `get cfg.w` is `(get cfg).w`, which is what a cell holding a
  record wants.
- **A compound `set x += 1` synthesizes its own `get`**
  (`parse::cell_get_at_root`), because that read has no source text to
  annotate.

`get` and `set` are keywords, so neither can name a binding, a function or a
method (`fn Point.get(p)` is a parse error). Like every keyword they can still
be a record key and a field name (`{get: 1}`, `r.get`).

## Containment

**No expression ever evaluates to a cell.** Reads dereference; there is no
syntax that yields the box. Storing a `var` in a record, passing it to a
function, or printing it all move the *contents* as of that moment. The only
way to share a box is closure capture, which is lexically visible.

```petal
var x = 1
let r = { a: x }
set x = 2
print(r, x)   // { a: 1 } 2
```

This invariant keeps the feature small: equality, hashing, `print`,
`value_to_json`, `get_state_json`, the type checker's value domain and
`HostData` need no cell case. Any new name-binding path (imports, for example)
is a place to re-check it; binding kind must travel with the name.

## Mutable state shared by several functions

A `var` is how several functions update the same data. `=` cannot do it, since
a function may not assign a name bound outside itself, and returning a fresh
copy from every helper stops being practical once the data is a handful of
lists that a loop writes thousands of times. Put the data in cells: every
function that can see a cell reads it with `get` and writes it with `set`.

There are two places to put the cells, and the choice is about how long the
data lives.

**For one call: local `var`s and nested functions.** Declare the cells in the
function that owns the work, then the helpers as closures over them, then the
algorithm:

```petal
fn settle(heights: list, passes: int) -> list
  var h = heights
  var moves = 0

  // Move one unit from a to b if a stands at least two higher.
  let level = fn(a, b)
    if get h[a] - get h[b] < 2 then return nil end
    set h[a] = get h[a] - 1
    set h[b] = get h[b] + 1
    set moves += 1
  end

  for pass in range(0, passes) do
    for i in range(0, len(h) - 1) do
      level(i, i + 1)
      level(i + 1, i)
    end
  end
  print("moves:", moves)
  h
end

print(settle([6, 0, 0, 2], 8))
```

`settle` is still a pure function of its arguments. Each call makes its own
cells, nothing of them outlives it, and what it returns is an ordinary value.
Its own code reads the cells bare (`len(h)`, and `h` on the last line), so only
the helpers pay for `get`. Prefer this shape whenever the data belongs to one
computation: a solver step, a parser, a layout pass.

**For the whole program: a top-level `var` or `state var`, and top-level
functions.** This is the shape for data that outlives any one call, which in a
script that re-runs every frame means a
[`state var`](../features/inline-state.md):

```petal
state var board = [0, 0, 0, 0]
state var turn = 0

fn mark(i: int)
  if get board[i] != 0 then return nil end
  set board[i] = 1 + get turn % 2
  set turn += 1
end

mark(2)
mark(2)       // taken: nothing happens
mark(0)
print(board, turn)   // [2, 0, 1, 0] 2
```

Every function reads these cells with `get`, however plain the function, and
module scope reads them bare. A top-level `var` without `state` is made afresh
on each run; `state var` keeps its value across runs. A module can export
either, and importers can read it but not write it
([module system](../module-system.md#an-exported-var-is-read-only-to-importers)).

A plain `state` cannot stand in for the second shape. It persists, but it is
still a dataflow binding, so a function that assigns one gets the same error
as for a `let`:

```petal ignore
state score = 0

fn award(n)
  score = score + n   // error: `score` is bound outside this function
end
```

Declare it `state var score = 0` and write `set score = get score + n`, or
keep the `state` and have the function return the new value for module scope
to assign. `petal apply-change convert-to-var game.ptl --target /score` makes
that conversion for you ([Cross-function assignment](#cross-function-assignment)).

Writes into a cell's container happen in place, so a helper that writes one
element costs one element, not a copy of the list; the next section has the
rule and the habits that keep it so. `examples/games/physics-playground` uses
both shapes: `physics.ptl` steps its solver with closures over local cells,
and `app.ptl` edits its `state var`s from top-level functions.

## Writing a container in place

`set xs[i] = v`, `set r.f = v` and `set xs = append(xs, v)` change the cell's
own list or record when the cell is its only holder, from any function that
can see the cell; so do `set m[k] = v` and `set m = remove(m, k)` on a
[map](../language-guide.md#maps). A loop can be factored into helpers without changing what it
costs. Values still behave as values: whatever took the contents earlier keeps
what it took, and the next write pays for one copy.

```petal
var xs = [1, 2, 3]
let snap = xs
set xs[0] = 99
print(snap, xs)   // [1, 2, 3] [99, 2, 3]
```

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

Three limits, all of which fall back to copying rather than to a wrong
answer. Each comes with the habit that avoids it:

- **A call between the read and the write.** `set xs = append(xs, f(x))` reads
  `xs`, then runs `f`, which might read or write `xs` itself. Bind the argument
  first: `let v = f(x)` then `set xs = append(xs, v)`.
- **Inner containers.** `set g[i][j] = v` rewrites `g` in place and copies row
  `i`; `set ps[i].x = v` copies one record. The bit covers the cell's top-level
  container only, since `get g[i]` hands a row out. Keep hot data one level
  deep: parallel flat lists (`xs`, `ys`, `vx`, `vy`) avoid both.
- **A write whose value is read.** A `set` is an expression, and `fn put(i, v)
  set xs[i] = v end` returns the whole list. Called as a statement, that result
  is dropped and the write stays in place; `let ys = put(i, v)` reads it, so
  that write hands the list out and the next one copies. Call a writing
  helper as a statement. The caller's answer
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
the call path that reached it ([Inline state](../features/inline-state.md)),
so there is one cell per slot:

- one per explicit key for `state(key) var`,
- one per callsite/iteration for a `state var` declared inside a function,
- exactly one for a top-level `state var`, the idiom for shared cross-function
  state, since module scope is a single call path.

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

When the mutation is intended, `petal apply-change convert-to-var <file>
--target /name` makes the conversion mechanically: the declaration (a `let`
becomes a `var`, a `state` a `state var`), every `=` (to `set`) and every read
across a function boundary (to `get`), following a `pub` binding into its
importers. It resolves mentions lexically, by the shadowing rules below. See
[../CLI.md](../CLI.md#apply-change--refactors-that-change-behaviour) and
`core/src/apply_change/`.

## Capture lag

A function reading a `let` that is rebound below it sees the earlier binding.
That is the defined behaviour: the later `let` is a new binding, and the
function reads the one in scope where it was written. Nothing is reported.

A module-level `state` is different. Writing it with `=` does not create a new
binding. It writes the persisted slot, and the *next* run of the file
initialises the name from that slot. So a function above the write reads one
run behind, every run:

```petal ignore
state scroll = 0

fn row_y(r)
  GRID_Y + (r - scroll) * ROW_H   // warning: reads `scroll` one run behind
end

scroll = scroll + 3
draw_row(row_y(2))                // uses the previous run's `scroll`
```

That is a **warning**, not an error: the program compiles and `petal check`
exits 0. Pass the value in when you want the current one:

```petal ignore
fn row_y(r, scroll)
  GRID_Y + (r - scroll) * ROW_H
end
```

A `state` that is never written after the function is fine to capture, which
is the common case.

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
at the outer level, where it can hit an imported function of the same name
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

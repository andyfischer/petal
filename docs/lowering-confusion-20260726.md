# Assignment across function boundaries: two bugs, and a plan

Investigation notes, 2026-07-26. Prompted by a workaround note in
`~/game-prototypes/worldsfair/ui/ptl/host/garden.ptl`, which reported a
"Petal bytecode-lowering bug" for module-level `state` assigned inside a
function. The report was real but the diagnosis was narrow. Two distinct
defects sit behind it, and one of them is a name-collision landmine.

Everything below was reproduced against `db85d67`.

---

## 1. What the language actually does today

**The rule that explains the whole surface: assigning to a binding from an
outer *function* creates a function-local shadow. The outer binding is never
modified.** What varies is only whether that shadow requires a *phi* — and if
it does, lowering fails.

| declaration site | assigned with no control flow | assigned inside `if`/`while`/`for` |
|---|---|---|
| module-level `let` | shadows silently | **lowering error** |
| module-level `state` | shadows silently | **lowering error** |
| lambda capture | shadows silently | **lowering error** |
| enclosing fn's local | shadows silently | **lowering error** |

The error is always `bytecode lowering failed: term tNN in block bN not in
this function`, raised by `Lower::flat`
(`rust/src/backend/bytecode/lower.rs:152`) when a phi's `inputs[0]` names a
term belonging to another function.

### 1a. The "working" case is not doing what it looks like

```petal
let i = 10
fn f()
  i = i + 1     // reads the capture, writes a function-local shadow
  i
end
print(f())      // 11
print(f())      // 11   <- not 12. Nothing accumulates.
print(i)        // 10   <- outer untouched
```

The same silent shadow applies to `state`, to lambda captures, and to a nested
`fn` assigning its enclosing function's local. It also applies to three further
syntactic forms:

```petal
let xs = [1,2,3]          let r = {a: 1}          let i = 0
fn f() xs[0] = 99         fn f() r.a = 99         fn f() bump(@i)
       xs end                    r end                   i end
f() -> [99, 2, 3]         f() -> { a: 99 }        f() -> 1
xs  -> [1, 2, 3]          r   -> { a: 1 }         i   -> 0
```

### 1b. One control-flow step away, it stops compiling

```petal
let i = 0
fn f()
  if false then i = 1 end   // condition is never taken; still fails
  i
end
// Error: bytecode lowering failed: term t92 in block b0 not in this function
```

Identical for `while` and `for`, for `state` as well as `let`, for lambdas, and
for nested functions.

### 1c. What must keep working — same-function rebinding is the core feature

None of these involve a function boundary. All are pure dataflow (a phi in the
block that owns the binding), and all must be untouched by any fix:

```petal
fn f(n)                    fn f(n)                  fn f()                    let i = 0
  let i = 0                  if n > 0 then            state i = 0             if true then i = 1 end
  if n > 0 then i = 1 end      n = 100                if true then            print(i)   // 1
  i                          end                        i = i + 1 end
end        // 1, 0           n                        i                       (top level: not a
                           end      // 100          end     // 1, 2            function boundary)
```

### 1d. Why this split is indefensible

The split is not semantic. Both sides do the same thing — create a local
shadow. One of them happens to need a phi, and the phi machinery has no way to
express "initialize from a term in another function." An implementation detail
is showing through as a language rule.

The `if` variant is arguably the honest half: at least it fails. The
non-control-flow half is worse, because the code reads as a dataflow edge from
`f` back out to `i` and there is no such edge. A reader who sees `i = i + 1`
inside `f` and concludes "this accumulates" is wrong, and nothing tells them.

---

## 2. The second bug: a phi hoisted onto a shadowed outer name

This one is not about assignment across functions at all. It was found by
lowering the whole corpus, and it hits **`petal-ui/prelude/ui.ptl` — the
shipped UI prelude.**

`ui.ptl:163-186`, `_wrap_segment`, reduced to its shape:

```petal
fn f(words)
  for w in words do
    while len(w) > 2 do
      let take = 2              // a fresh local, correct in every way
      while take < 3 do
        take = take + 1
      end
      w = slice(w, take, len(w))
    end
  end
  99
end
print(f(["abcd"]))
// Error: bytecode lowering failed: term t269 in block b0 not in this function
```

Rename `take` to `zzz` and it compiles and runs. **The only difference is the
name.** `take` collides with `std::take` from the auto-loaded core prelude
(`rust/prelude/std.ptl:91`), and the IR shows exactly what went wrong — a phi
in the *function body block*, initialized from the root-block std closure:

```
t945 r10 = Phi [t269] ; take        <- t269 = MakeClosure(fn15) ; std::take
t946 r12 = ForLoop [t942] -> block75
```

The `let take` at the inner level is ignored when the phi is hoisted out to the
for-loop's parent block; at that level the name resolves to the module-level
`std::take` instead. Removing the outer `for`, or removing the use of `take`
after the inner `while`, both make it compile — so the trigger is a phi being
lifted past the block that owns the `let`.

**Why this is the more urgent of the two.** `_wrap_segment` is correct,
idiomatic, mutation-free-in-spirit code. It broke because the *standard
library* gained a function named `take`. Every future addition to `std` is a
potential break of any user code that uses that name as a local, with an error
message that points at neither the `let` nor the collision. It is also a
silent tax on the prelude's namespace.

This bug should be fixed regardless of what is decided about `mut`/`var`.
It is not an escape-hatch case — there is nothing here anyone wanted to do
differently.

---

## 3. Corpus evidence

158 `.ptl` files across `~/petal`, `~/garden`, `~/game-prototypes/worldsfair`,
`~/biz/petal-lang.org`, `~/biz/experiment-cube-browser`,
`~/biz/experiment-todo-app`, `~/biz/hotlaps`.

| | |
|---|---|
| `petal check` passes | 156 / 158 (2 failures are unimplemented syntax in `docs/examples/aspirational/`) |
| `petal show-bytecode` passes | 152 / 158 |
| fail **only** at lowering | 4 — `petal-ui/prelude/ui.ptl`, and the SDL `invaders` / `platformer` / `tetris` examples |

Two things follow.

**`petal check` does not lower to bytecode**, so it exits 0 on a program that
cannot run. That is a real gap on its own: `check` is what CI and editors call.
Lowering belongs in `check`.

**The 4 lowering failures split across both bugs**, one each way:

- `petal-ui/prelude/ui.ptl` is §2 (the shadowed-name phi).
- **`invaders.ptl`, `platformer.ptl` and `tetris.ptl` are §1b** — and all three
  are therefore *completely broken today*. They do not run; they abort before
  the first frame.

The shape in all three is the same, and it is worth quoting because it is the
strongest argument for the escape hatch (`invaders.ptl:199`):

```petal
state score = 0
// ...
aliens = map(aliens, fn(a)
  if a.alive && !hit && rects_collide(...) then
    hit = true                    // capture, inside an `if`, inside a lambda
    score += 10
    explosions = append(explosions, { ... })
```

Accumulating into an outer binding from inside a `map` callback is exactly what
`var`/`set` is for. These are not sites to rewrite as `let` locals — the intent
is genuine mutation, there is no dataflow reading of them, and today the
language offers no way to express it. They are the migration's first customers.

**The blast radius of §1a is still unmeasured, and a regex cannot measure it.**
A text sweep produced hundreds of candidates that turned out to be top-level
assignments inside top-level `if`s — legal, common, and untouched by any
proposal here. The only trustworthy sweep is the compiler check itself, which
is why it is step 1 of the plan below.

---

## 4. Decision

Assignment to a name bound outside the current function becomes a **compile
error**, at the assignment site, uniformly across all four declaration sites
and all four syntactic forms (`x =`, `xs[i] =`, `r.f =`, `@x`).

```
error: `i` is bound outside `f`
  |
4 |   i = i + 1
  |   ^ assignment here creates a local shadow; it does not modify `i`
  |
  = help: use `let i = ...` for a new local, or return the value
  = help: declare it `var i = ...` if it really must be mutable
```

The motivation is Petal's core commitment: programs should read as dataflow
diagrams. A function whose body appears to reach out and modify enclosing scope
is a lie about the graph — today a *harmless* lie, since the mutation does not
happen, which makes it harder to catch, not easier.

Note for migration: code currently relying on §1a is **not** correctly migrated
to `var`. Today it shadows; under `var` it would genuinely mutate, changing
behavior (`11, 11` becomes `11, 12`). Silent-shadow sites should become `let`
locals. `var` is the escape hatch for code that *wanted* mutation and could not
have it — not a compatibility shim.

---

## 5. The escape hatch: survey

Petal needs a way to say "this one really is a mutable slot." The concept is
well-trodden; the surface syntax choice is not obvious.

### 5a. How other languages spell it

| Language | Immutable | Mutable | Write syntax | Notes |
|---|---|---|---|---|
| Rust | `let x` | `let mut x` | `x = 1` | Closures mutate captures via `FnMut`; borrow checker enforces **exclusive** access |
| Swift | `let x` | `var x` | `x = 1` | Two declaration keywords, no modifier |
| Kotlin | `val x` | `var x` | `x = 1` | " |
| Scala | `val x` | `var x` | `x = 1` | " |
| Nim | `let x` | `var x` | `x = 1` | Plus `const` for compile-time |
| Zig | `const x` | `var x` | `x = 1` | " |
| JavaScript | `const x` | `let x` | `x = 1` | Closures share the binding — the exact semantics proposed here |
| Go | — | `var x` / `x :=` | `x = 1` | All bindings mutable |
| **F#** | `let x` | `let mutable x` | **`x <- 1`** | *Distinct write operator* |
| **Verse** (Epic) | `x := 0` | `var x : int = 0` | **`set x = 1`** | *Distinct write keyword* |
| OCaml / SML | `let x` | `let x = ref 0` | `x := 1`, read `!x` | Mutability is a **value** (a cell), not a binding property |
| Clojure | `def` | `(atom 0)` | `(reset! a 1)` | Cell, plus explicit deref |
| Haskell | everything | `IORef` / `STRef` | `writeIORef` | Cell, in a monad |
| Elixir | `x = 1` | — | — | Rebinding shadows; **has the same block trap** — assignment inside `if` does not escape, and the answer is `x = if ... end` |
| Gleam / Elm / Roc | everything | — | — | No mutation at all |

Three families, and Petal has to pick one:

1. **Modifier on the declaration, ordinary `=` to write** (Rust, Swift, Kotlin,
   Nim, Zig, JS). Cheapest to write; the cost is that `=` means two different
   things depending on a declaration that may be far away.
2. **Distinct write syntax** (F# `<-`, Verse `set`). Every mutation is visible
   *at the mutation site* with no lookup.
3. **Mutability as a value** (OCaml `ref`, Clojure `atom`). Most explicit,
   heaviest syntax, and it leaks a cell into the value domain.

### 5b. On `mut` specifically

The concern that `mut` "looks like Rust but does not behave like Rust" is
half right, and the accurate version is sharper.

Behaviorally, Rust is *close*: a Rust closure can mutate a captured `let mut`
binding, and

```rust
let mut i = 0;
let mut f = || i += 1;
f(); f();
// i == 2
```

is exactly the semantics proposed for Petal. So the objection is not that the
behavior differs.

The real mismatch is the **guarantee**. In Rust, `mut` is fundamentally about
*exclusive access* — the borrow checker proves no one else holds a reference
while you mutate, which is what makes `mut` safe to reason about. Petal's
would be a shared mutable cell with no aliasing discipline at all: two closures
can capture the same `var` and interleave writes freely.

That is the worst kind of resemblance — identical surface, inverted safety
property. A Rust programmer reading Petal `mut` would import a guarantee that
is not there. `var` carries no such baggage: it says "variable" and every
language that uses it means exactly what Petal would mean.

**Decided: `var`, not `mut`.**

### 5c. On the write syntax

Petal has a specific reason to prefer family 2 that most languages do not.
In Petal, `=` *already* has a meaning — rebind the name to a new term, a
dataflow edge. Overloading it to also mean "write through to a cell" puts two
opposite operations behind one glyph, distinguished only by a declaration
elsewhere. For a language whose stated goal is that programs read as dataflow
diagrams, the non-dataflow edges are precisely the ones that must be visible
locally.

`set x = 1` (Verse) over `x <- 1` (F#): no new operator token, no lexer
ambiguity with `<`, greppable, and it reads naturally in a language with
`end`-delimited blocks. The cost is verbosity at every write, which is
appropriate for an escape hatch.

**Decided: `var x = 0` to declare, `set x = 1` to write.**

```petal
var count = 0
state var hits = 0

fn tally(x)
  if x > 0 then
    set count = count + 1
  end
end
```

This also makes the §4 error message better, because the fix is unambiguous:
if `set` is used on a non-`var`, say so; if `=` is used on a `var`, say so.

---

## 6. Proposed semantics

### 6a. The language rule, in full

- `=` on a name bound in the **current function** (or at top level) — unchanged.
  A dataflow rebind, lowering to a phi. No cell, no runtime cost. This is the
  overwhelmingly common case and it does not move.
- `=` on a name bound **outside the current function** — compile error (§4).
- `var x = <init>` declares a **cell**. `state var x = <init>` declares a cell
  that persists across frames.
- `set x = <expr>` writes through the cell. Legal from anywhere the name is in
  scope, including inside a function, inside a conditional, inside a closure.
- Reading a `var` name is ordinary: `x + 1` reads the cell's current contents.

### 6b. `=` and `set` are disjoint, in both directions

Each binding kind accepts exactly one write keyword, and rejects the other:

| binding | `=` | `set` |
|---|---|---|
| `let`, `state`, fn params, loop vars | dataflow rebind | **error** — "`x` is not a `var`; use `x = …`" |
| `var`, `state var` | **error** — "`x` is a `var`; use `set x = …`" | cell write |

Erroring in only one direction would leave `=` still meaning two different
things depending on a distant declaration — exactly the ambiguity `set` was
chosen to remove (§5c). Both directions error, so either mistake names the
keyword you wanted.

This extends to field and index targets. `r.a = 1` and `xs[0] = 1` are dataflow
rebinds today (build a new value, rebind the name) and stay that way for `let`
bindings; they become `set r.a = 1` / `set xs[0] = 1` when the base name is a
`var`.

`set` on an undeclared name is the ordinary unknown-name error, not a
declaration — `set` never introduces a binding.

### 6c. Implementation: cells

`Value::Cell(CellId)`, a heap object holding one `Value`. Three IR ops:
`CellNew` (from the init expression), `CellRead`, `CellWrite`.

A `var` declaration binds a term whose value is the cell. Every source-level
*read* of the name compiles to `CellRead(cell_term)`; every `set` compiles to
`CellWrite(cell_term, value)`.

This falls out well:

- **Closure capture needs no changes at all.** Captures are already by value
  (`MakeClosure` with `capture_outer_tids`, `rust/src/compiler/function.rs`),
  and the captured value *is* the cell id — so the closure shares the cell
  automatically. This is Lua/JS upvalue semantics, for free.
- **`var` names never need a phi.** They leave the SSA machinery entirely,
  which is what makes conditional and cross-function assignment work.
- **`Heap::fork` already isolates them**, so speculative execution and
  what-if frames stay sound with no extra work.
- **`state var` is natural**: the state slot holds the cell, `StateInit`
  creates it once, reads and writes go through it, persistence is automatic.

### 6d. The containment invariant

**No expression ever evaluates to a `Value::Cell`.** Reads dereference; there
is no syntax that yields the cell itself. Passing `x` to a function passes the
*contents*, so there is no aliasing through arguments — only through closure
capture, which is explicit and lexically visible.

This invariant is what keeps the change small. Because cells never reach user
code, nothing changes in equality, hashing, `print`, `value_to_json`,
`get_state_json`, the type checker's value domain, or `HostData`. Worth an
assertion and a fuzzer invariant, not just a comment.

### 6e. The cost, stated plainly

**Provenance degrades at every `CellRead`.** `trace_provenance`, `slice`, and
`ExplainTerm` answer "what influenced this?" by walking dataflow edges. A cell
read has no such edge — its value came from whichever `CellWrite` ran last,
which is a dynamic fact. Backward walks must stop at a cell read and report it
honestly ("read of var `x`; last written at …" if the trace buffer is on,
otherwise "unknown"), never silently return a partial graph.

This is not a defect to be fixed later. It *is* the reason `var` is an escape
hatch and not the default, and it is the argument for making writes verbose:
every `set` is a place the dataflow story goes dark, and the reader should be
able to see them.

---

## 7. Plan

**Step 1 — Measure (do this first).** Implement the §4 check in the compiler
as a *warning* on the non-fatal `Diagnostic` channel (`rust/src/diagnostic.rs`
+ `Program.warnings`, the same channel the type checker uses). `needs_capture`
in `rust/src/compiler/function.rs:166` already answers the question; this is
a small change. Run it over all 158 corpus files and count real §1a sites.
That number decides how much migration tooling step 4 needs. Nothing breaks
during this step.

**Step 2 — Fix §2, independently.** The hoisted-phi-onto-a-shadowed-name bug is
not an escape-hatch case and should not wait on any of this. Fix the phi
placement so it initializes from the binding that is actually in scope at the
`let`, add `ui.ptl`'s `_wrap_segment` shape as a regression test, and confirm
the 4 corpus failures drop to 0. Keep
`lowering_reports_cross_function_term_reference_as_error` — it corrupts IR by
hand and remains valid as the malformed-IR guard.

**Step 3 — Lower in `petal check`.** One-line-ish, and it is why §2 went
unnoticed: CI and editors call `check`, which today exits 0 on programs that
cannot run.

**Step 4 — Land `var` / `set`.** Lexer tokens; `parse_let`/`parse_state`
(`rust/src/parse.rs:222,238`) plus the CST projection in `cst_project.rs`;
`StmtKind::Let`/`State` gain an `is_var` flag and a new `StmtKind::Set`;
`Value::Cell` + heap object; `CellNew`/`CellRead`/`CellWrite` in `TermOp` and
the bytecode. Then: type checker (a `var`'s writes must stay assignable to its
declared type), `petal lint` (formatter must preserve the keywords, and the
IR-equivalence gate must hold), `transfer_state` (cells in state survive hot
reload), `ir_serialize`/`ir_validate`, the in-place/COW uniqueness analysis
(cell contents are aliased and never unique), and provenance (§6e). Per
`bytecode-future-ideas.md`, new opcodes must earn a differential-fuzzer soak
before going default-on.

**Step 5 — Flip the warning to an error, and migrate.** Corpus first, then
downstream projects. Migration is per-site judgment, not mechanical: a silent
shadow becomes a `let` local; genuine intended mutation becomes `var` + `set`.

## 8. Resolved secondary decisions

All settled 2026-07-26. Recorded here because each one shapes the parser or the
migration, and none is re-litigated without a reason.

- **`var` is allowed at top level.** Strictly unnecessary — a top-level `let`
  reassigned inside a top-level `if` already works as a phi — but rejecting it
  would mean a name's declaration has to change when code moves into a
  function, which is exactly the friction that produced §1. Cells stay rare by
  convention and by `set`'s verbosity, not by a scope rule.
- **`@` stays a `let`-only rebind.** `f(@x)` desugars to `x = f(x)`, which is
  an `=` form and therefore illegal on a `var`. The linter's rebind rule
  (`rust/src/lint/rebind.rs`) must not propose `@` on `var` names, and `@` on a
  `var` is a compile error pointing at `set x = f(x)`.
- **`state(key) var` gives each key its own cell.** `state(h) var hp = 100`
  creates one cell per key, on first touch, exactly as the non-`var` form
  creates one slot per key. `sweep_untouched_state` reclaims the slot and the
  cell together — the cell is reachable only from the slot, so no extra
  bookkeeping is needed.
- **A cell cannot escape into a record, list, or argument.** This is the
  containment invariant (§6d) restated: no expression evaluates to a cell, so
  `{ a: x }` stores `x`'s *contents*, and `f(x)` passes the contents. Sharing a
  cell is possible only by closure capture, which is lexically visible. If
  someone later wants a first-class shared cell, that is a different feature
  (an OCaml-style `ref` *value*) and should be argued on its own.

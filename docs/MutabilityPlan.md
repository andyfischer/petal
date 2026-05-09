# Eliminating Mutability from Petal

> **Status: IMPLEMENTED.** All five steps below have shipped. `TermOp::Assign`
> no longer exists in the IR; rebindings inside child blocks lower to a
> pure-dataflow `TermOp::Phi` join (see `rust/src/program.rs:119` and the
> `phi_outs` field on `Block`). The `history()` API has been removed and
> `petal explain` now walks pure provenance for every name.
>
> This document is preserved as design context — it explains *why* Petal's IR
> is purely immutable, what the migration looked like, and what the
> alternative (`Assign`-based) mental model would have cost. The "current
> state" and "the plan" sections describe the pre-migration codebase.
>
> See [Architecture.md](Architecture.md) (the "Phi terms" section) for the
> shipping documentation, and [CLI.md](CLI.md) for the `Phi` op's IR JSON
> shape.

A plan to make Petal's IR match its true philosophy: there are no mutable
variables. There is a dataflow graph of terms; names are labels that point at
terms; rebinding a name in source code creates a new term and reattaches the
label.

---

## Philosophy

> Petal is a pure dataflow language. Programs are graphs of terms. Each term
> has fixed inputs, an operation, and a result; once executed, a term's
> result never changes. Variable names are labels attached to terms. When
> source code "reassigns" a variable, the compiler emits a new term and moves
> the label — it does not mutate anything at runtime. The user's mental model
> of "mutable variables" is a convenience over an immutable graph.

This philosophy gives Petal its identity:
- **Dataflow tooling is exact.** `show-provenance`, `show-dependents`,
  `explain`, the trace buffer — they all work because terms are immutable
  nodes in a DAG.
- **Debugging is values, not state.** "Why does X have this value?" reduces
  to walking edges in the graph, not replaying mutations.
- **Optimization is straightforward.** Common subexpression elimination,
  dead-term elimination, constant folding all become trivial graph passes
  when there's no mutation to reason about.

The current implementation is *almost* pure but cheats in one place. This
plan eliminates the cheat.

---

## Current state — where the philosophy holds and where it breaks

### Holds: top-level rebinding ✅

Source:
```petal
let x = 1
x = 2
print(x)
```

IR:
```
t68 r68 = Constant(c0) [] ; x   <- name "x" attached to t68
t70 r70 = Copy [t69]   ; x      <- new term, name "x" reattached
t73 r73 = Copy [t70]            <- print reads the *latest* term named x
```

Two distinct terms; `print` reads the rebound one. No mutation. Exactly the
philosophy.

### Breaks: rebinding inside child blocks ❌

Source:
```petal
let total = 0
for x in [1, 2, 3] {
  total = total + x
}
print(total)
```

IR (simplified):
```
block0:
  t68 r68 = Constant(0) ; total
  t74 r74 = ForLoop [...] -> block1
  t81 r76 = Copy [t68]                 <- post-loop reads t68's REGISTER

block1 (loop body):
  t76 r1 = Copy [t68]                  <- reads t68's CURRENT register value
  t78 r3 = Add [t76, t77]
  t79 r4 = Assign(t68) [t78]           <- runtime register write
```

`TermOp::Assign(t)` is the mutation primitive: at runtime it writes its first
input into `t`'s register, in `t`'s frame, walking parent_frame links to find
the right frame.

Same pattern in conditionals:
```petal
let x = 1
if x > 0 { x = 10 } else { x = 20 }
print(x)
```
emits `Assign(t68)` in each branch, and the post-if `print` reads t68's
mutated register.

### The single primitive to remove

`TermOp::Assign(TermId)` in `rust/src/program.rs:111`. It is the *only*
runtime mutation operation in the entire IR. Everything else is pure dataflow.

Eliminating it eliminates mutability from the language.

### Why the current compiler chose mutation

This is the SSA "phi node" problem. When two control-flow paths converge
("after the if", "after the loop"), what does `x` mean? You need *some*
mechanism to merge the two possible bindings. The simplest mechanism is "let
both branches write to the same register and read it after." That's `Assign`.

The functional/SSA alternative is to make the join *itself* a term whose
inputs are the candidate values from each branch. That's a phi node. It's
just as expressive but stays inside the dataflow graph.

---

## What this delta costs us today

### `history()` exists only because of `Assign`

`rust/src/trace.rs::TraceBuffer::history()` was added so `petal explain` could
answer "what values did `total` have over time?" It works by scanning for
every term that writes into the target's register — i.e. the target itself
plus every `Assign(target)`.

If `Assign` didn't exist, `history` would degenerate into "the chain of terms
that have been named X in source order" — which is just provenance walking
backward through the rebind chain. `explain` and `history` collapse into one
operation.

### CLI output uses imperative language

`petal explain` prints `History (5 writes): set t79 = 6` etc. The word "set"
is wrong under the philosophy. There are no writes; there are rebindings.

### Documentation talks about "mutable locals" and "reassignment"

`docs/debugging-visibility.md` and various code comments use the words
"mutated," "reassignment," "writes." This reinforces the wrong mental model
and makes the philosophical pitch harder to deliver.

### The trace buffer over-records

For each `Assign` event the trace records `inputs[0]` (the new value) as the
"value being written." A pure model would just record one event per term
execution and read the term's own register. The Assign-aware path in
`trace_term` could go away.

---

## The plan

Five steps. Each step is independently committable, ships value on its own,
and leaves the codebase in a working state. After step 5, `TermOp::Assign`
no longer exists.

### Step 1 — Terminology and doc pass (cheap, immediate clarity)

**Goal:** make the language and documentation describe the philosophy
accurately, even before any compiler change.

**Why first:** zero risk, high clarity payoff, primes future readers
(including LLM agents) for the model the rest of the plan ships.

**Files to touch:**
- `docs/debugging-visibility.md` — replace "mutated locals," "reassignment,"
  "writes" with "rebinding," "name reattachment," "the term currently named
  X." Add a paragraph at the top explaining the dataflow philosophy and
  noting that `Assign` is the one remaining mutation primitive (forward
  reference to this plan).
- `docs/MutabilityPlan.md` — *this file*; already done.
- `rust/src/trace.rs` — rename comments and doc strings on `history`,
  `HistoryEntry`, `HistoryKind`. Keep the function names for now (they get
  removed in step 5).
- `rust/src/cli.rs::Command::Explain` execute branch — change `History (N
  writes):` to `Rebindings (N):` and `set` to `rebind` in the row format.
- Comments in `rust/src/eval.rs` near the `Assign` exec branch (~line 520)
  noting this is a known philosophy violation, scheduled for removal.
- `rust/src/program.rs` — doc comment on `TermOp::Assign` explaining it's the
  legacy mutation escape hatch and pointing at this plan.

**Verify:** `npx vitest run` still passes (only docs and strings changed).
Run `petal explain --term total` on a loop accumulator and confirm the new
"Rebindings" header reads naturally.

**Estimated time:** 1 hour.

---

### Step 2 — Conditional rebindings via phi terms

**Goal:** stop emitting `Assign(t)` from inside `if`/`else` branches. Use a
phi term in the parent block instead.

**Why:** simpler of the two compiler changes. Petal already has if-as-
expression — the branches already produce values. We just need the compiler
to recognize "rebinding inside a branch" as desugaring to "use the if-term as
the new binding."

**The transformation:**

Source:
```petal
let x = 1
if cond {
  x = 10
} else {
  x = 20
}
print(x)
```

Today's IR (uses Assign):
```
t68 r68 = Constant(1) ; x
t72 = Branch [cond] -> block1, block2
t77 = Copy [t68]      <- print, reads mutated t68

block1: t73 = Const(10); t74 = Assign(t68) [t73]
block2: t75 = Const(20); t76 = Assign(t68) [t75]
```

Target IR (pure):
```
t68 r68 = Constant(1) ; x
t72 = Branch [cond] -> block1, block2
t77 = Phi(branches: [t73, t75]) ; x   <- new term, name "x" reattached
t78 = Copy [t77]                       <- print reads t77

block1: t73 = Const(10)
block2: t75 = Const(20)
```

Or, equivalently and simpler: lower `if c { x = a } else { x = b }` exactly
the same as `let x = if c { a } else { b }`. The "if as statement with
rebindings" form becomes pure sugar for "let = if-expression."

**Implementation sketch:**

1. In `rust/src/compiler.rs`, find where `if` statements are lowered. Today
   the compiler walks each branch and emits `Assign` for any name written.
2. Detect the "rebind set": the set of names rebound in *any* branch. For
   each name in the set, every branch must produce a value for that name
   (currently the implicit value of the branch's last expression — for an
   `x = 10` statement, that's `10`).
3. After lowering both branches, emit a new term in the parent block for
   each name in the rebind set. Each such term takes one input per branch
   (the branch's value-for-this-name) and uses a new `TermOp::Phi { branches:
   Vec<TermId> }` op (or reuse the existing branch result mechanism if it
   exists).
4. The new term gets the rebound name (`x`), so subsequent reads of `x` in
   the parent block see it via the normal "find latest term named X"
   resolution.
5. Stop emitting `Assign` from branch bodies. Each `x = 10` inside a branch
   becomes a regular term (possibly anonymous) whose result is the new value.

**Edge cases to handle:**
- Only one branch rebinds: the other branch must "carry forward" the previous
  value. The phi term takes the prior term as its input for that branch.
- Nested ifs: the inner if's phi term becomes the input to the outer phi.
- A rebind that introduces a new name (no prior `let`): error in current
  Petal anyway, no special handling needed.
- `else if` chains: each `Branch` desugars to nested ifs; phi composition
  follows naturally.
- Match arms: the same treatment applies. Each arm produces a value-per-
  rebound-name; a phi term joins them after the match.

**Verify:**
- `petal show-ir -e 'let x = 1; if true { x = 10 } else { x = 20 }; print(x)'`
  shows no `Assign` terms.
- `petal explain --term x` on the same shows the phi term and both branch
  inputs.
- All vitest tests pass. Several `if` tests likely need IR-shape updates.
- Add a new test asserting "no `Assign` op in compiled IR for any
  if/else/match snippet."

**Estimated time:** half a day.

---

### Step 3 — Loop carries via auto-promotion

**Goal:** stop emitting `Assign(t)` from inside loop bodies. Lift any name
rebound in a loop body to an explicit loop carry: an input/output pair on
the loop term.

**Why:** the harder compiler change but the higher payoff. Loops are where
users hit the mutability illusion most often (accumulators, counters, running
state). Getting this right means functional dataflow even for imperative-
looking source.

**The transformation:**

Source:
```petal
let total = 0
for x in [1, 2, 3] {
  total = total + x
}
print(total)
```

Today's IR (uses Assign in body):
```
t68 r68 = Constant(0) ; total
t74 = ForLoop [list] -> block1
t81 = Copy [t68]                       <- mutated value

block1 (body): t78 = Add [t68, x]; t79 = Assign(t68) [t78]
```

Target IR (pure, with carries):
```
t68 r68 = Constant(0) ; total          <- initial value
t74 = ForLoop { list: [list], carries_in: [t68] } -> block1
                                       <- t74's *result* is the carry-out
t81 = Copy [t74] ; total               <- post-loop reads the loop term itself

block1 (body):
  carry: t75 (synthetic) ; total       <- the per-iteration "current total"
  body: t78 = Add [t75, x]
  body_result_carries: [t78]           <- this is the new value of total
```

Key points:
- The loop term itself becomes the value of any carry after the loop.
- Inside the body, references to the carry name (`total`) resolve to a
  synthetic per-iteration term (similar to how loop variables like `x`
  already work).
- The body block has an explicit "carry-out" list — the terms that should
  become the next iteration's carry-in.
- If the loop never executes, the loop term's result for that carry is the
  initial input.

**Implementation sketch:**

1. Extend `TermOp::ForLoop` and `TermOp::WhileLoop` (or add new variants) to
   carry a `Vec<TermId>` of carry inputs and matching `Vec<RegisterIndex>`
   of carry slots in the body block. Could also be modeled as a list per
   carry: `carries: Vec<{name, init, body_slot}>`.
2. In `rust/src/compiler.rs`, when lowering a loop body:
   a. Walk the body once to find all names that are rebound (write
      detection). Today these become `Assign` calls; the new pass finds them
      first.
   b. For each such name, check that there's an in-scope binding before the
      loop. If not, it's still legal Petal but needs an implicit nil init —
      lower it to `let name = nil` injected before the loop.
   c. Create a synthetic per-iteration term in the body block for each
      carry, similar to how the loop variable works. Bind the carry name to
      it inside the body.
   d. Lower the body normally — but rebinds within the body now hit the
      same "create new term, reattach name in this scope" logic that already
      works at the top level. No `Assign`.
   e. At the end of the body, collect the latest term for each carry name
      and stash them as the loop body block's `carries_out`.
   f. Wire the loop term's carry-in to the initial term (the pre-loop one)
      and the carry-out to the body's carries_out.
3. The evaluator (`rust/src/eval.rs::exec_for_loop` /
   `exec_while_loop`) needs an update:
   - At iteration start, write each carry-in into the body block's carry slot.
   - At iteration end, read the carry-out terms' registers and feed them to
     the next iteration's carry-in slots.
   - At loop exit, the loop term's result becomes a tuple/record of the
     final carry values (or for the single-carry case, just the value).
4. Reads of the loop result post-loop need to go through the loop term, not
   the original `let` term. The compiler must rebind the carry name to the
   loop term in the parent block after the loop body lowers.

**Multi-carry case:** loops that rebind multiple names. The cleanest model:
the loop term's result is a tuple, and each post-loop read pulls a different
field. Or: emit one fresh "extract carry N" term per name, all reading from
the loop term. This keeps the dataflow graph clean.

**Edge cases:**
- `break`/`continue`: carries are the body's last assignment to each
  carried name before the break point. `break` short-circuits the body, so
  whatever the carry was at the break point becomes the final carry.
- Nested loops with shared carry names: each loop introduces its own carry
  scope; the inner loop's carries don't leak to the outer.
- `while` loops: same as `for`, but the loop variable is absent. Carries
  work identically.
- A loop that doesn't rebind anything: no carries, no IR shape change. The
  common case (drawing/printing inside a loop) stays simple.
- A name read inside the loop but never rebound: stays as a normal capture
  from the parent block, no carry needed.

**Verify:**
- `petal show-ir -e 'let total = 0; for x in [1,2,3] { total = total + x }
  print(total)'` shows no `Assign` and a loop term with explicit carries.
- `petal explain --term total` shows: the initial term, the per-iteration
  body terms (one per iteration), and the loop term as the post-loop value.
  This is what `history` is faking today.
- A new test: "no `Assign` op in compiled IR for any of the example
  programs."
- All existing example tests still pass.

**Estimated time:** one day. The compiler pass is the meat; the evaluator
update is straightforward; the IR shape change ripples through serializer
and tests.

---

### Step 4 — Delete `TermOp::Assign`

**Goal:** with steps 2 and 3 complete, nothing emits `Assign`. Delete it.

**Why:** the philosophy isn't real until the primitive is gone. As long as
`Assign` exists in the IR, it can be reintroduced, and users/maintainers
will think the language has mutation.

**Files to touch:**
- `rust/src/program.rs:111` — delete the variant.
- `rust/src/eval.rs` — delete the `TermOp::Assign(...) => { ... }` arm in
  `exec_term` (around line 520) and the helpers it uses.
- `rust/src/ir_display.rs` — delete the `Assign(t{})` formatting.
- `rust/src/ir_serialize.rs` — delete any custom serialization paths.
- `rust/src/compiler.rs` — confirm no construction sites remain.
- Any test or example that explicitly uses Assign in its expected IR shape.

**Verify:**
- `cargo build` compiles cleanly with no warnings about exhaustive matches
  on `TermOp` (Rust will surface all the affected `match` arms).
- `cargo test` and `npx vitest run` both pass.
- `grep -r 'TermOp::Assign' rust/src/` returns zero results.

**Estimated time:** 1 hour, bounded by however many tests depended on the
existence of Assign in IR snapshots.

---

### Step 5 — Collapse `history` into `explain`

**Goal:** with `Assign` gone, `trace::TraceBuffer::history()` is dead code.
Every term in the trace is its own thing; every rebinding is a normal term
event. `explain` walks provenance and reads recorded values; that's
sufficient. Delete `history`, `HistoryEntry`, `HistoryKind`, and the History
section in the `petal explain` CLI output.

**Why:** keep one mental model in the user-facing tools. Two views ("history"
and "provenance") existed only because of `Assign`'s split between
"writes" and "reads."

**The new explain behavior:**

For a name like `total` that gets rebound four times in a loop, `petal
explain --term total` should show:
- The latest term named `total` (the loop term)
- Its inputs (the carry-out of the body, the initial value, the iteration
  list)
- Walking back through provenance, the per-iteration body terms and their
  inputs
- All the way back to the initial `let total = 0`

This is exactly what `explain` already does for non-mutated values. After
step 3, mutated values *become* non-mutated values in the graph, so the
existing code Just Works.

**Files to touch:**
- `rust/src/trace.rs` — delete `history()`, `HistoryEntry`, `HistoryKind`.
- `rust/src/cli.rs::Command::Explain` execute branch — delete the History
  section in both text and JSON output.
- `ts/tools/petal-mcp.ts::ExplainTerm` — no change needed; the JSON shape
  shrinks but the tool description still applies.
- `docs/debugging-visibility.md` — remove any mention of "history of writes."

**Verify:**
- `petal explain --term total` on a loop accumulator now shows a chain that
  includes every iteration's body term and the loop term. The values are
  visible because the trace recorded them.
- All tests pass.
- The MCP tool still returns useful output.

**Estimated time:** 1 hour.

---

## Total estimate

- Step 1: 1 hour (docs)
- Step 2: 4 hours (if/else)
- Step 3: 8 hours (loops)
- Step 4: 1 hour (delete Assign)
- Step 5: 1 hour (collapse history)

**Total: ~2 days of focused work.** Each step lands on its own; the
philosophy migration can pause between any two steps without leaving the
codebase in a broken state.

---

## Order rationale

- **Step 1 first** because it costs nothing and primes the user/agent mental
  model. Future steps make more sense to a reader who has internalized the
  vocabulary.
- **Step 2 before step 3** because conditionals are simpler and prove out
  the phi-term approach. Loops reuse the same idea with a control-flow
  twist.
- **Step 4 immediately after step 3** because the philosophy isn't real
  until the primitive is gone.
- **Step 5 last** because it deletes user-facing behavior. Doing it before
  step 3 would leave loop-rebound variables un-debuggable.

---

## Risks

1. **Loop carry inference is subtle.** A name read inside a loop and never
   rebound is *not* a carry — it's a capture. Getting this wrong creates
   spurious carries and changes program behavior. Mitigation: the detector
   only promotes names that are *both read and rebound* inside the body.
2. **Multi-carry tuples are awkward.** A loop that rebinds three names
   needs three post-loop reads. Either model the loop result as a tuple
   (clean) or emit per-carry extract terms (also clean, slightly more IR).
   Either is fine; pick one and stick with it.
3. **Phi-term semantics in the evaluator.** The phi term needs to know
   which branch executed so it picks the right input. Today the evaluator
   already tracks branch results via register writes; the phi term reads
   from whichever branch's last term wrote. The cleanest implementation
   makes the phi an explicit term that the if/match emits.
4. **Hot reload state preservation.** Petal's hot-reload tries to keep state
   across reloads. State is keyed by `StateKey`; that's separate from
   register-level rebinds and shouldn't be affected. Worth confirming during
   step 3.
5. **Backwards compatibility.** Users with `.ptl` files containing
   imperative-shaped loops should see no behavior change. The only visible
   difference is that `petal show-ir` now shows a cleaner graph.

---

## What "done" looks like

After all five steps:

```bash
$ grep -rn 'TermOp::Assign' rust/src/
# (no results)

$ ./ts/bin/run-petal.ts explain --term total -e 'let total = 0
for x in [1, 2, 3] {
  total = total + x
}
print(total)'

Explain t81 (total):
  Provenance chain:
    => t81 total [line 5, column 7] = 6           <- the loop term, post-loop
     . t78 - [line 3, column 11] = 6              <- iteration 3 body result
     . t78 - [line 3, column 11] = 3              <- iteration 2
     . t78 - [line 3, column 11] = 1              <- iteration 1
     . t68 total [line 1, column 13] = 0          <- initial value
```

One unified view. No mention of "writes." Every value visible. Pure
dataflow.

The trace buffer continues to record term events; it just doesn't need a
special-case path for mutations because there aren't any.

The compiler is shorter (no Assign emission paths). The evaluator is
shorter (no Assign exec branch). The IR is one variant smaller. The
philosophy is the implementation.

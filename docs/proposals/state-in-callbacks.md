# Proposal: `state` in higher-order callbacks is an error

Status: implemented, 23 October 2026: the VM rule and its error in 41b0b961,
the `petal check` pass in 4cf32b48, native `any`/`all`/`find` in be2609c2.
The user docs are
[inline-state.md, State is not allowed in callbacks](../features/inline-state.md#state-is-not-allowed-in-callbacks)
and the runtime design is [state-call-paths.md §3.3](../dev/state-call-paths.md#33-vm).
Where the implementation differs from the text below, a note says so.

Decided by the language owner, 23 October 2026: `any`/`all`/`find` become
native and error like `count` (question 1); the provable cases are reported by
`petal check` (question 2); `run_state` is deferred (question 3). Question 4,
the scope of "callback", is decided: builtins only for this phase. The
owner's first choice was all callbacks, but the measurement in
[Scope: all callbacks](#scope-all-callbacks-measured) found that it breaks
petal-ui's container widgets. A later phase may revisit it.

## Summary

Before this change, a `state` declaration reached from a callback of `map`,
`filter`, `reduce`, `sort`, `sort_by` or `forEach` got **one slot shared by
every element** of that builtin call (the old "Callbacks run at the builtin's
callsite" section of inline-state.md, and
[state-call-paths.md §3.3](../dev/state-call-paths.md#33-vm)). The callback is
a step in a computation over a list, not a place in the program, and a slot
that silently threads a value from one element to the next is a hidden
accumulator. This proposal removes that behaviour: **executing a `state` or
`state var` declaration while a higher-order builtin's callback is running is
a runtime error**, with a hint that points at `for` (one slot per element) or
at `reduce`'s accumulator (a value carried across elements).

The second question was whether to add a construct that runs a stateful
function with its state passed in and returned as an ordinary value (a
`runState`). **Decided: defer it.** Nothing in the ecosystem needs it
today, and `for` loops and `reduce` already cover what it would be used for.
A design is sketched below so it can be picked up if a use appears.

**Recommendation: adopt the error, without a warning period.** A prototype
breaks no application, example, game or Garden code in any project that uses
Petal. It adds no measurable cost.

## Motivation

The current rule surprises people in two ways:

- `map(xs, f)` with `f` stateful gives a different result from
  `for x in xs do f(x) end`. The loop gives each element its own slot. The
  `map` call gives every element the same one.
- Within the standard library, the same predicate behaves differently
  depending on which helper calls it. `any`, `all` and `find` loop in Petal,
  so each element gets its own slot. `count`, `min_by`, `max_by`, `top_k`,
  `group_by` and `flat_map` go through `map` or `filter`, so every element
  shares one slot (measured, see "Before and after" 3).

What the corpus shows (static sweep of every tree listed in `CLAUDE.local.md`,
plus a runtime A/B with a prototype):

| Tree | `.ptl` files | Higher-order calls with a function argument | Callbacks that reach `state` |
|---|---|---|---|
| `~/petal` (examples, test, integrations, core, garden) | 405 | 331 | 0 |
| `~/cheesecake` | 375 | 597 | 0 |
| `~/worlds-fair/ui/ptl` | 20 | 1 | 0 |
| `~/petal-creative`, `~/tools` | 6 + 9 | 2 + 2 | 0 |
| `~/.garden`, petal-lang.org, experiments | 71 + 5 + 2 | 0 | 0 |

- The sweep closes over the call graph, so it also counts callbacks that reach
  `state` through a petal-ui widget (`button`, `checkbox`, `slider`,
  `text_field`, …). Name resolution is a superset, so the risk is false
  positives, not misses. It does not trace closures held in variables.
- Lists of stateful widgets are always rendered with `for`, roughly 100+
  sites to 0. `map` over stateful widgets is not an idiom anywhere.
- So there are no latent bugs to find (rows sharing hover state), and nothing
  relies on the shared slot. The behaviour exists only in:
  - its documentation, `inline-state.md:160-177`;
  - the test that pins it, `core/tests/state_call_paths.rs:796`
    (`a_closure_run_by_map_shares_one_slot_across_the_elements`);
  - a C-bridge test fixture that happens to use `map`
    (`integrations/petal-c-bridge/src/vm.rs:3011`, `tests/bridge_tests.cpp:891`);
  - the static-fn fuzz generator (`core/src/backend/bytecode/static_fn_fuzz.rs:366,502`).

The case for removing it is therefore about the language's rules, not about
existing bugs. `state` means "this value belongs to a place in the program
and persists across runs". A builtin callback has no place of its own, so the
current rule invents one by sharing.

## Design

### The rule

A `state` or `state var` **declaration** (its `StateInit`, and reads and
writes of that slot) executed while a higher-order builtin's callback is on
the stack is a runtime error. That covers the callback body, functions it
calls (transitively), and loops inside it.

Not affected:

- Reading top-level `state`, or `set` on a top-level `state var` cell, from a
  callback. These name one slot that is shared on purpose, not one that the
  callback creates. `integrations/petal-desktop-sdl/examples/invaders.ptl:207`
  and `platformer.ptl:175` do this (`set score += 1` inside `map`), and the
  prototype leaves them working (verified).
- `memo` inside a callback: it is keyed by value, not by path.
- Host calls (`Env::call_function`, the C bridge): these keep the host
  callsite rule ([state-call-paths.md §2.5](../dev/state-call-paths.md#25-host-entry-points)).
- `state` in a function called from a `for` loop, which keeps its slot per
  iteration.

### Which builtins

The six that drive a Petal closure synchronously: `map`, `filter`, `reduce`,
`forEach`, `sort_by`, and `sort` with a comparator. These are the only Rust
callers of `Vm::call_closure_sync` apart from the host path. petal-ui and the
other native libraries never call back into Petal (checked). As implemented,
`any`, `all` and `find` are natives too (below), so the list is nine, kept in
one table, `callback_intrinsic` in `vm/native.rs`.

### The error

Shown at the `state` declaration, naming the builtin and the user's callsite.
As implemented (the proposal's draft had a `-->` location line and the label
text under the carets; the shape below is the VM's existing error rendering):

```
Error: `state n` cannot be used inside a callback of `map` [line 2, column 5]
  |
2 |     state n = 0
  |     ^^^^^^^^^^^
note: `map` runs the callback here, once per element, at line 6, column 21
  |
6 | fn apply_all(xs, f) map(xs, fn(x) f(x) end) end
  |                     -----------------------
help: state needs a fixed place in the program, and a callback has none.
      For a value per element that persists across runs, use a loop:
          for x in xs do f(x) end   (add `key …` to follow items)
      For a value carried from one element to the next, use reduce's accumulator.
```

When the builtin call is inside a prelude function (`count` calls `filter`),
the note points at the user's call into the prelude (`` `count` runs the
callback through `filter` here ``), not at `std.ptl`. "Prelude" is any module
the entry file imports implicitly, so a host's prelude counts too. The help
text names the loop's call `f(x)`, not the callback's actual name.

### Standard library consistency

`any`, `all` and `find` should raise the same error as `count`. Otherwise a
stateful predicate works through `any` and fails through `count`. Two ways to
do it:

1. Make them native intrinsics that short-circuit (`builtin_any`, …), driven
   by `call_closure_sync` like `filter`. This is also faster.
2. Keep them in Petal and mark the bundled prelude's `pred(x)` calls as
   callback calls.

Decided: 1, which needs no new compiler concept.

### Reported by `petal check` (decided)

The runtime error is authoritative. `petal check` also reports the cases that
are provable without running anything:

- a lambda literal passed to a callback builtin whose body declares `state`;
- a function passed by name that declares `state`, directly or through
  statically resolved calls.

It is a compiler pass, `core/src/compiler/callback_state.rs`, a sibling of
`keyed_state.rs`. Its findings go on `Program::warnings` with error severity,
so `petal check` fails on them, `petal run` refuses to start, and the LSP
shows them, all through the existing check path (`core/src/check.rs`).

- **Error, not warning:** a provable case fails whenever it runs, the same
  reasoning `check` already applies to calls to undefined names.
- **A live-reloading host** still prints the error and runs the script
  anyway, as with any check error.
- **The call graph**: as implemented, the pass reads the compiled IR and
  closes "declares state" over the statically resolved calls itself
  (breadth-first, so the shortest chain is reported), rather than reusing
  `core/src/static_classify`. A function that hands its own parameter
  straight to a callback builtin makes that parameter a callback slot too,
  which is how `count`, `min_by`, `group_by` and user wrappers are reported
  at the user's line. The method form `xs.map(f)` is covered when nothing
  else in the program could take the call.
- **Callbacks that arrive through a parameter or variable** are left to the
  runtime error. The pass reports only what it can prove, so it has no false
  positives. One gap found in implementation: `top_k(xs, k, stateful)`,
  whose `key` parameter has a default, is left to the runtime error too.
- **The message** is one line with the rule id, for example:
  `` `state n` cannot be used inside a callback of `map`: `map` runs `seen` on line 6, once per element. State needs a fixed place in the program, … [state-in-callback] ``.

## Before and after

All "before" snippets were run with a release build of `main`. All "after"
snippets were run against the prototype (patch in the session scratchpad,
below) except where marked "proposed".

### 1. The documented demo (the case this proposal removes)

Before, `docs/features/inline-state.md:167`:

```petal
fn seen(x)
    state n = 0
    n += 1
    n
end
print(map([10, 20, 30], seen))   // [1, 2, 3]
```

After, the same code is an error. `petal check` can prove it, so the program
does not start:

```
error: `state n` cannot be used inside a callback of `map`: `map` runs `seen` on line 6, once per element. State needs a fixed place in the program, and a callback has none. For a value per element that persists across runs, use a loop: `for x in xs do f(x) end` (add `key …` to follow items). For a value carried from one element to the next, use reduce's accumulator. [state-in-callback]
 --> [line 2, column 5]
  |
2 |     state n = 0
  |     ^^^^^^^^^^^
1 error found by check; the program was not run
```

Written as intended:

```petal
fn seen(x)
    state n = 0
    n += 1
    n
end

// a value carried across elements: the accumulator
print(reduce([10, 20, 30], [], fn(acc, x) append(acc, len(acc) + 1) end))  // [1, 2, 3]

// a value per element that persists across runs: a loop
print(for x in [10, 20, 30] do seen(x) end)   // [1, 1, 1] on the first run
```

For the writer: one less rule to learn ("callbacks share a slot") and one
error in its place. For the reader: a `map` over a list is a pure transform
again, so it can be read without looking for `state` in the callee.

### 2. A test fixture that used `map` by accident

Before, `integrations/petal-c-bridge/src/vm.rs:3011`:

```petal
fn bump(k)
    state count = 0
    count += 1
    count
end
let bumped = map(["a", "b"], fn(k) bump(k) end)
```

After (proposed):

```petal
let bumped = for k in ["a", "b"] do bump(k) end
```

The fixture only needs some state inside a function. The test's slot count
(`report["slots"] == 8`) becomes 9: two slots, one per iteration. The same
fixture is in `bridge_tests.cpp:891`. The vendored copies in
`~/cheesecake/third_party/petal` and `~/worlds-fair/vendor/petal` change only
when those projects re-vendor.

### 3. The standard library (where the change makes things consistent)

Before (both run on `main`):

```petal
fn picky(x)
    state calls = 0
    calls += 1
    x > 1
end
print(any([1, 2, 3], picky))     // true: a slot per element
print(count([1, 2, 3], picky))   // 2: one shared slot
```

After, with the prototype, `any` still ran and `count` errored at
`std.ptl:67`. As implemented, `petal check` reports both at the declaration,
citing the user's lines (`` `count` runs, through `filter`, `picky` on line
7 ``), and at run time both error with the note on the user's line.

### 4. Where the old form stays right: shared cells from a callback

`integrations/petal-desktop-sdl/examples/invaders.ptl:207` writes a top-level
`state var score` with `set` inside `map`. This is unchanged and still the
right form: the sharing is declared at the top level and is visible at every
`set`.

### 5. Hot reload

A live edit that adds `state` to a helper that some `map` calls makes the
next run fail with the error at the new line. That is the same experience as
any other runtime error after a reload: the host shows the error, and the
previous state map is untouched because the run aborts before the sweep.
Nothing is half-written (checked in the prototype). Removing the `state`
again recovers. No reload rule changes, because slots under a callback path
no longer exist to transfer.

### Migration

There is nothing to migrate in application code. Four test or fixture sites
are rewritten by hand (listed under Implementation). No automatic fix is
worth building for zero sites.

## Assessment

### Project goals

**Findings.** The change removes a special case from the state model.
Afterwards a slot exists only where the program has a place (callsite, loop
iteration, keyed item), which is the property hot reload and the JSON state
dump (`counter/n`, `[3]/row/hovered`) rely on. Before this change a callback slot rendered
under the builtin's callsite (`map/n`), a path that does not correspond to
anything the author wrote per element.

**Opinion.** This fits "first-class state" better than the current rule: the
model becomes "state lives at places", with no exception.

### Ease of writing

**Findings.** A newcomer coming from JavaScript expects `xs.map(f)` and a
`for` loop to do the same thing. React's Rules of Hooks forbid hooks in
callbacks for the same reason, and its linter enforces that. The error turns
a silent difference into a message that names the fix.

**Opinion.** It is a net reduction in what has to be learned. The cost is one
more error that someone porting loop code to `map` can hit. The hint has to
be good, because two different fixes (`for` or `reduce`) apply depending on
intent.

### Bug-freedom

**Findings.**
- It prevents rows rendered through `map` from sharing one widget's state.
  That bug does not exist in the corpus today, but only because authors
  already use `for`.
- It prevents a hidden accumulator whose value also depends on the previous
  run, because the slot persists. `seen` above returns `[4, 5, 6]` on the
  second frame.
- It creates no silent failures, since the result is a loud error.
- The differential oracle and golden corpus are unaffected: the prototype's
  `verify.ts --plan compiler` A/B over 152 runnable files was trace-identical.

**Opinion.** A clear gain, for a pattern that is currently rare.

### Performance

**Findings.** `petal bench`, release builds, 5 alternating runs:

- `state_heavy` (20k state read/write calls): 14.09–14.90 ms before,
  14.20–15.05 ms after.
- `map_heavy` (2k small maps): 1.103–1.136 ms before, 1.098–1.128 ms after.

Both are within noise. The prototype's cost is one counter around each
builtin call and one predictable branch on each state instruction. No
optimizer analysis changes. Escape analysis (§3.7) only considered empty-path state anyway.

### Live coding and hot reload

Covered in "Before and after" 5. The reloaded program still equals a fresh
compile, because the rule is runtime-only and depends on no reload history.

### Introspection, debugging and program manipulation

**Findings.** The change removes one source of hard-to-explain values: a
callback slot's value depends on how many elements ran before it, on earlier
runs, and on sort's comparison order. That order is an implementation detail
of the merge sort, which makes a stateful comparator's results unexplainable.
Provenance, slicing and time travel lose nothing.

### Maintainability

**Findings.** The change touches the VM only: about 25 lines when cleaned up
(`stack.rs`, `vm/native.rs`, `vm/dispatch.rs`), plus the error rendering. The
prototype is about 75 lines, with a duplicated dispatch helper. It removes a
rule from `state-call-paths.md` (§3.3 "Intrinsic closures get the
intrinsic's callsite") and the open item in §5. It adds one invariant: any
future native that drives a Petal callback must be listed in
`callback_intrinsic` (`vm/native.rs`), so that it runs under the record. There is precedent for an error scoped to a callback:
`core/src/native_fn.rs:1422`, "a test cannot wait inside a callback of map,
filter, reduce, forEach or sort".

### Domains

| Domain | Effect |
|---|---|
| Graphics and creative coding | Neutral. No callback state in petal-creative. |
| Games | Neutral. Cheesecake has 597 callback calls and 4 in-function `state` declarations, none reached. The invaders and platformer `set score` pattern is kept. |
| Productivity and UI apps | Helps a little: rows rendered through `map` can no longer share widget state. Nobody does that today. |
| Dashboards and data | Neutral to positive: `map`/`filter`/`reduce` pipelines are guaranteed free of `state` in their callbacks. |
| Web, editor scripting (Garden, `~/.garden`), embedding hosts | Neutral. No uses. Host calls are unchanged. |

### Compatibility

There are no application breakages in any tree. Two core tests fail (one by
design, one fuzz threshold: "44/60 kept values" against 45, because the
generator emits `map` over stateful functions). The C-bridge fixture changes
its slot count, and the vendored copies follow on re-vendor. A warning period
buys nothing when there are zero users, so ship the error directly.

### Prior art

- **React**: hooks may not be called inside callbacks, loops or conditions.
  This is enforced by `eslint-plugin-react-hooks` statically and by a runtime
  "rendered more hooks" error. Lists of stateful children use an extracted
  component plus `key`, which is Petal's keyed `for`.
- **Jetpack Compose**: `remember` inside `forEach` works only because
  `forEach` is inlined, so the call is positional, like Petal's `for`. Lazy
  lists use `key`.
- **Lustre and Scade**: `map<<node, N>>` creates one node instance per
  element, so each element gets its own state. This is the "per-element
  slots" alternative below. It works there because arrays have a static size.
- **Haskell**: `runState :: State s a -> s -> (a, s)` is the construct in
  question 3.

## Alternatives considered

1. **Do nothing** (keep the shared slot and its doc). It is harmless because
   nobody uses it, but it remains a documented rule that contradicts what
   `state` means and makes `count` and `any` disagree.
2. **Per-element slots** (Lustre-style: push an `Index` part per element in
   `builtin_map`). This makes `map` behave like `for`. It is rejected because
   it endorses state in callbacks: `filter` and `sort` call the callback a
   data-dependent number of times in a data-dependent order (merge-sort
   comparisons), so "per element" is ill-defined for them. There is also no
   key form. `docs/tasks/state-loop-keys.md` open question 4 can be closed
   with "no".
3. **Lint or warning only.** It costs the same to build as the error and
   leaves the rule in place.
4. **Compile-time error only.** It cannot see callbacks passed through
   variables or parameters. It is useful as an addition to the runtime error
   (see "Reported by `petal check`"), not as a replacement.

## Scope: all callbacks (measured)

The owner's first answer to question 4 was that the rule should cover every
callback, not only the six builtins. Taken literally: `state` is an error
anywhere below a call whose callee is a function value (a parameter, a local,
a closure field, `fs[i]()`), as opposed to a named function. A second
prototype measured that (`proto-dyn.patch` in the session scratchpad).

**How it was built.** The bytecode cannot tell `f()` from `widget()`, because
both are `call rN` after copy propagation. So the compiler marks dynamic call
terms: callees bound as values (params, in-function `let`, `var`, loop and
pattern variables) and non-name callees. The VM flags frames pushed by those
calls, and by `x.f()` when it resolves to a closure field. Named functions,
`m::f`, constructors, `let`-bound lambdas and module-level aliases count as
static.

**Measured.**

| | Builtin-only rule | All-callbacks rule |
|---|---|---|
| Dynamic call sites in the corpus | — | petal 32, cheesecake 24, all others 0 |
| Corpus code broken | none | `examples/ui/layers/app.ptl:110,113` |
| petal-ui tests failing | 0 | 8 (nested `cached_layer`; layers app parity tests) |
| core tests failing | 2 (artifacts) | 3 (adds `state_decl_ids::unbound_lambdas_do_not_collide_with_each_other`) |
| Places where two executions shared one slot (`PETAL_DUP_STATE`, 298 runnable files) | 0 | 0 |
| Cost on `state_heavy` | noise | ~2.5% (frame walk; an inherited flag would remove it) |

**What breaks is petal-ui's container widgets.** `with_layer(n, body)`
(`ui.ptl:824`), `ui_measure(body)` (`:982`), `ui_overlay`, `layer` and
`cached_layer(key, r, body)` (`:1141`) all take a body closure and call it
once. Under the broad rule:

- the body could not contain any stateful widget: `button`, `tooltip`,
  `slider`, `text_field`, the animation helpers;
- `cached_layer` could not be nested, because it declares `state id` and
  `state seen` itself and then runs `body()`;
- `examples/ui/layers/app.ptl` measures and draws a popover panel of buttons
  this way.

The rewrite would be bracket calls (`let prev = ui_layer(10) … ui_layer(prev)`,
begin/end pairs for measure and cache). These lose `ui_measure`'s re-run and
nested caching.

**Why these callbacks are not the problem.** A body closure called once
already has a stable place. Its path is the container's callsite, then the
`body` callsite inside the container, then the closure's own declarations.
That is exactly as stable as a named helper. Callbacks run in a user-written
`for` loop get per-iteration parts like any other call. The "one slot shared
by many executions" defect exists only where a native drives a callback
repeatedly with no per-element part, which is the six builtins. The
duplicate-slot detector found no other instance.

**Narrower definitions that were considered.**

- "Builtins plus callbacks called by Petal library helpers": essentially the
  same breakage, because the library helpers that call callbacks are the
  container widgets.
- "Callbacks invoked inside loops": these already get per-iteration keys, so
  there is nothing to fix.

**Opinion.** The evidence supports the builtin-only rule, with `any`, `all`
and `find` made native (decision 1) so that "builtin" covers every list
helper. The broad rule forbids a correct, documented idiom in order to catch
a defect that does not occur outside the builtins.

A separate oddity remains. A closure stored and run later (bloom's
`defer_paint`, cheesecake's command registries) keys its state under the path
of the code that runs it, not the code that created it. There are no
instances today, and the builtin rule does not address it. If it ever
matters, a runtime duplicate-slot diagnostic (the `PETAL_DUP_STATE` detector,
productized) targets it without false positives.

## Question 3: a construct that turns state into a value

What was asked for: call a stateful function "non-statefully", with its state
going in as an argument and coming back out as a value. A minimal design,
**deferred** (decided 23 October 2026):

```petal
// proposed, not implemented
let r = run_state(seen, nil, 10)        // nil: a fresh state
// r.value == 1, r.state is an opaque state bundle
let r2 = run_state(seen, r.state, 20)   // r2.value == 2
```

- `run_state(f, st, ...args)` calls `f` with a **detached** state store,
  seeded from `st`. Paths are relative to the `run_state` call, so a bundle
  can be passed to any call. It returns `{value, state}`. Slots that this
  call did not touch are dropped from the returned bundle, matching a run's
  sweep.
- Nothing touches the program's own state map, so `run_state` may be called
  from a callback. The callback rule still applies inside it.
- The open choice is what a bundle is. An opaque value is simple and hides
  path hashes. A `Map` keyed by rendered path (`n`, `row/hovered`) is
  inspectable, but rendered labels are display-only and not unique (§3.6).

Why defer it:

- **No demand.** There are zero sites in the corpus. The cases it would serve
  (carry across elements, persist per element) are already `reduce` and
  `for`.
- **Cost.** It needs a second state namespace on `Stack`. Touch capture for
  memo scopes, the sweep, pending initializers and `state_json` would all have
  to know about it. That is a larger change than the error itself.
- **Clumsier than the alternatives where it would apply.** Threading
  `r.state` through `reduce` is longer than the `for` loop it replaces.

Real reasons that could revive it: unit-testing a stateful widget from a
chosen starting state, or running N independent copies of a stateful
simulation stored in a list. If either appears in practice, build this design
then. It fits the introspection goal: state becomes a value that tools and
tests can hold.

## Implementation sketch

Order, each step green before the next:

Done as described, except where noted: steps 1, 2 and 4 in 41b0b961, step 3
in be2609c2, step 6 in 4cf32b48, step 5 with the docs commit.

1. **VM rule.**
   - Add a `callback_depth` counter and an innermost-builtin record on
     `Stack`. As implemented there is no counter: `Stack::callback` holds the
     innermost `BuiltinCallback` (name, call term, frame count), saved and
     restored around each builtin, which also handles nesting.
   - Wrap the six intrinsic dispatches in `vm/native.rs` with them.
   - Check the record in `StateInit`, `StateRead` and `StateWrite`
     (`vm/dispatch.rs`), skipping `memo`.
   - Keep the record balanced on every error path.
   - Delete the `site` threading comment in `native.rs:218`. The site is still
     passed, because host calls use the same driver.
2. **Error rendering.** Annotate at the declaration's term, with a secondary
   label at the user's builtin callsite. Skip bundled prelude frames when
   choosing that callsite. As implemented, `StateInit` has no span in the
   source map, so `Vm::state_in_callback` rebuilds the declaration's span at
   error time; `errors.rs` gained `annotate_error_with` for the note and
   help.
3. **std consistency.** Make `any`, `all` and `find` native short-circuiting
   intrinsics, add tests that a stateful predicate errors through every std
   helper, and regenerate `docs/stdlib.json`. As implemented, the
   `stdlib.json` update was hand-merged, because the extractor currently
   drops the petal-ui input builtins. Being natives, the three also take
   method syntax (`xs.any(f)`).
4. **Tests and fixtures.**
   - Replace `a_closure_run_by_map_shares_one_slot_across_the_elements` with
     tests that it errors for each of the six builtins (including transitive,
     nested-loop and `state var` cases), and that `memo`, top-level reads,
     top-level `set` and host calls still work.
   - Fix the static-fn fuzz generator (no `map` over generated stateful
     functions).
   - Change the C-bridge fixture to a `for` loop and update the slot count
     (Rust and C++).
5. **Docs.**
   - Rewrite `inline-state.md` "Callbacks run at the builtin's callsite" as
     "State is not allowed in callbacks", with the two fixes.
   - Update `state-call-paths.md` §2.2a, §3.3 and §5.
   - Close `state-loop-keys.md` open question 4.
   - Add a CHANGELOG entry under breaking changes.
6. **`petal check`.**
   - Add `core/src/compiler/callback_state.rs` next to `keyed_state.rs`, built
     on a call-graph closure of "declares state".
   - Push its errors onto `Program::warnings`.
   - Tests: the provable cases are reported, a callback through a parameter is
     not, and the LSP shows the finding.
7. **Ecosystem.** Nothing to fix. Note in the release that cheesecake and
   worlds-fair pick up the change and the fixture edit on re-vendor.

Testing: `cargo test` across core, petal-ui, petal-query, garden and
desktop-sdl (the prototype ran all of them), the `verify.ts --plan compiler`
A/B over the corpus, and `petal bench` for `state_heavy` and `map_heavy`.

## Open questions

Decided on 23 October 2026:

1. `any`, `all` and `find` become native and error like `count`.
2. The provable cases are reported by `petal check`.
3. `run_state` is deferred.
4. The rule covers the callback builtins only for this phase:
   - `map`, `filter`, `reduce`, `forEach`, `sort_by`, and `sort` with a
     comparator;
   - plus `any`, `all` and `find` once they are native.

   Callbacks that user or library functions call (petal-ui's `body`
   closures) are not covered. See
   [Scope: all callbacks](#scope-all-callbacks-measured) for what a later
   phase would have to solve.

Left for a later phase:

- If the rule is ever widened, decide what to do with petal-ui's container
  widgets: rewrite them as begin/end pairs, or add an opt-in for functions
  that call their body once.
- Whether to productize the duplicate-slot detector for stored closures run
  later.

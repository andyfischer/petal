# `state` inside `map` and the other element-wise builtins

Status: plan, 2026-11-20. Not started.

Today `state` inside a builtin's callback is a runtime error
([state-in-callbacks.md](../proposals/state-in-callbacks.md)). Under this
plan, `map(xs, f)` with a stateful `f` behaves the way
`for x in xs do f(x)` already does: each element gets its own slots, keyed by
index, or by a key the caller gives.

## Why

The error replaced an older behaviour (one slot shared by every element) that
nothing in the corpus relied on. It has a cost:

- **Implicit widget identity.** The click-handling plan (petal-creative
  session, 2026-11-20) gives `interact` a stable identity from `stable_id()`,
  which is `state id = unique_id()` (core `5946a261`). Any widget drawn from
  a `map` callback would then become a runtime error, and so would every
  bloom control drawn that way. Building widget rows with `map` is ordinary UI
  code. The rule as it stands would push every stateful library to avoid
  `state` entirely.
- **Helpers stop being transparent.** A function that works in a `for` loop
  fails inside `group_by` or `flat_map`, with an error that points at code
  the user did not write.

## The rule after this change

A callback that a builtin runs **once per element, in element order** gets a
per-element part on its path, the way a loop iteration does. `state` in it,
and in everything it calls, is allowed and persists per element across runs.

| Builtin | Calls the callback | State |
|---|---|---|
| `map` | once per element, in order | per element |
| `forEach` | once per element, in order | per element |
| `sort_by` | the key function once per element, before sorting (`intrinsics.rs:417`) | per element |
| `reduce` | once per element, in order | still an error; second round |
| `any`, `all`, `find` | once per element until decided | still an error; second round |
| `filter` | once per element, in order | still an error; second round |
| `sort` with a comparator | once per **comparison**, data-dependent order | **still an error** |

**First round: `map`, `forEach` and `sort_by`'s key.** That covers the case
this plan exists for (widgets drawn from `map`) and every std helper but
`count`. `reduce`, `any`, `all`, `find` and `filter` keep today's error until
the first round has shipped; each one is the same change to one more
intrinsic.

Every std helper built on `map` follows automatically: `min_by`, `max_by`,
`top_k`, `group_by`, `unique_by` and `flat_map` (`core/prelude/std.ptl`).
`count` is `len(filter(xs, pred))`, so it stays an error with `filter`.

The comparator form of `sort` keeps today's error and message, because a
comparison has no element of its own and a merge sort's comparisons change
with the data.

In the second round, short-circuiting (`any`, `all`, `find`) behaves like a
loop that breaks early. The untouched-slot sweep at the end of a run drops
the slots of elements that were not reached, as it does for loop iterations a
`break` skipped. This goes in the docs, not around them.

### Keys

By default the part is the element's **index**, like `for x in xs`. Keying by
index means a slot follows the position: if the list reorders, slots stay put
and the items move. That is the same trade-off `for` makes, and the same
remedy applies, a key:

```petal
# per item, following the item when the list reorders
let rows = map(tasks, row, key: "id")                 # a field name
let rows = map(tasks, row, key: fn(t) t.owner.id)     # or a function

# today's equivalent
let rows = for t in tasks key t.id do row(t)
```

`key:` is a named argument that takes either form, as lodash's iteratee
shorthand does (`_.keyBy(xs, "id")` or `_.keyBy(xs, x => x.id)`):

- **A string** names a field: the key of element `x` is `x[name]`, read
  without calling into Petal. It works on records and maps. A missing field
  is an error naming the element's index and the field, as reading a
  missing field is anywhere else in Petal (no `?? nil` fallback, since a key
  that is silently `nil` for several elements shows up later as a duplicate
  instead). Only a single field name, not a dotted path: `"a.b"` is the field
  named `a.b`. A nested key takes the function form.
- **A function** of the element returns the key. It is called once per
  element before the callback, outside any per-element part, and may not
  declare `state` (the same error as today).
- **Anything else** (a number, a list, `nil`) is an error:
  ``map(): `key:` must be a field name (a string) or a function of the element``.
  A string is never callable, so the two forms can't be confused.
- The key value is hashed exactly as `for … key` hashes it, and the part is
  `PathPart::Key(h)`, so a `map` and a `for` over the same items with the same
  key build slots of the same shape.
- **Duplicate keys are an error**, with the keyed loop's wording
  (`Duplicate map key …`, state-call-paths.md §2). A silent duplicate means two
  elements share a slot, which is the bug the old rule had.
- A map (`Dict`) walked by `any`, `all` or `find`: key by the dict's key, not
  its position. That is free, since the key is what the walk visits, and it is
  stable when entries are inserted.
- `key:` is accepted by every builtin in the "per element" rows above. The
  first version can ship it on `map` and `forEach` only, if the others need
  more work; see the open questions.

## How it works

The work is in the VM's intrinsics. Normal Petal functions can't do this, for
two reasons: the builtin runs the callback from Rust through
`Vm::call_closure_sync`, and the path part has to sit in the callback's own
frame.

### Paths today

A frame's path (`vm/frame.rs`, `LocalPath`) is the parts it adds to its
caller's: `[Call(site)]` at push, then one `Index`/`Key` part per active loop,
set by `push_loop_idx`, `set_loop_idx_top` and `set_loop_key_top`, and popped
at `LoopPop`. A `state` slot's key is the declaration id under the
concatenated path (`Vm::state_key`).

A builtin's callback frame is pushed by `call_closure_sync(func, args, site)`
with path `[Call(site)]`, `site` being the `map(...)` call. Every element got
the same path, which is why the old rule shared one slot. Today
`Stack::callback` is set around the call (`vm/native.rs:308`), and every
`StateInit` checks it and raises `state_in_callback`
(`vm/dispatch.rs:608,632,709`).

### The change

1. **An element part on the callback frame.** Add
   `call_closure_sync_at(func, args, site, part: Option<PathPart>)`. After the
   frame's `Call(site)` it pushes `part` as the frame's first loop-shaped
   part, giving `[Call(site), Index(i)]` or `[Call(site), Key(h)]`. The
   callee's own loops push and pop above it. The pop guard in `pop_loop_idx`
   already refuses to pop a part the frame did not push, as long as the
   callee's `LoopPush`/`LoopPop` are balanced, which the compiler guarantees.
   The existing `call_closure_sync` becomes `call_closure_sync_at(.., None)`
   for the host path (`Env::call_function`), which keeps its rule.
2. **The intrinsics pass it.** `builtin_map`, `builtin_for_each` and
   `builtin_sort_by`'s key pass `Some(Index(i))`, or `Some(Key(h))` when
   `key:` was given. `builtin_sort_cmp` / `merge_sort_by` and `builtin_filter`
   pass `None` and stay under the callback record.
3. **The callback record for the rest.** `callback_intrinsic`
   (`vm/native.rs:341`) lists which builtins set `Stack::callback` around
   their whole run (`native.rs:308`). It gains a per-element flag. `map`,
   `forEach` and `sort_by` leave the record **as it was**, rather than
   replacing it or clearing it, so a `map` running inside a `sort`
   comparator or a `filter` predicate still errors: that outer callback has
   no element part, so the inner `map`'s slots would be shared across the
   outer calls. `reduce`, `any`, `all`, `find`, `filter` and comparator
   `sort` keep setting it, and so does the `key:` function's call. The hint
   in `state_in_callback` names `map` as the per-element form.
   `call_key_here` (`frame.rs:321`) already returns `None` under the record
   and otherwise keys by the full path, so native call keys become
   per-element with no change.
4. **Keys.** `map` and friends take `key:` through their `native fn`
   declarations in `core/packages/core/src/collections.ptl` (named-argument
   binding already exists, `bind_native_args`). One helper,
   `Vm::element_keys(name, elements, key) -> Vec<u64>`, resolves both forms:
   a string reads each element's field on the heap (records by field, maps by
   key) with no Petal call; a callable goes through `call_closure_sync_at(..,
   None)` under the callback record. Either way it hashes with
   `dict::key_hash`, as `Inst::LoopKey` does (`dispatch.rs:154`), records the
   label in `Stack::loop_key_labels` for state dumps, and checks for
   duplicates with the keyed loop's wording. It computes all keys first, then
   runs the callbacks. Computing keys first means a
   duplicate is reported before any element's state is touched.
5. **The fast path.** `vm/fast.rs` has its own `set_loop_idx`
   (`fast.rs:534`). Check whether a frame pushed by `call_closure_sync` can
   enter `run_straight` with a loop part already on its path. If it can,
   `run_straight` must treat that part as below its own loops. A test
   (`state_call_paths.rs`) pins this, with a stateful callback whose body
   contains its own loop.
6. **Speculation.** `vm/spec.rs:426` walks path parts. Element parts are
   ordinary `Index`/`Key` parts, so it should need nothing. A test runs
   `speculate` over a `map` with state.
7. **Call memo.** `calls.rs:732` builds a memo scope's path from the callee
   frame's. A memoized function called from a `map` callback must see the
   element part, or two elements would share a memo entry. Test it.

### Rust callers

When Rust calls Petal, it can say which state the call runs under and manage
that state. The case: a native widget (a table, a tree view, a virtualized
list) drives a Petal callback per row, and each row's `state` and
`stable_id()` must persist per row, follow the row when rows reorder, and go
away when the row is deleted.

Today `Env::call_function(stack, name, args)` (`env/run.rs:453`) keys the
call's state under a root path made from the function's name
(state-call-paths.md §2.5), so every call shares one set of slots.

**A state scope type.** A public, opaque `StatePart` in `crate::stack`,
built only through constructors, so hosts never touch the hashing:

```rust
StatePart::index(3)                     // a position, like a `for` iteration
StatePart::key(42)                      // the part `for … key 42` makes
StatePart::key("row:42")                // the part `for … key "row:42"` makes
StatePart::key_value(&env, v)?          // any Petal value; errors if it can't be a key
```

- `key` takes `impl Into<KeyInput>`, implemented for the integer types,
  `&str`, `String` and `bool`. It can't fail and needs no `Env`: strings and
  numbers hash by content alone (`value::hash_into`, `value.rs:822`: a type
  tag, then the bytes or `num_key`), so `KeyInput` reproduces that hash in
  Rust. A test pins `StatePart::key("x")` and `StatePart::key(7)` to the
  parts Petal's `for … key "x"` / `key 7` build.
- `key_value` is for a value the host got from Petal (a record's id). It
  goes through `dict::key_hash`, so a function, a pending value or NaN is
  refused as a keyed loop refuses it.
- Both record the key's label in `Stack::loop_key_labels`, so state dumps
  show `"row:42"` the same way whichever constructor made it.
- `index(3)` and `key(3)` are different parts, as `for x in xs` and
  `for x in xs key 3` are.

**Calling under a scope.**

```rust
// state under [host root of `name`, ...parts]
env.call_function_at(stack, name, args, &[StatePart::key(&env, row_id)?])?;
// the same, for a closure value the host holds (a callback it was handed)
env.call_closure_at(stack, callable, args, &[StatePart::index(i)])?;
```

Both go through `call_closure_sync_at`, which pushes the parts after the
frame's `Call` part, as the intrinsics do. A duplicate within one host
"pass" is the host's business. The host can ask for the same checking with
`env.begin_keyed(stack)`, which returns a guard whose `call_at` refuses a
repeated key, with the loop's wording.

**Managing the state.**

```rust
env.state_under(stack, name, &parts) -> Vec<(String, Value)>   // read, for inspectors and tests
env.clear_state_under(stack, name, &parts)                      // a row was deleted: drop its slots now
env.keep_state_under(stack, name, &live: &[Vec<StatePart>])     // drop every scope under `name` not in `live`
```

`keep_state_under` is the host's version of the untouched-slot sweep. A
host-driven call isn't part of a script run, so the run-end sweep can't tell
a row that is gone from one that just wasn't drawn this time. The host says
which rows are live, the way a keyed loop does implicitly.

**From a native function.** Natives can't call back into Petal today: only
the VM's intrinsics drive closures (state-in-callbacks.md, "Which
builtins"). A native that takes a callback (`table(rows, fn(row) …)`) would
need `PetalCxt::call_at(callable, args, part)`, which runs the closure on the
current stack under the caller's path plus `part`. That is the larger piece
of work, since it lets natives re-enter the VM. It is listed as a separate
step below, and the `Env` API above doesn't wait for it.

### Compile-time pass

`core/src/compiler/callback_state.rs` (`petal lint`, rule
`state-in-callback`) reports provable cases ahead of time. It narrows the same
way: report only a callback of `filter` (and so `count`), a comparator passed
to `sort`, or a `key:` function that reaches `state`. The lint's tests and `docs/` entries change with it.

## Tests

New, in `core/tests/all/state_call_paths.rs` or a new `state_in_map.rs`:

- `map(xs, f)` with a stateful `f`: each element keeps its own value across
  three runs, and the values equal those of `for x in xs do f(x)`.
- `key:`: a reorder between runs keeps each item's value, with `key: "id"`
  and with `key: fn(t) t.id`, and the two forms build the same slots (and
  the same slots as `for t in xs key t.id`). A new item gets fresh state,
  and a removed item's slot is swept.
- `key: "id"` on a list of maps (`Dict`), and on records; a missing field
  errors with the element's index; `key: 3` and `key: nil` error with the
  "field name or function" message.
- Duplicate keys raise the error before any callback runs.
- `reduce`, `forEach`, `sort_by`'s key and the std helpers (`group_by`,
  `flat_map`, `min_by`): one slot per element.
- `any` / `find`: elements past the deciding one are not reached, and their
  slots are swept, matching a `for` with `break`.
- Nested: `map` inside a `map` callback, and a `for` inside a `map` callback
  (paths `[Call, Index(i), …, Call, Index(j)]`, and
  `[Call, Index(i), Index(k)]`).
- `stable_id()` from a `map` callback: one id per element, stable across
  runs, and following items under `key:`.
- A stateful callback whose body has its own loop (the fast path).
- `speculate` over a stateful `map` restores every element's slot.
- Hot reload (`transfer_state`) keeps per-element slots, since they match on
  the declaration's base key.
- `sort(xs, cmp)` with a stateful `cmp`, and `filter` / `count` / `reduce`
  / `any` / `all` / `find` with a stateful callback, still error, and so
  does a stateful `map` inside a `sort` comparator or a `filter` predicate.
- Host: `call_function_at` with two keys keeps two sets of slots across
  calls; `clear_state_under` drops one; `keep_state_under` drops the
  others; `StatePart::key` refuses a function value.

Changed:

- `state_call_paths.rs` tests that pin the error for `map` and `forEach`
  become tests of the per-element behaviour. The ones for `filter`,
  `count`, `reduce` and `any` stay.
- `integrations/petal-c-bridge` (`src/vm.rs:3011`, `tests/bridge_tests.cpp:891`):
  check that the fixture still means what it says.
- `core/src/backend/bytecode/static_fn_fuzz.rs:366,502`: the generator stops
  avoiding `state` in `map` callbacks.

## Docs

- `docs/features/inline-state.md`: replace "State is not allowed in
  callbacks" with a section headed "Callbacks of `map` and friends". It covers
  the per-element rule, `key:`, early exit in `any`/`all`/`find`, and the
  callbacks that still error (`sort`'s comparator, `filter` and `count`).
- `docs/dev/state-call-paths.md` §3.3: the element part on callback frames.
- `docs/proposals/state-in-callbacks.md`: a status line linking here.
- `CHANGELOG.md` under "Changed": `state` in `map`/`reduce`/
  `forEach`/`any`/`all`/`find`/`sort_by` callbacks is now per element; `key:`
  is new.

## Cost

Each callback call pushes one more path part, and the builtin sets it per
element. That is the same cost a `for` loop iteration already pays. Measure
the `map`-heavy and `state_heavy` benchmarks (`docs/dev/performance.md`)
before and after. The duplicate-key check allocates one hash set per keyed
call, and only keyed calls pay it.

## Order of work

1. `call_closure_sync_at` and the element part for `map` only. Turn the
   pinned-error test for `map` into the per-element test, and confirm the
   fast-path, memo and speculation tests.
2. `forEach` and `sort_by`'s key; the std helpers built on `map`.
3. `key:` on `map` and `forEach` (field name or function), with duplicate
   detection.
4. The per-element flag on the callback record, and the lint pass narrowed
   to match.
5. Docs, CHANGELOG, the status note on the proposal.
6. The `Env` host API: `StatePart`, `call_function_at`, `call_closure_at`,
   `state_under`, `clear_state_under`, `keep_state_under`.
7. The downstream work this unblocks: `interact`'s implicit `stable_id()`
   identity, and bloom's `probe` on `interact`.
8. Second round: `reduce`, `any`, `all`, `find` and `filter` (so `count`),
   and `key:` on them if wanted.
9. `PetalCxt::call_at`, for natives that take a callback.

## Decided

- 2026-11-20: `key:` takes a field name or a function, like lodash's
  iteratee shorthand. No dotted paths for now.
- 2026-11-20: the first round is `map`, `forEach` and `sort_by`'s key.
  `reduce`, `any`, `all`, `find` and `filter` come after it ships.
- 2026-11-20: Rust callers can choose the state a call runs under and
  manage it ("Rust callers" above).
- 2026-11-20: `StatePart` has `index`, `key` (plain Rust values, hashed as
  Petal hashes the same value) and `key_value` (a Petal value). No separate
  `named`: `key("row:42")` is that. A host key and a Petal key with equal
  values are the same part. The hash is Rust's `DefaultHasher`, which, as
  for every keyed loop, isn't guaranteed stable across Rust versions; that
  matters only if state snapshots are ever saved to disk.
- 2026-11-20: **state belongs to the call site, never to the lambda.** A
  closure's `state` is keyed by the path of the code that runs it (the call
  sites, loop iterations and element parts on the stack at the call), not by
  the closure value or where it was created. So one lambda run from two
  places has two sets of slots, and a closure stored and run later (bloom's
  `defer_paint`, a command registry) keeps its state where it is run. Two
  different lambdas run from the same place (a `map` whose callback is
  chosen at runtime) get separate slots too, because the slot key includes
  the `state` declaration's own id. Nothing in this plan attaches state to a
  closure value, and nothing should: a closure is recreated each run, so
  identity-keyed state would be lost or would leak.

## Open questions

1. **`key:` on `sort_by`** (first round) and on the second-round builtins.
   These are rarely stateful, so `key:` on them may never be asked for.
2. **The C bridge** (`integrations/petal-c-bridge`) would want the same host
   API. Its scope type would be an opaque handle built from an index, a
   string or a Petal value. Is it needed in the first host round?

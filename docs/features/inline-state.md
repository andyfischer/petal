# Inline state: the `state` keyword

Petal has a first-class way to declare persistent state, with the `state`
keyword. Variables declared with `state` are persisted across time.

This system is an enabler for a lot of Petal's features including [hot reloading](hot-reload.md).
Since the Petal runtime knows what state is essential to preserve, it's
able to preserve that state across a live code modification.

# Contents

- [Declaring state](#declaring-state)
- [State is nested](#state-is-nested)
- [Interaction with control flow](#interaction-with-control-flow)
  - [Keyed loops](#keyed-loops)
- [Removed: `state(key)`](#removed-statekey)
- [Sharing on purpose](#sharing-on-purpose)
- [Callbacks of `map` and friends](#callbacks-of-map-and-friends)
- [`state var`](#state-var)
- [Reading and writing from other functions](#reading-and-writing-from-other-functions)
- [How long a slot lives](#how-long-a-slot-lives)
- [State is not a constant](#state-is-not-a-constant)
- [Pending initializers](#pending-initializers)
- [State and hot reload](#state-and-hot-reload)
- [State from the host side](#state-from-the-host-side)
- [See also](#see-also)

## Declaring state

State vars are declared with an initializer.

In this example, the `ticks` number would start at 0 and increase by 1 every time this script is called:

```petal
state ticks = 0
ticks += 1
```

## State is nested

State vars can be declared in nested blocks and inside function calls.

For an example let's set up a `stable_random` function. Our function will
create a random number and store it.

```
fn stable_random() 
  state val = rand_int(100)
  return val

print(stable_random())
```

If we run this program, it will pick a random number (let's say `42`), and
then the same program will print `42` every time.

In the debugger we could inspect the persisted state for this program and
see something like:

TODO: example.

Then, let's say we have multiple calls to the same function:

```
fn stable_random() 
  state val = rand_int(100)
  return val

print("first: ", stable_random())
print("second: ", stable_random())
print("third: ", stable_random())
```

Each call will create a *different* random number that's stored separately.

The output might look like:

```
first: 39
second: 14
third: 77
```

In the debugger, the state might look like:

TODO: example.

## Interaction with control flow

### Loops

If `state` is used inside a loop, then it creates a list or map, with one element per iteration.

By default it creates a list, with each iteration of the loop stored using its iteration index.

You can also use the `key` syntax to specify a stable key to store state.

Example:

```petal
let tasks = [{id: 7, title: "write"}, {id: 3, title: "test"}]
for t in tasks key t.id do
  print(row(t))
```

Specifying a key will make the contents much more stable, in
case the incoming list has ordering changes, or if you
skip elements with `continue`.

### Conditionals

If `state` is used in an `if` block, then the data is only retained on runs where
that condition is true. If the condition fails then any nested state gets deleted.

### Recursive calls

If `state` is used in a recursive function call, then it creates a tree data structure.

TODO: Example here

## Sharing on purpose

By nature, `state` containers are hard to share because they're created
separately for each call path.

If you **want** to share the same stateful value across multiple parts of the
program then the "escape hatch" approach is to use `state var` at the top
level. These values can be accessed and modified anywhere using `get` and `set`.

The `var` approach is discouraged when not needed since it makes your code
harder to reason about. But in some cases, it's necesary.

Example:

```petal
state var theme = "dark"

fn ui_theme()      get theme end
fn theme_set(t)    set theme = t end

theme_set("light")
print(ui_theme())   # light
```

One useful approach is to store a Map in a state var:

```petal
state var hp = Map()

fn health(id, damage)
  set hp[id] = (get hp[id] ?? 100) - damage
  get hp[id]
print(health("goblin", 10), health("orc", 30), health("goblin", 5))  # 90 70 85
```

## Removed: `state(key)`

Until 2026-11-13 a `state` could carry a key: `state(key) name = init` keyed
its slot by the key's value and ignored the call path, so every caller that
reached the declaration with an equal key landed on one slot. That is a
global hidden in a function, and it broke the rule that state is nested per
call path. The form was deprecated on 2026-10-22 and is now a compile error
that names the replacements. Each use is written as one of four things:

- **State per item of a list:** a [keyed loop](#keyed-loops),
  `for t in tasks key t.id do`, with a plain `state` in its body (or in the
  functions it calls).
- **Sharing on purpose:** a top-level `state var` holding a Map, as in
  [Sharing on purpose](#sharing-on-purpose).
- **A cache of a pure result:** [`memo(key)`](memo.md), but only when the
  initializer reads nothing except its key. A memo is shared by every
  caller with an equal key, and it is recomputed after a reload or a run
  that does not reach it. So `memo(k) v = k * scale` hands the first
  caller's `scale` to every caller, and `memo(id) tint = random(0.0, 1.0)`
  draws a new tint after each reload.
- **One slot per caller:** plain `state`.

`petal migrate-old-code` (migration `state-key-deprecated`) still reads the
removed form. It makes the `memo` rewrite only when it can see from the
syntax that the initializer is a pure function of the key (built from
literals, the names the key reads, operators, and list, record, field and
index forms) and nothing writes the slot. It does not make the rewrite for
an initializer that calls anything, including a pure helper such as
`measure(text)`. Those sites are reported for a hand edit, and the author
chooses among the four.

## Callbacks of `map` and friends

`map`, `forEach` and `sort_by` (its key function) call your function once per
element, in element order. Each call gets its own place, the way an iteration
of a `for` loop does: `state` in the callback, and in everything it calls, is
kept per element across runs.

```petal
fn seen(x)
  state n = 0
  n += 1
  n
print(map([10, 20, 30], seen))   # [1, 1, 1], then [2, 2, 2] on the next run
```

`map(xs, f)` keeps exactly the slots `for x in xs do f(x)` would. The std
helpers built on `map` (`min_by`, `max_by`, `top_k`, `group_by`, `unique_by`,
`flat_map`) follow, so widget rows built with `map` work, and `stable_id()`
gives one id per element.

### Keys

By default an element's slots belong to its **position**, as in an unkeyed
`for`: if the list reorders, the slots stay put and the items move. Give
`key:` to make them follow the item:

```petal
let rows = map(tasks, row, key: "id")                 # a field name
let rows = map(tasks, row, key: fn(t) t.owner.id)     # or a function
let rows = for t in tasks key t.id do row(t)          # the same slots as key: "id"
```

- A string names a field: element `x`'s key is `x[name]`, on a record or a
  `Map`. A missing field is an error naming the element's index. `"a.b"` is
  the field named `a.b`; use the function form for a nested key.
- A function of the element returns the key. It runs once per element before
  any callback, and may not declare `state` itself.
- Anything else is an error: ``map(): `key:` must be a field name (a string)
  or a function of the element``.
- Keys are hashed as `for … key` hashes them, so a keyed `map` and a keyed
  `for` over the same items build slots of the same shape.
- Two elements with the same key are an error (`Duplicate map key …`),
  reported before any callback runs: they would share their `state`.

`forEach` takes `key:` the same way. When the list shrinks, the slots of the
elements that are gone are swept at the end of the run, as a loop's are.

### Callbacks that still can't declare `state`

`filter` (and so `count`), `reduce`, `any`, `all`, `find`, and a comparator
passed to `sort` don't give their callback a place of its own yet; a
comparator never will, since a comparison has no element and a merge sort's
comparisons change with the data. Declaring `state` or `state var` while one
of these callbacks runs is an error. That covers the callback's body, every
function it calls, loops inside them, and a `map` inside them (its slots would
still be shared across the outer calls).

```petal
fn seen(x)
  state n = 0
  n += 1
  n > 1
print(filter([10, 20, 30], seen))
```

`petal lint` can prove this one, so the program does not start:

```
error: `state n` cannot be used inside a callback of `filter`: `filter` runs `seen` on line 6, once per element. State needs a fixed place in the program, and this callback has none. For a value per element that persists across runs, use `map` or a loop: `map(xs, f)` (add `key: "id"` to follow items) or `for x in xs do f(x)` (add `key …`). For a value carried from one element to the next, use reduce's accumulator. [state-in-callback]
 --> [line 2, column 5]
  |
2 |     state n = 0
  |     ^^^^^^^^^^^
1 error found by check; the program was not run
```

The compile check only reports what it can prove. A callback that arrives
through a parameter or a `var`, or `state` the callback reaches only on some
paths (under an `if`, a `match` or a loop, or after an early `return`), is
caught when it runs instead, with a note at the builtin call (or at your call
of a std helper such as `count`, not into the prelude) and the same help.

There are two fixes, depending on what the state was for:

```petal
# a value per element that persists across runs: map, or a loop
print(map([10, 20, 30], seen))                # [false, false, false], then all true
print(for x in [10, 20, 30] do seen(x) end)   # the same

# a value carried from one element to the next: reduce's accumulator
print(reduce([10, 20, 30], [], fn(acc, x) append(acc, len(acc) + 1) end))   # [1, 2, 3]
```

What stays allowed in any callback:

- reading top-level `state`, and `set` on a top-level `state var`: that slot
  is shared on purpose, and declared where everyone can see it;
- `memo`, which is keyed by value, not by place;
- `state` reached through a higher-order function you write, such as
  petal-ui's `with_layer`, `ui_measure` and `cached_layer` bodies: the
  `body()` call inside it is a callsite like any other;
- a function the host calls (`Env::call_function`), which keeps the
  [host callsite rule](#state-from-the-host-side).

```petal
state limit = 2
state var total = 0

fn over(x)
  set total += x   # a top-level cell, shared on purpose
  x > limit        # a top-level state read
print(filter([1, 2, 3, 4], over), get total)   # [3, 4] 10
```

The plan is [tasks/state-in-map.md](../tasks/state-in-map.md); the earlier
design and its measurements are in
[proposals/state-in-callbacks.md](../proposals/state-in-callbacks.md).

## `state var`

`state var` combines persistence with a [mutable cell](../syntax/var.md). The
slot holds the cell, so the value survives across runs and hot reloads, and it
can also be written with `set` from inside another function or a callback.

```petal
state var hits = 0
set hits = hits + 1
print(hits)
```

The slot is keyed like any other: a `state var` at the top level is one
shared cell, and a `state var` inside a function is one cell per call path.
For one cell's worth of state per key, hold a Map in a top-level `state var`.
Use `state var` only when a plain
`state` cannot express the write. A read of a plain `state` carries its
dataflow edges, and a read of a cell does not
([var.md, Provenance](../syntax/var.md#provenance)).

## Pending initializers

A [pending value](../dev/pending-values-plan.md) (the result of an
asynchronous fetch that has not arrived) is not committed by a `state`
initializer. The slot stays uninitialized, reads yield the pending value for
that run, and the initializer runs again on each run until it produces a
value that is not pending. So `state user = fetch(url)` means "initialize
this state from the fetch, once it arrives", and it does not store the
loading state forever. The rule is per slot.

## State and hot reload

Every part of a slot's identity is derived from names, never from positions
in the file. The declaration is identified by its name and its enclosing
module and function chain. Each callsite is identified by the callee's
spelling plus its ordinal among identically spelled calls in that function.

- Editing elsewhere in the file (reordering declarations, adding lines,
  reformatting) keeps state.
- Renaming a state variable, renaming a callee, or inserting an earlier call
  to the same callee changes an identity. That slot, or that callsite's
  subtree of slots, starts fresh.
- Editing a state's initializer does not change a slot that already exists.
  The reload report lists it as a stale seed when the initializer read a
  value that changed.

What a reload does to the rest of the program, and what it reports, is in
[hot-reload.md](hot-reload.md).

## State from the host side

A host can read and write top-level state by name, dump every slot as JSON,
and snapshot or restore the whole map. Slots under a call path have no name to
address them by; they appear in the JSON dump under their path
(`[3]/row/hovered`, `counter/n`).

- Rust: [ffi.md, Retained state](../usage/ffi.md#retained-state).
- C and C++: [embedding-c.md](../usage/embedding-c.md).

A function that the host calls directly starts a path of its own. Repeated
host calls of one function share slots with each other, but not with calls of
that function made inside the program. To share a value across both, use a
top-level `state var`.

A host can also choose a place for a call's state (one per row of a native
table, keyed by the row's id) and drop it when the row goes away:
[ffi.md, Retained state](../usage/ffi.md#retained-state).

## See also

- [`examples/console/state.ptl`](../../examples/console/state.ptl) and
  [`examples/console/reactive_ui.ptl`](../../examples/console/reactive_ui.ptl):
  runnable examples.
- [`var`: mutable cells](../syntax/var.md): `var`, `set` and `get`.
- [Cached results: `memo`](memo.md): a pure value cached by key, shared by
  every caller.
- [Call-path keyed `state`](../dev/state-call-paths.md): the design, the
  runtime key, and the measured cost.
